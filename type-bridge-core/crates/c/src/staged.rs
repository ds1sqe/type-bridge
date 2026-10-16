//! ABI 1.7 deferred transactions: decoded v3 plans are staged on one
//! transaction and nothing reaches the provider until commit.
//!
//! C never serializes plans. A plan handle is decoded from canonical
//! `query-plan/v3` bytes against declared schema bytes, so its bytes are the
//! same ones the Rust builders produce.

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
use crate::runtime::{StagedDatabaseGuard, TypeBridgeDatabase, initialize_handle_outputs};

/// Opaque validated `query-plan/v3` plan.
pub struct TypeBridgePlan {
    plan: QueryPlanV3,
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

/// Answer kinds a staged plan resolves to.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgePlanAnswerKind {
    Rows = 1,
    Count = 2,
    Exists = 3,
    Matched = 4,
}

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
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_plan, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if out_plan.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
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
        let plan = match decode_query_plan_v3(plan, &schema) {
            Ok(value) => value,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "the plan bytes are invalid"),
                    out_diagnostics,
                );
            }
        };
        // SAFETY: the output slot was proven writable above.
        unsafe { out_plan.write(Box::into_raw(Box::new(TypeBridgePlan { plan }))) };
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
    guarded(|| {
        if plan.is_null() || out_len.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: a non-null plan is a live handle by contract.
        let Ok(encoded) = (unsafe { &*plan }).plan.canonical_bytes() else {
            return TypeBridgeStatus::ExecutionFailed;
        };
        // SAFETY: out_len is writable by contract.
        unsafe { out_len.write(encoded.len()) };
        if buffer.is_null() || capacity < encoded.len() {
            return TypeBridgeStatus::ResourceLimit;
        }
        // SAFETY: buffer names `capacity >= encoded.len()` writable bytes.
        unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), buffer, encoded.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Free a plan handle and null its owner slot. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_free(plan: *mut *mut TypeBridgePlan) {
    if plan.is_null() {
        return;
    }
    // SAFETY: the owner slot holds null or a handle this library allocated.
    unsafe { drop(Box::from_raw_opt(plan.replace(std::ptr::null_mut()))) };
}

trait FromRawOpt<T> {
    unsafe fn from_raw_opt(raw: *mut T) -> Option<Box<T>>;
}

impl<T> FromRawOpt<T> for Box<T> {
    unsafe fn from_raw_opt(raw: *mut T) -> Option<Box<T>> {
        // SAFETY: forwarded caller contract.
        (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
    }
}

/// Transaction kinds accepted by [`type_bridge_database_transaction`].
pub const TYPE_BRIDGE_TRANSACTION_READ: u32 = 1;
pub const TYPE_BRIDGE_TRANSACTION_WRITE: u32 = 2;

/// Open one deferred transaction. No provider transaction opens until commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_transaction(
    database: *const TypeBridgeDatabase,
    kind: u32,
    out_transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        let kind = match kind {
            TYPE_BRIDGE_TRANSACTION_READ => TransactionKind::Read,
            TYPE_BRIDGE_TRANSACTION_WRITE => TransactionKind::Write,
            _ => return TypeBridgeStatus::InvalidArgument,
        };
        if database.is_null() || out_transaction.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: a non-null database is a live handle by contract.
        let database = unsafe { &*database };
        let (transaction, database) =
            database.staged_transaction(TransactionOptions::default().with_kind(kind));
        let handle = Box::new(TypeBridgeStagedTransaction {
            transaction,
            database,
        });
        // SAFETY: the output slot was proven writable above.
        unsafe { out_transaction.write(Box::into_raw(handle)) };
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
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_pending, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if transaction.is_null() || plan.is_null() || out_pending.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: non-null handles are live and not shared across threads by contract.
        let (transaction, plan) = unsafe { (&mut *transaction, &*plan) };
        match transaction.transaction.stage_plan(&plan.plan) {
            Ok(pending) => {
                // SAFETY: the output slot was proven writable above.
                unsafe {
                    out_pending.write(Box::into_raw(Box::new(TypeBridgePending { pending })))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(pending_diagnostic(&error), out_diagnostics),
        }
    })
}

/// Commit every staged plan in one provider transaction. The handle is
/// consumed unless the outcome is unknown: then it stays live so
/// [`type_bridge_staged_transaction_commit_retry`] can resend the same request.
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
    let mut unused: *mut TypeBridgePending = std::ptr::null_mut();
    // SAFETY: only the diagnostics slot is initialized here.
    if let Err(status) = unsafe { initialize_handle_outputs(&mut unused, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        // SAFETY: the owner slot holds null or a live handle by contract.
        if transaction.is_null() || unsafe { (*transaction).is_null() } {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: checked non-null above.
        let handle = unsafe { &mut **transaction };
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
        let keep = handle.transaction.has_unknown_outcome();
        if !keep {
            // SAFETY: the slot owns the handle; consume it.
            unsafe { drop(Box::from_raw(transaction.replace(std::ptr::null_mut()))) };
        }
        match outcome {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(error) => return_execution_error(error.diagnostic(), out_diagnostics),
        }
    })
}

/// Discard every staged plan and consume the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_rollback(
    transaction: *mut *mut TypeBridgeStagedTransaction,
) -> TypeBridgeStatus {
    guarded(|| {
        if transaction.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the owner slot holds null or a handle this library allocated.
        if let Some(mut handle) =
            unsafe { Box::from_raw_opt(transaction.replace(std::ptr::null_mut())) }
        {
            // Deferred: nothing reached the provider, so rollback cannot fail
            // in a way the caller can act on.
            let _ = handle.transaction.rollback();
        }
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
    // SAFETY: the owner slot holds null or a handle this library allocated.
    if let Some(mut handle) =
        unsafe { Box::from_raw_opt(transaction.replace(std::ptr::null_mut())) }
    {
        let _ = handle.transaction.close();
    }
}

fn answer(pending: *const TypeBridgePending) -> Result<Arc<PlanAnswer>, TypeBridgeStatus> {
    if pending.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: a non-null pending handle is live by contract.
    (unsafe { &*pending })
        .pending
        .get()
        .map_err(|_| TypeBridgeStatus::InUse)
}

/// Report the resolved answer kind and, for count, exists (0/1), matched
/// and rows, its scalar (`rows` reports the row count).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_answer(
    pending: *const TypeBridgePending,
    out_kind: *mut TypeBridgePlanAnswerKind,
    out_value: *mut u64,
) -> TypeBridgeStatus {
    guarded(|| {
        if out_kind.is_null() || out_value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let answer = match answer(pending) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let (kind, value) = match answer.as_ref() {
            PlanAnswer::Rows(rows) => (TypeBridgePlanAnswerKind::Rows, rows.len() as u64),
            PlanAnswer::Count(count) => (TypeBridgePlanAnswerKind::Count, *count),
            PlanAnswer::Exists(exists) => (TypeBridgePlanAnswerKind::Exists, u64::from(*exists)),
            PlanAnswer::Matched(matched) => (TypeBridgePlanAnswerKind::Matched, *matched),
            #[allow(unreachable_patterns)]
            _ => return TypeBridgeStatus::Unsupported,
        };
        // SAFETY: both outputs are writable by contract.
        unsafe {
            out_kind.write(kind);
            out_value.write(value);
        }
        TypeBridgeStatus::Ok
    })
}

/// Free a pending handle and null its owner slot. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_free(pending: *mut *mut TypeBridgePending) {
    if pending.is_null() {
        return;
    }
    // SAFETY: the owner slot holds null or a handle this library allocated.
    unsafe { drop(Box::from_raw_opt(pending.replace(std::ptr::null_mut()))) };
}
