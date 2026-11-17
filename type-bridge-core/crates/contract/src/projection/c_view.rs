//! One layout authority for generated C model views on Linux x86_64.
use super::*;

/// Stable identity of a generated C view struct.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CViewKey {
    /// Concrete field-bearing struct, before a subtype wrapper.
    Exact {
        /// Concrete model identity.
        model: TypeId,
        /// Whether this is a nonrecursive role-player view.
        player: bool,
    },
    /// Nominal result or nested-player view, possibly a tagged subtype union.
    Model {
        /// Nominal declared model identity.
        model: TypeId,
        /// Whether this is a nonrecursive role-player view.
        player: bool,
    },
    /// A relation role's closed union of accepted player models.
    Role {
        /// Relation declaring this accepted-player family.
        owner: TypeId,
        /// Exact role identity.
        role: RoleId,
    },
}
/// Meaning of a member in a field-bearing C view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CViewMemberKind {
    /// Borrowed NUL-terminated IID pointer.
    Iid,
    /// Exact package model ordinal.
    Kind,
    /// Deleted-root snapshot flag; false for every nested player.
    Deleted,
    /// Scalar or a pointer to an owner-held scalar array.
    Field {
        /// Exact projected ownership.
        field: OwnsFactId,
        /// Canonical scalar domain.
        domain: ValueTypeTag,
        /// Whether the member points to an array.
        list: bool,
    },
    /// Field presence; nested-player fields always carry it.
    FieldPresent {
        /// Exact projected ownership.
        field: OwnsFactId,
    },
    /// Scalar array element count.
    FieldCount {
        /// Exact projected ownership.
        field: OwnsFactId,
    },
    /// Pointer to owner-held player views.
    Role {
        /// Exact projected role.
        role: RoleId,
    },
    /// Player-view element count.
    RoleCount {
        /// Exact projected role.
        role: RoleId,
    },
}
/// One pinned member offset and ABI size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CViewMember {
    /// Semantic member identity, independent of emitted spelling.
    pub kind: CViewMemberKind,
    /// Byte offset in the struct.
    pub offset: usize,
    /// Member extent.
    pub size: usize,
    /// Required member alignment.
    pub alignment: usize,
}
/// One exact model variant of a closed C union.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CViewVariant {
    /// Actual concrete model identity.
    pub model: TypeId,
    /// Package model ordinal used as its tag.
    pub tag: u32,
    /// Concrete field-bearing layout selected by this variant.
    pub layout: CViewKey,
}
/// Complete size, alignment and offsets of one emitted view type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CViewLayout {
    /// Stable layout identity.
    pub key: CViewKey,
    /// Total padded size.
    pub size: usize,
    /// Required struct alignment.
    pub alignment: usize,
    /// Struct members, in declaration order.
    pub members: Vec<CViewMember>,
    /// Alias of an already defined layout, when no subtype wrapper is needed.
    pub alias: Option<CViewKey>,
    /// Offset of the union member, if this is a tagged union.
    pub union_offset: Option<usize>,
    /// Union variants; an empty closed union uses a one-byte reserved member.
    pub variants: Vec<CViewVariant>,
}
/// Shared C view inventory and its versioned layout digest.
#[derive(Clone, Debug)]
pub struct CViewLayouts {
    layouts: BTreeMap<CViewKey, CViewLayout>,
    digest: [u8; 32],
}
impl CViewLayouts {
    /// Compute layouts for the pinned Linux x86_64 C ABI. This is pure metadata;
    /// callers must separately refuse execution on unsupported target ABIs.
    pub fn for_projection(projection: &RuntimeProjection) -> Result<Self, Diagnostic> {
        if projection.target() != BindingTarget::C {
            return Err(layout_error());
        }
        let things = projection
            .models()
            .values()
            .filter(|model| matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation))
            .collect::<Vec<_>>();
        let mut layouts = BTreeMap::new();
        for player in [true, false] {
            for model in things
                .iter()
                .filter(|model| !model.declaration().is_abstract())
            {
                let layout = exact(projection, model, player)?;
                layouts.insert(layout.key.clone(), layout);
            }
            for model in &things {
                let variants = descendants(projection, model.id())
                    .into_iter()
                    .map(|model| variant(projection, model.id(), player))
                    .collect::<Result<Vec<_>, _>>()?;
                let key = CViewKey::Model {
                    model: model.id().clone(),
                    player,
                };
                let alias = (variants.len() == 1
                    && variants[0].model == *model.id()
                    && !model.declaration().is_abstract())
                .then(|| variants[0].layout.clone());
                let layout = wrapped(key, variants, alias, &layouts)?;
                layouts.insert(layout.key.clone(), layout);
            }
        }
        for model in &things {
            for role in model.query_tokens().roles().values() {
                let accepted = role
                    .accepted_players()
                    .iter()
                    .flat_map(|base| descendants(projection, base))
                    .map(|model| model.id().clone())
                    .collect::<BTreeSet<_>>();
                let variants = accepted
                    .iter()
                    .map(|id| variant(projection, id, true))
                    .collect::<Result<Vec<_>, _>>()?;
                let key = CViewKey::Role {
                    owner: model.id().clone(),
                    role: role.role().clone(),
                };
                let alias = if role.accepted_players().len() == 1 {
                    Some(CViewKey::Model {
                        model: role
                            .accepted_players()
                            .first()
                            .expect("one accepted model")
                            .clone(),
                        player: true,
                    })
                } else {
                    None
                };
                let layout = wrapped(key, variants, alias, &layouts)?;
                layouts.insert(layout.key.clone(), layout);
            }
        }
        let bytes = to_canonical_json(&(
            "linux-x86_64-c-view/v1",
            layouts.values().collect::<Vec<_>>(),
        ))?;
        let digest = Fingerprint::compute(
            FingerprintDomain::new("typebridge.c-view-layout").expect("static domain"),
            CanonicalizationVersion::new("typebridge.c-view-layout/v1").expect("static version"),
            None,
            &bytes,
        )
        .digest()
        .bytes();
        Ok(Self { layouts, digest })
    }
    /// Get one validated layout by identity.
    pub fn get(&self, key: &CViewKey) -> Option<&CViewLayout> {
        self.layouts.get(key)
    }
    /// Iterate in stable key order. Alias dependencies may precede or follow a key.
    pub fn iter(&self) -> impl Iterator<Item = (&CViewKey, &CViewLayout)> {
        self.layouts.iter()
    }
    /// Digest passed by generated plan_open and checked by the native runtime.
    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }
}
fn layout_error() -> Diagnostic {
    invalid_projection(
        "c_plan_view_layout_invalid",
        "The projected model cannot be represented by the pinned C view ABI",
    )
}
fn descendants<'a>(projection: &'a RuntimeProjection, base: &TypeId) -> Vec<&'a ModelProjection> {
    projection
        .models()
        .values()
        .filter(|model| {
            !model.declaration().is_abstract()
                && model.id().kind() == base.kind()
                && (model.id() == base || model.complete_read().nominal_upcasts().contains(base))
        })
        .collect()
}
fn variant(
    projection: &RuntimeProjection,
    model: &TypeId,
    player: bool,
) -> Result<CViewVariant, Diagnostic> {
    let tag = projection
        .projected_token_ordinal(&ProjectedTokenIdentity::Model(model.clone()))
        .ok_or_else(layout_error)?;
    Ok(CViewVariant {
        model: model.clone(),
        tag,
        layout: CViewKey::Exact {
            model: model.clone(),
            player,
        },
    })
}
fn rounded(size: usize, alignment: usize) -> Result<usize, Diagnostic> {
    size.checked_add(alignment - 1)
        .map(|size| size / alignment * alignment)
        .filter(|size| *size <= 65_535)
        .ok_or_else(layout_error)
}
struct StructLayout {
    size: usize,
    alignment: usize,
    members: Vec<CViewMember>,
}
impl StructLayout {
    fn new() -> Self {
        Self {
            size: 0,
            alignment: 1,
            members: Vec::new(),
        }
    }
    fn member(
        &mut self,
        kind: CViewMemberKind,
        size: usize,
        alignment: usize,
    ) -> Result<(), Diagnostic> {
        let offset = rounded(self.size, alignment)?;
        self.size = offset
            .checked_add(size)
            .filter(|size| *size <= 65_535)
            .ok_or_else(layout_error)?;
        self.alignment = self.alignment.max(alignment);
        self.members.push(CViewMember {
            kind,
            offset,
            size,
            alignment,
        });
        Ok(())
    }
    fn finish(self, key: CViewKey) -> Result<CViewLayout, Diagnostic> {
        Ok(CViewLayout {
            key,
            size: rounded(self.size, self.alignment)?,
            alignment: self.alignment,
            members: self.members,
            alias: None,
            union_offset: None,
            variants: Vec::new(),
        })
    }
}
fn exact(
    projection: &RuntimeProjection,
    model: &ModelProjection,
    player: bool,
) -> Result<CViewLayout, Diagnostic> {
    let mut layout = StructLayout::new();
    layout.member(CViewMemberKind::Iid, 8, 8)?;
    layout.member(CViewMemberKind::Kind, 4, 4)?;
    layout.member(CViewMemberKind::Deleted, 1, 1)?;
    for field in model.complete_read().fields() {
        let token = &model.query_tokens().fields()[field.token()];
        let attribute = TypeId::new(TypeKind::Attribute, token.id().attribute().label().as_str())
            .map_err(|_| layout_error())?;
        let domain = projection
            .models()
            .get(&attribute)
            .and_then(|attribute| attribute.declaration().value_type())
            .ok_or_else(layout_error)?;
        let list = field.multiplicity().container() != ProjectedContainer::Scalar;
        let (size, alignment) = if !list && domain == ValueTypeTag::Boolean {
            (1, 1)
        } else {
            (8, 8)
        };
        layout.member(
            CViewMemberKind::Field {
                field: token.id().clone(),
                domain,
                list,
            },
            size,
            alignment,
        )?;
        if list {
            layout.member(
                CViewMemberKind::FieldCount {
                    field: token.id().clone(),
                },
                8,
                8,
            )?;
        } else if player || !field.multiplicity().required() {
            layout.member(
                CViewMemberKind::FieldPresent {
                    field: token.id().clone(),
                },
                1,
                1,
            )?;
        }
    }
    if !player {
        for role in model.complete_read().roles().keys() {
            layout.member(CViewMemberKind::Role { role: role.clone() }, 8, 8)?;
            layout.member(CViewMemberKind::RoleCount { role: role.clone() }, 8, 8)?;
        }
    }
    layout.finish(CViewKey::Exact {
        model: model.id().clone(),
        player,
    })
}
fn wrapped(
    key: CViewKey,
    variants: Vec<CViewVariant>,
    alias: Option<CViewKey>,
    layouts: &BTreeMap<CViewKey, CViewLayout>,
) -> Result<CViewLayout, Diagnostic> {
    if let Some(alias) = alias {
        let parent = layouts.get(&alias).ok_or_else(layout_error)?;
        return Ok(CViewLayout {
            key,
            size: parent.size,
            alignment: parent.alignment,
            members: parent.members.clone(),
            alias: Some(alias),
            union_offset: parent.union_offset,
            variants: parent.variants.clone(),
        });
    }
    let mut layout = StructLayout::new();
    layout.member(CViewMemberKind::Kind, 4, 4)?;
    let mut union_size = 1;
    let mut union_alignment = 1;
    for variant in &variants {
        let member = layouts.get(&variant.layout).ok_or_else(layout_error)?;
        union_size = union_size.max(member.size);
        union_alignment = union_alignment.max(member.alignment);
    }
    let union_offset = rounded(layout.size, union_alignment)?;
    layout.size = union_offset
        .checked_add(union_size)
        .filter(|size| *size <= 65_535)
        .ok_or_else(layout_error)?;
    layout.alignment = layout.alignment.max(union_alignment);
    let mut layout = layout.finish(key)?;
    layout.union_offset = Some(union_offset);
    layout.variants = variants;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_abi_alignment_and_hosted_object_extent_are_checked() {
        let key = CViewKey::Exact {
            model: TypeId::new(TypeKind::Entity, "item").unwrap(),
            player: false,
        };
        let mut layout = StructLayout::new();
        layout.member(CViewMemberKind::Iid, 8, 8).unwrap();
        layout.member(CViewMemberKind::Kind, 4, 4).unwrap();
        layout.member(CViewMemberKind::Deleted, 1, 1).unwrap();
        layout.member(CViewMemberKind::Iid, 8, 8).unwrap();
        let layout = layout.finish(key).unwrap();
        assert_eq!((layout.size, layout.alignment), (24, 8));
        assert_eq!(
            layout
                .members
                .iter()
                .map(|member| member.offset)
                .collect::<Vec<_>>(),
            [0, 8, 12, 16]
        );
        assert!(rounded(65_535, 8).is_err());
        assert!(rounded(usize::MAX, 8).is_err());
    }
}
