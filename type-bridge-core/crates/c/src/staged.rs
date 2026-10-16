//! ABI 1.7 deferred transactions: decoded v3 plans are staged on one
//! transaction and nothing reaches the provider until commit.
//!
//! C never serializes plans. A plan handle is decoded from canonical
//! `query-plan/v3` bytes against declared schema bytes, so its bytes are the
//! same ones the Rust builders produce.
//!
//! Every entry point validates like the 1.6 transaction entry points: output
//! slots may be unaligned; outputs that overlap each other, an input buffer,
//! a live input handle or bytes that handle retains are rejected with
//! `INVALID_ARGUMENT` before anything is written; otherwise every non-null
//! output is cleared before any other check, so it is initialized on every
//! path.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::v3::{QueryPlanV3, decode_query_plan_v3};
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::session::staged::{
    Pending, PendingError, PlanAnswer, Transaction, TransactionKind, TransactionOptions,
};

use crate::abi::{TypeBridgeStatus, guarded};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::runtime::{StagedDatabaseGuard, TypeBridgeDatabase, check_database_borrowed_ranges};

/// Opaque validated `query-plan/v3` plan.
pub struct TypeBridgePlan {
    plan: QueryPlanV3,
    // Encoded once at decode so the accessor cannot fail and its source
    // range can be fenced against caller buffers.
    canonical: Vec<u8>,
}

/// Opaque deferred transaction. Commit, rollback and close consume it.
pub struct TypeBridgeStagedTransaction {
    transaction: Transaction,
    // Dropped after the transaction so the database stays busy until then.
    database: StagedDatabaseGuard,
}

/// Opaque staged answer, readable after a successful commit.
pub struct TypeBridgePending {
    pending: Pending<PlanAnswer>,
}

/// Answer kinds a staged plan resolves to (`type_bridge_plan_answer_kind_t`).
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgePlanAnswerKind {
    Rows = 1,
    Count = 2,
    Exists = 3,
    Matched = 4,
}

/// Transaction kinds accepted by [`type_bridge_database_transaction`].
pub const TYPE_BRIDGE_TRANSACTION_READ: u32 = 1;
pub const TYPE_BRIDGE_TRANSACTION_WRITE: u32 = 2;

fn invalid(code: &str, message: &'static str) -> SdkExecutionDiagnostic {
    let code = SdkDiagnosticCode::new(code)
        .or_else(|_| SdkDiagnosticCode::new("invalid_plan"))
        .expect("static diagnostic code is canonical");
    let message = SdkDiagnosticMessage::new(message).expect("static message is canonical");
    SdkExecutionDiagnostic::invalid_input(code, message)
}

fn contract_error(diagnostic: &Diagnostic, message: &'static str) -> SdkExecutionDiagnostic {
    invalid(diagnostic.code().as_str(), message)
}

fn pending_diagnostic(error: &PendingError) -> SdkExecutionDiagnostic {
    match error {
        PendingError::Failed {
            failure: Some(failure),
            context,
            ..
        } => SdkExecutionDiagnostic::from_failure(failure, context.as_ref().clone()),
        PendingError::Refused { diagnostic, .. } => diagnostic.as_ref().clone(),
        PendingError::InvalidPlan(diagnostic) => {
            contract_error(diagnostic, "the plan cannot be staged")
        }
        PendingError::NotAllowedInKind(diagnostic) => contract_error(
            diagnostic,
            "the plan is not allowed in this transaction kind",
        ),
        PendingError::ReadOnly => invalid(
            "staged_read_only",
            "a read transaction cannot stage a write plan",
        ),
        PendingError::NotCommitted => invalid(
            "staged_not_committed",
            "the staged answer is read after commit",
        ),
        _ => invalid(
            "staged_transaction_lifecycle",
            "the staged transaction is no longer usable",
        ),
    }
}

fn check_object<T>(
    preflight: &DirectOutputPreflight,
    pointer: *const T,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        return Ok(());
    }
    preflight.check_bytes(pointer.cast(), size_of::<T>())
}

/// A null pointer cannot overlap an output; its length is judged after the
/// outputs are initialized.
fn check_input_bytes(
    preflight: &DirectOutputPreflight,
    pointer: *const u8,
    length: usize,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        return Ok(());
    }
    preflight.check_bytes(pointer.cast(), length)
}

/// # Safety
/// A non-null `plan` is a live handle.
unsafe fn check_plan(
    preflight: &DirectOutputPreflight,
    plan: *const TypeBridgePlan,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, plan)?;
    if !plan.is_null() {
        // SAFETY: the complete live handle was fenced above.
        let canonical = &unsafe { &*plan }.canonical;
        preflight.check_bytes(canonical.as_ptr().cast(), canonical.len())?;
    }
    Ok(())
}

