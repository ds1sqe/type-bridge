//! Lazy typed columns for the generic held rows result.
use super::idioms::ValueBase;
use super::selections::Selection;
use super::terminals::TypeBridgeHeldPlan;
use super::*;
const MODEL: u32 = 6;

/// A borrowed typed model or scalar column. Generated constructors set its private fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanColumn {
    pub(super) value: ValueBase,
    pub(super) domain: u32,
    pub(super) model_ordinal: u32,
}

/// Hold one nominal model column without recording or dereferencing the recording.
/// # Safety
/// The returned column borrows the recording until it is consumed or discarded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_model_column(
    recording: *mut TypeBridgePlanBuilder,
    binding: u32,
    model_ordinal: u32,
) -> TypeBridgePlanColumn {
    TypeBridgePlanColumn {
        value: ValueBase {
            builder: recording.cast(),
            kind: MODEL,
            id: binding,
            owner: binding,
            field: 0,
        },
        domain: 0,
        model_ordinal,
    }
}

/// Hold columns in their supplied order, consuming lazy scalar fields/expressions left to right.
/// No program is compiled until this marker is staged. Oversized arrays fail
/// preflight without touching the marker.
/// # Safety
/// The recording is live and exclusively borrowed. Columns borrow live recordings.
/// The writable marker may be unaligned and cannot alias any input/retained package bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_returning(
    builder: *mut TypeBridgePlanBuilder,
    columns: *const TypeBridgePlanColumn,
    count: usize,
    output: *mut TypeBridgeHeldPlan,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs =
            match direct_output_preflight(&[(output.cast(), size_of::<TypeBridgeHeldPlan>())]) {
                Ok(outputs) => outputs,
                Err(status) => return status,
            };
        if !builder.is_null() {
            if let Err(status) =
                outputs.check_bytes(builder.cast(), size_of::<TypeBridgePlanBuilder>())
            {
                return status;
            }
            // SAFETY: complete live recording was fenced above.
            if let Err(status) = unsafe { &*builder }.check_borrowed_ranges(&outputs) {
                return status;
            }
        }
        let Some(bytes) = count
            .checked_mul(size_of::<TypeBridgePlanColumn>())
            .filter(|bytes| *bytes <= isize::MAX as usize)
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        // Oversized arrays cannot be deeply fenced, so do not initialize outputs.
        if count > type_bridge_contract::limits::MAX_SELECTED_SLOTS {
            return TypeBridgeStatus::InvalidArgument;
        }
        if !columns.is_null()
            && let Err(status) = outputs.check_bytes(columns.cast(), bytes)
        {
            return status;
        }
        if !columns.is_null() {
            for index in 0..count {
                // SAFETY: complete array is readable and was fenced above.
                let column = unsafe { columns.add(index).read_unaligned() };
                if !column.value.builder.is_null() {
                    if let Err(status) = outputs
                        .check_bytes(column.value.builder, size_of::<TypeBridgePlanBuilder>())
                    {
                        return status;
                    }
                    // SAFETY: nonnull column recordings remain live by contract.
                    if let Err(status) =
                        unsafe { &*column.value.builder.cast::<TypeBridgePlanBuilder>() }
                            .check_borrowed_ranges(&outputs)
                    {
                        return status;
                    }
                }
            }
        }
        if output.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: marker is writable and disjoint from every input.
        unsafe {
            output.write_unaligned(TypeBridgeHeldPlan {
                recording: ptr::null_mut(),
                selection: 0,
                reserved: 0,
            })
        };
        if builder.is_null() || (columns.is_null() && count != 0) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller lends the recording exclusively.
        let builder_ref = unsafe { &mut *builder };
        if let Err(status) = builder_ref.require_open() {
            return status;
        }
        if count == 0 {
            builder_ref.fail(invalid(
                "plan_returning_empty",
                "A rows plan requires at least one result column",
            ));
            return TypeBridgeStatus::InvalidArgument;
        }
        if builder_ref.error.is_some() {
            return super::idioms::statement_status(builder_ref);
        }
        for index in 0..count {
            // SAFETY: bounded, fenced input array remains live.
            let column = unsafe { columns.add(index).read_unaligned() };
            if column.value.builder != builder.cast() {
                builder_ref.fail(invalid(
                    "c_plan_builder_foreign_handle",
                    "The result column belongs to another recording or is not builder-bound",
                ));
                return TypeBridgeStatus::InvalidArgument;
            }
            if column.value.kind != MODEL && value_type(column.domain as i32).is_none() {
                builder_ref.fail(value_invalid());
                return TypeBridgeStatus::InvalidArgument;
            }
        }
        let mut bindings = Vec::with_capacity(count);
        let mut models = Vec::new();
        for index in 0..count {
            // SAFETY: as above; each column is consumed in constructor order.
            let column = unsafe { columns.add(index).read_unaligned() };
            let binding = if column.value.kind == MODEL {
                let Some(binding) = builder_ref
                    .binding(column.value.owner)
                    .filter(|binding| builder_ref.core.contains_binding(*binding))
                else {
                    builder_ref.fail(id_unknown());
                    return TypeBridgeStatus::InvalidArgument;
                };
                let Some(declared) = builder_ref
                    .models
                    .get(column.model_ordinal as usize)
                    .and_then(Option::as_ref)
                    .and_then(|model| builder_ref.types.get(model.type_id as usize))
                    .and_then(Option::as_ref)
                else {
                    builder_ref.fail(id_unknown());
                    return TypeBridgeStatus::InvalidArgument;
                };
                models.push((binding, declared.clone()));
                binding
            } else {
                let Some(binding) = builder_ref.column_binding(column.value) else {
                    return super::idioms::statement_status(builder_ref);
                };
                binding
            };
            bindings.push(binding);
        }
        match builder_ref.hold(Selection::Rows {
            columns: bindings,
            models,
        }) {
            Ok(selection) => {
                // SAFETY: output remains fenced and writable.
                unsafe {
                    output.write_unaligned(TypeBridgeHeldPlan {
                        recording: builder,
                        selection,
                        reserved: 0,
                    })
                };
                TypeBridgeStatus::Ok
            }
            Err(status) => status,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_columns_cannot_clear_an_output_inside_a_foreign_recording() {
        let mut first = TypeBridgePlanBuilder::new();
        let mut foreign = TypeBridgePlanBuilder::new();
        let model = foreign.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("item".into()));
        let binding = foreign.thing(model, PlanBuilder::exact);
        // SAFETY: the column borrows a live recording; no core work occurs here.
        let column = unsafe { type_bridge_plan_model_column(&mut foreign, binding, 0) };
        let columns = vec![column; type_bridge_contract::limits::MAX_SELECTED_SLOTS + 1];
        let output = (&raw mut foreign).cast::<TypeBridgeHeldPlan>();
        // SAFETY: the deliberately aliased output must be rejected without writing.
        assert_eq!(
            unsafe {
                type_bridge_plan_returning(&mut first, columns.as_ptr(), columns.len(), output)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(
            foreign.core.contains_binding(
                type_bridge_contract::migration_assertion::BindingId::new(
                    u16::try_from(binding).unwrap()
                )
                .unwrap()
            )
        );
        assert!(first.error.is_none());
        assert!(foreign.error.is_none());
    }
}
