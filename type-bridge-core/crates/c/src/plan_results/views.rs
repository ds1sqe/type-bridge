//! One aligned owner-held block for generated view structs and their arrays.
use super::*;
use type_bridge_contract::projection::c_view::{
    CViewKey, CViewLayout, CViewLayouts, CViewMemberKind,
};

pub(super) struct ViewStorage {
    data: Vec<u64>,
    roots: Vec<RootStorage>,
}
struct RootStorage {
    declared: TypeId,
    base: usize,
    size: usize,
    rows: usize,
}
impl ViewStorage {
    #[cfg(test)]
    pub(super) fn build(
        layouts: &CViewLayouts,
        declared: &TypeId,
        models: &[Vec<ModelViewData>],
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::build_columns(layouts, std::slice::from_ref(declared), models)
    }
    pub(super) fn build_columns(
        layouts: &CViewLayouts,
        declarations: &[TypeId],
        models: &[Vec<ModelViewData>],
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let layouts_source = layouts;
        let layouts = declarations
            .iter()
            .map(|declared| {
                layouts
                    .get(&CViewKey::Model {
                        model: declared.clone(),
                        player: false,
                    })
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let populate = |arena: &mut Arena,
                        mut roots: Option<&mut Vec<RootStorage>>|
         -> Result<(), SdkExecutionDiagnostic> {
            for (slot, root) in layouts.iter().enumerate() {
                let base = arena.reserve(models.len(), root.size, root.alignment)?;
                for (row, columns) in models.iter().enumerate() {
                    let model = columns
                        .get(slot)
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    arena.model(layouts_source, root, model, base + row * root.size)?;
                }
                if let Some(roots) = roots.as_deref_mut() {
                    roots.push(RootStorage {
                        declared: declarations[slot].clone(),
                        base,
                        size: root.size,
                        rows: models.len(),
                    });
                }
            }
            Ok(())
        };
        let mut sizing = Arena {
            data: None,
            cursor: 0,
        };
        populate(&mut sizing, None)?;
        let words = sizing
            .cursor
            .checked_add(7)
            .map(|bytes| bytes / 8)
            .ok_or_else(crate::allocation::allocation_exhausted)?;
        let mut data = Vec::new();
        crate::allocation::try_reserve(
            &mut data,
            words,
            crate::allocation::AllocationSite::PlanViewStorage,
        )
        .map_err(|_| crate::allocation::allocation_exhausted())?;
        data.resize(words, 0);
        let mut writing = Arena {
            data: Some(data),
            cursor: 0,
        };
        let mut roots = Vec::with_capacity(declarations.len());
        populate(&mut writing, Some(&mut roots))?;
        debug_assert_eq!(sizing.cursor, writing.cursor);
        Ok(Self {
            data: writing.data.expect("writing owns its block"),
            roots,
        })
    }
    pub(super) fn fence(&self, outputs: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        outputs.check_bytes(
            self.data.as_ptr().cast(),
            self.data.len() * size_of::<u64>(),
        )
    }
    pub(super) fn row(&self, row: usize) -> Option<&[u8]> {
        self.row_slot(row, 0)
    }
    pub(super) fn row_slot(&self, row: usize, slot: usize) -> Option<&[u8]> {
        let root = self.roots.get(slot)?;
        if row >= root.rows {
            return None;
        }
        let start = row.checked_mul(root.size)?.checked_add(root.base)?;
        let end = start.checked_add(root.size)?;
        // SAFETY: initialized u64 storage is stable and exposed read-only as bytes.
        unsafe {
            std::slice::from_raw_parts(
                self.data.as_ptr().cast(),
                self.data.len() * size_of::<u64>(),
            )
        }
        .get(start..end)
    }
    pub(super) fn slot_model(&self, slot: usize) -> Option<&TypeId> {
        self.roots.get(slot).map(|root| &root.declared)
    }
}
struct Arena {
    data: Option<Vec<u64>>,
    cursor: usize,
}
impl Arena {
    fn reserve(
        &mut self,
        count: usize,
        size: usize,
        alignment: usize,
    ) -> Result<usize, SdkExecutionDiagnostic> {
        let start = self
            .cursor
            .checked_add(alignment - 1)
            .map(|cursor| cursor / alignment * alignment)
            .ok_or_else(crate::allocation::allocation_exhausted)?;
        self.cursor = count
            .checked_mul(size)
            .and_then(|bytes| start.checked_add(bytes))
            .filter(|bytes| *bytes <= isize::MAX as usize)
            .ok_or_else(crate::allocation::allocation_exhausted)?;
        Ok(start)
    }
    fn pointer(&self, offset: usize) -> *const u8 {
        self.data.as_ref().map_or(ptr::null(), |data| {
            // SAFETY: the offset was reserved within this initialized block.
            unsafe { data.as_ptr().cast::<u8>().add(offset) }
        })
    }
    fn write<T: Copy>(&mut self, offset: usize, value: T) -> Result<(), SdkExecutionDiagnostic> {
        if let Some(data) = &mut self.data {
            let end = offset
                .checked_add(size_of::<T>())
                .filter(|end| *end <= data.len() * size_of::<u64>())
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let _ = end;
            // SAFETY: the shared layout and the checked extent admit this complete
            // primitive C member. The block remains exclusively borrowed while filled.
            unsafe {
                data.as_mut_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<T>()
                    .write_unaligned(value)
            };
        }
        Ok(())
    }
    fn scalar(&mut self, offset: usize, scalar: &Scalar) -> Result<(), SdkExecutionDiagnostic> {
        match scalar {
            Scalar::Long(value) => self.write(offset, *value),
            Scalar::Double(value) => self.write(offset, *value),
            Scalar::Boolean(value) => self.write(offset, u8::from(*value)),
            Scalar::Text(text) => self.write(offset, text.as_ptr()),
        }
    }
    fn model<'a>(
        &mut self,
        layouts: &'a CViewLayouts,
        mut layout: &'a CViewLayout,
        model: &ModelViewData,
        offset: usize,
    ) -> Result<(), SdkExecutionDiagnostic> {
        while let Some(alias) = &layout.alias {
            layout = layouts
                .get(alias)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        }
        let tag = model
            .package
            ._projection
            .projected_token_ordinal(&ProjectedTokenIdentity::Model(model.type_id.clone()))
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        if let Some(union_offset) = layout.union_offset {
            self.write(offset, tag)?;
            let variant = layout
                .variants
                .iter()
                .find(|variant| variant.model == model.type_id)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            return self.model(
                layouts,
                layouts
                    .get(&variant.layout)
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?,
                model,
                offset + union_offset,
            );
        }
        let CViewKey::Exact {
            model: declared, ..
        } = &layout.key
        else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        if declared != &model.type_id {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        for member in &layout.members {
            let output = offset + member.offset;
            match &member.kind {
                CViewMemberKind::Iid => self.write(output, model.iid.as_ptr())?,
                CViewMemberKind::Kind => self.write(output, tag)?,
                CViewMemberKind::Deleted => self.write(output, u8::from(model.deleted))?,
                CViewMemberKind::Field {
                    field,
                    domain,
                    list,
                } => {
                    let values = model.fields.get(field).map_or(&[][..], Vec::as_slice);
                    if *list {
                        let size = if *domain == type_bridge_contract::value::ValueTypeTag::Boolean
                        {
                            1
                        } else {
                            8
                        };
                        let base = self.reserve(values.len(), size, size)?;
                        for (index, value) in values.iter().enumerate() {
                            self.scalar(base + index * size, value)?;
                        }
                        self.write(
                            output,
                            if values.is_empty() {
                                ptr::null()
                            } else {
                                self.pointer(base)
                            },
                        )?;
                    } else if let Some(value) = values.first() {
                        self.scalar(output, value)?;
                    }
                }
                CViewMemberKind::FieldPresent { field } => self.write(
                    output,
                    u8::from(
                        model
                            .fields
                            .get(field)
                            .is_some_and(|values| !values.is_empty()),
                    ),
                )?,
                CViewMemberKind::FieldCount { field } => {
                    self.write(output, model.fields.get(field).map_or(0, Vec::len))?
                }
                CViewMemberKind::Role { role } => {
                    let players = model.roles.get(role).map_or(&[][..], Vec::as_slice);
                    let player_layout = layouts
                        .get(&CViewKey::Role {
                            owner: declared.clone(),
                            role: role.clone(),
                        })
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    let base =
                        self.reserve(players.len(), player_layout.size, player_layout.alignment)?;
                    for (index, player) in players.iter().enumerate() {
                        self.model(
                            layouts,
                            player_layout,
                            player,
                            base + index * player_layout.size,
                        )?;
                    }
                    self.write(
                        output,
                        if players.is_empty() {
                            ptr::null()
                        } else {
                            self.pointer(base)
                        },
                    )?;
                }
                CViewMemberKind::RoleCount { role } => {
                    self.write(output, model.roles.get(role).map_or(0, Vec::len))?
                }
            }
        }
        Ok(())
    }
}
