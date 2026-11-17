//! ABI 2.0 deferred transactions: decoded plans are staged on one
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

use crate::staged_answers::PendingState;
use std::mem::size_of;
use std::ptr;
use std::sync::{Arc, Weak};
use tracing::Instrument as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::{QueryPlan, decode_query_plan};
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::session::staged::{
    PendingError, PlanAnswer, Transaction, TransactionKind, TransactionOptions,
};

use crate::abi::{TypeBridgeStatus, guarded};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::query::{StagedProxyGuard, TypeBridgeProxyConnection};
use crate::runtime::{
    StagedDatabaseGuard, TypeBridgeDatabase, check_database_borrowed_ranges,
    poisoned_transaction_diagnostic,
};

/// Configuration for a deferred transaction. A null options pointer keeps defaults.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeStagedTransactionOptionsV1 {
    /// Exact size of this struct.
    pub struct_size: u32,
    /// Must equal 1.
    pub version: u32,
    /// Positive execution duration; zero or negative means no deadline.
    pub timeout_milliseconds: i64,
    /// Exactly zero (off) or one (on).
    pub collect_phase_timestamps: u32,
}

impl TypeBridgeStagedTransactionOptionsV1 {
    fn options(self) -> Result<TransactionOptions, TypeBridgeStatus> {
        if self.struct_size as usize != size_of::<Self>()
            || self.version != 1
            || self.collect_phase_timestamps > 1
        {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        Ok(TransactionOptions::default()
            .with_timeout(type_bridge_orm::QueryTimeout::from_signed_milliseconds(
                self.timeout_milliseconds,
            ))
            .with_collect_phase_timestamps(self.collect_phase_timestamps == 1))
    }
}

/// Opaque validated `query-plan/v3` plan.
pub struct TypeBridgePlan {
    pub(crate) source: Arc<CompiledRawPlan>,
}
pub(crate) struct CompiledRawPlan {
    pub(crate) plan: QueryPlan,
    pub(crate) canonical: Vec<u8>,
}
impl AsRef<[QueryPlan]> for CompiledRawPlan {
    fn as_ref(&self) -> &[QueryPlan] {
        std::slice::from_ref(&self.plan)
    }
}
impl TypeBridgePlan {
    pub(crate) fn new(plan: QueryPlan, canonical: Vec<u8>) -> Self {
        Self {
            source: Arc::new(CompiledRawPlan { plan, canonical }),
        }
    }
    pub(crate) fn from_shared(source: Arc<CompiledRawPlan>) -> Self {
        Self { source }
    }
}

/// Opaque deferred transaction: a reusable logical transaction. Commit and
/// rollback keep it; only close releases it.
pub struct TypeBridgeStagedTransaction {
    transaction: Transaction,
    // Set while a commit runs and left set by a contained panic, so a
    // half-run commit admits only rollback and close.
    poisoned: bool,
    // Dropped after the transaction so the database stays busy until then.
    owner: StagedOwner,
    pending_views: Vec<Weak<PendingState>>,
}

/// What a staged transaction runs on: a direct database, or a proxy
/// connection's I/O runtime.
pub(crate) enum StagedOwner {
    Database(StagedDatabaseGuard),
    Proxy(StagedProxyGuard),
}

impl StagedOwner {
    fn package_state(&self) -> &Arc<crate::abi::SchemaPackageState> {
        match self {
            Self::Database(guard) => guard.package_state(),
            Self::Proxy(guard) => guard.package_state(),
        }
    }

    fn projected_origin(&self) -> type_bridge_orm::ProjectedQueryOrigin {
        match self {
            Self::Database(guard) => guard.projected_origin(),
            Self::Proxy(_) => type_bridge_orm::ProjectedQueryOrigin::remote_unbound(),
        }
    }

    fn debug_dispatch(&self) -> Option<tracing::Dispatch> {
        match self {
            Self::Database(guard) => guard.debug_dispatch(),
            Self::Proxy(_) => None,
        }
    }
    pub(crate) fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        match self {
            Self::Database(guard) => guard.block_on(future),
            Self::Proxy(guard) => guard.block_on(future),
        }
    }

    pub(crate) fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        match self {
            Self::Database(guard) => guard.check_borrowed_ranges(preflight),
            Self::Proxy(guard) => guard.check_borrowed_ranges(preflight),
        }
    }
}

impl TypeBridgeStagedTransaction {
    pub(crate) fn package_state(&self) -> &Arc<crate::abi::SchemaPackageState> {
        self.owner.package_state()
    }

    pub(crate) fn stage_query(
        &mut self,
        registry: &type_bridge_orm::_registry::DescriptorRegistry,
        request: Arc<type_bridge_orm::ValidatedMatchRequest>,
        limits: type_bridge_orm::MatchExecutionLimits,
    ) -> Result<
        (
            type_bridge_orm::session::staged::Pending<type_bridge_orm::ValidatedMatchResult>,
            type_bridge_orm::ProjectedQueryOrigin,
        ),
        SdkExecutionDiagnostic,
    > {
        if self.poisoned {
            return Err(poisoned_transaction_diagnostic());
        }
        let dispatch = self.owner.debug_dispatch();
        let pending = match dispatch {
            Some(dispatch) => tracing::dispatcher::with_default(&dispatch, || {
                self.transaction
                    .stage_shared_match_with_limits(registry, request, limits)
            }),
            None => self
                .transaction
                .stage_shared_match_with_limits(registry, request, limits),
        }
        .map_err(|error| pending_diagnostic(&error))?;
        Ok((pending, self.owner.projected_origin()))
    }
}

/// Opaque staged answer, readable after a successful commit.
#[repr(C)]
pub struct TypeBridgePending {
    pub(crate) tag: u64,
    pub(crate) state: Arc<PendingState>,
}

/// Answer kinds a staged plan resolves to (`type_bridge_plan_answer_kind_t`).
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeBridgePlanAnswerKind {
    Rows = 1,
    Count = 2,
    Exists = 3,
    Matched = 4,
    Documents = 5,
}

