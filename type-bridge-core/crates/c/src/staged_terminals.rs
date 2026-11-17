//! Scalar result accessors use the existing pending cursor and lifecycle checks.
use crate::abi::{TypeBridgeStatus, guarded};
use crate::generated_preflight::direct_output_preflight;
use crate::staged::{TypeBridgePending, answer_status};
use std::mem::size_of;
use type_bridge_orm::session::staged::PlanAnswer;

unsafe fn scalar<T: Copy>(
    pending: *const TypeBridgePending,
    output: *mut T,
    empty: T,
    read: impl FnOnce(&PlanAnswer) -> Option<T>,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[(output.cast(), size_of::<T>())]) {
            Ok(outputs) => outputs,
            Err(status) => return status,
        };
        // SAFETY: any nonnull input borrows a live cursor for this call.
        let tag = match unsafe {
            crate::staged_answers::check_cursor_ranges(
                &outputs,
                pending.cast(),
                size_of::<TypeBridgePending>(),
            )
        } {
            Ok(tag) => tag,
            Err(status) => return status,
        };
        if output.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: writable output is disjoint from all exposed cursor ranges.
        unsafe { output.write_unaligned(empty) };
        if tag != Some(crate::staged_answers::PENDING_TAG) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the actual pending cursor and its complete range were checked above.
        let answer = match unsafe { &*pending }.state.pending.get() {
            Ok(answer) => answer,
            Err(error) => return answer_status(&error),
        };
        match read(&answer) {
            Some(value) => {
                // SAFETY: as above; output was fenced and initialized.
                unsafe { output.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            None => TypeBridgeStatus::InvalidArgument,
        }
    })
}

/// Read a held matched result. Read-only matched plans use the core count answer.
/// # Safety
/// Pending is a live cursor; output is writable, possibly unaligned, and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_matched_pending_result(
    pending: *const TypeBridgePending,
    output: *mut u64,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pending/output contract.
    unsafe {
        scalar(pending, output, 0, |answer| match answer {
            PlanAnswer::Matched(value) | PlanAnswer::Count(value) => Some(*value),
            _ => None,
        })
    }
}
/// Read a held count result without narrowing its unsigned count.
/// # Safety
/// Pending is a live cursor; output is writable, possibly unaligned, and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_count_pending_result(
    pending: *const TypeBridgePending,
    output: *mut u64,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pending/output contract.
    unsafe {
        scalar(pending, output, 0, |answer| match answer {
            PlanAnswer::Count(value) => Some(*value),
            _ => None,
        })
    }
}
/// Read a held exists result.
/// # Safety
/// Pending is a live cursor; output is writable, possibly unaligned, and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_exists_pending_result(
    pending: *const TypeBridgePending,
    output: *mut bool,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pending/output contract.
    unsafe {
        scalar(pending, output, false, |answer| match answer {
            PlanAnswer::Exists(value) => Some(*value),
            _ => None,
        })
    }
}