/// # Safety
/// A non-null `transaction` is a live handle.
unsafe fn check_transaction(
    preflight: &DirectOutputPreflight,
    transaction: *const TypeBridgeStagedTransaction,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, transaction)?;
    if !transaction.is_null() {
        // SAFETY: the complete live handle was fenced above.
        unsafe { &*transaction }
            .database
            .check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

/// Fence one handle constructor's two outputs against each other and its
/// inputs, then clear both. Nothing is written when the fence fails.
///
/// # Safety
/// Each non-null output names one writable, possibly unaligned, pointer slot.
unsafe fn prepare_handle_outputs<T>(
    out_handle: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    inputs: impl FnOnce(&DirectOutputPreflight) -> Result<(), TypeBridgeStatus>,
) -> Result<(), TypeBridgeStatus> {
    let outputs = direct_output_preflight(&[
        (out_handle.cast(), size_of::<*mut T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])?;
    inputs(&outputs)?;
    if !out_handle.is_null() {
        // SAFETY: the slot is writable and overlaps no input or other output.
        unsafe { out_handle.write_unaligned(ptr::null_mut()) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: as above.
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_handle.is_null() || out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(())
}

/// Fence a consuming terminal's owner slot, its live handle and the
/// diagnostics output, then clear the diagnostics output.
///
/// # Safety
/// A non-null owner slot is readable and writable and holds null or a live
/// handle; a non-null diagnostics slot is writable. Both may be unaligned.
unsafe fn prepare_terminal_outputs(
    transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<*mut TypeBridgeStagedTransaction, TypeBridgeStatus> {
    let outputs = direct_output_preflight(&[
        (
            transaction.cast(),
            size_of::<*mut TypeBridgeStagedTransaction>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])?;
    let value = if transaction.is_null() {
        ptr::null_mut()
    } else {
        // SAFETY: the owner slot is readable by contract.
        unsafe { transaction.read_unaligned() }
    };
    // SAFETY: a non-null owner names a live handle by contract.
    unsafe { check_transaction(&outputs, value) }?;
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: the slot is writable and overlaps no fenced input.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    if transaction.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(value)
}

/// # Safety
/// `ptr` names `len` readable bytes, or is null with `len == 0`.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return (len == 0).then_some(&[]);
    }
    // SAFETY: the caller contract above.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Decode canonical `query-plan/v3` bytes under declared schema bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_v3_decode(
    schema: *const u8,
    schema_len: usize,
    plan: *const u8,
    plan_len: usize,
    out_plan: *mut *mut TypeBridgePlan,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the outputs are fenced against both input buffers first.
    if let Err(status) = unsafe {
        prepare_handle_outputs(out_plan, out_diagnostics, |outputs| {
            check_input_bytes(outputs, schema, schema_len)?;
            check_input_bytes(outputs, plan, plan_len)
        })
    } {
        return status;
    }
    guarded(|| {
        // SAFETY: byte views follow the documented pointer/length contract.
        let (Some(schema), Some(plan)) = (unsafe { bytes(schema, schema_len) }, unsafe {
            bytes(plan, plan_len)
        }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let schema = match decode_declared_schema(schema) {
            Ok(value) => value,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "the declared schema bytes are invalid"),
                    out_diagnostics,
                );
            }
        };
        let decoded = decode_query_plan_v3(plan, &schema)
            .and_then(|plan| plan.canonical_bytes().map(|canonical| (plan, canonical)));
        let (plan, canonical) = match decoded {
            Ok(value) => value,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "the plan bytes are invalid"),
                    out_diagnostics,
                );
            }
        };
        let handle = Box::new(TypeBridgePlan { plan, canonical });
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_plan.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Write the canonical bytes of `plan` into a caller buffer. With a null or
/// short buffer, only the required length is reported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_v3_canonical_bytes(
    plan: *const TypeBridgePlan,
    buffer: *mut u8,
    capacity: usize,
    out_len: *mut usize,
) -> TypeBridgeStatus {
    let length_output = (out_len.cast(), size_of::<usize>());
    // A null or empty buffer is a length query and writes no bytes.
    let checked = if buffer.is_null() || capacity == 0 {
        direct_output_preflight(&[length_output])
    } else {
        direct_output_preflight(&[length_output, (buffer.cast(), capacity)])
    };
    let outputs = match checked {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: a non-null plan is a live handle by contract.
    if let Err(status) = unsafe { check_plan(&outputs, plan) } {
        return status;
    }
    if out_len.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: the slot is writable and overlaps no fenced input.
    unsafe { out_len.write_unaligned(0) };
    if plan.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: a non-null plan is a live handle by contract.
    let encoded = &unsafe { &*plan }.canonical;
    // SAFETY: as above.
    unsafe { out_len.write_unaligned(encoded.len()) };
    if buffer.is_null() || capacity < encoded.len() {
        return TypeBridgeStatus::ResourceLimit;
    }
    // SAFETY: buffer names `capacity >= encoded.len()` writable bytes that
    // overlap neither the plan nor `out_len`.
    unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), buffer, encoded.len()) };
    TypeBridgeStatus::Ok
}

/// Free a plan handle and null its owner slot. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_free(plan: *mut *mut TypeBridgePlan) {
    // SAFETY: forwarded owner-slot contract.
    unsafe { free_slot(plan) }
}

/// # Safety
/// A non-null slot is readable and writable (possibly unaligned) and holds
/// null or a handle this library allocated.
unsafe fn free_slot<T>(slot: *mut *mut T) {
    if slot.is_null() {
        return;
    }
    // SAFETY: the caller contract above.
    let value = unsafe { slot.read_unaligned() };
    if value.is_null() {
        return;
    }
    // SAFETY: as above; the slot is cleared before the handle drops.
    unsafe { slot.write_unaligned(ptr::null_mut()) };
    let _ = guarded(|| {
        // SAFETY: the slot owned this handle and no longer names it.
        drop(unsafe { Box::from_raw(value) });
        TypeBridgeStatus::Ok
    });
}