/// Transaction kinds accepted by [`type_bridge_database_transaction`].
pub const TYPE_BRIDGE_TRANSACTION_READ: u32 = 1;
pub const TYPE_BRIDGE_TRANSACTION_WRITE: u32 = 2;

pub(crate) fn invalid(code: &str, message: &'static str) -> SdkExecutionDiagnostic {
    let code = SdkDiagnosticCode::new(code)
        .or_else(|_| SdkDiagnosticCode::new("invalid_plan"))
        .expect("static diagnostic code is canonical");
    let message = SdkDiagnosticMessage::new(message).expect("static message is canonical");
    SdkExecutionDiagnostic::invalid_input(code, message)
}

pub(crate) fn contract_error(
    diagnostic: &Diagnostic,
    message: &'static str,
) -> SdkExecutionDiagnostic {
    invalid(diagnostic.code().as_str(), message)
}

pub(crate) fn pending_diagnostic(error: &PendingError) -> SdkExecutionDiagnostic {
    match error {
        PendingError::Failed { diagnostic, .. } | PendingError::Refused { diagnostic, .. } => {
            diagnostic.as_ref().clone()
        }
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
        let canonical = &unsafe { &*plan }.source.canonical;
        preflight.check_bytes(canonical.as_ptr().cast(), canonical.len())?;
    }
    Ok(())
}

/// # Safety
/// A non-null `transaction` is a live handle.
pub(crate) unsafe fn check_transaction(
    preflight: &DirectOutputPreflight,
    transaction: *const TypeBridgeStagedTransaction,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, transaction)?;
    if !transaction.is_null() {
        // SAFETY: the complete live handle was fenced above.
        unsafe { &*transaction }
            .owner
            .check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

/// Fence one handle constructor's two outputs against each other and its
/// inputs, then clear both. Nothing is written when the fence fails.
///
/// # Safety
/// Each non-null output names one writable, possibly unaligned, pointer slot.
pub(crate) unsafe fn prepare_handle_outputs<T>(
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

/// Fence a commit's diagnostics output against the live handle and the
/// bytes it retains, then clear it.
///
/// # Safety
/// A non-null `transaction` is a live handle; a non-null diagnostics slot is
/// writable and may be unaligned.
unsafe fn prepare_commit_outputs(
    transaction: *const TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    let outputs = direct_output_preflight(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )])?;
    // SAFETY: a non-null transaction names a live handle by contract.
    unsafe { check_transaction(&outputs, transaction) }?;
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: the slot is writable and overlaps no fenced input.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    if transaction.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(())
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
pub unsafe extern "C" fn type_bridge_plan_decode(
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
        let decoded = decode_query_plan(plan, &schema)
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
        let handle = Box::new(TypeBridgePlan::new(plan, canonical));
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_plan.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Explain a bound plan as owned canonical JSON, excluding given values.
/// Close the returned buffer with `type_bridge_canonical_bytes_close`.
///
/// # Safety
/// The plan must be live. Output slots must be writable and must not overlap
/// each other, the plan, or its retained canonical bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_explain(
    plan: *const TypeBridgePlan,
    out_bytes: *mut *mut crate::canonical_archive::TypeBridgeCanonicalBytes,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: validate and initialize caller outputs before reading the plan.
    if let Err(status) = unsafe {
        prepare_handle_outputs(out_bytes, out_diagnostics, |outputs| {
            check_plan(outputs, plan)
        })
    } {
        return status;
    }
    if plan.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: a non-null plan is live by the caller contract.
        let plan = unsafe { &*plan };
        let bytes = match type_bridge_orm::plan_explain::explain(&plan.source.plan)
            .and_then(|explanation| explanation.canonical_bytes())
        {
            Ok(bytes) => bytes,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "the plan cannot be explained"),
                    out_diagnostics,
                );
            }
        };
        let handle = match crate::allocation::try_box(
            crate::allocation::AllocationSite::CanonicalBytesHandle,
            crate::canonical_archive::TypeBridgeCanonicalBytes { bytes },
        ) {
            Ok(handle) => handle,
            Err(_) => {
                return return_execution_error(
                    crate::allocation::allocation_exhausted(),
                    out_diagnostics,
                );
            }
        };
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_bytes.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Write the canonical bytes of `plan` into a caller buffer. With a null or
/// short buffer, only the required length is reported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_canonical_bytes(
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
    let encoded = &unsafe { &*plan }.source.canonical;
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
pub(crate) unsafe fn free_slot<T>(slot: *mut *mut T) {
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
    // SAFETY: forwards the same handle/output contracts with default options.
    unsafe {
        type_bridge_database_transaction_with_options(
            database,
            kind,
            ptr::null(),
            out_transaction,
            out_diagnostics,
        )
    }
}

/// Open a deferred transaction with an explicit timeout and phase collection policy.
/// A null options pointer preserves the default 60000 ms and collection off.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_transaction_with_options(
    database: *const TypeBridgeDatabase,
    kind: u32,
    options: *const TypeBridgeStagedTransactionOptionsV1,
    out_transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        check_object(outputs, options)?;
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
        let options = if options.is_null() {
            TransactionOptions::default()
        } else {
            // SAFETY: the options were fenced against both outputs above.
            match unsafe { options.read_unaligned() }.options() {
                Ok(options) => options,
                Err(status) => return status,
            }
        };
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
        let (transaction, database) = match database.staged_transaction(options.with_kind(kind)) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let handle = Box::new(TypeBridgeStagedTransaction {
            transaction,
            poisoned: false,
            owner: StagedOwner::Database(database),
            pending_views: Vec::new(),
        });
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_transaction.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Open one deferred transaction on a proxy connection. The server's staged
/// advertisement is fetched now and must name the connection's executor and
/// reply key; nothing else is sent until commit, which is one signed
/// exchange carrying every staged plan. The transaction may outlive the
/// connection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_transaction(
    proxy: *const TypeBridgeProxyConnection,
    kind: u32,
    out_transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwards the same handle/output contracts with default options.
    unsafe {
        type_bridge_proxy_connection_transaction_with_options(
            proxy,
            kind,
            ptr::null(),
            out_transaction,
            out_diagnostics,
        )
    }
}

