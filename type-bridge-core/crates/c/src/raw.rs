//! Unchecked TypeQL over the shared direct/proxy raw lifecycle.

use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use type_bridge_orm::session::raw::{RawQueryError, RawTransaction};
use type_bridge_orm::{CommitFailureCertainty, TxType};

use crate::abi::{TypeBridgeByteView, TypeBridgeDiagnostics, TypeBridgeStatus, close_box, guarded};
use crate::allocation::{AllocationSite, ReservedBox};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::query::TypeBridgeProxyConnection;
use crate::runtime::{TypeBridgeDatabase, check_database_borrowed_ranges};
use crate::staged::StagedOwner;

/// Owned provider JSON; its borrowed view remains valid until close.
pub struct TypeBridgeRawResult {
    bytes: Vec<u8>,
}

/// An eager direct raw transaction or a single-query proxy context.
pub struct TypeBridgeRawTransaction {
    inner: RawTransaction,
    owner: StagedOwner,
    poisoned: bool,
}

impl Drop for TypeBridgeRawTransaction {
    fn drop(&mut self) {
        // Close never commits; retain the runtime until provider cleanup finishes.
        let _ = catch_unwind(AssertUnwindSafe(|| self.owner.block_on(self.inner.close())));
    }
}

impl TypeBridgeRawTransaction {
    fn execute(&mut self, text: &str) -> Result<Vec<u8>, RawQueryError> {
        self.require_usable()?;
        self.poisoned = true;
        let result = self.owner.block_on(async {
            let bytes = self.inner.execute_query(text).await?;
            self.inner.decode_answer(&bytes)?;
            Ok(bytes)
        });
        self.poisoned = false;
        result
    }

    fn require_usable(&self) -> Result<(), RawQueryError> {
        if self.poisoned {
            Err(local_error(
                "raw_transaction_poisoned",
                "raw transaction panicked; close it",
            ))
        } else {
            Ok(())
        }
    }

    fn commit(&mut self) -> Result<(), RawQueryError> {
        self.require_usable()?;
        self.poisoned = true;
        let result = self.owner.block_on(self.inner.commit());
        self.poisoned = false;
        result
    }

    fn rollback(&mut self) -> Result<(), RawQueryError> {
        self.require_usable()?;
        self.poisoned = true;
        let result = self.owner.block_on(self.inner.rollback());
        self.poisoned = false;
        result
    }

    fn one(&mut self, text: &str) -> Result<Vec<u8>, RawQueryError> {
        let result = self
            .execute(text)
            .and_then(|bytes| self.commit().map(|()| bytes));
        let closed = self.owner.block_on(self.inner.close());
        let bytes = result?;
        closed?;
        Ok(bytes)
    }
}

fn local_error(code: &str, message: &str) -> RawQueryError {
    RawQueryError {
        code: code.into(),
        message: message.into(),
        status: 500,
        provider_code: None,
        commit_outcome: None,
    }
}

unsafe fn publish_error(
    error: RawQueryError,
    out: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let status = if error.commit_outcome == Some(CommitFailureCertainty::Unknown) {
        TypeBridgeStatus::CommitOutcomeUnknown
    } else {
        TypeBridgeStatus::ExecutionFailed
    };
    let certainty = error.commit_outcome.map(|value| match value {
        CommitFailureCertainty::Unknown => "unknown",
        CommitFailureCertainty::DefinitelyAborted => "definitely_aborted",
    });
    // Raw diagnostics preserve provider fields without the typed diagnostic limits/redaction.
    let encoded = serde_json::to_vec(&serde_json::json!([{
        "category": "provider", "code": error.code, "message": error.message, "path": [],
        "details": {"status": error.status, "provider_code": error.provider_code, "commit_outcome": certainty}
    }])).expect("raw errors contain JSON primitives");
    // SAFETY: all callers preflight and initialize this output slot.
    unsafe { out.write_unaligned(Box::into_raw(Box::new(TypeBridgeDiagnostics { encoded }))) };
    status
}

#[derive(Clone, Copy)]
enum Target {
    Database(*const TypeBridgeDatabase),
    Proxy(*const TypeBridgeProxyConnection),
}