/// Open one deferred transaction. No provider transaction opens until commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_transaction(
    database: *const TypeBridgeDatabase,
    kind: u32,
    out_transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        check_object(outputs, database)?;
        if !database.is_null() {
            // SAFETY: the complete database handle was fenced above.
            check_database_borrowed_ranges(outputs, unsafe { &*database })?;
        }
        Ok(())
    };
    // SAFETY: the outputs are fenced against the database and its retained
    // package and server-version bytes first.
    if let Err(status) = unsafe { prepare_handle_outputs(out_transaction, out_diagnostics, inputs) }
    {
        return status;
    }
    guarded(|| {
        let kind = match kind {
            TYPE_BRIDGE_TRANSACTION_READ => TransactionKind::Read,
            TYPE_BRIDGE_TRANSACTION_WRITE => TransactionKind::Write,
            _ => return TypeBridgeStatus::InvalidArgument,
        };
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: a non-null database is a live handle by contract.
        let database = unsafe { &*database };
        let (transaction, database) =
            match database.staged_transaction(TransactionOptions::default().with_kind(kind)) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let handle = Box::new(TypeBridgeStagedTransaction {
            transaction,
            database,
        });
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_transaction.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Stage one plan. The pending answer resolves at commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_stage(
    transaction: *mut TypeBridgeStagedTransaction,
    plan: *const TypeBridgePlan,
    out_pending: *mut *mut TypeBridgePending,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        // SAFETY: non-null handles are live by contract.
        unsafe {
            check_transaction(outputs, transaction)?;
            check_plan(outputs, plan)
        }
    };
    // SAFETY: the outputs are fenced against both live handles first.
    if let Err(status) = unsafe { prepare_handle_outputs(out_pending, out_diagnostics, inputs) } {
        return status;
    }
    guarded(|| {
        if transaction.is_null() || plan.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: non-null handles are live and not shared across threads by contract.
        let (transaction, plan) = unsafe { (&mut *transaction, &*plan) };
        match transaction.transaction.stage_plan(&plan.plan) {
            Ok(pending) => {
                let handle = Box::new(TypeBridgePending { pending });
                // SAFETY: the output slot was fenced and initialized above.
                unsafe { out_pending.write_unaligned(Box::into_raw(handle)) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(pending_diagnostic(&error), out_diagnostics),
        }
    })
}

/// Commit every staged plan in one provider transaction. The handle is
/// consumed unless the transaction can retry an unknown outcome: then it
/// stays live so [`type_bridge_staged_transaction_commit_retry`] can resend
/// the same request. A null owner (already consumed) is `INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_commit(
    transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { finish_commit(transaction, out_diagnostics, false) }
}

/// Resend the exact request of a commit whose outcome is unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_commit_retry(
    transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { finish_commit(transaction, out_diagnostics, true) }
}

unsafe fn finish_commit(
    transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    retry: bool,
) -> TypeBridgeStatus {
    // SAFETY: forwarded owner and diagnostics slot contract.
    let value = match unsafe { prepare_terminal_outputs(transaction, out_diagnostics) } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if value.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // Take the owner before dispatch: a panic then drops it and leaves
        // the slot null instead of naming a hollow handle.
        // SAFETY: the slot owned this live handle.
        let mut handle = unsafe { Box::from_raw(value) };
        // SAFETY: the owner slot was fenced and is writable.
        unsafe { transaction.write_unaligned(ptr::null_mut()) };
        let outcome = {
            let tx = &mut handle.transaction;
            handle.database.block_on(async {
                if retry {
                    tx.commit_retry().await
                } else {
                    tx.commit().await
                }
            })
        };
        if handle.transaction.has_unknown_outcome() {
            // SAFETY: as above; the same owner returns for commit_retry.
            unsafe { transaction.write_unaligned(Box::into_raw(handle)) };
        } else {
            drop(handle);
        }
        match outcome {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(error) => return_execution_error(error.diagnostic(), out_diagnostics),
        }
    })
}

/// Discard every staged plan and consume the handle. A null owner is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_rollback(
    transaction: *mut *mut TypeBridgeStagedTransaction,
) -> TypeBridgeStatus {
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: the owner slot is readable by contract.
    let value = unsafe { transaction.read_unaligned() };
    if value.is_null() {
        return TypeBridgeStatus::Ok;
    }
    // SAFETY: the owner slot is writable; it is cleared before the drop.
    unsafe { transaction.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        // SAFETY: the slot owned this live handle.
        let mut handle = unsafe { Box::from_raw(value) };
        // Deferred: nothing reached the provider, so rollback cannot fail
        // in a way the caller can act on.
        let _ = handle.transaction.rollback();
        TypeBridgeStatus::Ok
    })
}

/// Close (and roll back) a staged transaction. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_close(
    transaction: *mut *mut TypeBridgeStagedTransaction,
) {
    if transaction.is_null() {
        return;
    }
    // SAFETY: the owner slot is readable by contract.
    let value = unsafe { transaction.read_unaligned() };
    if value.is_null() {
        return;
    }
    // SAFETY: the owner slot is writable; it is cleared before the drop.
    unsafe { transaction.write_unaligned(ptr::null_mut()) };
    let _ = guarded(|| {
        // SAFETY: the slot owned this live handle.
        let mut handle = unsafe { Box::from_raw(value) };
        let _ = handle.transaction.close();
        TypeBridgeStatus::Ok
    });
}

fn answer_status(error: &PendingError) -> TypeBridgeStatus {
    match error {
        PendingError::NotCommitted => TypeBridgeStatus::InUse,
        // The commit already reported the typed failure; rollback and close
        // discarded the answer for good.
        _ => TypeBridgeStatus::ExecutionFailed,
    }
}