/// Open a deferred transaction with an explicit timeout and phase collection policy.
/// A null options pointer preserves the default 60000 ms and collection off.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_transaction_with_options(
    proxy: *const TypeBridgeProxyConnection,
    kind: u32,
    options: *const TypeBridgeStagedTransactionOptionsV1,
    out_transaction: *mut *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        check_object(outputs, options)?;
        check_object(outputs, proxy)?;
        if !proxy.is_null() {
            // SAFETY: the complete connection handle was fenced above.
            unsafe { &*proxy }.check_borrowed_ranges(outputs)?;
        }
        Ok(())
    };
    // SAFETY: the outputs are fenced against the connection and its retained
    // package bytes first.
    if let Err(status) = unsafe { prepare_handle_outputs(out_transaction, out_diagnostics, inputs) }
    {
        return status;
    }
    guarded(|| {
        let options = if options.is_null() {
            TransactionOptions::default()
        } else {
            // SAFETY: the options were fenced against both outputs above.
            match unsafe { options.read_unaligned() }.options() {
                Ok(options) => options,
                Err(status) => return status,
            }
        };
        let kind = match kind {
            TYPE_BRIDGE_TRANSACTION_READ => TransactionKind::Read,
            TYPE_BRIDGE_TRANSACTION_WRITE => TransactionKind::Write,
            _ => return TypeBridgeStatus::InvalidArgument,
        };
        if proxy.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: a non-null connection is a live handle by contract.
        let proxy = unsafe { &*proxy };
        let (transaction, guard) = match proxy.staged_transaction(options.with_kind(kind)) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let handle = Box::new(TypeBridgeStagedTransaction {
            transaction,
            poisoned: false,
            owner: StagedOwner::Proxy(guard),
            pending_views: Vec::new(),
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
    let _entry = crate::perf_stages::entry().entered();
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
        if transaction.poisoned {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        let dispatch = transaction.owner.debug_dispatch();
        let pending = match dispatch {
            Some(dispatch) => tracing::dispatcher::with_default(&dispatch, || {
                transaction
                    .transaction
                    .stage_shared_plan(Arc::clone(&plan.source), 0)
            }),
            None => transaction
                .transaction
                .stage_shared_plan(Arc::clone(&plan.source), 0),
        };
        match pending {
            Ok(pending) => {
                let state = Arc::new(PendingState::new(pending, &plan.source.plan));
                transaction.pending_views.push(Arc::downgrade(&state));
                let handle = Box::new(TypeBridgePending {
                    tag: crate::staged_answers::PENDING_TAG,
                    state,
                });
                // SAFETY: the output slot was fenced and initialized above.
                unsafe { out_pending.write_unaligned(Box::into_raw(handle)) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(pending_diagnostic(&error), out_diagnostics),
        }
    })
}

/// Stage a borrowed generated terminal. The first typed stage freezes its recording.
/// Repeated staging retains the same lazily compiled source; the pending may
/// outlive the recording. A marker itself is invalid after its recording is freed.
///
/// # Safety
/// Inputs are live and exclusively borrowed. Outputs are writable, possibly
/// unaligned owner slots, disjoint from all inputs and their retained bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_stage_selection(
    transaction: *mut TypeBridgeStagedTransaction,
    builder: *mut crate::plan_builder::TypeBridgePlanBuilder,
    selection: u32,
    out_pending: *mut *mut TypeBridgePending,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    use crate::plan_builder::selections::{AuthoredAnswers, AuthoredPending, SelectionProgram};
    let inputs = |outputs: &DirectOutputPreflight| {
        // SAFETY: caller retains both live input handles.
        unsafe {
            check_transaction(outputs, transaction)?;
        }
        check_object(outputs, builder)?;
        if !builder.is_null() {
            // SAFETY: the complete builder was fenced above.
            unsafe { &*builder }.check_borrowed_ranges(outputs)?;
        }
        Ok(())
    };
    // SAFETY: both owner outputs are fenced before either is initialized.
    if let Err(status) = unsafe { prepare_handle_outputs(out_pending, out_diagnostics, inputs) } {
        return status;
    }
    if transaction.is_null() || builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: exclusive, live inputs by contract.
        let (transaction, builder) = unsafe { (&mut *transaction, &mut *builder) };
        if transaction.poisoned {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        let source = match builder.finish_selection(selection) {
            Ok(source) => source,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let mut stage = || match &source.program {
            SelectionProgram::Raw(plans) => transaction
                .transaction
                .stage_plans(Arc::clone(&source), plans.returns_documents())
                .map(AuthoredAnswers::Raw),
            SelectionProgram::Models(_) => transaction
                .transaction
                .stage_plan_fetch(
                    Arc::clone(&source),
                    Arc::clone(
                        source
                            .registry
                            .as_ref()
                            .expect("model program retains its registry"),
                    ),
                    Default::default(),
                )
                .map(AuthoredAnswers::Models),
        };
        let dispatch = transaction.owner.debug_dispatch();
        let answers = match dispatch {
            Some(dispatch) => tracing::dispatcher::with_default(&dispatch, stage),
            None => stage(),
        };
        let answers = match answers {
            Ok(answers) => answers,
            Err(error) => {
                return return_execution_error(pending_diagnostic(&error), out_diagnostics);
            }
        };
        let origin = match &transaction.owner {
            StagedOwner::Database(guard) => guard.projected_origin(),
            StagedOwner::Proxy(_) => type_bridge_orm::ProjectedQueryOrigin::remote_unbound(),
        };
        let state = Arc::new(PendingState::authored(AuthoredPending::new(
            source, answers, origin,
        )));
        transaction.pending_views.push(Arc::downgrade(&state));
        let handle = Box::new(TypeBridgePending {
            tag: crate::staged_answers::PENDING_TAG,
            state,
        });
        // SAFETY: the output was fenced and initialized above.
        unsafe { out_pending.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Commit every staged plan in one provider transaction. The handle stays
/// usable for new work on every outcome; after an unknown outcome (such as a
/// lost proxy reply) the caller reads the database to learn whether it
/// applied. Nothing staged is a no-op success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_commit(
    transaction: *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe { finish_commit(transaction, out_diagnostics) }
}

unsafe fn finish_commit(
    transaction: *mut TypeBridgeStagedTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let entry = crate::perf_stages::entry();
    let _entered = entry.enter();
    // SAFETY: forwarded handle and diagnostics slot contract.
    if let Err(status) = unsafe { prepare_commit_outputs(transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        // SAFETY: a non-null handle is live and not shared across threads by contract.
        let handle = unsafe { &mut *transaction };
        if handle.poisoned {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        handle.poisoned = true;
        let tx = &mut handle.transaction;
        let outcome = handle.owner.block_on(
            tx.commit()
                // The provider may poll on another thread; the span follows it.
                .instrument(entry.clone()),
        );
        handle.poisoned = false;
        for state in handle
            .pending_views
            .drain(..)
            .filter_map(|state| state.upgrade())
        {
            state.prepare();
        }
        match outcome {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(error) => return_execution_error(error.diagnostic(), out_diagnostics),
        }
    })
}

/// Discard every staged plan; the handle stays
/// usable and a poisoned handle recovers. A null handle is `INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_staged_transaction_rollback(
    transaction: *mut TypeBridgeStagedTransaction,
) -> TypeBridgeStatus {
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: a non-null handle is live and not shared across threads by contract.
        let handle = unsafe { &mut *transaction };
        // Deferred: nothing reached the provider. Only a closed transaction
        // refuses, and close also frees the handle.
        match handle.transaction.rollback() {
            Ok(()) => {
                handle.poisoned = false;
                handle.pending_views.clear();
                TypeBridgeStatus::Ok
            }
            Err(_) => TypeBridgeStatus::ExecutionFailed,
        }
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

pub(crate) fn answer_status(error: &PendingError) -> TypeBridgeStatus {
    match error {
        PendingError::NotCommitted => TypeBridgeStatus::InUse,
        // The commit already reported the typed failure; rollback and close
        // discarded the answer for good.
        _ => TypeBridgeStatus::ExecutionFailed,
    }
}

fn answer(pending: &TypeBridgePending) -> Result<Arc<PlanAnswer>, TypeBridgeStatus> {
    pending
        .state
        .pending
        .get()
        .map_err(|error| answer_status(&error))
}

/// Report the resolved answer kind and, for count, exists (0/1), matched
/// and rows, its scalar (`rows` reports the row count).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_pending_answer(
    pending: *const TypeBridgePending,
    out_kind: *mut u32,
    out_value: *mut u64,
) -> TypeBridgeStatus {
    let _entry = crate::perf_stages::entry().entered();
    let outputs = match direct_output_preflight(&[
        (out_kind.cast(), size_of::<u32>()),
        (out_value.cast(), size_of::<u64>()),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: a non-null input is a live cursor retained by the caller.
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
    if !out_kind.is_null() {
        // SAFETY: the slot is writable and overlaps no fenced input.
        unsafe { out_kind.write_unaligned(0) };
    }
    if !out_value.is_null() {
        // SAFETY: as above.
        unsafe { out_value.write_unaligned(0) };
    }
    if tag != Some(crate::staged_answers::PENDING_TAG) || out_kind.is_null() || out_value.is_null()
    {
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
            PlanAnswer::Documents(documents) => (
                TypeBridgePlanAnswerKind::Documents,
                u64::try_from(documents.len()).unwrap_or(u64::MAX),
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
    use type_bridge_contract::query_plan::query_plan_capability_vocabulary;
    use type_bridge_contract::query_remote::{CAP_QUERY_REMOTE_STAGED, CAP_QUERY_REMOTE_WRITE};
    use type_bridge_contract::query_remote::{
        RemoteCapabilities, RemoteExecutorBinding, RemoteSigningPublicKey,
    };
    use type_bridge_contract::sdk_diagnostic::{SdkCommitFailureOutcome, SdkProviderOperation};
    use type_bridge_orm::session::backend::QueryResult;
    use type_bridge_orm::{ClassifiedCommitError, CommitFailureCertainty, TxType};

    use super::*;
    use crate::TypeBridgeByteView;
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

    const SCHEMA: &[u8] = include_bytes!("../../../../tests/contracts/plan/schema.json");
    const PURCHASE: &[u8] = include_bytes!("../../../../tests/contracts/plan/purchase.json");
    const ROWS: &[u8] = include_bytes!("../../../../tests/contracts/plan/rows.json");
    const COUNT: &[u8] = include_bytes!("../../../../tests/contracts/plan/count.json");
    const EXISTS: &[u8] = include_bytes!("../../../../tests/contracts/plan/exists.json");
    const BATCH: &[u8] = include_bytes!("../../../../tests/contracts/plan/batch.json");
    const HEADER: &str = include_str!("../include/typebridge/type_bridge.h");

    type Diagnostics = *mut TypeBridgeExecutionDiagnostics;

    fn decode(plan: &[u8]) -> *mut TypeBridgePlan {
        let mut handle = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let status = unsafe {
            type_bridge_plan_decode(
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
                .map(|row| serde_json::json!({ "row": row, "account": {"kind":"thing", "iid":format!("0x{:02x}", row + 1), "type_id":{"kind":"entity", "label":"account"}} }))
                .collect(),
        ))
    }

    fn commit(transaction: *mut TypeBridgeStagedTransaction) -> (TypeBridgeStatus, Diagnostics) {
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

    #[test]
    fn debug_listener_is_scoped_to_its_database() {
        unsafe extern "C" fn record(view: TypeBridgeByteView, context: *mut std::ffi::c_void) {
            // SAFETY: the test retains this vector and the callback receives live bytes.
            let records = unsafe { &*(context.cast::<std::sync::Mutex<Vec<serde_json::Value>>>()) };
            let bytes = unsafe { std::slice::from_raw_parts(view.data, view.length) };
            records
                .lock()
                .unwrap()
                .push(serde_json::from_slice(bytes).unwrap());
        }
        let records = std::sync::Mutex::new(Vec::<serde_json::Value>::new());
        let a = fixture(vec![rows(0)], COMMIT_SUCCESS);
        let b = fixture(vec![rows(0)], COMMIT_SUCCESS);
        // SAFETY: listener context lives until cleared below; handles are exclusive.
        assert_eq!(
            unsafe {
                crate::runtime::type_bridge_debug_listener_set(
                    &*a.database,
                    Some(record),
                    (&records as *const _ as *mut std::sync::Mutex<Vec<serde_json::Value>>).cast(),
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut plan = decode(COUNT);
        let mut transaction = open(&b, TYPE_BRIDGE_TRANSACTION_READ);
        let mut pending = stage(transaction, plan);
        assert_eq!(commit(transaction).0, TypeBridgeStatus::Ok);
        assert!(records.lock().unwrap().is_empty());
        // SAFETY: these owned handles are no longer in use.
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_staged_transaction_close(&mut transaction);
        }
        transaction = open(&a, TYPE_BRIDGE_TRANSACTION_READ);
        pending = stage(transaction, plan);
        assert_eq!(commit(transaction).0, TypeBridgeStatus::Ok);
        {
            let events = records.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|record| record["name"] == "type_bridge.plan.commit"),
                "{events:?}"
            );
        }
        // A running transaction can retain a dispatch after the callback is cleared.
        let retained = unsafe { &*transaction }.owner.debug_dispatch().unwrap();
        // SAFETY: clear the callback before its context is released and close owned handles.
        unsafe {
            crate::runtime::type_bridge_debug_listener_set(&*a.database, None, ptr::null_mut());
            type_bridge_pending_free(&mut pending);
            type_bridge_staged_transaction_close(&mut transaction);
            type_bridge_plan_free(&mut plan);
        }
        let count = records.lock().unwrap().len();
        tracing::dispatcher::with_default(&retained, || {
            tracing::debug!(target: "type_bridge.plan", "late event");
        });
        assert_eq!(records.lock().unwrap().len(), count);
        // Closing the database also ends the callback context's lifetime.
        let mut database = Box::into_raw(a.database);
        // SAFETY: the database is owned and the records remain live through close.
        assert_eq!(
            unsafe {
                crate::runtime::type_bridge_debug_listener_set(
                    database,
                    Some(record),
                    (&records as *const _ as *mut std::sync::Mutex<Vec<serde_json::Value>>).cast(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        assert!(database.is_null());
        tracing::dispatcher::with_default(&retained, || {
            tracing::debug!(target: "type_bridge.plan", "late event after close");
        });
        assert_eq!(records.lock().unwrap().len(), count);
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
    fn proxy_transaction_refuses_a_null_connection_and_clears_its_outputs() {
        let mut transaction = ptr::dangling_mut::<TypeBridgeStagedTransaction>();
        let mut diagnostics = ptr::dangling_mut::<TypeBridgeExecutionDiagnostics>();
        for kind in [TYPE_BRIDGE_TRANSACTION_WRITE, 0] {
            // SAFETY: both output slots are live locals.
            let status = unsafe {
                type_bridge_proxy_connection_transaction(
                    ptr::null(),
                    kind,
                    &mut transaction,
                    &mut diagnostics,
                )
            };
            assert_eq!(status, TypeBridgeStatus::InvalidArgument);
            assert!(transaction.is_null() && diagnostics.is_null());
        }
    }

    #[test]
    fn decode_round_trips_canonical_bytes_and_reports_diagnostics() {
        let mut plan = decode(PURCHASE);
        let mut length = usize::MAX;
        assert_eq!(
            unsafe { type_bridge_plan_canonical_bytes(plan, ptr::null_mut(), 0, &mut length) },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(length, PURCHASE.len());
        let mut short = vec![0xa5; PURCHASE.len() - 1];
        assert_eq!(
            unsafe {
                type_bridge_plan_canonical_bytes(plan, short.as_mut_ptr(), short.len(), &mut length)
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
                type_bridge_plan_canonical_bytes(plan, exact.as_mut_ptr(), exact.len(), &mut length)
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(exact, PURCHASE, "the contract fixture bytes round-trip");
        // Null handles and outputs.
        assert_eq!(
            unsafe { type_bridge_plan_canonical_bytes(plan, ptr::null_mut(), 0, ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        length = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_plan_canonical_bytes(ptr::null(), ptr::null_mut(), 0, &mut length)
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
                type_bridge_plan_decode(
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
                type_bridge_plan_decode(
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
                type_bridge_plan_decode(
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
                type_bridge_plan_decode(
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
                type_bridge_plan_decode(
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
                type_bridge_plan_canonical_bytes(plan, storage.as_mut_ptr(), storage.len(), out_len)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(storage, [0; 64]);
        let own = unsafe { &*plan }.source.canonical.as_ptr().cast_mut();
        let mut length = usize::MAX;
        assert_eq!(
            unsafe { type_bridge_plan_canonical_bytes(plan, own, PURCHASE.len(), &mut length) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(length, usize::MAX);
        assert_eq!(
            unsafe {
                type_bridge_plan_canonical_bytes(
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

        // Commit with diagnostics over the live handle.
        let status =
            unsafe { type_bridge_staged_transaction_commit(transaction, transaction.cast()) };
        assert_eq!(status, TypeBridgeStatus::InvalidArgument);
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

        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert_eq!(fixture.state.opened_transactions(), vec![TxType::Write]);
        assert_eq!(fixture.state.query_count(), 4);
        assert_eq!(
            fixture
                .state
                .commits
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
        // Answers outlive the transaction.
        unsafe { type_bridge_staged_transaction_close(&mut transaction) };
        assert!(transaction.is_null());
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
        // Rollback discards a write transaction's staged work and keeps it.
        let mut purchase = decode(PURCHASE);
        let mut write = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut staged = stage(write, purchase);
        let original = write;
        unsafe {
            type_bridge_staged_transaction_close(&mut read_only);
            assert_eq!(
                type_bridge_staged_transaction_rollback(write),
                TypeBridgeStatus::Ok
            );
        }
        assert!(read_only.is_null());
        assert_eq!(write, original, "rollback keeps the handle");
        // Rollback discarded the staged answer for good.
        assert_eq!(read(staged), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        unsafe {
            type_bridge_staged_transaction_close(&mut write);
            type_bridge_pending_free(&mut staged);
            type_bridge_plan_free(&mut plan);
            type_bridge_plan_free(&mut purchase);
        }
        assert!(write.is_null());
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

        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert_eq!(state.opened_transactions(), vec![TxType::Read]);
        // The committed handle stays open and keeps the database busy.
        assert_eq!(close_database(&mut database), TypeBridgeStatus::InUse);
        // Nothing staged: commit is a no-op success.
        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert_eq!(state.opened_transactions(), vec![TxType::Read]);
        // Null handles and outputs are rejected; the diagnostics output is cleared.
        let mut diagnostics = ptr::dangling_mut();
        let status =
            unsafe { type_bridge_staged_transaction_commit(ptr::null_mut(), &mut diagnostics) };
        assert_eq!(status, TypeBridgeStatus::InvalidArgument);
        assert!(diagnostics.is_null(), "the diagnostics output is cleared");
        assert_eq!(
            unsafe { type_bridge_staged_transaction_commit(transaction, ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(transaction) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(ptr::null_mut()) },
            TypeBridgeStatus::InvalidArgument
        );
        // Close is the only release, and it is idempotent.
        unsafe {
            type_bridge_staged_transaction_close(&mut transaction);
            type_bridge_staged_transaction_close(&mut transaction);
            type_bridge_staged_transaction_close(ptr::null_mut());
        }
        assert!(transaction.is_null());
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
    fn commit_failures_keep_the_handle_and_the_released_detail_set() {
        // An unknown outcome leaves the handle for new work.
        let fixture_unknown = fixture(vec![rows(2)], COMMIT_UNKNOWN);
        let plan = decode(BATCH);
        let mut transaction = open(&fixture_unknown, TYPE_BRIDGE_TRANSACTION_WRITE);
        let original = transaction;
        let mut pending = stage(transaction, plan);
        let (status, mut diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::CommitOutcomeUnknown);
        assert_eq!(diagnostic(diagnostics).1, "commit_outcome_unknown");
        close_diagnostics(&mut diagnostics);
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        unsafe { type_bridge_pending_free(&mut pending) };
        assert_eq!(fixture_unknown.state.opened_transactions().len(), 1);
        unsafe { type_bridge_staged_transaction_close(&mut transaction) };
        assert!(transaction.is_null() && !original.is_null());

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
        let (status, mut diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::ExecutionFailed);
        let mut released = Box::into_raw(Box::new(execution_diagnostics_handle(vec![
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted),
        ])));
        assert_eq!(c_shape(diagnostics), c_shape(released));
        // ABI 1.7 reads the typed conflict through its own accessor.
        let mut failure = std::mem::MaybeUninit::uninit();
        assert_eq!(
            unsafe {
                crate::execution_diagnostic::type_bridge_execution_diagnostics_failure_get_v1(
                    diagnostics,
                    0,
                    failure.as_mut_ptr(),
                )
            },
            TypeBridgeStatus::Ok
        );
        let failure: crate::execution_diagnostic::TypeBridgeExecutionFailureV1 =
            unsafe { failure.assume_init() };
        assert_eq!(
            (
                failure.outcome,
                failure.phase,
                failure.reason,
                failure.retry_safe
            ),
            (1, 2, 1, 1)
        );
        assert_ne!(failure.lock_kind, 0, "a conflict reports its lock kind");
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_close(&mut released) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        // A typed rejection is a definite outcome: the handle stages again.
        let mut next = stage(transaction, plan);
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_pending_free(&mut next);
            type_bridge_staged_transaction_close(&mut transaction);
        }

        // A provider panic poisons the handle: only rollback and close remain.
        let Fixture { database, .. } = fixture(vec![rows(2)], COMMIT_PANIC);
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
        let original = transaction;
        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Panic);
        assert!(diagnostics.is_null());
        assert_eq!(transaction, original);
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        let mut refused = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_staged_transaction_stage(
                    transaction,
                    plan,
                    &mut refused,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(refused.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        close_diagnostics(&mut diagnostics);
        let (status, mut diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::ExecutionFailed);
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        close_diagnostics(&mut diagnostics);
        assert_eq!(close_database(&mut database), TypeBridgeStatus::InUse);
        // Rollback recovers the handle; close releases it and the database.
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(transaction) },
            TypeBridgeStatus::Ok
        );
        let mut again = stage(transaction, plan);
        unsafe {
            type_bridge_staged_transaction_close(&mut transaction);
        }
        assert_eq!(read(again), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_pending_free(&mut again);
        }
        let mut plan = plan;
        unsafe { type_bridge_plan_free(&mut plan) };
    }

    #[test]
    fn one_handle_commits_again_after_rollback_and_after_commit() {
        let fixture = fixture(vec![rows(2), rows(2)], COMMIT_SUCCESS);
        let mut plan = decode(BATCH);
        let mut transaction = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        let original = transaction;

        // Rollback, then stage and commit on the same handle.
        let mut ghost = stage(transaction, plan);
        assert_eq!(
            unsafe { type_bridge_staged_transaction_rollback(transaction) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read(ghost), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        let mut kept = stage(transaction, plan);
        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        let matched = TypeBridgePlanAnswerKind::Matched as u32;
        assert_eq!(read(kept), (TypeBridgeStatus::Ok, matched, 2));
        assert_eq!(
            fixture.state.query_count(),
            1,
            "the rolled-back plan never ran"
        );

        // Commit, then stage again: the second commit is its own provider transaction.
        let mut again = stage(transaction, plan);
        let (status, diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert_eq!(read(again), (TypeBridgeStatus::Ok, matched, 2));
        assert_eq!(read(kept), (TypeBridgeStatus::Ok, matched, 2));
        assert_eq!(transaction, original);
        assert_eq!(
            fixture.state.opened_transactions(),
            vec![TxType::Write, TxType::Write]
        );
        assert_eq!(
            fixture
                .state
                .commits
                .load(std::sync::atomic::Ordering::Acquire),
            2
        );
        unsafe {
            type_bridge_staged_transaction_close(&mut transaction);
            type_bridge_pending_free(&mut ghost);
            type_bridge_pending_free(&mut kept);
            type_bridge_pending_free(&mut again);
            type_bridge_plan_free(&mut plan);
        }
        assert!(transaction.is_null());
    }

    #[test]
    fn unknown_proxy_outcome_keeps_the_handle_for_new_work() {
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
        // Every reply is lost, so each commit's outcome is unknown.
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
        let mut capabilities = query_plan_capability_vocabulary();
        for capability in [CAP_QUERY_REMOTE_STAGED, CAP_QUERY_REMOTE_WRITE] {
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

        let (status, mut diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::CommitOutcomeUnknown);
        assert_eq!(transaction, original, "an unknown outcome keeps the owner");
        close_diagnostics(&mut diagnostics);
        assert_eq!(read(pending), (TypeBridgeStatus::ExecutionFailed, 0, 0));
        assert_eq!(close_database(&mut database), TypeBridgeStatus::InUse);

        // New work is a new request under a fresh nonce.
        let mut again = stage(transaction, plan);
        let (status, mut diagnostics) = commit(transaction);
        assert_eq!(status, TypeBridgeStatus::CommitOutcomeUnknown);
        assert_eq!(transaction, original);
        close_diagnostics(&mut diagnostics);
        {
            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert_ne!(sent[0], sent[1], "each commit is its own request");
        }
        unsafe { type_bridge_staged_transaction_close(&mut transaction) };
        assert!(transaction.is_null());
        assert_eq!(close_database(&mut database), TypeBridgeStatus::Ok);
        unsafe {
            type_bridge_pending_free(&mut pending);
            type_bridge_pending_free(&mut again);
            type_bridge_plan_free(&mut plan);
        }
    }
    #[test]
    fn borrowed_cursors_follow_pending_commit_and_owner_lifetimes() {
        use crate::staged_answers::*;
        let response = Response::Result(QueryResult::Documents(vec![serde_json::json!({
            "_iid":"0x01", "_type":"account", "attributes":{"id":"account-b","score":1}
        })]));
        let fixture = fixture(vec![response], COMMIT_SUCCESS);
        let mut plan = decode(include_bytes!(
            "../../../../tests/contracts/plan/fetch_entity.json"
        ));
        let mut transaction = open(&fixture, TYPE_BRIDGE_TRANSACTION_WRITE);
        let mut pending = stage(transaction, plan);
        let mut count = usize::MAX;
        // SAFETY: live handles and writable distinct outputs throughout.
        unsafe {
            assert_eq!(
                type_bridge_pending_document_count(pending, &mut count),
                TypeBridgeStatus::InUse
            );
            assert_eq!(count, 0);
            type_bridge_plan_free(&mut plan);
            let (status, diagnostics) = commit(transaction);
            assert_eq!(status, TypeBridgeStatus::Ok);
            assert!(diagnostics.is_null());
            type_bridge_staged_transaction_close(&mut transaction);
            assert_eq!(
                read(pending),
                (
                    TypeBridgeStatus::Ok,
                    TypeBridgePlanAnswerKind::Documents as u32,
                    1
                )
            );
            assert_eq!(
                type_bridge_pending_document_count(pending, &mut count),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, 1);
            let mut doc = ptr::null();
            assert_eq!(
                type_bridge_pending_document(pending, 1, &mut doc),
                TypeBridgeStatus::InvalidArgument
            );
            assert!(doc.is_null());
            assert_eq!(
                type_bridge_pending_document(pending, 0, &mut doc),
                TypeBridgeStatus::Ok
            );
            let mut value = 0;
            let label = crate::abi::TypeBridgeByteView {
                data: b"score".as_ptr(),
                length: 5,
            };
            assert_eq!(
                type_bridge_document_attribute_long(doc, label, 0, &mut value),
                TypeBridgeStatus::Ok
            );
            assert_eq!(value, 1);
            assert_eq!(
                type_bridge_pending_row_count(pending, &mut count),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(count, 0);
            type_bridge_pending_free(&mut pending);
            assert!(pending.is_null());
            // `doc` and borrowed byte views expire here; they must not be used.
        }
    }
    #[test]
    fn mock_pending_keeps_all_scalar_domains_and_listed_players_after_commit() {
        use crate::staged_answers::*;
        let fixture = fixture(
            vec![Response::Result(QueryResult::Documents(vec![
                crate::staged_answers::tests::answer(),
            ]))],
            COMMIT_SUCCESS,
        );
        let core_plan = crate::staged_answers::tests::plan();
        let bytes = core_plan.canonical_bytes().unwrap();
        let mut plan = Box::into_raw(Box::new(TypeBridgePlan::new(core_plan, bytes)));
        let mut transaction = open(&fixture, TYPE_BRIDGE_TRANSACTION_READ);
        let mut pending = stage(transaction, plan);
        // SAFETY: live handles, readable labels and distinct writable outputs.
        unsafe {
            type_bridge_plan_free(&mut plan);
            let (status, diagnostics) = commit(transaction);
            assert_eq!(status, TypeBridgeStatus::Ok);
            assert!(diagnostics.is_null());
            type_bridge_staged_transaction_close(&mut transaction);
            let mut doc = ptr::null();
            assert_eq!(
                type_bridge_pending_document(pending, 0, &mut doc),
                TypeBridgeStatus::Ok
            );
            let mut player = ptr::null();
            assert_eq!(
                type_bridge_document_role(doc, 0, &mut player),
                TypeBridgeStatus::Ok
            );
            assert!(!player.is_null());
            let label = |text: &'static [u8]| crate::abi::TypeBridgeByteView {
                data: text.as_ptr(),
                length: text.len(),
            };
            let mut count = 0;
            assert_eq!(
                type_bridge_document_attribute_count(player, label(b"key"), &mut count),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, 2);
            let mut long = 0;
            assert_eq!(
                type_bridge_document_attribute_long(player, label(b"measure-long"), 0, &mut long),
                TypeBridgeStatus::Ok
            );
            assert_eq!(long, i64::MIN);
            let mut duration = crate::abi::TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            };
            assert_eq!(
                type_bridge_document_attribute_duration(
                    player,
                    label(b"elapsed"),
                    0,
                    &mut duration
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                std::slice::from_raw_parts(duration.data, duration.length),
                b"P1D"
            );
            assert_eq!(
                type_bridge_document_role(doc, 1, &mut player),
                TypeBridgeStatus::Ok
            );
            assert!(player.is_null());
            assert_eq!(
                type_bridge_document_role(doc, 0, &mut player),
                TypeBridgeStatus::Ok
            );
            let mut text = crate::abi::TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            };
            assert_eq!(
                type_bridge_document_attribute_string(player, label(b"key"), 1, &mut text),
                TypeBridgeStatus::Ok
            );
            let before = std::slice::from_raw_parts(text.data, text.length).to_vec();
            assert_eq!(
                type_bridge_row_value_kind(
                    pending.cast(),
                    label(b"key"),
                    text.data.cast_mut().cast()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(std::slice::from_raw_parts(text.data, text.length), before);
            assert_eq!(
                type_bridge_document_attribute_kind(
                    pending.cast(),
                    label(b"key"),
                    1,
                    text.data.cast_mut().cast()
                ),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(std::slice::from_raw_parts(text.data, text.length), before);
            type_bridge_pending_free(&mut pending);
        }
    }
}

#[cfg(test)]
mod explain_tests {
    use super::*;
    #[test]
    fn owned_explanation_matches_core_and_checks_output_aliases() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/contracts/plan");
        let schema = type_bridge_contract::schema::decode_declared_schema(
            &std::fs::read(root.join("schema.json")).unwrap(),
        )
        .unwrap();
        let bytes = std::fs::read(root.join("purchase.json")).unwrap();
        let plan = type_bridge_contract::query_plan::decode_query_plan(&bytes, &schema).unwrap();
        let expected = type_bridge_orm::plan_explain::explain(&plan)
            .unwrap()
            .canonical_bytes()
            .unwrap();
        let plan = TypeBridgePlan::new(plan, bytes);
        let mut output = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the plan and distinct output slots are live for the call.
        assert_eq!(
            unsafe { type_bridge_plan_explain(&plan, &mut output, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        // SAFETY: a successful call returned this owned buffer.
        assert_eq!(unsafe { &*output }.bytes, expected);
        // SAFETY: the exclusively owned buffer slot is live.
        assert_eq!(
            unsafe { crate::canonical_archive::type_bridge_canonical_bytes_close(&mut output) },
            TypeBridgeStatus::Ok
        );
        // SAFETY: both output slots are intentionally aliased to test preflight.
        assert_eq!(
            unsafe {
                type_bridge_plan_explain(
                    &plan,
                    (&mut diagnostics as *mut *mut TypeBridgeExecutionDiagnostics).cast(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        // SAFETY: null plan with writable outputs is an invalid-input probe.
        assert_eq!(
            unsafe { type_bridge_plan_explain(ptr::null(), &mut output, &mut diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(output.is_null());
    }
}

#[cfg(test)]
mod pending_diagnostic_redaction_tests {
    use super::*;
    #[test]
    fn pending_failure_uses_the_canonical_diagnostic_without_server_text() {
        let error = PendingError::Failed {
            diagnostic: Box::new(
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::commit_failure(
                    type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome::Unknown,
                ),
            ),
            op_index: None,
            message: "signed-server-secret".into(),
            failure: Some(Box::new(
                type_bridge_contract::failure::Failure::Indeterminate { code: None },
            )),
            context: Default::default(),
        };
        assert!(!error.to_string().contains("signed-server-secret"));
        let surfaced = pending_diagnostic(&error);
        assert!(
            !serde_json::to_string(&surfaced.host_json())
                .unwrap()
                .contains("signed-server-secret")
        );
        assert_eq!(surfaced.code().as_str(), "commit_outcome_unknown");
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn staged_policy_defaults_and_signed_timeouts() {
        let defaults = TransactionOptions::default();
        assert_eq!(
            defaults.timeout.unwrap_or_default().milliseconds(),
            Some(60_000)
        );
        assert!(!defaults.collect_phase_timestamps);
        let mut value = TypeBridgeStagedTransactionOptionsV1 {
            struct_size: size_of::<TypeBridgeStagedTransactionOptionsV1>() as u32,
            version: 1,
            timeout_milliseconds: 0,
            collect_phase_timestamps: 1,
        };
        for (input, expected) in [(0, None), (-1, None), (86_400_001, Some(86_400_001))] {
            value.timeout_milliseconds = input;
            let options = value.options().unwrap();
            assert_eq!(options.timeout.unwrap().milliseconds(), expected);
            assert!(options.collect_phase_timestamps);
        }
        value.collect_phase_timestamps = 2;
        assert!(matches!(
            value.options(),
            Err(TypeBridgeStatus::InvalidArgument)
        ));
        value.collect_phase_timestamps = 0;
        value.version = 2;
        assert!(matches!(
            value.options(),
            Err(TypeBridgeStatus::InvalidArgument)
        ));
    }

    #[test]
    fn staged_options_are_fenced_before_output_initialization() {
        let mut value = TypeBridgeStagedTransactionOptionsV1 {
            struct_size: size_of::<TypeBridgeStagedTransactionOptionsV1>() as u32,
            version: 1,
            timeout_milliseconds: 60_000,
            collect_phase_timestamps: 0,
        };
        let alias = (&mut value as *mut TypeBridgeStagedTransactionOptionsV1).cast();
        // SAFETY: the call must refuse the output overlapping the complete options.
        let status = unsafe {
            type_bridge_database_transaction_with_options(
                ptr::null(),
                TYPE_BRIDGE_TRANSACTION_WRITE,
                &value,
                alias,
                ptr::null_mut(),
            )
        };
        assert_eq!(status, TypeBridgeStatus::InvalidArgument);
        assert_eq!(value.version, 1);
        assert_eq!(
            value.struct_size as usize,
            size_of::<TypeBridgeStagedTransactionOptionsV1>()
        );
    }
}