impl Target {
    unsafe fn check(self, outputs: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        match self {
            Self::Database(value) => {
                if value.is_null() {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
                outputs.check_bytes(value.cast(), size_of::<TypeBridgeDatabase>())?;
                // SAFETY: caller retains a complete database for the call.
                check_database_borrowed_ranges(outputs, unsafe { &*value })
            }
            Self::Proxy(value) => {
                if value.is_null() {
                    return Err(TypeBridgeStatus::InvalidArgument);
                }
                outputs.check_bytes(value.cast(), size_of::<TypeBridgeProxyConnection>())?;
                // SAFETY: caller retains a complete connection for the call.
                unsafe { &*value }.check_borrowed_ranges(outputs)
            }
        }
    }

    unsafe fn open(self, kind: TxType) -> Result<TypeBridgeRawTransaction, RawQueryError> {
        let (inner, owner) = match self {
            Self::Database(value) => {
                // SAFETY: preflight checked the live, nonnull database.
                let (inner, guard) = unsafe { &*value }.raw_transaction(kind)?;
                (inner, StagedOwner::Database(guard))
            }
            Self::Proxy(value) => {
                // SAFETY: preflight checked the live, nonnull connection.
                let (inner, guard) = unsafe { &*value }.raw_transaction(kind)?;
                (inner, StagedOwner::Proxy(guard))
            }
        };
        Ok(TypeBridgeRawTransaction {
            inner,
            owner,
            poisoned: false,
        })
    }
}

fn kind(value: u32) -> Result<TxType, TypeBridgeStatus> {
    match value {
        1 => Ok(TxType::Read),
        2 => Ok(TxType::Write),
        3 => Ok(TxType::Schema),
        _ => Err(TypeBridgeStatus::InvalidArgument),
    }
}

unsafe fn outputs<T>(
    out: *mut *mut T,
    errors: *mut *mut TypeBridgeDiagnostics,
    inputs: impl FnOnce(&DirectOutputPreflight) -> Result<(), TypeBridgeStatus>,
) -> Result<(), TypeBridgeStatus> {
    let preflight = direct_output_preflight(&[
        (out.cast(), size_of::<*mut T>()),
        (errors.cast(), size_of::<*mut TypeBridgeDiagnostics>()),
    ])?;
    inputs(&preflight)?;
    if out.is_null() || errors.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: complete input graph and both disjoint output slots were fenced first.
    unsafe {
        out.write_unaligned(ptr::null_mut());
        errors.write_unaligned(ptr::null_mut());
    }
    Ok(())
}

unsafe fn text<'a>(value: TypeBridgeByteView) -> Result<&'a str, TypeBridgeStatus> {
    let bytes = if value.length == 0 {
        &[]
    } else {
        if value.data.is_null() || value.length > isize::MAX as usize {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        // SAFETY: caller retains the input byte range for this call.
        unsafe { std::slice::from_raw_parts(value.data, value.length) }
    };
    std::str::from_utf8(bytes).map_err(|_| TypeBridgeStatus::InvalidArgument)
}

unsafe fn open(
    target: Target,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: target and outputs follow the public contract.
    if let Err(status) = unsafe { outputs(out, errors, |preflight| target.check(preflight)) } {
        return status;
    }
    guarded(|| {
        let kind = match kind(transaction_kind) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let reservation = match ReservedBox::try_new(AllocationSite::RawTransactionHandle) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::ResourceLimit,
        };
        // SAFETY: target was preflighted and retained throughout open.
        match unsafe { target.open(kind) } {
            Ok(value) => {
                // SAFETY: output was fenced and initialized.
                unsafe { out.write_unaligned(Box::into_raw(reservation.initialize(value))) };
                TypeBridgeStatus::Ok
            }
            // SAFETY: error output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

unsafe fn execute_one(
    target: Target,
    query: TypeBridgeByteView,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawResult,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller ranges remain valid throughout this call.
    if let Err(status) = unsafe {
        outputs(out, errors, |preflight| {
            target.check(preflight)?;
            preflight.check_bytes(query.data.cast(), query.length)
        })
    } {
        return status;
    }
    guarded(|| {
        let kind = match kind(transaction_kind) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: query is borrowed only during the call.
        let query = match unsafe { text(query) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let reservation = match ReservedBox::try_new(AllocationSite::RawResultHandle) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::ResourceLimit,
        };
        // SAFETY: target was checked before output initialization.
        let result =
            unsafe { target.open(kind) }.and_then(|mut transaction| transaction.one(query));
        match result {
            Ok(bytes) => {
                // SAFETY: output was fenced and initialized before provider work.
                unsafe {
                    out.write_unaligned(Box::into_raw(
                        reservation.initialize(TypeBridgeRawResult { bytes }),
                    ))
                };
                TypeBridgeStatus::Ok
            }
            // SAFETY: error output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

/// Open an eager raw transaction; close discards uncommitted direct work.
/// # Safety
/// Inputs are live; outputs are writable and disjoint from all inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_raw_transaction(
    database: *const TypeBridgeDatabase,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { open(Target::Database(database), transaction_kind, out, errors) }
}

/// Open a proxy raw context; at most one query is dispatched.
/// # Safety
/// Inputs are live; outputs are writable and disjoint from all inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_raw_transaction(
    proxy: *const TypeBridgeProxyConnection,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { open(Target::Proxy(proxy), transaction_kind, out, errors) }
}

/// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
/// to TypeDB as written, and TypeDB's output comes back as it
/// answered, in this SDK's native data types. TypeBridge adds no
/// validation, rewriting, hydration, hooks, limits or extra
/// queries. It runs in read, write and schema transactions and
/// accepts any TypeQL, schema-breaking changes included. The caller
/// owns its correctness; TypeBridge makes no promise about its
/// effects. On a proxy connection it posts to the server's opt-in
/// `/v3/raw` endpoint, which is off by default.
/// # Safety
/// Inputs are live; outputs are writable and disjoint from all inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_execute_query(
    database: *const TypeBridgeDatabase,
    query: TypeBridgeByteView,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawResult,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        execute_one(
            Target::Database(database),
            query,
            transaction_kind,
            out,
            errors,
        )
    }
}

/// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
/// to TypeDB as written, and TypeDB's output comes back as it
/// answered, in this SDK's native data types. TypeBridge adds no
/// validation, rewriting, hydration, hooks, limits or extra
/// queries. It runs in read, write and schema transactions and
/// accepts any TypeQL, schema-breaking changes included. The caller
/// owns its correctness; TypeBridge makes no promise about its
/// effects. On a proxy connection it posts to the server's opt-in
/// `/v3/raw` endpoint, which is off by default.
/// # Safety
/// Inputs are live; outputs are writable and disjoint from all inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_execute_query(
    proxy: *const TypeBridgeProxyConnection,
    query: TypeBridgeByteView,
    transaction_kind: u32,
    out: *mut *mut TypeBridgeRawResult,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { execute_one(Target::Proxy(proxy), query, transaction_kind, out, errors) }
}

unsafe fn check_transaction(
    value: *const TypeBridgeRawTransaction,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    if value.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    preflight.check_bytes(value.cast(), size_of::<TypeBridgeRawTransaction>())?;
    // SAFETY: caller retains a complete live raw transaction.
    unsafe { &*value }.owner.check_borrowed_ranges(preflight)
}

/// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
/// to TypeDB as written, and TypeDB's output comes back as it
/// answered, in this SDK's native data types. TypeBridge adds no
/// validation, rewriting, hydration, hooks, limits or extra
/// queries. It runs in read, write and schema transactions and
/// accepts any TypeQL, schema-breaking changes included. The caller
/// owns its correctness; TypeBridge makes no promise about its
/// effects. On a proxy connection it posts to the server's opt-in
/// `/v3/raw` endpoint, which is off by default.
/// # Safety
/// Transaction is exclusively borrowed; text and output slots remain live and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_transaction_execute_query(
    transaction: *mut TypeBridgeRawTransaction,
    query: TypeBridgeByteView,
    out: *mut *mut TypeBridgeRawResult,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains all inputs throughout execution.
    if let Err(status) = unsafe {
        outputs(out, errors, |preflight| {
            check_transaction(transaction, preflight)?;
            preflight.check_bytes(query.data.cast(), query.length)
        })
    } {
        return status;
    }
    guarded(|| {
        // SAFETY: caller retains query bytes throughout the call.
        let query = match unsafe { text(query) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let reservation = match ReservedBox::try_new(AllocationSite::RawResultHandle) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::ResourceLimit,
        };
        // SAFETY: complete nonnull transaction is exclusively borrowed for this call.
        match unsafe { &mut *transaction }.execute(query) {
            Ok(bytes) => {
                // SAFETY: output was fenced and initialized.
                unsafe {
                    out.write_unaligned(Box::into_raw(
                        reservation.initialize(TypeBridgeRawResult { bytes }),
                    ))
                };
                TypeBridgeStatus::Ok
            }
            // SAFETY: error output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

unsafe fn finish(
    transaction: *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
    commit: bool,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[(
        errors.cast(),
        size_of::<*mut TypeBridgeDiagnostics>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: caller retains the complete transaction.
    if let Err(status) = unsafe { check_transaction(transaction, &preflight) } {
        return status;
    }
    if errors.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: output was checked against the transaction graph.
    unsafe { errors.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        // SAFETY: caller exclusively borrows the preflighted nonnull transaction.
        let transaction = unsafe { &mut *transaction };
        match if commit {
            transaction.commit()
        } else {
            transaction.rollback()
        } {
            Ok(()) => TypeBridgeStatus::Ok,
            // SAFETY: output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

/// Commit raw direct work; a completed proxy write is already committed.
/// # Safety
/// Exclusively borrow the live transaction and a disjoint writable error slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_transaction_commit(
    transaction: *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { finish(transaction, errors, true) }
}

/// Roll back direct work; a completed proxy write refuses rollback.
/// # Safety
/// Exclusively borrow the live transaction and a disjoint writable error slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_transaction_rollback(
    transaction: *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { finish(transaction, errors, false) }
}

/// Close and clear a raw transaction without committing; repeated close is safe.
/// # Safety
/// The owner slot and diagnostics output are writable, disjoint, and uniquely owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_transaction_close(
    transaction: *mut *mut TypeBridgeRawTransaction,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        if transaction.is_null() || errors.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let preflight = match direct_output_preflight(&[
            (
                transaction.cast(),
                size_of::<*mut TypeBridgeRawTransaction>(),
            ),
            (errors.cast(), size_of::<*mut TypeBridgeDiagnostics>()),
        ]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller uniquely owns this readable/writable slot.
        let value = unsafe { transaction.read_unaligned() };
        if !value.is_null() {
            // SAFETY: the nonnull slot owns a live raw transaction.
            if let Err(status) = unsafe { check_transaction(value, &preflight) } {
                return status;
            }
        }
        // SAFETY: output slots were fenced against each other and the complete input.
        unsafe {
            transaction.write_unaligned(ptr::null_mut());
            errors.write_unaligned(ptr::null_mut());
        }
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // SAFETY: ownership moved out of the unique caller slot.
        let mut value = unsafe { Box::from_raw(value) };
        match value.owner.block_on(value.inner.close()) {
            Ok(()) => TypeBridgeStatus::Ok,
            // SAFETY: error output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

/// Borrow exact provider JSON bytes until the result is closed.
/// # Safety
/// Result is live; output is writable and disjoint from the result and its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_result_json(
    result: *const TypeBridgeRawResult,
    out: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight =
            match direct_output_preflight(&[(out.cast(), size_of::<TypeBridgeByteView>())]) {
                Ok(value) => value,
                Err(status) => return status,
            };
        if result.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        if let Err(status) = preflight.check_bytes(result.cast(), size_of::<TypeBridgeRawResult>())
        {
            return status;
        }
        // SAFETY: caller retains the complete result during this borrow.
        let bytes = &unsafe { &*result }.bytes;
        if let Err(status) = preflight.check_bytes(bytes.as_ptr().cast(), bytes.len()) {
            return status;
        }
        // SAFETY: output is disjoint from the result and every borrowed byte.
        unsafe {
            out.write_unaligned(TypeBridgeByteView {
                data: bytes.as_ptr(),
                length: bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Free a raw result and clear its owner slot; repeated close is safe.
/// # Safety
/// Caller uniquely owns a writable pointer slot containing a live result or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_raw_result_close(
    result: *mut *mut TypeBridgeRawResult,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract.
    unsafe { close_box(result) }
}

/// Execute unchecked TypeQL in the engine-owned migration callback transaction.
/// The returned result uses the ordinary raw JSON ownership contract.
/// # Safety
/// The callback transaction, text and disjoint output slots remain live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_transaction_execute_query(
    transaction: *const crate::runtime::TypeBridgeWriteTransaction,
    query: TypeBridgeByteView,
    out: *mut *mut TypeBridgeRawResult,
    errors: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: caller retains all inputs throughout execution.
    if let Err(status) = unsafe {
        outputs(out, errors, |preflight| {
            if transaction.is_null() {
                return Err(TypeBridgeStatus::InvalidArgument);
            }
            preflight.check_bytes(
                transaction.cast(),
                size_of::<crate::runtime::TypeBridgeWriteTransaction>(),
            )?;
            crate::runtime::check_write_transaction_borrowed_ranges(preflight, &*transaction)?;
            preflight.check_bytes(query.data.cast(), query.length)
        })
    } {
        return status;
    }
    guarded(|| {
        // SAFETY: complete live handle and text were checked above.
        let transaction = unsafe { &*transaction };
        let query = match unsafe { text(query) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        if !transaction.is_engine_owned() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let reservation = match ReservedBox::try_new(AllocationSite::RawResultHandle) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::ResourceLimit,
        };
        let result = match transaction.context() {
            Some(context) => transaction.block_on(context.execute_query(query)),
            None => Err(local_error(
                "raw_transaction_closed",
                "migration transaction is closed",
            )),
        };
        match result {
            Ok(bytes) => {
                // SAFETY: output was fenced and initialized.
                unsafe {
                    out.write_unaligned(Box::into_raw(
                        reservation.initialize(TypeBridgeRawResult { bytes }),
                    ))
                };
                TypeBridgeStatus::Ok
            }
            // SAFETY: output was fenced and initialized.
            Err(error) => unsafe { publish_error(error, errors) },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_crud::tests::{byte_view, package};
    use std::sync::{Arc, Mutex};
    use type_bridge_orm::OrmError;
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };

    #[derive(Clone)]
    struct Backend(Arc<Mutex<Vec<String>>>);
    impl DriverBackend for Backend {
        fn is_open(&self) -> bool {
            true
        }
        fn open_transaction(
            &self,
            _: &str,
            _: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            panic!("raw calls must not use typed transaction admission")
        }
        fn open_raw_transaction(
            &self,
            _: &str,
            kind: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, RawQueryError>> {
            self.0.lock().unwrap().push(format!("open:{kind:?}"));
            let tx: Box<dyn TransactionOps> = Box::new(self.clone());
            Box::pin(async move { Ok(tx) })
        }
    }
    impl TransactionOps for Backend {
        fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            panic!("typed query")
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed commit")
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed rollback")
        }
        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed close")
        }
        fn query_raw<'a>(
            &'a mut self,
            query: &'a str,
        ) -> BoxFuture<'a, Result<Vec<u8>, RawQueryError>> {
            self.0.lock().unwrap().push(query.into());
            Box::pin(async move {
                if query == "provider failure" {
                    Err(RawQueryError {
                        code: "provider.CODE/23".into(),
                        message: "verbatim provider endpoint and secret".repeat(1000),
                        status: 409,
                        provider_code: Some("original".into()),
                        commit_outcome: Some(CommitFailureCertainty::DefinitelyAborted),
                    })
                } else {
                    Ok(br#" {"integer":18446744073709551615,"fraction":1.2300} "#.to_vec())
                }
            })
        }
        fn commit_raw(&mut self) -> BoxFuture<'_, Result<(), RawQueryError>> {
            self.0.lock().unwrap().push("commit".into());
            Box::pin(async { Ok(()) })
        }
        fn close_raw(&mut self) -> BoxFuture<'_, Result<(), RawQueryError>> {
            self.0.lock().unwrap().push("close".into());
            Box::pin(async { Ok(()) })
        }
    }

    fn database() -> (TypeBridgeDatabase, Arc<Mutex<Vec<String>>>) {
        let package = package("raw");
        let events = Arc::new(Mutex::new(Vec::new()));
        let database =
            crate::runtime::test_orm_database(Box::new(Backend(events.clone())), "raw", None);
        (
            TypeBridgeDatabase::from_test_database(Arc::clone(package.state()), database),
            events,
        )
    }

    unsafe fn error_json(errors: *mut TypeBridgeDiagnostics) -> serde_json::Value {
        // SAFETY: test owns the returned diagnostic handle.
        let value = serde_json::from_slice(&unsafe { &*errors }.encoded).unwrap();
        let mut errors = errors;
        // SAFETY: return the unique diagnostic owner.
        assert_eq!(
            unsafe { crate::abi::type_bridge_diagnostics_close(&mut errors) },
            TypeBridgeStatus::Ok
        );
        value
    }

    #[test]
    fn direct_raw_pair_preserves_bytes_and_eager_lifecycle_for_all_kinds() {
        for kind in 1..=3 {
            let (database, events) = database();
            let mut transaction = ptr::null_mut();
            let mut errors = ptr::null_mut();
            // SAFETY: fixture and all outputs are live and disjoint.
            unsafe {
                assert_eq!(
                    type_bridge_database_raw_transaction(
                        &database,
                        kind,
                        &mut transaction,
                        &mut errors
                    ),
                    TypeBridgeStatus::Ok
                );
                for query in ["schema-breaking text", "second exact query"] {
                    let mut result = ptr::null_mut();
                    assert_eq!(
                        type_bridge_raw_transaction_execute_query(
                            transaction,
                            byte_view(query.as_bytes()),
                            &mut result,
                            &mut errors
                        ),
                        TypeBridgeStatus::Ok
                    );
                    let mut view = byte_view(b"");
                    assert_eq!(
                        type_bridge_raw_result_json(result, &mut view),
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(
                        std::slice::from_raw_parts(view.data, view.length),
                        br#" {"integer":18446744073709551615,"fraction":1.2300} "#
                    );
                    assert_eq!(
                        type_bridge_raw_result_json(result, view.data.cast_mut().cast()),
                        TypeBridgeStatus::InvalidArgument
                    );
                    assert_eq!(
                        type_bridge_raw_result_close(&mut result),
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(
                        type_bridge_raw_result_close(&mut result),
                        TypeBridgeStatus::Ok
                    );
                }
                assert_eq!(events.lock().unwrap().len(), 3);
                assert_eq!(
                    type_bridge_raw_transaction_commit(transaction, &mut errors),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    type_bridge_raw_transaction_close(&mut transaction, &mut errors),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    type_bridge_raw_transaction_close(&mut transaction, &mut errors),
                    TypeBridgeStatus::Ok
                );
            }
            assert_eq!(
                &events.lock().unwrap()[1..],
                [
                    "schema-breaking text",
                    "second exact query",
                    if kind == 1 { "close" } else { "commit" }
                ]
            );
        }
    }

    #[test]
    fn database_pair_commits_once_and_raw_errors_keep_unbounded_provider_details() {
        let (database, events) = database();
        let mut result = ptr::null_mut();
        let mut errors = ptr::null_mut();
        // SAFETY: fixture and writable output slots remain disjoint and live.
        unsafe {
            assert_eq!(
                type_bridge_database_execute_query(
                    &database,
                    byte_view(b"unchecked"),
                    2,
                    &mut result,
                    &mut errors
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_raw_result_close(&mut result),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_database_execute_query(
                    &database,
                    byte_view(b"provider failure"),
                    2,
                    &mut result,
                    &mut errors
                ),
                TypeBridgeStatus::ExecutionFailed
            );
            assert!(result.is_null());
            let json = error_json(errors);
            assert_eq!(json[0]["code"], "provider.CODE/23");
            assert_eq!(
                json[0]["message"],
                "verbatim provider endpoint and secret".repeat(1000)
            );
            assert_eq!(json[0]["details"]["status"], 409);
            assert_eq!(json[0]["details"]["commit_outcome"], "definitely_aborted");
        }
        assert_eq!(
            &*events.lock().unwrap(),
            &[
                "open:Write",
                "unchecked",
                "commit",
                "open:Write",
                "provider failure",
                "close"
            ]
        );
    }

    #[test]
    fn proxy_raw_reuses_shared_single_query_and_unknown_decode_rules() {
        for (bytes, expected) in [
            (b"{\"ok\":true}".as_slice(), TypeBridgeStatus::Ok),
            (b"not json", TypeBridgeStatus::CommitOutcomeUnknown),
        ] {
            let (database, _) = database();
            let (_, owner) = database.staged_transaction(Default::default()).unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let captured = calls.clone();
            let bytes = bytes.to_vec();
            let inner = RawTransaction::proxy(
                Arc::new(move |request| {
                    captured.lock().unwrap().push(request);
                    let bytes = bytes.clone();
                    Box::pin(async move { Ok(bytes) })
                }),
                TxType::Write,
                std::num::NonZeroU64::new(60_000).unwrap(),
            );
            let mut transaction = Box::into_raw(Box::new(TypeBridgeRawTransaction {
                inner,
                owner: StagedOwner::Database(owner),
                poisoned: false,
            }));
            let mut result = ptr::null_mut();
            let mut errors = ptr::null_mut();
            // SAFETY: test uniquely owns the transaction and output slots.
            unsafe {
                assert_eq!(
                    type_bridge_raw_transaction_execute_query(
                        transaction,
                        byte_view(b"unchanged raw text"),
                        &mut result,
                        &mut errors
                    ),
                    expected
                );
                if expected == TypeBridgeStatus::Ok {
                    assert_eq!(
                        type_bridge_raw_result_close(&mut result),
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(
                        type_bridge_raw_transaction_rollback(transaction, &mut errors),
                        TypeBridgeStatus::ExecutionFailed
                    );
                    assert_eq!(
                        error_json(errors)[0]["code"],
                        "raw_transaction_already_committed"
                    );
                } else {
                    assert_eq!(error_json(errors)[0]["code"], "commit_outcome_unknown");
                }
                assert_eq!(
                    type_bridge_raw_transaction_execute_query(
                        transaction,
                        byte_view(b"second"),
                        &mut result,
                        &mut errors
                    ),
                    TypeBridgeStatus::ExecutionFailed
                );
                assert_eq!(
                    error_json(errors)[0]["code"],
                    "raw_transaction_single_query"
                );
                assert_eq!(
                    type_bridge_raw_transaction_close(&mut transaction, &mut errors),
                    TypeBridgeStatus::Ok
                );
            }
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&calls[0]).unwrap()["query"],
                "unchanged raw text"
            );
        }
    }

    #[test]
    fn raw_aliases_and_result_reservation_fail_before_provider_dispatch() {
        let (database, events) = database();
        let mut text = [b'x'; 64];
        let original = text;
        let mut errors = ptr::dangling_mut();
        // SAFETY: intentional output/input alias is rejected without writes.
        unsafe {
            assert_eq!(
                type_bridge_database_execute_query(
                    &database,
                    byte_view(&text),
                    2,
                    text.as_mut_ptr().cast(),
                    &mut errors
                ),
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert_eq!(text, original);
        assert_eq!(errors, ptr::dangling_mut());
        let mut result = ptr::dangling_mut();
        let _failure = crate::allocation::inject_failure(AllocationSite::RawResultHandle, 0);
        // SAFETY: valid disjoint output slots; the reservation fails before open.
        unsafe {
            assert_eq!(
                type_bridge_database_execute_query(
                    &database,
                    byte_view(b"unchecked"),
                    2,
                    &mut result,
                    &mut errors
                ),
                TypeBridgeStatus::ResourceLimit
            );
        }
        assert!(result.is_null());
        assert!(errors.is_null());
        assert!(events.lock().unwrap().is_empty());
    }
}