fn answer(pending: &TypeBridgePending) -> Result<Arc<PlanAnswer>, TypeBridgeStatus> {
    pending.pending.get().map_err(|error| answer_status(&error))
}

/// Report the resolved answer kind and, for count, exists (0/1), matched
/// and rows, its scalar (`rows` reports the row count).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_answer(
    pending: *const TypeBridgePending,
    out_kind: *mut u32,
    out_value: *mut u64,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (out_kind.cast(), size_of::<u32>()),
        (out_value.cast(), size_of::<u64>()),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = check_object(&outputs, pending) {
        return status;
    }
    if !out_kind.is_null() {
        // SAFETY: the slot is writable and overlaps no fenced input.
        unsafe { out_kind.write_unaligned(0) };
    }
    if !out_value.is_null() {
        // SAFETY: as above.
        unsafe { out_value.write_unaligned(0) };
    }
    if pending.is_null() || out_kind.is_null() || out_value.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: a non-null pending handle is live by contract.
        let answer = match answer(unsafe { &*pending }) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let (kind, value) = match answer.as_ref() {
            PlanAnswer::Rows(rows) => (
                TypeBridgePlanAnswerKind::Rows,
                u64::try_from(rows.len()).unwrap_or(u64::MAX),
            ),
            PlanAnswer::Count(count) => (TypeBridgePlanAnswerKind::Count, *count),
            PlanAnswer::Exists(exists) => (TypeBridgePlanAnswerKind::Exists, u64::from(*exists)),
            PlanAnswer::Matched(matched) => (TypeBridgePlanAnswerKind::Matched, *matched),
            #[allow(unreachable_patterns)]
            _ => return TypeBridgeStatus::Unsupported,
        };
        // SAFETY: both outputs were fenced and initialized above.
        unsafe {
            out_kind.write_unaligned(kind as u32);
            out_value.write_unaligned(value);
        }
        TypeBridgeStatus::Ok
    })
}

