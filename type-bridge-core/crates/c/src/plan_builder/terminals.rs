//! Typed scalar terminals over the same borrowed selection arena.
use super::selections::Selection;
use super::*;

/// Shared ABI layout of a borrowed scalar-terminal marker.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeHeldPlan {
    pub(crate) recording: *mut TypeBridgePlanBuilder,
    pub(crate) selection: u32,
    pub(crate) reserved: u32,
}
macro_rules! terminal {
    ($function:ident, $selection:ident) => {
        /// Hold a scalar terminal without freezing or compiling the recording.
        /// # Safety
        /// Builder is live and exclusively borrowed. The writable marker may
        /// be unaligned and cannot alias the recording or retained package bytes.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            builder: *mut TypeBridgePlanBuilder,
            output: *mut TypeBridgeHeldPlan,
        ) -> TypeBridgeStatus {
            guarded(|| {
                let outputs = match direct_output_preflight(&[(
                    output.cast(),
                    size_of::<TypeBridgeHeldPlan>(),
                )]) {
                    Ok(outputs) => outputs,
                    Err(status) => return status,
                };
                if !builder.is_null() {
                    if let Err(status) =
                        outputs.check_bytes(builder.cast(), size_of::<TypeBridgePlanBuilder>())
                    {
                        return status;
                    }
                    // SAFETY: complete recording is fenced and live by contract.
                    if let Err(status) = unsafe { &*builder }.check_borrowed_ranges(&outputs) {
                        return status;
                    }
                }
                if output.is_null() {
                    return TypeBridgeStatus::InvalidArgument;
                }
                let empty = TypeBridgeHeldPlan {
                    recording: ptr::null_mut(),
                    selection: 0,
                    reserved: 0,
                };
                // SAFETY: writable marker is disjoint from every retained input.
                unsafe { output.write_unaligned(empty) };
                if builder.is_null() {
                    return TypeBridgeStatus::InvalidArgument;
                }
                // SAFETY: exclusively borrowed live recording by contract.
                match unsafe { &mut *builder }.hold(Selection::$selection) {
                    Ok(selection) => {
                        // SAFETY: output remains writable and fenced.
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
    };
}
terminal!(type_bridge_plan_matched, Matched);
terminal!(type_bridge_plan_count, Count);
terminal!(type_bridge_plan_exists, Exists);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_markers_fence_outputs_and_leave_unstaged_selections_uncompiled() {
        let mut builder = TypeBridgePlanBuilder::new();
        let mut output = TypeBridgeHeldPlan {
            recording: ptr::dangling_mut(),
            selection: 99,
            reserved: 99,
        };
        // SAFETY: valid markers use independent writable storage. The hostile
        // recording overlap is rejected before any output write or mutation.
        unsafe {
            assert_eq!(
                type_bridge_plan_count(&mut builder, &mut output),
                TypeBridgeStatus::Ok
            );
            assert!(ptr::eq(output.recording, &builder));
            assert_eq!(output.selection, 0);
            assert_eq!(output.reserved, 0);
            assert_eq!(
                type_bridge_plan_exists(
                    &mut builder,
                    (&mut builder as *mut TypeBridgePlanBuilder).cast()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(builder.selections.len(), 1);
            assert_eq!(
                type_bridge_plan_matched(ptr::null_mut(), &mut output),
                TypeBridgeStatus::InvalidArgument
            );
            assert!(output.recording.is_null());
            builder.frozen = true;
            assert_eq!(
                type_bridge_plan_exists(&mut builder, &mut output),
                TypeBridgeStatus::InvalidArgument
            );
            assert!(output.recording.is_null());
            assert_eq!(builder.selections.len(), 1);
            assert!(builder.error.is_none());
            assert_eq!(
                builder.last_rejected.as_ref().unwrap().code().as_str(),
                "plan_already_built"
            );
        }
    }
}