/// Free a pending handle and null its owner slot. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_free(pending: *mut *mut TypeBridgePending) {
    // SAFETY: forwarded owner-slot contract.
    unsafe { free_slot(pending) }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use type_bridge_contract::capability::CapabilityId;
    use type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary;
    use type_bridge_contract::query_remote::{
        RemoteCapabilities, RemoteExecutorBinding, RemoteSigningPublicKey,
    };
    use type_bridge_contract::query_remote_v3::{
        CAP_QUERY_REMOTE_IDEMPOTENCY_V3, CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3,
    };
    use type_bridge_contract::sdk_diagnostic::{SdkCommitFailureOutcome, SdkProviderOperation};
    use type_bridge_orm::session::backend::QueryResult;
    use type_bridge_orm::{ClassifiedCommitError, CommitFailureCertainty, TxType};

    use super::*;
    use crate::entity_crud::tests::{
        COMMIT_CONFLICT, COMMIT_CONFLICT_MESSAGE, COMMIT_PANIC, COMMIT_SUCCESS, COMMIT_UNKNOWN,
        Fixture, Response, close_diagnostics, diagnostic, fixture,
    };
    use crate::execution_diagnostic::{
        empty_detail_view, empty_diagnostic_view, execution_diagnostics_handle,
        type_bridge_execution_diagnostics_close, type_bridge_execution_diagnostics_detail_get_v1,
        type_bridge_execution_diagnostics_get_v1,
    };
    use crate::runtime::type_bridge_database_close;

    const SCHEMA: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/schema.json");
    const PURCHASE: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/purchase.json");
    const ROWS: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/rows.json");
    const COUNT: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/count.json");
    const EXISTS: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/exists.json");
    const BATCH: &[u8] = include_bytes!("../../../../tests/contracts/plan_v3/batch.json");
    const HEADER: &str = include_str!("../include/typebridge/type_bridge.h");

    type Diagnostics = *mut TypeBridgeExecutionDiagnostics;

    fn decode(plan: &[u8]) -> *mut TypeBridgePlan {
        let mut handle = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let status = unsafe {
            type_bridge_plan_v3_decode(
                SCHEMA.as_ptr(),
                SCHEMA.len(),
                plan.as_ptr(),
                plan.len(),
                &mut handle,
                &mut diagnostics,
            )
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert!(!handle.is_null());
        handle
    }

    fn open(fixture: &Fixture, kind: u32) -> *mut TypeBridgeStagedTransaction {
        let mut transaction = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let status = unsafe {
            type_bridge_database_transaction(
                &*fixture.database,
                kind,
                &mut transaction,
                &mut diagnostics,
            )
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        transaction
    }

    fn stage(
        transaction: *mut TypeBridgeStagedTransaction,
        plan: *const TypeBridgePlan,
    ) -> *mut TypeBridgePending {
        let mut pending = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let status = unsafe {
            type_bridge_staged_transaction_stage(transaction, plan, &mut pending, &mut diagnostics)
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        pending
    }

    fn read(pending: *const TypeBridgePending) -> (TypeBridgeStatus, u32, u64) {
        let (mut kind, mut value) = (u32::MAX, u64::MAX);
        let status = unsafe { type_bridge_pending_answer(pending, &mut kind, &mut value) };
        (status, kind, value)
    }

    fn rows(count: usize) -> Response {
        Response::Result(QueryResult::Rows(
            (0..count)
                .map(|row| serde_json::json!({ "row": row }))
                .collect(),
        ))
    }

    fn commit(
        transaction: &mut *mut TypeBridgeStagedTransaction,
    ) -> (TypeBridgeStatus, Diagnostics) {
        let mut diagnostics = ptr::dangling_mut();
        let status =
            unsafe { type_bridge_staged_transaction_commit(transaction, &mut diagnostics) };
        (status, diagnostics)
    }

    fn close_database(database: &mut *mut TypeBridgeDatabase) -> TypeBridgeStatus {
        let mut diagnostics = ptr::dangling_mut();
        let status = unsafe { type_bridge_database_close(database, &mut diagnostics) };
        if !diagnostics.is_null() {
            close_diagnostics(&mut diagnostics);
        }
        status
    }

    /// Category, path count and detail keys as an ABI 1.6 consumer reads them.
    fn c_shape(handle: *const TypeBridgeExecutionDiagnostics) -> (u32, usize, Vec<String>) {
        let mut view = empty_diagnostic_view();
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_get_v1(handle, 0, &mut view) },
            TypeBridgeStatus::Ok
        );
        let keys = (0..view.detail_count)
            .map(|index| {
                let mut detail = empty_detail_view();
                assert_eq!(
                    unsafe {
                        type_bridge_execution_diagnostics_detail_get_v1(
                            handle,
                            0,
                            index,
                            &mut detail,
                        )
                    },
                    TypeBridgeStatus::Ok
                );
                let key = unsafe { std::slice::from_raw_parts(detail.key.data, detail.key.length) };
                String::from_utf8(key.to_vec()).unwrap()
            })
            .collect();
        (view.category as u32, view.path_count, keys)
    }

    #[test]
    fn kind_values_match_the_header() {
        for (name, value) in [
            (
                "TYPE_BRIDGE_TRANSACTION_READ 1u",
                TYPE_BRIDGE_TRANSACTION_READ,
            ),
            (
                "TYPE_BRIDGE_TRANSACTION_WRITE 2u",
                TYPE_BRIDGE_TRANSACTION_WRITE,
            ),
        ] {
            assert!(HEADER.contains(&format!("#define {name}")), "{name}");
            assert!(name.ends_with(&format!(" {value}u")));
        }
        for (name, kind) in [
            ("ROWS", TypeBridgePlanAnswerKind::Rows),
            ("COUNT", TypeBridgePlanAnswerKind::Count),
            ("EXISTS", TypeBridgePlanAnswerKind::Exists),
            ("MATCHED", TypeBridgePlanAnswerKind::Matched),
        ] {
            let line = format!("TYPE_BRIDGE_PLAN_ANSWER_{name} = {}", kind as u32);
            assert!(HEADER.contains(&line), "{line}");
        }
        assert_eq!(size_of::<TypeBridgePlanAnswerKind>(), size_of::<u32>());
    }

    #[test]
    fn decode_round_trips_canonical_bytes_and_reports_diagnostics() {
        let mut plan = decode(PURCHASE);
        let mut length = usize::MAX;
        assert_eq!(
            unsafe { type_bridge_plan_v3_canonical_bytes(plan, ptr::null_mut(), 0, &mut length) },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(length, PURCHASE.len());
        let mut short = vec![0xa5; PURCHASE.len() - 1];
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(
                    plan,
                    short.as_mut_ptr(),
                    short.len(),
                    &mut length,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(length, PURCHASE.len());
        assert!(
            short.iter().all(|byte| *byte == 0xa5),
            "a short buffer stays untouched"
        );
        let mut exact = vec![0; PURCHASE.len()];
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(
                    plan,
                    exact.as_mut_ptr(),
                    exact.len(),
                    &mut length,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(exact, PURCHASE, "the contract fixture bytes round-trip");
        // Null handles and outputs.
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(plan, ptr::null_mut(), 0, ptr::null_mut())
            },
            TypeBridgeStatus::InvalidArgument
        );
        length = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(ptr::null(), ptr::null_mut(), 0, &mut length)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(length, 0, "out_len is initialized on failure");

        // Contract failures carry a diagnostic and no handle.
        let mut noncanonical = PURCHASE.to_vec();
        noncanonical.push(b'\n');
        for (schema, bytes) in [
            (SCHEMA, b"{}".as_slice()),
            (SCHEMA, noncanonical.as_slice()),
            (b"{}".as_slice(), PURCHASE),
        ] {
            let mut handle = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            let status = unsafe {
                type_bridge_plan_v3_decode(
                    schema.as_ptr(),
                    schema.len(),
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut handle,
                    &mut diagnostics,
                )
            };
            assert_eq!(status, TypeBridgeStatus::InvalidArgument);
            assert!(handle.is_null());
            let (_, code, message) = diagnostic(diagnostics);
            assert!(!code.is_empty());
            assert!(message.ends_with("are invalid"), "{message}");
            close_diagnostics(&mut diagnostics);
        }

        // A null buffer with a length clears both outputs and writes no diagnostic.
        let mut handle = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_decode(
                    ptr::null(),
                    1,
                    PURCHASE.as_ptr(),
                    PURCHASE.len(),
                    &mut handle,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(handle.is_null() && diagnostics.is_null());

        unsafe { type_bridge_plan_free(&mut plan) };
        assert!(plan.is_null());
        unsafe { type_bridge_plan_free(&mut plan) };
        unsafe { type_bridge_plan_free(ptr::null_mut()) };
    }

    #[test]
    fn hostile_outputs_are_rejected_before_any_write() {
        // Two outputs naming one slot, or an output inside an input buffer.
        let mut shared = ptr::dangling_mut::<TypeBridgePlan>();
        let sentinel = shared;
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_decode(
                    SCHEMA.as_ptr(),
                    SCHEMA.len(),
                    PURCHASE.as_ptr(),
                    PURCHASE.len(),
                    &mut shared,
                    (&raw mut shared).cast(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(shared, sentinel);
        let mut input = PURCHASE.to_vec();
        let mut diagnostics = ptr::dangling_mut();
        let inside = unsafe { input.as_mut_ptr().add(3) }.cast::<*mut TypeBridgePlan>();
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_decode(
                    SCHEMA.as_ptr(),
                    SCHEMA.len(),
                    input.as_ptr(),
                    input.len(),
                    inside,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(input, PURCHASE);
        assert_eq!(diagnostics, ptr::dangling_mut());

        // Unaligned, non-overlapping slots are accepted.
        let mut slots = [0u8; 2 * size_of::<usize>() + 2];
        let out_plan = unsafe { slots.as_mut_ptr().add(1) }.cast::<*mut TypeBridgePlan>();
        let out_diagnostics =
            unsafe { slots.as_mut_ptr().add(1 + size_of::<usize>()) }.cast::<Diagnostics>();
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_decode(
                    SCHEMA.as_ptr(),
                    SCHEMA.len(),
                    PURCHASE.as_ptr(),
                    PURCHASE.len(),
                    out_plan,
                    out_diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let plan = unsafe { out_plan.read_unaligned() };
        assert!(!plan.is_null());
        assert!(unsafe { out_diagnostics.read_unaligned() }.is_null());

        // canonical_bytes: buffer over out_len, or over the plan's own bytes.
        let mut storage = [0u8; 64];
        let out_len = storage.as_mut_ptr().cast::<usize>();
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(
                    plan,
                    storage.as_mut_ptr(),
                    storage.len(),
                    out_len,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(storage, [0; 64]);
        let own = unsafe { &*plan }.canonical.as_ptr().cast_mut();
        let mut length = usize::MAX;
        assert_eq!(
            unsafe { type_bridge_plan_v3_canonical_bytes(plan, own, PURCHASE.len(), &mut length) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(length, usize::MAX);
        assert_eq!(
            unsafe {
                type_bridge_plan_v3_canonical_bytes(
                    plan,
                    plan.cast(),
                    size_of::<TypeBridgePlan>(),
                    &mut length,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(length, usize::MAX);

        // Staging into the transaction or plan handle storage.
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let profile = fixture.package.state.semantic_profile.as_ptr();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_transaction(
                    &*fixture.database,
                    TYPE_BRIDGE_TRANSACTION_WRITE,
                    profile.cast_mut().cast(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostics, ptr::dangling_mut());
        let mut transaction = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        for out_pending in [
            transaction.cast::<*mut TypeBridgePending>(),
            plan.cast::<*mut TypeBridgePending>(),
            profile.cast_mut().cast(),
        ] {
            assert_eq!(
                unsafe {
                    type_bridge_staged_transaction_stage(
                        transaction,
                        plan,
                        out_pending,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert_eq!(diagnostics, ptr::dangling_mut());
        let pending = stage(transaction, plan);

        // Commit with diagnostics over the owner slot or the live handle.
        let original = transaction;
        assert_eq!(
            unsafe {
                type_bridge_staged_transaction_commit(
                    &mut transaction,
                    (&raw mut transaction).cast(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { type_bridge_staged_transaction_commit(&mut transaction, original.cast()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(transaction, original);
        assert_eq!(fixture.state.query_count(), 0);

        // pending_answer: overlapping outputs, or outputs over the handle.
        let mut answer = [0xffu8; 12];
        assert_eq!(
            unsafe {
                type_bridge_pending_answer(
                    pending,
                    answer.as_mut_ptr().cast(),
                    answer.as_mut_ptr().add(2).cast(),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(answer, [0xff; 12]);
        let mut kind = u32::MAX;
        assert_eq!(
            unsafe { type_bridge_pending_answer(pending, &mut kind, pending.cast()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(kind, u32::MAX);
        // Unaligned, disjoint outputs are initialized before IN_USE.
        let mut unaligned = [0xffu8; 13];
        assert_eq!(
            unsafe {
                type_bridge_pending_answer(
                    pending,
                    unaligned.as_mut_ptr().add(1).cast(),
                    unaligned.as_mut_ptr().add(5).cast(),
                )
            },
            TypeBridgeStatus::InUse
        );
        assert_eq!(&unaligned[1..13], &[0; 12]);

        let mut pending = pending;
        unsafe { type_bridge_staged_transaction_close(&mut transaction) };
        assert!(transaction.is_null());
        unsafe { type_bridge_pending_free(&mut pending) };
        // Frees accept the same unaligned owner slot and clear it.
        unsafe { type_bridge_plan_free(out_plan) };
        assert!(unsafe { out_plan.read_unaligned() }.is_null());
    }

    #[test]
    fn staged_answers_resolve_after_one_commit() {
        let fixture = fixture(vec![rows(3), rows(2), rows(1), rows(2)], COMMIT_SUCCESS);
        let mut plans = [ROWS, COUNT, EXISTS, BATCH].map(decode);
        let mut transaction = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut pending = plans.map(|plan| stage(transaction, plan));
        // Plans outlive nothing they staged: free them before commit.
        for plan in &mut plans {
            unsafe { type_bridge_plan_free(plan) };
        }
        assert_eq!(read(pending[0]), (TypeBridgeStatus::InUse, 0, 0));
        assert_eq!(fixture.state.query_count(), 0, "staging sends nothing");
        assert!(fixture.state.opened_transactions().is_empty());

        let (status, diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(transaction.is_null() && diagnostics.is_null());
        assert_eq!(fixture.state.opened_transactions(), vec![TxType::Write]);
        assert_eq!(fixture.state.query_count(), 4);
        assert_eq!(
            fixture
                .state
                .commits
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
        // Answers outlive the consumed transaction.
        let expected = [
            (TypeBridgePlanAnswerKind::Rows, 3),
            (TypeBridgePlanAnswerKind::Count, 2),
            (TypeBridgePlanAnswerKind::Exists, 1),
            (TypeBridgePlanAnswerKind::Matched, 2),
        ];
        for (handle, (kind, value)) in pending.iter().zip(expected) {
            assert_eq!(read(*handle), (TypeBridgeStatus::Ok, kind as u32, value));
        }
        assert_eq!(
            unsafe { type_bridge_pending_answer(pending[0], ptr::null_mut(), ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(read(ptr::null()), (TypeBridgeStatus::InvalidArgument, 0, 0));
        for handle in &mut pending {
            unsafe { type_bridge_pending_free(handle) };
            assert!(handle.is_null());
            unsafe { type_bridge_pending_free(handle) };
        }
        unsafe { type_bridge_pending_free(ptr::null_mut()) };
    }

    #[test]
    fn transaction_kinds_and_staging_refusals() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut transaction = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        for kind in [0, 3, u32::MAX] {
            assert_eq!(
                unsafe {
                    type_bridge_database_transaction(
                        &*fixture.database,
                        kind,
                        &mut transaction,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::InvalidArgument
            );
            assert!(transaction.is_null() && diagnostics.is_null());
        }
        assert_eq!(
            unsafe {
                type_bridge_database_transaction(
                    ptr::null(),
                    TYPE_BRIDGE_TRANSACTION_READ,
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );

        let mut plan = decode(BATCH);
        let mut read_only = open(&fixture, TYPE_BRIDGE_TRANSACTION_READ);
        let mut pending = ptr::dangling_mut();
        let status = unsafe {
            type_bridge_staged_transaction_stage(read_only, plan, &mut pending, &mut diagnostics)
        };
        assert_eq!(status, TypeBridgeStatus::InvalidArgument);
        assert!(pending.is_null());
        assert_eq!(diagnostic(diagnostics).1, "staged_read_only");
        close_diagnostics(&mut diagnostics);
        for (transaction, plan) in [
            (read_only, ptr::null()),
            (ptr::null_mut(), plan.cast_const()),
        ] {
            assert_eq!(
                unsafe {
                    type_bridge_staged_transaction_stage(
                        transaction,
                        plan,
                        &mut pending,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::InvalidArgument
            );
            assert!(pending.is_null() && diagnostics.is_null());
        }
        // Rollback consumes a write transaction with staged work.
        let mut purchase = decode(PURCHASE);
        let mut write = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut staged = stage(write, purchase);
        unsafe {
            type_bridge_staged_transaction_close(&mut read_only);
            type_bridge_staged_transaction_rollback(&mut write);
            type_bridge_plan_free(&mut plan);
            type_bridge_plan_free(&mut purchase);
        }
        assert!(read_only.is_null() && write.is_null());
        // Rollback discarded the staged answer for good.
        assert_eq!(read(staged), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        unsafe { type_bridge_pending_free(&mut staged) };
        assert!(fixture.state.opened_transactions().is_empty());
    }

    #[test]
    fn lifecycle_is_in_use_until_consumed_and_frees_are_idempotent() {
        let Fixture {
            database, state, ..
        } = fixture(vec![rows(1)], COMMIT_SUCCESS);
        let mut database = Box::into_raw(database);
        let mut plan = decode(ROWS);
        let mut transaction = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_transaction(
                    database,
                    TYPE_BRIDGE_TRANSACTION_READ,
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut pending = stage(transaction, plan);
        let original = database;
        assert_eq!(close_database(&mut database), TypeBridgeStatus::InUse);
        assert_eq!(database, original, "a busy database keeps its owner");

        let (status, diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(transaction.is_null() && diagnostics.is_null());
        assert_eq!(state.opened_transactions(), vec![TxType::Read]);
        // Use after consume: commit and retry reject the null owner; rollback,
        // close and the frees are no-ops.
        for retry in [false, true] {
            let mut diagnostics = ptr::dangling_mut();
            let status = unsafe {
                if retry {
                    type_bridge_staged_transaction_commit_retry(&mut transaction, &mut diagnostics)
                } else {
                    type_bridge_staged_transaction_commit(&mut transaction, &mut diagnostics)
                }
            };
            assert_eq!(status, TypeBridgeStatus::InvalidArgument);
            assert!(diagnostics.is_null(), "the diagnostics output is cleared");
        }
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe { type_bridge_staged_transaction_commit(ptr::null_mut(), &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { type_bridge_staged_transaction_commit(&mut transaction, ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(&mut transaction) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        unsafe {
            type_bridge_staged_transaction_close(&mut transaction);
            type_bridge_staged_transaction_close(ptr::null_mut());
        }
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        assert!(database.is_null());
        // The answer and plan outlive the database.
        assert_eq!(
            read(pending),
            (
                TypeBridgeStatus::Ok,
                TypeBridgePlanAnswerKind::Rows as u32,
                1
            )
        );
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_pending_free(&mut pending);
            type_bridge_plan_free(&mut plan);
            type_bridge_plan_free(&mut plan);
        }
        assert!(pending.is_null() && plan.is_null());
    }

    #[test]
    fn commit_failures_consume_the_handle_and_keep_the_released_detail_set() {
        // A direct provider commit cannot retry: unknown still consumes.
        let fixture_unknown = fixture(vec![rows(1)], COMMIT_UNKNOWN);
        let plan = decode(BATCH);
        let mut transaction = open(&fixture_unknown, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut pending = stage(transaction, plan);
        let (status, mut diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::CommitOutcomeUnknown);
        assert!(transaction.is_null());
        assert_eq!(diagnostic(diagnostics).1, "commit_outcome_unknown");
        close_diagnostics(&mut diagnostics);
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        unsafe { type_bridge_pending_free(&mut pending) };

        // A typed commit rejection reaches C with the ABI 1.6 detail set.
        assert!(
            ClassifiedCommitError::Driver {
                certainty: CommitFailureCertainty::DefinitelyAborted,
                message: COMMIT_CONFLICT_MESSAGE.to_owned(),
            }
            .failure()
            .is_some(),
            "the fake rejection carries a typed failure"
        );
        let conflict = fixture(vec![rows(2)], COMMIT_CONFLICT);
        let mut transaction = open(&conflict, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut pending = stage(transaction, plan);
        let (status, mut diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::ExecutionFailed);
        assert!(transaction.is_null());
        let mut released = Box::into_raw(Box::new(execution_diagnostics_handle(vec![
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted),
        ])));
        assert_eq!(c_shape(diagnostics), c_shape(released));
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_close(&mut released) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        unsafe { type_bridge_pending_free(&mut pending) };

        // A provider panic consumes the owner and releases the database.
        let Fixture { database, .. } = fixture(vec![rows(1)], COMMIT_PANIC);
        let mut database = Box::into_raw(database);
        let mut transaction = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_transaction(
                    database,
                    TYPE_BRIDGE_TRANSACTION_WRITE,
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut pending = stage(transaction, plan);
        let (status, diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::Panic);
        assert!(transaction.is_null() && diagnostics.is_null());
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        unsafe { type_bridge_pending_free(&mut pending) };
        let mut plan = plan;
        unsafe { type_bridge_plan_free(&mut plan) };
    }

    #[test]
    fn retryable_unknown_outcome_keeps_the_handle_for_commit_retry() {
        let Fixture { database, .. } = fixture(vec![], COMMIT_SUCCESS);
        let mut database = Box::into_raw(database);
        let mut transaction = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_transaction(
                    database,
                    TYPE_BRIDGE_TRANSACTION_WRITE,
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        // Every reply is lost, so each attempt's outcome stays unknown.
        let sent = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let log = Arc::clone(&sent);
        let exchange: type_bridge_orm::session::StagedExchangeFn = Arc::new(move |bytes| {
            log.lock().unwrap().push(bytes);
            Box::pin(async {
                Err(SdkExecutionDiagnostic::provider_failure(
                    SdkProviderOperation::Write,
                ))
            })
        });
        let mut capabilities = query_plan_v3_capability_vocabulary();
        for capability in [
            CAP_QUERY_REMOTE_STAGED_V3,
            CAP_QUERY_REMOTE_WRITE_V3,
            CAP_QUERY_REMOTE_IDEMPOTENCY_V3,
        ] {
            capabilities.insert(CapabilityId::new(capability).unwrap());
        }
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("c-staged-executor-01", "c-staged-epoch-0001").unwrap(),
            RemoteSigningPublicKey::from_bytes([7; 32]),
        )
        .encode()
        .unwrap();
        // SAFETY: the handle is live and unshared; only its staging backend changes.
        unsafe { &mut *transaction }.transaction = Transaction::over_proxy(
            decode_declared_schema(SCHEMA).unwrap(),
            &advertisement,
            exchange,
            TransactionOptions::default(),
        )
        .unwrap();
        let mut plan = decode(PURCHASE);
        let mut pending = stage(transaction, plan);
        let original = transaction;

        let (status, mut diagnostics) = commit(&mut transaction);
        assert_eq!(status, TypeBridgeStatus::CommitOutcomeUnknown);
        assert_eq!(transaction, original, "a retryable outcome keeps the owner");
        close_diagnostics(&mut diagnostics);
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        assert_eq!(close_database(&mut database), TypeBridgeStatus::InUse);
        // A plain commit cannot abandon the unknown attempt.
        let (status, mut diagnostics) = commit(&mut transaction);
        assert_ne!(status, TypeBridgeStatus::Ok);
        assert_eq!(transaction, original);
        close_diagnostics(&mut diagnostics);

        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_staged_transaction_commit_retry(&mut transaction, &mut diagnostics)
            },
            TypeBridgeStatus::CommitOutcomeUnknown
        );
        assert_eq!(transaction, original);
        close_diagnostics(&mut diagnostics);
        {
            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), 2, "only commit and commit_retry send");
            assert_eq!(sent[0], sent[1], "the retry resends the identical request");
        }

        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(&mut transaction) },
            TypeBridgeStatus::Ok
        );
        assert!(transaction.is_null());
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_plan_free(&mut plan);
        }
    }
}
