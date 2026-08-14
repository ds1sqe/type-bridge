//! Synchronous C runtime, database, transaction, and cancellation boundary.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::runtime::{Builder, Runtime};
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
    SdkProviderOperation,
};
use type_bridge_orm::{
    AnswerCancellation, ConnectOptions, Database, OrmError, PreparedSecureConnectOptions,
    QueryExecutionDeadline, QueryExecutionResourceLimits, SecureConnectError, SecureConnectOptions,
    TlsMode, TransactionContext, TransactionContextState, TxType,
    is_identity_safe_provider_address, lower_execution_error,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded,
    initialize_view,
};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, execution_diagnostics_handle, return_execution_error,
};
use crate::generated_preflight::{DirectOutputPreflight, direct_output_preflight};
use crate::policy::{
    DATABASE_CONFIG_V2_VERSION, DATABASE_CUSTOM_ROOT_CA_BYTES_MAX, TLS_CUSTOM_ROOT_CA,
    TLS_DISABLED, TLS_NATIVE_ROOTS, TypeBridgeDatabaseConfigV2, parse_common_limits,
};
use crate::query::TypeBridgeQueryExecutionLimitsV1;

const RUNTIME_CONFIG_VERSION: u32 = 1;
const DATABASE_CONFIG_VERSION: u32 = 1;
const MIN_RUNTIME_WORKER_THREADS: u32 = 1;
const MAX_RUNTIME_WORKER_THREADS: u32 = 64;
const MAX_ADDRESS_BYTES: usize = 4 * 1024;
const MAX_DATABASE_NAME_BYTES: usize = 256;
const MAX_USERNAME_BYTES: usize = 4 * 1024;
const MAX_PASSWORD_BYTES: usize = 64 * 1024;
const REQUIRED_SEMANTIC_PROFILE: &[u8] = b"typedb-3.12.1/v1";

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Database, OrmError>> + Send + 'static>>;
type ControlledConnectFuture =
    Pin<Box<dyn Future<Output = Result<Database, SdkExecutionDiagnostic>> + Send + 'static>>;

trait DatabaseConnector: Send + Sync {
    fn connect(&self, input: DatabaseConnectInput) -> ConnectFuture;

    fn connect_v2(&self, input: DatabaseConnectInputV2) -> ControlledConnectFuture;
}

struct TypeDbConnector;

impl DatabaseConnector for TypeDbConnector {
    fn connect(&self, input: DatabaseConnectInput) -> ConnectFuture {
        Box::pin(async move {
            Database::connect_with_options(
                &input.address,
                &input.database,
                &input.username,
                &input.password,
                ConnectOptions {
                    http_port: input.http_port,
                    tls: input.tls_native_roots,
                    server_version: None,
                },
            )
            .await
        })
    }

    fn connect_v2(&self, input: DatabaseConnectInputV2) -> ControlledConnectFuture {
        Box::pin(async move {
            Database::connect_prepared_secure_with_control(
                &input.address,
                &input.database,
                &input.username,
                &input.password,
                input.transport,
                input.limits,
                input.deadline,
                input.cancellation,
            )
            .await
            .map_err(lower_secure_connect_error)
        })
    }
}

/// Version-1 configuration for one hidden Tokio runtime.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeRuntimeConfigV1 {
    /// Exact `sizeof` of this structure.
    pub struct_size: u32,
    /// Runtime configuration layout version.
    pub version: u32,
    /// Tokio worker threads, inclusively bounded from 1 through 64.
    pub worker_threads: u32,
    /// Explicit alignment and compatible-growth padding; must be zero.
    pub reserved0: u32,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Version-1 direct TypeDB database connection configuration.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TypeBridgeDatabaseConfigV1 {
    /// Exact `sizeof` of this structure.
    pub struct_size: u32,
    /// Database configuration layout version.
    pub version: u32,
    /// TypeDB gRPC address as bounded copied UTF-8.
    pub address: TypeBridgeByteView,
    /// TypeDB database name as bounded copied UTF-8.
    pub database: TypeBridgeByteView,
    /// Username as bounded copied UTF-8.
    pub username: TypeBridgeByteView,
    /// Password as bounded copied UTF-8. Empty is admitted.
    pub password: TypeBridgeByteView,
    /// Authoritative HTTP version-probe port in the range 1 through 65535.
    pub http_port: u32,
    /// Zero for plaintext or one for native-root TLS on both probe and driver.
    pub tls_mode: u32,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// Opaque synchronous runtime owning one Tokio multi-thread scheduler.
pub struct TypeBridgeRuntime {
    state: Arc<RuntimeState>,
}

struct RuntimeState {
    runtime: Runtime,
    connector: Arc<dyn DatabaseConnector>,
    database_children: AtomicUsize,
}

/// Opaque database connection bound to one exact verified C schema package.
pub struct TypeBridgeDatabase {
    state: Arc<DatabaseState>,
}

pub(crate) struct DatabaseState {
    runtime: Arc<RuntimeState>,
    _package: Arc<SchemaPackageState>,
    database: Arc<Database>,
    server_version: Vec<u8>,
    answer_ceiling: Option<QueryExecutionResourceLimits>,
    transaction_children: AtomicUsize,
}

impl Drop for DatabaseState {
    fn drop(&mut self) {
        self.runtime
            .database_children
            .fetch_sub(1, Ordering::AcqRel);
    }
}

struct TransactionState {
    database: Arc<DatabaseState>,
    context: Option<TransactionContext>,
    answer_ceiling: Option<QueryExecutionResourceLimits>,
    poisoned: AtomicBool,
}

impl Drop for TransactionState {
    fn drop(&mut self) {
        self.database
            .transaction_children
            .fetch_sub(1, Ordering::AcqRel);
    }
}

/// Opaque active read transaction. It cannot be passed to write terminals.
pub struct TypeBridgeReadTransaction {
    state: TransactionState,
}

/// Opaque active write transaction. Commit, rollback, and close consume it.
pub struct TypeBridgeWriteTransaction {
    state: TransactionState,
}

/// Opaque sticky thread-safe signal checked before dispatch and throughout typed-query answers.
pub struct TypeBridgeCancellation {
    requested: AtomicBool,
    answer: type_bridge_orm::AnswerCancellation,
}

impl TypeBridgeDatabase {
    pub(crate) fn package_state(&self) -> &Arc<SchemaPackageState> {
        &self.state._package
    }

    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.state.runtime.runtime.block_on(future)
    }

    pub(crate) fn orm_database(&self) -> &Database {
        &self.state.database
    }

    pub(crate) fn answer_ceiling(&self) -> QueryExecutionResourceLimits {
        self.state.answer_ceiling.unwrap_or_default()
    }

    pub(crate) fn policy_answer_ceiling(&self) -> Option<QueryExecutionResourceLimits> {
        self.state.answer_ceiling
    }

    pub(crate) fn open_transaction_context(
        &self,
        tx_type: TxType,
    ) -> Result<TransactionContext, OrmError> {
        self.block_on(self.state.database.transaction_context(tx_type))
    }

    pub(crate) fn close_transaction_context(
        &self,
        context: &TransactionContext,
    ) -> Result<(), OrmError> {
        self.block_on(context.close())
    }

    pub(crate) fn rollback_transaction_context(
        &self,
        context: &TransactionContext,
    ) -> Result<(), OrmError> {
        self.block_on(context.rollback())
    }

    pub(crate) fn commit_transaction_context_classified(
        &self,
        context: &TransactionContext,
    ) -> Result<(), SdkExecutionDiagnostic> {
        commit_context_sdk(&self.state.runtime.runtime, context)
    }

    #[cfg(test)]
    pub(crate) fn from_test_database(package: Arc<SchemaPackageState>, database: Database) -> Self {
        Self::from_test_database_with_answer_ceiling(package, database, None)
    }

    #[cfg(test)]
    pub(crate) fn from_test_database_with_answer_ceiling(
        package: Arc<SchemaPackageState>,
        database: Database,
        answer_ceiling: Option<QueryExecutionResourceLimits>,
    ) -> Self {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the C ABI test runtime builds");
        let runtime = Arc::new(RuntimeState {
            runtime,
            connector: Arc::new(TypeDbConnector),
            database_children: AtomicUsize::new(1),
        });
        Self {
            state: Arc::new(DatabaseState {
                runtime,
                _package: package,
                database: Arc::new(database),
                server_version: b"3.12.1".to_vec(),
                answer_ceiling,
                transaction_children: AtomicUsize::new(0),
            }),
        }
    }
}

impl TypeBridgeReadTransaction {
    #[cfg(test)]
    pub(crate) fn from_test_context_with_answer_ceiling(
        database: &TypeBridgeDatabase,
        context: TransactionContext,
        answer_ceiling: Option<QueryExecutionResourceLimits>,
    ) -> Self {
        increment_child(&database.state.transaction_children)
            .expect("the C ABI test transaction child count remains bounded");
        Self {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling,
                poisoned: AtomicBool::new(false),
            },
        }
    }

    pub(crate) fn package_state(&self) -> &Arc<SchemaPackageState> {
        &self.state.database._package
    }

    pub(crate) fn context(&self) -> Option<&TransactionContext> {
        self.state.context.as_ref()
    }

    pub(crate) fn answer_ceiling(&self) -> QueryExecutionResourceLimits {
        self.state.answer_ceiling.unwrap_or_default()
    }

    pub(crate) fn policy_answer_ceiling(&self) -> Option<QueryExecutionResourceLimits> {
        self.state.answer_ceiling
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.state.poisoned.load(Ordering::Acquire)
    }

    pub(crate) fn mark_poisoned(&self) {
        self.state.poisoned.store(true, Ordering::Release);
    }

    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.state.database.runtime.runtime.block_on(future)
    }
}

impl TypeBridgeWriteTransaction {
    #[cfg(test)]
    pub(crate) fn from_test_context_with_answer_ceiling(
        database: &TypeBridgeDatabase,
        context: TransactionContext,
        answer_ceiling: Option<QueryExecutionResourceLimits>,
    ) -> Self {
        increment_child(&database.state.transaction_children)
            .expect("the C ABI test transaction child count remains bounded");
        Self {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling,
                poisoned: AtomicBool::new(false),
            },
        }
    }

    pub(crate) fn package_state(&self) -> &Arc<SchemaPackageState> {
        &self.state.database._package
    }

    pub(crate) fn context(&self) -> Option<&TransactionContext> {
        self.state.context.as_ref()
    }

    pub(crate) fn answer_ceiling(&self) -> QueryExecutionResourceLimits {
        self.state.answer_ceiling.unwrap_or_default()
    }

    pub(crate) fn policy_answer_ceiling(&self) -> Option<QueryExecutionResourceLimits> {
        self.state.answer_ceiling
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.state.poisoned.load(Ordering::Acquire)
    }

    pub(crate) fn mark_poisoned(&self) {
        self.state.poisoned.store(true, Ordering::Release);
    }

    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.state.database.runtime.runtime.block_on(future)
    }
}

impl TypeBridgeCancellation {
    pub(crate) fn is_requested_before_dispatch(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    pub(crate) fn answer_cancellation(&self) -> type_bridge_orm::AnswerCancellation {
        self.answer.clone()
    }
}

fn rollback_only_commit_diagnostic(
    runtime: &Runtime,
    context: &TransactionContext,
) -> Option<SdkExecutionDiagnostic> {
    runtime.block_on(async {
        if context.lifecycle_state().await != TransactionContextState::RollbackOnly {
            return None;
        }
        Some(
            context
                .commit_sdk()
                .await
                .expect_err("a rollback-only context cannot dispatch provider commit"),
        )
    })
}

fn commit_context_sdk(
    runtime: &Runtime,
    context: &TransactionContext,
) -> Result<(), SdkExecutionDiagnostic> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(context.commit_sdk())
    })) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(diagnostic)) => Err(diagnostic),
        Err(_) => Err(SdkExecutionDiagnostic::commit_failure(
            SdkCommitFailureOutcome::Unknown,
        )),
    }
}

fn commit_context_sdk_controlled(
    runtime: &Runtime,
    context: &TransactionContext,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<(), SdkExecutionDiagnostic> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let future = context.commit_sdk();
            tokio::pin!(future);
            tokio::select! {
                biased;
                result = &mut future => result,
                _ = cancellation.cancelled() => {
                    cancellation.cancel();
                    future.await
                },
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.instant())) => {
                    future.await
                },
            }
        })
    })) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(diagnostic)) => Err(diagnostic),
        Err(_) => Err(SdkExecutionDiagnostic::commit_failure(
            SdkCommitFailureOutcome::Unknown,
        )),
    }
}

struct DatabaseConnectInput {
    address: String,
    database: String,
    username: String,
    password: String,
    http_port: u16,
    tls_native_roots: bool,
}

struct DatabaseConnectInputV2 {
    address: String,
    database: String,
    username: String,
    password: String,
    transport: PreparedSecureConnectOptions,
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

struct DatabaseConfigV2Preflight {
    config: TypeBridgeDatabaseConfigV2,
    connection_limits: QueryExecutionResourceLimits,
    answer_limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
}

struct PreparedDatabaseConfigV2 {
    address: String,
    database: String,
    username: String,
    password: String,
    transport: PreparedSecureConnectOptions,
    connection_limits: QueryExecutionResourceLimits,
    answer_limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C execution diagnostic code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C execution diagnostic message is valid")
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn unsupported(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::unsupported_capability(code(code_value), message(message_value))
}

fn resource_limit(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(code(code_value), message(message_value))
}

fn lifecycle_statement_limit() -> SdkExecutionDiagnostic {
    resource_limit(
        "provider_statement_limit",
        "The transaction lifecycle operation exceeds its provider dispatch ceiling",
    )
}

fn require_lifecycle_dispatch(
    limits: QueryExecutionResourceLimits,
) -> Result<(), SdkExecutionDiagnostic> {
    if limits.effective().statements == 0 {
        Err(lifecycle_statement_limit())
    } else {
        Ok(())
    }
}

fn tls_configuration_error(error: &SecureConnectError) -> SdkExecutionDiagnostic {
    let code_value = error
        .configuration_code()
        .unwrap_or("c_database_tls_configuration_invalid");
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new(code_value)
            .expect("typed TLS configuration codes are canonical SDK diagnostic codes"),
        message("The database TLS policy or captured trust material is invalid"),
    )
}

fn lower_secure_connect_error(error: SecureConnectError) -> SdkExecutionDiagnostic {
    if error.configuration_code().is_some() {
        return tls_configuration_error(&error);
    }
    let error: OrmError = error.into_runtime_error().into();
    lower_execution_error(error, SdkProviderOperation::Connect)
}

fn in_use_diagnostic() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_provider_parent_in_use",
        "The provider parent cannot close while child handles remain open",
    )
}

pub(crate) fn poisoned_transaction_diagnostic() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::transaction_failure(
        code("c_transaction_poisoned"),
        message(
            "A provider panic poisoned this transaction; only rollback or close remains permitted",
        ),
    )
}

fn initialize_diagnostics_output(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: the C contract requires a writable output slot.
    unsafe { out_diagnostics.write(ptr::null_mut()) };
    Ok(())
}

fn check_preflight_object<T>(
    preflight: &DirectOutputPreflight,
    pointer: *const T,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        Ok(())
    } else {
        preflight.check_bytes(pointer.cast(), size_of::<T>())
    }
}

unsafe fn check_database_config_v2_ranges(
    preflight: &DirectOutputPreflight,
    config: *const TypeBridgeDatabaseConfigV2,
) -> Result<(), TypeBridgeStatus> {
    check_preflight_object(preflight, config)?;
    if config.is_null() {
        return Ok(());
    }
    // SAFETY: the complete outer descriptor was fenced before this unaligned read.
    let config = unsafe { config.read_unaligned() };
    for view in [
        config.address,
        config.database,
        config.username,
        config.password,
        config.custom_root_ca_pem,
    ] {
        preflight.check_bytes(view.data.cast(), view.length)?;
    }
    Ok(())
}

unsafe fn check_package_ranges(
    preflight: &DirectOutputPreflight,
    package: *const TypeBridgeSchemaPackage,
) -> Result<(), TypeBridgeStatus> {
    check_preflight_object(preflight, package)?;
    if !package.is_null() {
        // SAFETY: the complete outer package handle was fenced above.
        preflight.check_package_borrowed_ranges(unsafe { &*package }.state())?;
    }
    Ok(())
}

pub(crate) fn check_database_borrowed_ranges(
    preflight: &DirectOutputPreflight,
    database: &TypeBridgeDatabase,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_package_borrowed_ranges(database.package_state())?;
    preflight.check_bytes(
        database.state.server_version.as_ptr().cast(),
        database.state.server_version.len(),
    )
}

pub(crate) fn check_read_transaction_borrowed_ranges(
    preflight: &DirectOutputPreflight,
    transaction: &TypeBridgeReadTransaction,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_package_borrowed_ranges(transaction.package_state())?;
    preflight.check_bytes(
        transaction.state.database.server_version.as_ptr().cast(),
        transaction.state.database.server_version.len(),
    )
}

pub(crate) fn check_write_transaction_borrowed_ranges(
    preflight: &DirectOutputPreflight,
    transaction: &TypeBridgeWriteTransaction,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_package_borrowed_ranges(transaction.package_state())?;
    preflight.check_bytes(
        transaction.state.database.server_version.as_ptr().cast(),
        transaction.state.database.server_version.len(),
    )
}

fn terminal_output_preflight<T>(
    owner: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    if owner.is_null() || out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    direct_output_preflight(&[
        (owner.cast(), size_of::<*mut T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])
}

unsafe fn check_database_transaction_open_ranges<T>(
    database: *const TypeBridgeDatabase,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if database.is_null()
        // Preserve the released V1 equality-only alias contract when no live
        // policy-bearing target exists.
        || unsafe { &*database }.policy_answer_ceiling().is_none()
    {
        if aliases(database, out_transaction)
            || aliases(database, out_diagnostics)
            || aliases(cancellation, out_transaction)
            || aliases(cancellation, out_diagnostics)
        {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        return Ok(());
    }
    let outputs = terminal_output_preflight(out_transaction, out_diagnostics)?;
    check_preflight_object(&outputs, database)?;
    check_preflight_object(&outputs, cancellation)?;
    if !database.is_null() {
        // SAFETY: the complete database handle was fenced above and remains
        // immutable throughout the constructor call.
        check_database_borrowed_ranges(&outputs, unsafe { &*database })?;
    }
    Ok(())
}

unsafe fn check_policy_terminal_ranges<T>(
    owner: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    has_policy: impl FnOnce(&T) -> bool,
    check_borrowed_ranges: impl FnOnce(&DirectOutputPreflight, &T) -> Result<(), TypeBridgeStatus>,
) -> Result<(), TypeBridgeStatus> {
    if owner.is_null() || out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if owner.cast::<()>() == out_diagnostics.cast::<()>() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: a terminal caller promises one readable/writable owner slot.
    let value = unsafe { owner.read_unaligned() };
    if value.is_null() {
        return Ok(());
    }
    // SAFETY: a non-null owner slot retains one live handle until dispatch.
    let handle = unsafe { &*value };
    if !has_policy(handle) {
        return Ok(());
    }
    let outputs = terminal_output_preflight(owner, out_diagnostics)?;
    check_preflight_object(&outputs, value)?;
    check_borrowed_ranges(&outputs, handle)
}

unsafe fn check_policy_database_terminal_ranges(
    database: *mut *mut TypeBridgeDatabase,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: the shared helper performs no writes and fences a V2-origin
    // database's complete retained graph before its terminal may write.
    unsafe {
        check_policy_terminal_ranges(
            database,
            out_diagnostics,
            |value| value.policy_answer_ceiling().is_some(),
            check_database_borrowed_ranges,
        )
    }
}

unsafe fn check_policy_read_transaction_terminal_ranges(
    transaction: *mut *mut TypeBridgeReadTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: the shared helper performs no writes and fences a V2-origin
    // transaction's complete retained graph before its terminal may write.
    unsafe {
        check_policy_terminal_ranges(
            transaction,
            out_diagnostics,
            |value| value.policy_answer_ceiling().is_some(),
            check_read_transaction_borrowed_ranges,
        )
    }
}

unsafe fn check_policy_write_transaction_terminal_ranges(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: the shared helper performs no writes and fences a V2-origin
    // transaction's complete retained graph before its terminal may write.
    unsafe {
        check_policy_terminal_ranges(
            transaction,
            out_diagnostics,
            |value| value.policy_answer_ceiling().is_some(),
            check_write_transaction_borrowed_ranges,
        )
    }
}

unsafe fn check_write_transaction_commit_ranges(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if transaction.is_null() || out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if transaction.cast::<()>() == out_diagnostics.cast::<()>()
        || (!cancellation.is_null()
            && (cancellation.cast::<()>() == transaction.cast::<()>()
                || cancellation.cast::<()>() == out_diagnostics.cast::<()>()))
    {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: a commit caller promises one readable/writable owner slot.
    let value = unsafe { transaction.read_unaligned() };
    if value.is_null()
        // A null owner has no V2 policy graph; retain the released terminal path.
        || unsafe { &*value }.policy_answer_ceiling().is_none()
    {
        return Ok(());
    }
    let outputs = terminal_output_preflight(transaction, out_diagnostics)?;
    check_preflight_object(&outputs, cancellation)?;
    check_preflight_object(&outputs, value)?;
    if !value.is_null() {
        // SAFETY: the complete live handle was fenced above and remains owned
        // by the caller until all pre-dispatch checks have succeeded.
        check_write_transaction_borrowed_ranges(&outputs, unsafe { &*value })?;
    }
    Ok(())
}

fn aliases<Input, Output>(input: *const Input, output: *mut Output) -> bool {
    !input.is_null() && !output.is_null() && input.cast::<()>() == output.cast::<()>()
}

unsafe fn initialize_handle_outputs<T>(
    out_handle: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if !out_handle.is_null() {
        // SAFETY: a non-null output slot is writable by the C caller contract.
        unsafe { out_handle.write(ptr::null_mut()) };
    }
    initialize_diagnostics_output(out_diagnostics)?;
    if out_handle.is_null() || out_handle.cast::<()>() == out_diagnostics.cast::<()>() {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

fn return_with_status(
    status: TypeBridgeStatus,
    diagnostic: SdkExecutionDiagnostic,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: every caller initializes this non-null output independently.
    unsafe {
        out_diagnostics.write(Box::into_raw(Box::new(execution_diagnostics_handle(vec![
            diagnostic,
        ]))))
    };
    status
}

fn is_cancelled(cancellation: *const TypeBridgeCancellation) -> bool {
    if cancellation.is_null() {
        return false;
    }
    // SAFETY: the caller retains a live cancellation handle during this call.
    unsafe { &*cancellation }.requested.load(Ordering::Acquire)
}

fn invocation_cancellation(cancellation: *const TypeBridgeCancellation) -> AnswerCancellation {
    if cancellation.is_null() {
        return AnswerCancellation::default();
    }
    // SAFETY: callers retain the live cancellation handle for the invocation.
    unsafe { &*cancellation }.answer_cancellation()
}

fn check_control(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<(), SdkExecutionDiagnostic> {
    if cancellation.is_cancelled() {
        Err(SdkExecutionDiagnostic::data_operation_cancelled())
    } else if deadline.is_expired() {
        Err(SdkExecutionDiagnostic::data_operation_deadline_exceeded())
    } else {
        Ok(())
    }
}

enum ControlledAwaitError<E> {
    Interrupted(SdkExecutionDiagnostic),
    Inner(E),
}

async fn await_controlled<F, T, E>(
    future: F,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<T, ControlledAwaitError<E>>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => result.map_err(ControlledAwaitError::Inner),
        _ = cancellation.cancelled() => Err(ControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_cancelled())),
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.instant())) => Err(ControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_deadline_exceeded())),
    }
}

fn cancellation_error(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    return_execution_error(
        SdkExecutionDiagnostic::cancelled_before_dispatch(),
        out_diagnostics,
    )
}

unsafe fn copied_utf8(
    view: TypeBridgeByteView,
    maximum: usize,
    allow_empty: bool,
) -> Result<String, SdkExecutionDiagnostic> {
    if view.length > maximum {
        return Err(resource_limit(
            "c_database_config_text_limit_exceeded",
            "A database configuration value exceeds its stable byte ceiling",
        ));
    }
    if view.length == 0 {
        return if allow_empty {
            Ok(String::new())
        } else {
            Err(invalid_input(
                "c_database_config_text_empty",
                "A required database configuration value is empty",
            ))
        };
    }
    if view.data.is_null() {
        return Err(invalid_input(
            "c_database_config_view_invalid",
            "A database configuration byte view is invalid",
        ));
    }
    // SAFETY: the caller promises readable bytes for this call; length is bounded above.
    let copied = unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec();
    let value = String::from_utf8(copied).map_err(|_| {
        invalid_input(
            "c_database_config_utf8_invalid",
            "A database configuration value is not valid UTF-8",
        )
    })?;
    if value.as_bytes().contains(&0) {
        return Err(invalid_input(
            "c_database_config_nul_invalid",
            "A database configuration value contains a NUL byte",
        ));
    }
    Ok(value)
}

unsafe fn snapshot_database_config(
    config: *const TypeBridgeDatabaseConfigV1,
) -> Result<DatabaseConnectInput, SdkExecutionDiagnostic> {
    if config.is_null() {
        return Err(invalid_input(
            "c_database_config_missing",
            "The database configuration is required",
        ));
    }
    // SAFETY: unaligned reads admit hostile-but-readable C descriptor storage.
    let config = unsafe { config.read_unaligned() };
    if config.struct_size as usize != size_of::<TypeBridgeDatabaseConfigV1>()
        || config.version != DATABASE_CONFIG_VERSION
        || config.reserved != [0; 4]
    {
        return Err(invalid_input(
            "c_database_config_layout_invalid",
            "The database configuration layout is invalid",
        ));
    }
    let http_port = u16::try_from(config.http_port)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            invalid_input(
                "c_database_config_http_port_invalid",
                "The database HTTP probe port is invalid",
            )
        })?;
    let tls_native_roots = match config.tls_mode {
        TLS_DISABLED => false,
        TLS_NATIVE_ROOTS => true,
        _ => {
            return Err(invalid_input(
                "c_database_config_tls_mode_invalid",
                "The database TLS mode is invalid",
            ));
        }
    };
    // SAFETY: the address is copied under its stable ceiling before grammar checks.
    let address = unsafe { copied_utf8(config.address, MAX_ADDRESS_BYTES, false) }?;
    if !is_identity_safe_provider_address(&address) {
        return Err(invalid_input(
            "c_database_address_invalid",
            "The database address is not a canonical credential-free provider endpoint",
        ));
    }
    if address.contains(',') {
        return Err(unsupported(
            "c_database_multi_endpoint_unsupported",
            "C database configuration version 1 supports exactly one provider endpoint",
        ));
    }
    Ok(DatabaseConnectInput {
        address,
        // SAFETY: every input is copied under a stable ceiling before dispatch.
        database: unsafe { copied_utf8(config.database, MAX_DATABASE_NAME_BYTES, false) }?,
        // SAFETY: every input is copied under a stable ceiling before dispatch.
        username: unsafe { copied_utf8(config.username, MAX_USERNAME_BYTES, false) }?,
        // SAFETY: every input is copied under a stable ceiling before dispatch.
        password: unsafe { copied_utf8(config.password, MAX_PASSWORD_BYTES, true) }?,
        http_port,
        tls_native_roots,
    })
}

unsafe fn preflight_database_config_v2(
    config: *const TypeBridgeDatabaseConfigV2,
) -> Result<DatabaseConfigV2Preflight, SdkExecutionDiagnostic> {
    if config.is_null() {
        return Err(invalid_input(
            "c_database_config_missing",
            "The database configuration is required",
        ));
    }
    // SAFETY: unaligned reads admit hostile-but-readable C descriptor storage.
    let config = unsafe { config.read_unaligned() };
    if config.struct_size as usize != size_of::<TypeBridgeDatabaseConfigV2>()
        || config.version != DATABASE_CONFIG_V2_VERSION
        || config.reserved != [0; 4]
    {
        return Err(invalid_input(
            "c_database_config_layout_invalid",
            "The database configuration layout is invalid",
        ));
    }
    let connection_limits = parse_common_limits(Some(config.connection_limits))?;
    let answer_limits = parse_common_limits(Some(config.answer_limits))?;
    let deadline = QueryExecutionDeadline::for_limits(connection_limits);
    Ok(DatabaseConfigV2Preflight {
        config,
        connection_limits,
        answer_limits,
        deadline,
    })
}

unsafe fn parse_call_limits(
    limits: *const TypeBridgeQueryExecutionLimitsV1,
) -> Result<QueryExecutionResourceLimits, SdkExecutionDiagnostic> {
    let value = if limits.is_null() {
        None
    } else {
        // SAFETY: unaligned reads admit hostile-but-readable C descriptor storage.
        Some(unsafe { limits.read_unaligned() })
    };
    parse_common_limits(value)
}

unsafe fn copied_custom_root(
    view: TypeBridgeByteView,
    tls_mode: u32,
) -> Result<Option<Arc<[u8]>>, SdkExecutionDiagnostic> {
    if tls_mode != TLS_CUSTOM_ROOT_CA {
        if view.length != 0 || !view.data.is_null() {
            return Err(invalid_input(
                "c_database_custom_root_must_be_empty",
                "Custom-root bytes must be canonical-empty outside custom-root TLS mode",
            ));
        }
        return Ok(None);
    }
    if view.length == 0 {
        return Err(invalid_input(
            "c_database_custom_root_missing",
            "Custom-root TLS mode requires a nonempty captured PEM bundle",
        ));
    }
    if view.length > DATABASE_CUSTOM_ROOT_CA_BYTES_MAX {
        return Err(resource_limit(
            "c_database_custom_root_limit_exceeded",
            "The captured custom-root PEM bundle exceeds its stable byte ceiling",
        ));
    }
    if view.data.is_null() {
        return Err(invalid_input(
            "c_database_custom_root_view_invalid",
            "The captured custom-root PEM byte view is invalid",
        ));
    }
    // SAFETY: the caller promises a readable range and its length is now bounded.
    let source = unsafe { std::slice::from_raw_parts(view.data, view.length) };
    if std::str::from_utf8(source).is_err() {
        return Err(invalid_input(
            "tls_custom_root_ca_invalid_pem",
            "The captured custom-root PEM bundle is not valid UTF-8 PEM",
        ));
    }
    let mut copied = Vec::new();
    copied.try_reserve_exact(source.len()).map_err(|_| {
        resource_limit(
            "c_allocation_exhausted",
            "The C runtime could not allocate bounded database configuration storage",
        )
    })?;
    copied.extend_from_slice(source);
    Ok(Some(Arc::from(copied.into_boxed_slice())))
}

unsafe fn prepare_database_config_v2(
    preflight: DatabaseConfigV2Preflight,
    cancellation: Option<&AnswerCancellation>,
) -> Result<PreparedDatabaseConfigV2, SdkExecutionDiagnostic> {
    if let Some(cancellation) = cancellation {
        check_control(preflight.deadline, cancellation)?;
    }

    let http_port = u16::try_from(preflight.config.http_port)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            invalid_input(
                "c_database_config_http_port_invalid",
                "The database HTTP probe port is invalid",
            )
        })?;
    let tls_mode = match preflight.config.tls_mode {
        TLS_DISABLED => TlsMode::Disabled,
        TLS_NATIVE_ROOTS => TlsMode::NativeRoots,
        TLS_CUSTOM_ROOT_CA => {
            TlsMode::CustomRootCa(PathBuf::from("typebridge-c-captured-custom-root.pem"))
        }
        _ => {
            return Err(invalid_input(
                "c_database_config_tls_mode_invalid",
                "The database TLS mode is invalid",
            ));
        }
    };

    // SAFETY: configuration text is copied under its stable ceiling before use.
    let address = unsafe { copied_utf8(preflight.config.address, MAX_ADDRESS_BYTES, false) }?;
    if !is_identity_safe_provider_address(&address) {
        return Err(invalid_input(
            "c_database_address_invalid",
            "The database address is not a canonical credential-free provider endpoint",
        ));
    }
    if address.contains(',') {
        return Err(unsupported(
            "c_database_multi_endpoint_unsupported",
            "C database configuration version 2 supports exactly one provider endpoint",
        ));
    }
    // SAFETY: the database name is copied under its stable ceiling before use.
    let database =
        unsafe { copied_utf8(preflight.config.database, MAX_DATABASE_NAME_BYTES, false) }?;

    // SAFETY: custom trust is copied once before credentials are inspected.
    let custom_root = unsafe {
        copied_custom_root(
            preflight.config.custom_root_ca_pem,
            preflight.config.tls_mode,
        )
    }?;
    let options = SecureConnectOptions {
        http_port,
        tls_mode,
        server_version: None,
    };
    let transport = match custom_root {
        Some(bytes) => options.prepare_transport_from_captured_custom_root(bytes),
        None => options.prepare_transport(),
    }
    .map_err(|error| tls_configuration_error(&error))?;
    if let Some(cancellation) = cancellation {
        check_control(preflight.deadline, cancellation)?;
    }

    // Credential bytes are copied only after layout, limits, endpoint, and trust validation.
    // SAFETY: both inputs are copied under stable ceilings before provider dispatch.
    let username = unsafe { copied_utf8(preflight.config.username, MAX_USERNAME_BYTES, false) }?;
    // SAFETY: an empty password remains an admitted exact copied value.
    let password = unsafe { copied_utf8(preflight.config.password, MAX_PASSWORD_BYTES, true) }?;
    if let Some(cancellation) = cancellation {
        check_control(preflight.deadline, cancellation)?;
    }

    Ok(PreparedDatabaseConfigV2 {
        address,
        database,
        username,
        password,
        transport,
        connection_limits: preflight.connection_limits,
        answer_limits: preflight.answer_limits,
        deadline: preflight.deadline,
    })
}

fn increment_child(counter: &AtomicUsize) -> Result<(), SdkExecutionDiagnostic> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(1)
        })
        .map(|_| ())
        .map_err(|_| {
            resource_limit(
                "c_provider_child_limit_exceeded",
                "The provider child handle counter exceeded its stable range",
            )
        })
}

fn package_is_exact_for_database(package: &SchemaPackageState) -> bool {
    package.semantic_profile == REQUIRED_SEMANTIC_PROFILE
}

fn open_runtime(
    worker_threads: usize,
    connector: Arc<dyn DatabaseConnector>,
) -> Result<TypeBridgeRuntime, SdkExecutionDiagnostic> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
    Ok(TypeBridgeRuntime {
        state: Arc::new(RuntimeState {
            runtime,
            connector,
            database_children: AtomicUsize::new(0),
        }),
    })
}

/// Open one hidden Tokio multi-thread runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_runtime_open_v1(
    config: *const TypeBridgeRuntimeConfigV1,
    out_runtime: *mut *mut TypeBridgeRuntime,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if !config.is_null()
        && (!out_runtime.is_null() && config.cast::<()>() == out_runtime.cast::<()>()
            || !out_diagnostics.is_null() && config.cast::<()>() == out_diagnostics.cast::<()>())
    {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_runtime, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if config.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: unaligned reads admit hostile-but-readable configuration storage.
        let config = unsafe { config.read_unaligned() };
        if config.struct_size as usize != size_of::<TypeBridgeRuntimeConfigV1>()
            || config.version != RUNTIME_CONFIG_VERSION
            || config.reserved0 != 0
            || config.reserved != [0; 4]
        {
            return return_execution_error(
                invalid_input(
                    "c_runtime_config_layout_invalid",
                    "The runtime configuration layout is invalid",
                ),
                out_diagnostics,
            );
        }
        if !(MIN_RUNTIME_WORKER_THREADS..=MAX_RUNTIME_WORKER_THREADS)
            .contains(&config.worker_threads)
        {
            return return_execution_error(
                resource_limit(
                    "c_runtime_worker_limit_invalid",
                    "The runtime worker count is outside its stable range",
                ),
                out_diagnostics,
            );
        }
        match open_runtime(config.worker_threads as usize, Arc::new(TypeDbConnector)) {
            Ok(runtime) => {
                // SAFETY: the initialized output remains writable.
                unsafe { out_runtime.write(Box::into_raw(Box::new(runtime))) };
                TypeBridgeStatus::Ok
            }
            Err(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        }
    })
}

/// Close a runtime only after every database child has closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_runtime_close(
    runtime: *mut *mut TypeBridgeRuntime,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if runtime.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if runtime.cast::<()>() == out_diagnostics.cast::<()>() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        // SAFETY: the caller owns this writable handle slot.
        let value = unsafe { runtime.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // SAFETY: the pointer belongs to this exact handle family.
        let handle = unsafe { &*value };
        if handle.state.database_children.load(Ordering::Acquire) != 0 {
            return return_with_status(
                TypeBridgeStatus::InUse,
                in_use_diagnostic(),
                out_diagnostics,
            );
        }
        // Clear before destruction so a contained destructor panic cannot leave a dangling pointer.
        unsafe { runtime.write(ptr::null_mut()) };
        // SAFETY: ownership returns to Rust exactly once.
        unsafe { drop(Box::from_raw(value)) };
        TypeBridgeStatus::Ok
    })
}

/// Open one reusable cancellation signal in the not-requested state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_cancellation_open(
    out_cancellation: *mut *mut TypeBridgeCancellation,
) -> TypeBridgeStatus {
    if out_cancellation.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: the caller supplies a writable output slot.
    unsafe { out_cancellation.write(ptr::null_mut()) };
    guarded(|| {
        // SAFETY: the initialized output remains writable.
        unsafe {
            out_cancellation.write(Box::into_raw(Box::new(TypeBridgeCancellation {
                requested: AtomicBool::new(false),
                answer: type_bridge_orm::AnswerCancellation::default(),
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Request sticky cancellation for pre-dispatch checks and in-flight typed-query answers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_cancellation_request(
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    guarded(|| {
        if cancellation.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller retains a live cancellation handle for this call.
        let cancellation = unsafe { &*cancellation };
        cancellation.requested.store(true, Ordering::Release);
        cancellation.answer.cancel();
        TypeBridgeStatus::Ok
    })
}

/// Return whether cancellation was requested as exact zero or one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_cancellation_is_requested(
    cancellation: *const TypeBridgeCancellation,
    out_requested: *mut u8,
) -> TypeBridgeStatus {
    let preflight = match direct_output_preflight(&[(out_requested.cast(), size_of::<u8>())]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if !cancellation.is_null()
        && let Err(status) =
            preflight.check_bytes(cancellation.cast(), size_of::<TypeBridgeCancellation>())
    {
        return status;
    }
    if !out_requested.is_null() {
        // SAFETY: complete-object alias preflight proved the output writable and disjoint.
        unsafe { out_requested.write(0) };
    }
    guarded(|| {
        if cancellation.is_null() || out_requested.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let requested = is_cancelled(cancellation);
        // SAFETY: the initialized output remains writable.
        unsafe { out_requested.write(u8::from(requested)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one cancellation handle and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_cancellation_close(
    cancellation: *mut *mut TypeBridgeCancellation,
) -> TypeBridgeStatus {
    guarded(|| {
        if cancellation.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller owns this exact handle slot.
        let value = unsafe { cancellation.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        unsafe { cancellation.write(ptr::null_mut()) };
        // SAFETY: ownership returns to Rust exactly once.
        unsafe { drop(Box::from_raw(value)) };
        TypeBridgeStatus::Ok
    })
}

/// Validate and snapshot one version-2 database policy without provider I/O.
#[unsafe(export_name = "type_bridge_database_config_validate_v2")]
pub(crate) unsafe extern "C" fn type_bridge_database_config_validate_v2_impl(
    config: *const TypeBridgeDatabaseConfigV2,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the complete descriptor and every caller-owned byte view are
    // fenced before the diagnostic slot is initialized.
    if let Err(status) = unsafe { check_database_config_v2_ranges(&outputs, config) } {
        return status;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        // SAFETY: the caller retains every configuration byte view for this call.
        let preflight = match unsafe { preflight_database_config_v2(config) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: preparation copies all caller-owned bytes before returning.
        // Validation deliberately does not execute the connection timeout: it
        // performs no provider work and has no cancellation argument.
        match unsafe { prepare_database_config_v2(preflight, None) } {
            Ok(_) => TypeBridgeStatus::Ok,
            Err(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        }
    })
}

/// Connect through one captured version-2 policy.
#[unsafe(export_name = "type_bridge_database_open_v2")]
pub(crate) unsafe extern "C" fn type_bridge_database_open_v2_impl(
    runtime: *const TypeBridgeRuntime,
    package: *const TypeBridgeSchemaPackage,
    config: *const TypeBridgeDatabaseConfigV2,
    cancellation: *const TypeBridgeCancellation,
    out_database: *mut *mut TypeBridgeDatabase,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (out_database.cast(), size_of::<*mut TypeBridgeDatabase>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        check_preflight_object(&outputs, runtime),
        // SAFETY: the complete package handle is checked before its retained bytes.
        unsafe { check_package_ranges(&outputs, package) },
        // SAFETY: the complete configuration is checked before its nested views.
        unsafe { check_database_config_v2_ranges(&outputs, config) },
        check_preflight_object(&outputs, cancellation),
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_database, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if runtime.is_null() || package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // Capture the sole absolute connection deadline before trust preparation,
        // credential copying, or provider dispatch.
        // SAFETY: caller retains the configuration and its nested views.
        let preflight = match unsafe { preflight_database_config_v2(config) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: caller retains both immutable parent handles during the call.
        let runtime = unsafe { &*runtime };
        let package = unsafe { &*package };
        if !package_is_exact_for_database(package.state()) {
            return return_execution_error(
                unsupported(
                    "c_database_semantic_profile_unsupported",
                    "The verified schema package targets a semantic profile unsupported by C database configuration version 2",
                ),
                out_diagnostics,
            );
        }
        let cancellation = invocation_cancellation(cancellation);
        // SAFETY: preparation copies all caller-owned views before provider dispatch.
        let prepared = match unsafe { prepare_database_config_v2(preflight, Some(&cancellation)) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let deadline = prepared.deadline;
        let answer_limits = prepared.answer_limits;
        let connection_limits = prepared.connection_limits;
        let handle_reservation = match ReservedBox::try_new(AllocationSite::DatabaseHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        if let Err(diagnostic) = check_control(deadline, &cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let future = runtime.state.connector.connect_v2(DatabaseConnectInputV2 {
            address: prepared.address,
            database: prepared.database,
            username: prepared.username,
            password: prepared.password,
            transport: prepared.transport,
            limits: connection_limits,
            deadline,
            cancellation: cancellation.clone(),
        });
        let database =
            match runtime
                .state
                .runtime
                .block_on(await_controlled(future, deadline, &cancellation))
            {
                Ok(value) => value,
                Err(
                    ControlledAwaitError::Interrupted(diagnostic)
                    | ControlledAwaitError::Inner(diagnostic),
                ) => {
                    return return_execution_error(diagnostic, out_diagnostics);
                }
            };
        if !database
            .server_version()
            .is_some_and(|version| version.major == 3 && version.minor == 12 && version.patch == 1)
        {
            let _ = database.close();
            return return_execution_error(
                unsupported(
                    "c_database_server_version_not_exact",
                    "The C database ABI requires an authoritative TypeDB 3.12.1 server",
                ),
                out_diagnostics,
            );
        }
        if database.check_given_stage_support().is_err() {
            let _ = database.close();
            return return_execution_error(
                unsupported(
                    "c_database_given_rows_unsupported",
                    "C database configuration version 2 requires the active band-9 GivenRows provider capability",
                ),
                out_diagnostics,
            );
        }
        if let Err(diagnostic) = increment_child(&runtime.state.database_children) {
            let _ = database.close();
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let state = DatabaseState {
            runtime: Arc::clone(&runtime.state),
            _package: Arc::clone(package.state()),
            database: Arc::new(database),
            server_version: b"3.12.1".to_vec(),
            answer_ceiling: Some(answer_limits),
            transaction_children: AtomicUsize::new(0),
        };
        // SAFETY: the initialized output remains writable.
        unsafe {
            out_database.write(Box::into_raw(handle_reservation.initialize(
                TypeBridgeDatabase {
                    state: Arc::new(state),
                },
            )))
        };
        TypeBridgeStatus::Ok
    })
}

/// Connect and bind one exact TypeDB 3.12.1 database to a verified C package.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_open_v1(
    runtime: *const TypeBridgeRuntime,
    package: *const TypeBridgeSchemaPackage,
    config: *const TypeBridgeDatabaseConfigV1,
    cancellation: *const TypeBridgeCancellation,
    out_database: *mut *mut TypeBridgeDatabase,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if aliases(runtime, out_database)
        || aliases(runtime, out_diagnostics)
        || aliases(package, out_database)
        || aliases(package, out_diagnostics)
        || aliases(config, out_database)
        || aliases(config, out_diagnostics)
        || aliases(cancellation, out_database)
        || aliases(cancellation, out_diagnostics)
    {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_database, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if runtime.is_null() || package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: configuration bytes are copied and validated before provider dispatch.
        let input = match unsafe { snapshot_database_config(config) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: caller retains both immutable parent handles during the call.
        let runtime = unsafe { &*runtime };
        let package = unsafe { &*package };
        if !package_is_exact_for_database(package.state()) {
            return return_execution_error(
                unsupported(
                    "c_database_semantic_profile_unsupported",
                    "The verified schema package targets a semantic profile unsupported by C database configuration version 1",
                ),
                out_diagnostics,
            );
        }
        if is_cancelled(cancellation) {
            return cancellation_error(out_diagnostics);
        }

        // Cancellation is deliberately not observed after this dispatch point.
        let database = match runtime
            .state
            .runtime
            .block_on(runtime.state.connector.connect(input))
        {
            Ok(value) => value,
            Err(error) => {
                return return_execution_error(
                    lower_execution_error(error, SdkProviderOperation::Connect),
                    out_diagnostics,
                );
            }
        };
        if !database
            .server_version()
            .is_some_and(|version| version.major == 3 && version.minor == 12 && version.patch == 1)
        {
            let _ = database.close();
            return return_execution_error(
                unsupported(
                    "c_database_server_version_not_exact",
                    "The C database ABI requires an authoritative TypeDB 3.12.1 server",
                ),
                out_diagnostics,
            );
        }
        if let Err(diagnostic) = increment_child(&runtime.state.database_children) {
            let _ = database.close();
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let state = DatabaseState {
            runtime: Arc::clone(&runtime.state),
            _package: Arc::clone(package.state()),
            database: Arc::new(database),
            server_version: b"3.12.1".to_vec(),
            answer_ceiling: None,
            transaction_children: AtomicUsize::new(0),
        };
        // SAFETY: the initialized output remains writable.
        unsafe {
            out_database.write(Box::into_raw(Box::new(TypeBridgeDatabase {
                state: Arc::new(state),
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Borrow the authoritative exact server version until the database closes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_server_version(
    database: *const TypeBridgeDatabase,
    out_version: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    if !database.is_null()
        && !out_version.is_null()
        && database.cast::<()>() == out_version.cast::<()>()
    {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: initialize before inspecting the input handle.
        if let Err(status) = unsafe { initialize_view(out_version) } {
            return status;
        }
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller retains a live database during this call.
        let database = unsafe { &*database };
        let value = &database.state.server_version;
        // SAFETY: the view borrows bytes owned by the live database handle.
        unsafe {
            out_version.write(TypeBridgeByteView {
                data: value.as_ptr(),
                length: value.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Close a database only after all transaction children close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_close(
    database: *mut *mut TypeBridgeDatabase,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if database.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if database.cast::<()>() == out_diagnostics.cast::<()>() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: V2-origin handles require their complete retained package and
    // server-version bytes fenced before the diagnostic slot can be written.
    if let Err(status) = unsafe { check_policy_database_terminal_ranges(database, out_diagnostics) }
    {
        return status;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        // SAFETY: the caller owns this exact handle slot.
        let value = unsafe { database.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // SAFETY: the handle remains live until successful close.
        let handle = unsafe { &*value };
        if handle.state.transaction_children.load(Ordering::Acquire) != 0 {
            return return_with_status(
                TypeBridgeStatus::InUse,
                in_use_diagnostic(),
                out_diagnostics,
            );
        }
        if let Err(error) = handle.state.database.close() {
            return return_execution_error(
                lower_execution_error(error, SdkProviderOperation::Close),
                out_diagnostics,
            );
        }
        unsafe { database.write(ptr::null_mut()) };
        // SAFETY: ownership returns to Rust exactly once.
        unsafe { drop(Box::from_raw(value)) };
        TypeBridgeStatus::Ok
    })
}

fn open_transaction_context_v2(
    database: &TypeBridgeDatabase,
    tx_type: TxType,
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<TransactionContext, SdkExecutionDiagnostic> {
    check_control(deadline, cancellation)?;
    require_lifecycle_dispatch(limits)?;
    let operation = match tx_type {
        TxType::Read => SdkProviderOperation::OpenReadTransaction,
        TxType::Write => SdkProviderOperation::OpenWriteTransaction,
        TxType::Schema => unreachable!("the C data runtime never opens schema transactions"),
    };
    match database.block_on(await_controlled(
        database.orm_database().transaction_context(tx_type),
        deadline,
        cancellation,
    )) {
        Ok(value) => Ok(value),
        Err(ControlledAwaitError::Interrupted(diagnostic)) => Err(diagnostic),
        Err(ControlledAwaitError::Inner(error)) => Err(lower_execution_error(error, operation)),
    }
}

/// Open one policy-aware read transaction.
#[unsafe(export_name = "type_bridge_read_transaction_open_v2")]
pub(crate) unsafe extern "C" fn type_bridge_read_transaction_open_v2_impl(
    database: *const TypeBridgeDatabase,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut TypeBridgeReadTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            out_transaction.cast(),
            size_of::<*mut TypeBridgeReadTransaction>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        check_preflight_object(&outputs, database),
        check_preflight_object(&outputs, limits),
        check_preflight_object(&outputs, cancellation),
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !database.is_null()
        // SAFETY: the complete outer database handle was fenced above.
        && let Err(status) = check_database_borrowed_ranges(&outputs, unsafe { &*database })
    {
        return status;
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable database handle throughout the call.
        let database = unsafe { &*database };
        // SAFETY: an optional limits descriptor remains readable for this call.
        let call_limits = match unsafe { parse_call_limits(limits) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let effective = call_limits.constrained_by(database.answer_ceiling());
        let deadline = QueryExecutionDeadline::for_limits(effective);
        let cancellation = invocation_cancellation(cancellation);
        if let Err(diagnostic) = check_control(deadline, &cancellation)
            .and_then(|()| require_lifecycle_dispatch(effective))
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let handle_reservation = match ReservedBox::try_new(AllocationSite::ReadTransactionHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let context = match open_transaction_context_v2(
            database,
            TxType::Read,
            effective,
            deadline,
            &cancellation,
        ) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.close_transaction_context(&context);
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeReadTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling: Some(effective),
                poisoned: AtomicBool::new(false),
            },
        };
        // SAFETY: the initialized output remains writable.
        unsafe { out_transaction.write(Box::into_raw(handle_reservation.initialize(transaction))) };
        TypeBridgeStatus::Ok
    })
}

/// Open one policy-aware write transaction.
#[unsafe(export_name = "type_bridge_write_transaction_open_v2")]
pub(crate) unsafe extern "C" fn type_bridge_write_transaction_open_v2_impl(
    database: *const TypeBridgeDatabase,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut TypeBridgeWriteTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            out_transaction.cast(),
            size_of::<*mut TypeBridgeWriteTransaction>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        check_preflight_object(&outputs, database),
        check_preflight_object(&outputs, limits),
        check_preflight_object(&outputs, cancellation),
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !database.is_null()
        // SAFETY: the complete outer database handle was fenced above.
        && let Err(status) = check_database_borrowed_ranges(&outputs, unsafe { &*database })
    {
        return status;
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable database handle throughout the call.
        let database = unsafe { &*database };
        // SAFETY: an optional limits descriptor remains readable for this call.
        let call_limits = match unsafe { parse_call_limits(limits) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let effective = call_limits.constrained_by(database.answer_ceiling());
        let deadline = QueryExecutionDeadline::for_limits(effective);
        let cancellation = invocation_cancellation(cancellation);
        if let Err(diagnostic) = check_control(deadline, &cancellation)
            .and_then(|()| require_lifecycle_dispatch(effective))
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let handle_reservation = match ReservedBox::try_new(AllocationSite::WriteTransactionHandle)
        {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let context = match open_transaction_context_v2(
            database,
            TxType::Write,
            effective,
            deadline,
            &cancellation,
        ) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.rollback_transaction_context(&context);
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeWriteTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling: Some(effective),
                poisoned: AtomicBool::new(false),
            },
        };
        // SAFETY: the initialized output remains writable.
        unsafe { out_transaction.write(Box::into_raw(handle_reservation.initialize(transaction))) };
        TypeBridgeStatus::Ok
    })
}

/// Open one package-fenced read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_open(
    database: *const TypeBridgeDatabase,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut TypeBridgeReadTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer handles and the database's borrowed byte graph
    // are checked read-only before either constructor output is initialized.
    if let Err(status) = unsafe {
        check_database_transaction_open_ranges(
            database,
            cancellation,
            out_transaction,
            out_diagnostics,
        )
    } {
        return status;
    }
    // We cannot delegate output allocation through `TransactionContext **`
    // because the public owner has a distinct opaque type. Initialize here and
    // perform the same admission directly.
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let database = unsafe { &*database };
        if is_cancelled(cancellation) {
            return cancellation_error(out_diagnostics);
        }
        let (context, handle_reservation) = if let Some(limits) = database.policy_answer_ceiling() {
            let deadline = QueryExecutionDeadline::for_limits(limits);
            // ABI 1.3 cancellation remains pre-dispatch-only, but an inherited
            // V2 timeout still bounds the provider await for every operation.
            let in_flight_cancellation = AnswerCancellation::default();
            if let Err(diagnostic) = check_control(deadline, &in_flight_cancellation)
                .and_then(|()| require_lifecycle_dispatch(limits))
            {
                return return_execution_error(diagnostic, out_diagnostics);
            }
            let reservation = match ReservedBox::try_new(AllocationSite::ReadTransactionHandle) {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
            };
            let context = match open_transaction_context_v2(
                database,
                TxType::Read,
                limits,
                deadline,
                &in_flight_cancellation,
            ) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
            (context, Some(reservation))
        } else {
            let context = match database
                .state
                .runtime
                .runtime
                .block_on(database.state.database.transaction_context(TxType::Read))
            {
                Ok(value) => value,
                Err(error) => {
                    return return_execution_error(
                        lower_execution_error(error, SdkProviderOperation::OpenReadTransaction),
                        out_diagnostics,
                    );
                }
            };
            (context, None)
        };
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.state.runtime.runtime.block_on(context.close());
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeReadTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling: database.state.answer_ceiling,
                poisoned: AtomicBool::new(false),
            },
        };
        let transaction = match handle_reservation {
            Some(reservation) => reservation.initialize(transaction),
            None => Box::new(transaction),
        };
        unsafe { out_transaction.write(Box::into_raw(transaction)) };
        TypeBridgeStatus::Ok
    })
}

/// Open one package-fenced write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_open(
    database: *const TypeBridgeDatabase,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut TypeBridgeWriteTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: complete outer handles and the database's borrowed byte graph
    // are checked read-only before either constructor output is initialized.
    if let Err(status) = unsafe {
        check_database_transaction_open_ranges(
            database,
            cancellation,
            out_transaction,
            out_diagnostics,
        )
    } {
        return status;
    }
    // SAFETY: independent output initialization is the constructor contract.
    if let Err(status) = unsafe { initialize_handle_outputs(out_transaction, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if database.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let database = unsafe { &*database };
        if is_cancelled(cancellation) {
            return cancellation_error(out_diagnostics);
        }
        let (context, handle_reservation) = if let Some(limits) = database.policy_answer_ceiling() {
            let deadline = QueryExecutionDeadline::for_limits(limits);
            // ABI 1.3 cancellation remains pre-dispatch-only, but an inherited
            // V2 timeout still bounds the provider await for every operation.
            let in_flight_cancellation = AnswerCancellation::default();
            if let Err(diagnostic) = check_control(deadline, &in_flight_cancellation)
                .and_then(|()| require_lifecycle_dispatch(limits))
            {
                return return_execution_error(diagnostic, out_diagnostics);
            }
            let reservation = match ReservedBox::try_new(AllocationSite::WriteTransactionHandle) {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
            };
            let context = match open_transaction_context_v2(
                database,
                TxType::Write,
                limits,
                deadline,
                &in_flight_cancellation,
            ) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
            (context, Some(reservation))
        } else {
            let context = match database
                .state
                .runtime
                .runtime
                .block_on(database.state.database.transaction_context(TxType::Write))
            {
                Ok(value) => value,
                Err(error) => {
                    return return_execution_error(
                        lower_execution_error(error, SdkProviderOperation::OpenWriteTransaction),
                        out_diagnostics,
                    );
                }
            };
            (context, None)
        };
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.state.runtime.runtime.block_on(context.close());
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeWriteTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                answer_ceiling: database.state.answer_ceiling,
                poisoned: AtomicBool::new(false),
            },
        };
        let transaction = match handle_reservation {
            Some(reservation) => reservation.initialize(transaction),
            None => Box::new(transaction),
        };
        unsafe { out_transaction.write(Box::into_raw(transaction)) };
        TypeBridgeStatus::Ok
    })
}

fn terminal_outputs_valid<T>(
    transaction: *mut *mut T,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> bool {
    !transaction.is_null()
        && !out_diagnostics.is_null()
        && transaction.cast::<()>() != out_diagnostics.cast::<()>()
}

/// Close one read transaction without committing and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_close(
    transaction: *mut *mut TypeBridgeReadTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if transaction.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if transaction.cast::<()>() == out_diagnostics.cast::<()>() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: V2-origin handles require their complete retained package and
    // server-version bytes fenced before the diagnostic slot can be written.
    if let Err(status) =
        unsafe { check_policy_read_transaction_terminal_ranges(transaction, out_diagnostics) }
    {
        return status;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        if !terminal_outputs_valid(transaction, out_diagnostics) {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { transaction.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // Acquire the owner before clearing its C slot. Keeping the box on the
        // stack guarantees child-count cleanup if provider close panics.
        let mut handle = unsafe { Box::from_raw(value) };
        unsafe { transaction.write(ptr::null_mut()) };
        let context = handle.state.context.take();
        let result = context.map(|context| {
            handle
                .state
                .database
                .runtime
                .runtime
                .block_on(context.close())
        });
        drop(handle);
        match result {
            None | Some(Ok(())) => TypeBridgeStatus::Ok,
            Some(Err(error)) => return_execution_error(
                lower_execution_error(error, SdkProviderOperation::Close),
                out_diagnostics,
            ),
        }
    })
}

/// Commit one write transaction exactly once and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_commit(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: the owner slot, live pointee, cancellation handle, and complete
    // retained package/version graph are fenced before diagnostics can change.
    if let Err(status) =
        unsafe { check_write_transaction_commit_ranges(transaction, cancellation, out_diagnostics) }
    {
        return status;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        if !terminal_outputs_valid(transaction, out_diagnostics) {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { transaction.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // SAFETY: the live handle remains caller-owned through every admission check.
        let live = unsafe { &*value };
        if live.state.poisoned.load(Ordering::Acquire) {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        let context = live
            .state
            .context
            .as_ref()
            .expect("a live write handle retains one context");
        if let Some(diagnostic) =
            rollback_only_commit_diagnostic(&live.state.database.runtime.runtime, context)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if is_cancelled(cancellation) {
            return cancellation_error(out_diagnostics);
        }
        let policy_deadline = if let Some(limits) = live.policy_answer_ceiling() {
            let deadline = QueryExecutionDeadline::for_limits(limits);
            let cancellation = invocation_cancellation(cancellation);
            if let Err(diagnostic) = check_control(deadline, &cancellation)
                .and_then(|()| require_lifecycle_dispatch(limits))
            {
                return return_execution_error(diagnostic, out_diagnostics);
            }
            Some(deadline)
        } else {
            None
        };
        // Cancellation was checked while the caller still owned a live handle.
        // From this point the terminal consumes its C slot even if provider
        // code panics. The stack-owned box releases the child count on unwind.
        let mut handle = unsafe { Box::from_raw(value) };
        unsafe { transaction.write(ptr::null_mut()) };
        let context = handle
            .state
            .context
            .take()
            .expect("a live write handle retains one context");
        // Caller cancellation is deliberately not observed after this dispatch
        // point. An inherited V2 deadline still requests internal cancellation,
        // then waits for and reports the provider's classified actual outcome.
        let result = match policy_deadline {
            Some(deadline) => commit_context_sdk_controlled(
                &handle.state.database.runtime.runtime,
                &context,
                deadline,
                &AnswerCancellation::default(),
            ),
            None => commit_context_sdk(&handle.state.database.runtime.runtime, &context),
        };
        drop(handle);
        match result {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        }
    })
}

/// Commit one policy-aware write transaction.
#[unsafe(export_name = "type_bridge_write_transaction_commit_v2")]
pub(crate) unsafe extern "C" fn type_bridge_write_transaction_commit_v2_impl(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            transaction.cast(),
            size_of::<*mut TypeBridgeWriteTransaction>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        check_preflight_object(&outputs, limits),
        check_preflight_object(&outputs, cancellation),
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    if !transaction.is_null() {
        // SAFETY: the caller promises a readable/writable owner slot.
        let value = unsafe { transaction.read_unaligned() };
        if let Err(status) = check_preflight_object(&outputs, value) {
            return status;
        }
        if !value.is_null()
            // SAFETY: the complete live handle object was fenced above.
            && let Err(status) =
                check_write_transaction_borrowed_ranges(&outputs, unsafe { &*value })
        {
            return status;
        }
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        if !terminal_outputs_valid(transaction, out_diagnostics) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller owns this exact handle slot for the terminal call.
        let value = unsafe { transaction.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // SAFETY: pre-dispatch rejection retains this exact live handle.
        let live = unsafe { &*value };
        if live.is_poisoned() {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        let context = live
            .state
            .context
            .as_ref()
            .expect("a live write handle retains one context");
        if let Some(diagnostic) =
            rollback_only_commit_diagnostic(&live.state.database.runtime.runtime, context)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        // SAFETY: an optional limits descriptor remains readable for this call.
        let call_limits = match unsafe { parse_call_limits(limits) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let effective = call_limits.constrained_by(live.answer_ceiling());
        let deadline = QueryExecutionDeadline::for_limits(effective);
        let cancellation = invocation_cancellation(cancellation);
        if let Err(diagnostic) = check_control(deadline, &cancellation)
            .and_then(|()| require_lifecycle_dispatch(effective))
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }

        // This dispatch point consumes the only public owner. Interruption after
        // it requests cooperative cancellation but cannot relabel the provider's
        // classified actual commit outcome.
        let mut handle = unsafe { Box::from_raw(value) };
        // SAFETY: caller supplied the unique writable owner slot.
        unsafe { transaction.write(ptr::null_mut()) };
        let context = handle
            .state
            .context
            .take()
            .expect("a live write handle retains one context");
        let result = commit_context_sdk_controlled(
            &handle.state.database.runtime.runtime,
            &context,
            deadline,
            &cancellation,
        );
        drop(handle);
        match result {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        }
    })
}

const _: unsafe extern "C" fn(
    *const TypeBridgeDatabaseConfigV2,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_database_config_validate_v2_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeRuntime,
    *const TypeBridgeSchemaPackage,
    *const TypeBridgeDatabaseConfigV2,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeDatabase,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_database_open_v2_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeDatabase,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeReadTransaction,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_read_transaction_open_v2_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeDatabase,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeWriteTransaction,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_write_transaction_open_v2_impl;
const _: unsafe extern "C" fn(
    *mut *mut TypeBridgeWriteTransaction,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_write_transaction_commit_v2_impl;

/// Roll back one write transaction exactly once and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_rollback(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if transaction.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if transaction.cast::<()>() == out_diagnostics.cast::<()>() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: V2-origin handles require their complete retained package and
    // server-version bytes fenced before rollback or close consumes the owner.
    if let Err(status) =
        unsafe { check_policy_write_transaction_terminal_ranges(transaction, out_diagnostics) }
    {
        return status;
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        if !terminal_outputs_valid(transaction, out_diagnostics) {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { transaction.read() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        // Consume before provider dispatch so unwinding cannot leave a hollow
        // non-null C handle or retain the database child count.
        let mut handle = unsafe { Box::from_raw(value) };
        unsafe { transaction.write(ptr::null_mut()) };
        let context = handle.state.context.take();
        let result = context.map(|context| {
            handle
                .state
                .database
                .runtime
                .runtime
                .block_on(context.rollback())
        });
        drop(handle);
        match result {
            None | Some(Ok(())) => TypeBridgeStatus::Ok,
            Some(Err(error)) => return_execution_error(
                lower_execution_error(error, SdkProviderOperation::Rollback),
                out_diagnostics,
            ),
        }
    })
}

/// Close an active write transaction by rolling it back and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_close(
    transaction: *mut *mut TypeBridgeWriteTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // Active close has the same fail-closed provider semantics as explicit rollback.
    // SAFETY: forwarded pointer-to-pointer ownership contract is documented publicly.
    unsafe { type_bridge_write_transaction_rollback(transaction, out_diagnostics) }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::mem::{MaybeUninit, size_of};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64};

    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_core_lib::version::{Version, VersionError};
    use type_bridge_orm::error::{ClassifiedCommitError, CommitFailureCertainty};
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        build_schema_authority, encode_schema_authority, normalize_documents, project, resolve,
    };
    use type_bridge_schema_codegen::CEmitter;

    use super::*;
    use crate::abi::{TypeBridgeSchemaPackageDescriptorV1, type_bridge_schema_package_close};
    use crate::allocation::inject_failure;
    use crate::execution_diagnostic::{
        TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailViewV1,
        TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
        type_bridge_execution_diagnostics_close, type_bridge_execution_diagnostics_detail_get_v1,
        type_bridge_execution_diagnostics_get_v1, type_bridge_execution_diagnostics_path_get_v1,
    };

    const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
"#;

    const COMMIT_SUCCESS: u8 = 0;
    const COMMIT_ABORTED: u8 = 1;
    const COMMIT_UNKNOWN: u8 = 2;
    const COMMIT_PANIC: u8 = 3;

    #[derive(Default)]
    struct FakeState {
        connects: AtomicUsize,
        opens: AtomicUsize,
        commits: AtomicUsize,
        rollbacks: AtomicUsize,
        transaction_closes: AtomicUsize,
        database_closes: AtomicUsize,
        fail_connect: AtomicBool,
        fail_version_probe: AtomicBool,
        fail_open: AtomicBool,
        pending_open: AtomicBool,
        supports_given_rows: AtomicBool,
        panic_transaction_close: AtomicBool,
        panic_rollback: AtomicBool,
        commit: AtomicU8,
        commit_delay_milliseconds: AtomicU64,
        commit_started: AtomicBool,
        inputs: Mutex<VecDeque<DatabaseConnectInput>>,
    }

    struct FakeConnector {
        state: Arc<FakeState>,
        version: Version,
    }

    impl DatabaseConnector for FakeConnector {
        fn connect(&self, input: DatabaseConnectInput) -> ConnectFuture {
            self.state.connects.fetch_add(1, Ordering::AcqRel);
            self.state
                .inputs
                .lock()
                .unwrap()
                .push_back(DatabaseConnectInput {
                    address: input.address.clone(),
                    database: input.database.clone(),
                    username: input.username.clone(),
                    password: input.password.clone(),
                    http_port: input.http_port,
                    tls_native_roots: input.tls_native_roots,
                });
            let state = Arc::clone(&self.state);
            let version = self.version;
            Box::pin(async move {
                if state.fail_version_probe.load(Ordering::Acquire) {
                    return Err(OrmError::UnsupportedVersion(VersionError::Probe(
                        "provider address=/private credential=secret".into(),
                    )));
                }
                if state.fail_connect.load(Ordering::Acquire) {
                    return Err(OrmError::Connection(
                        "provider address=private password=secret".into(),
                    ));
                }
                Ok(Database::with_backend(
                    Box::new(FakeBackend { state, version }),
                    input.database,
                ))
            })
        }

        fn connect_v2(&self, input: DatabaseConnectInputV2) -> ControlledConnectFuture {
            let state = Arc::clone(&self.state);
            let version = self.version;
            Box::pin(async move {
                check_control(input.deadline, &input.cancellation)?;
                if input.limits.effective().statements == 0 {
                    return Err(lifecycle_statement_limit());
                }
                state.connects.fetch_add(1, Ordering::AcqRel);
                state
                    .inputs
                    .lock()
                    .unwrap()
                    .push_back(DatabaseConnectInput {
                        address: input.address.clone(),
                        database: input.database.clone(),
                        username: input.username.clone(),
                        password: input.password.clone(),
                        http_port: 0,
                        tls_native_roots: false,
                    });
                let _transport = input.transport;
                if state.fail_version_probe.load(Ordering::Acquire)
                    || state.fail_connect.load(Ordering::Acquire)
                {
                    return Err(SdkExecutionDiagnostic::provider_failure(
                        SdkProviderOperation::Connect,
                    ));
                }
                Ok(Database::with_backend(
                    Box::new(FakeBackend { state, version }),
                    input.database,
                ))
            })
        }
    }

    struct FakeBackend {
        state: Arc<FakeState>,
        version: Version,
    }

    impl DriverBackend for FakeBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.opens.fetch_add(1, Ordering::AcqRel);
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                if state.pending_open.load(Ordering::Acquire) {
                    std::future::pending::<()>().await;
                }
                if state.fail_open.load(Ordering::Acquire) {
                    return Err(OrmError::Connection(
                        "provider endpoint=/private credential=secret".into(),
                    ));
                }
                Ok(Box::new(FakeTransaction { state, tx_type }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn close_connection(&self) -> Result<(), OrmError> {
            self.state.database_closes.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn server_version(&self) -> Option<Version> {
            Some(self.version)
        }

        fn supports_given_rows(&self) -> bool {
            self.state.supports_given_rows.load(Ordering::Acquire)
        }
    }

    struct FakeTransaction {
        state: Arc<FakeState>,
        tx_type: TxType,
    }

    impl TransactionOps for FakeTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { Ok(QueryResult::Ok) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("C ABI must use classified commit") })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.commits.fetch_add(1, Ordering::AcqRel);
                state.commit_started.store(true, Ordering::Release);
                let delay = state.commit_delay_milliseconds.load(Ordering::Acquire);
                if delay != 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                match state.commit.load(Ordering::Acquire) {
                    COMMIT_SUCCESS => Ok(()),
                    COMMIT_ABORTED => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::DefinitelyAborted,
                        message: "credential=secret definitely aborted".into(),
                    }),
                    COMMIT_UNKNOWN => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::Unknown,
                        message: "address=/private outcome unknown".into(),
                    }),
                    COMMIT_PANIC => panic!("fake commit panic"),
                    value => panic!("unknown fake commit behavior {value}"),
                }
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.rollbacks.fetch_add(1, Ordering::AcqRel);
                assert!(
                    !state.panic_rollback.load(Ordering::Acquire),
                    "fake rollback panic"
                );
                Ok(())
            })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let state = Arc::clone(&self.state);
            let tx_type = self.tx_type;
            Box::pin(async move {
                assert_eq!(tx_type, TxType::Read);
                state.transaction_closes.fetch_add(1, Ordering::AcqRel);
                assert!(
                    !state.panic_transaction_close.load(Ordering::Acquire),
                    "fake read-close panic"
                );
                Ok(())
            })
        }
    }

    struct PackageEvidence {
        authority: Vec<u8>,
        declared: Vec<u8>,
        projection: Vec<u8>,
        semantic: Vec<u8>,
        binding: Vec<u8>,
        scope: Vec<u8>,
        profile: Vec<u8>,
    }

    impl PackageEvidence {
        fn descriptor(&self) -> TypeBridgeSchemaPackageDescriptorV1 {
            TypeBridgeSchemaPackageDescriptorV1 {
                struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>() as u32,
                abi_major: crate::abi::ABI_MAJOR,
                abi_minor: crate::abi::ABI_MINOR,
                schema_authority_json: view(&self.authority),
                declared_schema_json: view(&self.declared),
                runtime_projection_json: view(&self.projection),
                semantic_fingerprint_json: view(&self.semantic),
                binding_fingerprint_json: view(&self.binding),
                managed_scope: view(&self.scope),
                semantic_profile: view(&self.profile),
                reserved: [0; 4],
            }
        }
    }

    fn view(value: &[u8]) -> TypeBridgeByteView {
        TypeBridgeByteView {
            data: value.as_ptr(),
            length: value.len(),
        }
    }

    fn package(profile: &str, prefix: &str) -> TypeBridgeSchemaPackage {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("c-execution-abi.yaml").unwrap(), SCHEMA)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new(profile).unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|value| CapabilityId::new(*value).unwrap())
            .collect();
        let scope = ManagedScopeId::new(format!("c-execution-{prefix}")).unwrap();
        let context = ManagedDeltaContext::new(scope.clone(), profile.clone(), available);
        let authority =
            build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
        let emitter = CEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let evidence = PackageEvidence {
            authority: encode_schema_authority(&authority),
            declared: encode_declared_schema(&declared).unwrap(),
            projection: to_canonical_json(&projection).unwrap(),
            semantic: to_canonical_json(projection.semantic_fingerprint()).unwrap(),
            binding: to_canonical_json(projection.projection_fingerprint()).unwrap(),
            scope: scope.as_str().as_bytes().to_vec(),
            profile: profile.as_str().as_bytes().to_vec(),
        };
        crate::schema_package::open(evidence.descriptor()).unwrap()
    }

    fn runtime(state: Arc<FakeState>, version: Version) -> *mut TypeBridgeRuntime {
        Box::into_raw(Box::new(
            open_runtime(2, Arc::new(FakeConnector { state, version })).unwrap(),
        ))
    }

    fn database_config() -> (
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        TypeBridgeDatabaseConfigV1,
    ) {
        let address = b"localhost:1729".to_vec();
        let database = b"workforce".to_vec();
        let username = b"admin".to_vec();
        let password = b"password".to_vec();
        let config = TypeBridgeDatabaseConfigV1 {
            struct_size: size_of::<TypeBridgeDatabaseConfigV1>() as u32,
            version: DATABASE_CONFIG_VERSION,
            address: view(&address),
            database: view(&database),
            username: view(&username),
            password: view(&password),
            http_port: 8000,
            tls_mode: TLS_DISABLED,
            reserved: [0; 4],
        };
        (address, database, username, password, config)
    }

    fn limits_descriptor(limits: QueryExecutionResourceLimits) -> TypeBridgeQueryExecutionLimitsV1 {
        TypeBridgeQueryExecutionLimitsV1 {
            struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
            version: 1,
            timeout_milliseconds: limits.timeout_milliseconds,
            items: limits.items,
            bytes: limits.bytes,
            graph_nodes: limits.graph_nodes,
            attribute_values: limits.attribute_values,
            collection_members: limits.collection_members,
            role_players: limits.role_players,
            statements: limits.statements,
            reserved0: 0,
            reserved: [0; 4],
        }
    }

    struct DatabaseConfigV2Fixture {
        _address: Vec<u8>,
        _database: Vec<u8>,
        _username: Vec<u8>,
        _password: Vec<u8>,
        _custom_root: Vec<u8>,
        config: TypeBridgeDatabaseConfigV2,
    }

    impl DatabaseConfigV2Fixture {
        fn new(
            tls_mode: u32,
            custom_root: Vec<u8>,
            connection_limits: QueryExecutionResourceLimits,
            answer_limits: QueryExecutionResourceLimits,
        ) -> Self {
            let address = b"localhost:1729".to_vec();
            let database = b"workforce".to_vec();
            let username = b"admin".to_vec();
            let password = b"private-password".to_vec();
            let custom_root_view = if custom_root.is_empty() {
                TypeBridgeByteView {
                    data: ptr::null(),
                    length: 0,
                }
            } else {
                view(&custom_root)
            };
            let config = TypeBridgeDatabaseConfigV2 {
                struct_size: size_of::<TypeBridgeDatabaseConfigV2>() as u32,
                version: DATABASE_CONFIG_V2_VERSION,
                address: view(&address),
                database: view(&database),
                username: view(&username),
                password: view(&password),
                http_port: 8000,
                tls_mode,
                custom_root_ca_pem: custom_root_view,
                connection_limits: limits_descriptor(connection_limits),
                answer_limits: limits_descriptor(answer_limits),
                reserved: [0; 4],
            };
            Self {
                _address: address,
                _database: database,
                _username: username,
                _password: password,
                _custom_root: custom_root,
                config,
            }
        }

        fn plaintext(answer_limits: QueryExecutionResourceLimits) -> Self {
            Self::new(
                TLS_DISABLED,
                Vec::new(),
                QueryExecutionResourceLimits::default(),
                answer_limits,
            )
        }
    }

    unsafe fn open_database(
        runtime: *mut TypeBridgeRuntime,
        package: *mut TypeBridgeSchemaPackage,
        cancellation: *const TypeBridgeCancellation,
    ) -> (
        *mut TypeBridgeDatabase,
        *mut TypeBridgeExecutionDiagnostics,
        TypeBridgeStatus,
    ) {
        let (_address, _database, _username, _password, config) = database_config();
        let mut database = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: all handles and descriptor views remain live for the call.
        let status = unsafe {
            type_bridge_database_open_v1(
                runtime,
                package,
                &config,
                cancellation,
                &mut database,
                &mut diagnostics,
            )
        };
        (database, diagnostics, status)
    }

    unsafe fn open_database_v2(
        runtime: *mut TypeBridgeRuntime,
        package: *mut TypeBridgeSchemaPackage,
        config: &TypeBridgeDatabaseConfigV2,
        cancellation: *const TypeBridgeCancellation,
    ) -> (
        *mut TypeBridgeDatabase,
        *mut TypeBridgeExecutionDiagnostics,
        TypeBridgeStatus,
    ) {
        let mut database = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: all handles and descriptor views remain live for the call.
        let status = unsafe {
            type_bridge_database_open_v2_impl(
                runtime,
                package,
                config,
                cancellation,
                &mut database,
                &mut diagnostics,
            )
        };
        (database, diagnostics, status)
    }

    unsafe fn close_diagnostics(diagnostics: &mut *mut TypeBridgeExecutionDiagnostics) {
        // SAFETY: the slot owns either null or an exact diagnostics handle.
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_close(diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    unsafe fn diagnostic_components(diagnostics: *const TypeBridgeExecutionDiagnostics) -> Vec<u8> {
        unsafe fn extend(output: &mut Vec<u8>, value: TypeBridgeByteView) {
            if value.length != 0 {
                assert!(!value.data.is_null());
                // SAFETY: diagnostic views borrow a live handle for their advertised length.
                output.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(value.data, value.length)
                });
            }
        }

        let mut output = Vec::new();
        let mut diagnostic = MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_get_v1(diagnostics, 0, diagnostic.as_mut_ptr())
            },
            TypeBridgeStatus::Ok
        );
        let diagnostic = unsafe { diagnostic.assume_init() };
        for value in [diagnostic.code, diagnostic.message] {
            unsafe { extend(&mut output, value) };
        }
        for index in 0..diagnostic.path_count {
            let mut path = MaybeUninit::<TypeBridgeExecutionDiagnosticPathViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_path_get_v1(
                        diagnostics,
                        0,
                        index,
                        path.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok
            );
            let path = unsafe { path.assume_init() };
            for value in [path.primary, path.secondary, path.tertiary] {
                unsafe { extend(&mut output, value) };
            }
        }
        for index in 0..diagnostic.detail_count {
            let mut detail = MaybeUninit::<TypeBridgeExecutionDiagnosticDetailViewV1>::uninit();
            assert_eq!(
                unsafe {
                    type_bridge_execution_diagnostics_detail_get_v1(
                        diagnostics,
                        0,
                        index,
                        detail.as_mut_ptr(),
                    )
                },
                TypeBridgeStatus::Ok
            );
            let detail = unsafe { detail.assume_init() };
            for value in [
                detail.key,
                detail.primary,
                detail.secondary,
                detail.tertiary,
                detail.quaternary,
            ] {
                unsafe { extend(&mut output, value) };
            }
        }
        output
    }

    unsafe fn diagnostic_code(diagnostics: *const TypeBridgeExecutionDiagnostics) -> String {
        let mut diagnostic = MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_get_v1(diagnostics, 0, diagnostic.as_mut_ptr())
            },
            TypeBridgeStatus::Ok
        );
        let diagnostic = unsafe { diagnostic.assume_init() };
        String::from_utf8(
            unsafe { std::slice::from_raw_parts(diagnostic.code.data, diagnostic.code.length) }
                .to_vec(),
        )
        .unwrap()
    }

    unsafe fn diagnostic_category(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeExecutionDiagnosticCategory {
        let mut diagnostic = MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_get_v1(diagnostics, 0, diagnostic.as_mut_ptr())
            },
            TypeBridgeStatus::Ok
        );
        unsafe { diagnostic.assume_init() }.category
    }

    #[test]
    fn v2_configuration_validates_captured_custom_root_without_provider_io_or_disclosure() {
        let valid_root = include_bytes!("../../core/tests/fixtures/valid-root.pem").to_vec();
        let valid = DatabaseConfigV2Fixture::new(
            TLS_CUSTOM_ROOT_CA,
            valid_root.clone(),
            QueryExecutionResourceLimits::default(),
            QueryExecutionResourceLimits::default(),
        );
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_config_validate_v2_impl(&valid.config, &mut diagnostics)
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());

        let zero_timeout = QueryExecutionResourceLimits {
            timeout_milliseconds: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let expired_but_provider_free = DatabaseConfigV2Fixture::new(
            TLS_CUSTOM_ROOT_CA,
            valid_root,
            zero_timeout,
            QueryExecutionResourceLimits::default(),
        );
        assert_eq!(
            unsafe {
                type_bridge_database_config_validate_v2_impl(
                    &expired_but_provider_free.config,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());

        let aliased = DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());
        let preserved_address = aliased._address.clone();
        // SAFETY: the deliberately hostile slot remains within the readable
        // address allocation, and successful preflight must reject before writing it.
        let nested_output = unsafe { aliased.config.address.data.add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe { type_bridge_database_config_validate_v2_impl(&aliased.config, nested_output) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(aliased._address, preserved_address);

        let invalid = DatabaseConfigV2Fixture::new(
            TLS_CUSTOM_ROOT_CA,
            b"private-password trust-byte-sentinel-9912".to_vec(),
            QueryExecutionResourceLimits::default(),
            QueryExecutionResourceLimits::default(),
        );
        assert_eq!(
            unsafe {
                type_bridge_database_config_validate_v2_impl(&invalid.config, &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "tls_custom_root_ca_invalid_pem"
        );
        let components = unsafe { diagnostic_components(diagnostics) };
        let exposed = String::from_utf8_lossy(&components);
        assert!(!exposed.contains("private-password"));
        assert!(!exposed.contains("trust-byte-sentinel-9912"));
        unsafe { close_diagnostics(&mut diagnostics) };

        let mut noncanonical =
            DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());
        let marker = [0_u8];
        noncanonical.config.custom_root_ca_pem = TypeBridgeByteView {
            data: marker.as_ptr(),
            length: 0,
        };
        assert_eq!(
            unsafe {
                type_bridge_database_config_validate_v2_impl(&noncanonical.config, &mut diagnostics)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_database_custom_root_must_be_empty"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
    }

    #[test]
    fn v2_result_handle_allocation_failures_precede_provider_dispatch() {
        let state = Arc::new(FakeState::default());
        state.supports_given_rows.store(true, Ordering::Release);
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "v2allocation")));
        let fixture = DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());

        let expired_connection_limits = QueryExecutionResourceLimits {
            timeout_milliseconds: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let expired_connection = DatabaseConfigV2Fixture::new(
            TLS_DISABLED,
            Vec::new(),
            expired_connection_limits,
            QueryExecutionResourceLimits::default(),
        );
        let (database, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &expired_connection.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let zero_statement_connection_limits = QueryExecutionResourceLimits {
            statements: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let zero_statement_connection = DatabaseConfigV2Fixture::new(
            TLS_DISABLED,
            Vec::new(),
            zero_statement_connection_limits,
            QueryExecutionResourceLimits::default(),
        );
        let (database, mut diagnostics, status) = unsafe {
            open_database_v2(
                runtime,
                package,
                &zero_statement_connection.config,
                ptr::null(),
            )
        };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_statement_limit"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let (database, mut diagnostics, status) = {
            let _failure = inject_failure(AllocationSite::DatabaseHandle, 0);
            unsafe { open_database_v2(runtime, package, &fixture.config, ptr::null()) }
        };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        let descriptor = limits_descriptor(QueryExecutionResourceLimits::default());

        let opens_before = state.opens.load(Ordering::Acquire);
        let mut read = ptr::null_mut();
        let status = {
            let _failure = inject_failure(AllocationSite::ReadTransactionHandle, 0);
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &descriptor,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            }
        };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert!(read.is_null());
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let mut write = ptr::null_mut();
        let status = {
            let _failure = inject_failure(AllocationSite::WriteTransactionHandle, 0);
            unsafe {
                type_bridge_write_transaction_open_v2_impl(
                    database,
                    &descriptor,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            }
        };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert!(write.is_null());
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_allocation_exhausted"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn v2_output_preflight_fences_nested_and_live_handle_storage_before_writes() {
        let state = Arc::new(FakeState::default());
        state.supports_given_rows.store(true, Ordering::Release);
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "v2alias")));
        let fixture = DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());
        let preserved_address = fixture._address.clone();
        // SAFETY: this hostile output starts inside the readable address bytes;
        // correct preflight rejects it without attempting an unaligned write.
        let nested_database_output = unsafe { fixture.config.address.data.add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeDatabase>();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_open_v2_impl(
                    runtime,
                    package,
                    &fixture.config,
                    ptr::null(),
                    nested_database_output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(diagnostics.is_null());
        assert_eq!(fixture._address, preserved_address);
        assert_eq!(state.connects.load(Ordering::Acquire), 0);

        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        let preserved_profile = unsafe { &*database }
            .package_state()
            .semantic_profile
            .clone();
        let profile_pointer = unsafe { &*database }
            .package_state()
            .semantic_profile
            .as_ptr();
        // SAFETY: the output range is deliberately forged into retained
        // package bytes and must be rejected before provider dispatch.
        let nested_transaction_output = unsafe { profile_pointer.add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeReadTransaction>();
        let descriptor = limits_descriptor(QueryExecutionResourceLimits::default());
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &descriptor,
                    ptr::null(),
                    nested_transaction_output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { &*database }.package_state().semantic_profile,
            preserved_profile
        );
        assert_eq!(state.opens.load(Ordering::Acquire), 0);

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let write_before = write;
        // SAFETY: the hostile diagnostic slot lies within the live transaction
        // object; complete-handle preflight must reject it without consuming.
        let nested_diagnostics =
            unsafe { write.cast::<u8>().add(1) }.cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut write,
                    &descriptor,
                    ptr::null(),
                    nested_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn legacy_preflight_routes_v1_exact_aliases_and_v2_complete_retained_graphs() {
        let package = package("typedb-3.12.1/v1", "legacyrouting");
        let state = Arc::new(FakeState::default());
        let mut v1_database = Box::into_raw(Box::new(TypeBridgeDatabase::from_test_database(
            Arc::clone(package.state()),
            Database::with_backend(
                Box::new(FakeBackend {
                    state: Arc::clone(&state),
                    version: Version::new(3, 12, 1),
                }),
                "legacy-routing-v1",
            ),
        )));
        let mut v2_database = Box::into_raw(Box::new(
            TypeBridgeDatabase::from_test_database_with_answer_ceiling(
                Arc::clone(package.state()),
                Database::with_backend(
                    Box::new(FakeBackend {
                        state: Arc::clone(&state),
                        version: Version::new(3, 12, 1),
                    }),
                    "legacy-routing-v2",
                ),
                Some(QueryExecutionResourceLimits::default()),
            ),
        ));
        let mut diagnostics = ptr::null_mut();
        let nested_profile_output = unsafe { package.state().semantic_profile.as_ptr().add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeReadTransaction>();
        assert_eq!(
            unsafe {
                check_database_transaction_open_ranges(
                    v1_database,
                    ptr::null(),
                    nested_profile_output,
                    &mut diagnostics,
                )
            },
            Ok(())
        );
        assert_eq!(
            unsafe {
                check_database_transaction_open_ranges(
                    v2_database,
                    ptr::null(),
                    nested_profile_output,
                    &mut diagnostics,
                )
            },
            Err(TypeBridgeStatus::InvalidArgument)
        );

        let nested_v1_database_output =
            unsafe { v1_database.cast::<u8>().add(1) }.cast::<*mut TypeBridgeWriteTransaction>();
        let nested_v2_database_output =
            unsafe { v2_database.cast::<u8>().add(1) }.cast::<*mut TypeBridgeWriteTransaction>();
        assert_eq!(
            unsafe {
                check_database_transaction_open_ranges(
                    v1_database,
                    ptr::null(),
                    nested_v1_database_output,
                    &mut diagnostics,
                )
            },
            Ok(())
        );
        assert_eq!(
            unsafe {
                check_database_transaction_open_ranges(
                    v2_database,
                    ptr::null(),
                    nested_v2_database_output,
                    &mut diagnostics,
                )
            },
            Err(TypeBridgeStatus::InvalidArgument)
        );

        let v1_context = unsafe { &*v1_database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        let mut v1_transaction = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*v1_database },
                v1_context,
                None,
            ),
        ));
        let v2_context = unsafe { &*v2_database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        let mut v2_transaction = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*v2_database },
                v2_context,
                Some(QueryExecutionResourceLimits::default()),
            ),
        ));
        let nested_profile_diagnostics =
            unsafe { package.state().semantic_profile.as_ptr().add(1) }
                .cast_mut()
                .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe {
                check_write_transaction_commit_ranges(
                    &mut v1_transaction,
                    ptr::null(),
                    nested_profile_diagnostics,
                )
            },
            Ok(())
        );
        assert_eq!(
            unsafe {
                check_write_transaction_commit_ranges(
                    &mut v2_transaction,
                    ptr::null(),
                    nested_profile_diagnostics,
                )
            },
            Err(TypeBridgeStatus::InvalidArgument)
        );
        let nested_v1_transaction_diagnostics = unsafe { v1_transaction.cast::<u8>().add(1) }
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        let nested_v2_transaction_diagnostics = unsafe { v2_transaction.cast::<u8>().add(1) }
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe {
                check_write_transaction_commit_ranges(
                    &mut v1_transaction,
                    ptr::null(),
                    nested_v1_transaction_diagnostics,
                )
            },
            Ok(())
        );
        assert_eq!(
            unsafe {
                check_write_transaction_commit_ranges(
                    &mut v2_transaction,
                    ptr::null(),
                    nested_v2_transaction_diagnostics,
                )
            },
            Err(TypeBridgeStatus::InvalidArgument)
        );

        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut v1_transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut v2_transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_close(&mut v1_database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_close(&mut v2_database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn legacy_lifecycle_on_v2_handles_deep_fences_every_retained_input_before_writes() {
        let state = Arc::new(FakeState::default());
        state.supports_given_rows.store(true, Ordering::Release);
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "legacydeep")));
        let fixture = DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());
        let (mut database, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &fixture.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());

        let database_before = database;
        let preserved_profile = unsafe { &*database }
            .package_state()
            .semantic_profile
            .clone();
        let profile_pointer = unsafe { &*database }
            .package_state()
            .semantic_profile
            .as_ptr();
        let preserved_version = unsafe { &*database }.state.server_version.clone();
        let version_pointer = unsafe { &*database }.state.server_version.as_ptr();
        let opens_before = state.opens.load(Ordering::Acquire);

        let nested_read_output = unsafe { profile_pointer.add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeReadTransaction>();
        diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    nested_read_output,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostics, ptr::dangling_mut());
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before);

        let nested_version_diagnostics = version_pointer
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        let mut write = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    nested_version_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, ptr::dangling_mut());
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before);
        assert_eq!(
            unsafe { &*database }.package_state().semantic_profile,
            preserved_profile
        );
        assert_eq!(
            unsafe { &*database }.state.server_version,
            preserved_version
        );

        diagnostics = ptr::null_mut();
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let read_before = read;
        let nested_profile_diagnostics = unsafe { profile_pointer.add(1) }
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, nested_profile_diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(database, database_before);
        assert_eq!(read, read_before);
        assert_eq!(state.database_closes.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, nested_version_diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(read, read_before);
        assert_eq!(state.transaction_closes.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(read.is_null());
        assert_eq!(state.transaction_closes.load(Ordering::Acquire), 1);

        write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let write_before = write;
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_rollback(&mut write, nested_profile_diagnostics)
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, nested_version_diagnostics) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 0);

        let nested_handle_diagnostics =
            unsafe { write.cast::<u8>().add(1) }.cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut write,
                    ptr::null(),
                    nested_handle_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut write,
                    ptr::null(),
                    nested_profile_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let nested_cancellation_diagnostics = unsafe { cancellation.cast::<u8>().add(1) }
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut write,
                    cancellation,
                    nested_cancellation_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(write.is_null());
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { &*database }.package_state().semantic_profile,
            preserved_profile
        );
        assert_eq!(
            unsafe { &*database }.state.server_version,
            preserved_version
        );

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn v2_database_requires_band9_and_transactions_intersect_and_inherit_answer_policy() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "v2policy")));
        let answer_limits =
            QueryExecutionResourceLimits::tightened(9_000, 11, 101, 13, 17, 19, 23, 2);
        let fixture = DatabaseConfigV2Fixture::plaintext(answer_limits);

        let (rejected, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &fixture.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Unsupported);
        assert!(rejected.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 1);
        assert_eq!(state.database_closes.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_database_given_rows_unsupported"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        state.supports_given_rows.store(true, Ordering::Release);
        let (mut database, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &fixture.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { &*database }.policy_answer_ceiling(),
            Some(answer_limits)
        );

        let call_limits = QueryExecutionResourceLimits::tightened(8_000, 7, 97, 11, 13, 17, 19, 1);
        let call_descriptor = limits_descriptor(call_limits);
        let expected = call_limits.constrained_by(answer_limits);
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &call_descriptor,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*read }.policy_answer_ceiling(), Some(expected));
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { &*write }.policy_answer_ceiling(),
            Some(answer_limits)
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let zero_statement_answer = QueryExecutionResourceLimits {
            statements: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let zero_statement_fixture = DatabaseConfigV2Fixture::plaintext(zero_statement_answer);
        let (mut zero_statement_database, mut diagnostics, status) = unsafe {
            open_database_v2(
                runtime,
                package,
                &zero_statement_fixture.config,
                ptr::null(),
            )
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        let opens_before = state.opens.load(Ordering::Acquire);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    zero_statement_database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(read.is_null());
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_statement_limit"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_database_close(&mut zero_statement_database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn v2_transaction_open_wakes_for_cancellation_and_deadline() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "v2opencontrol")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        state.pending_open.store(true, Ordering::Release);

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let cancellation_address = cancellation as usize;
        let observed = Arc::clone(&state);
        let requester = std::thread::spawn(move || {
            while observed.opens.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            assert_eq!(
                unsafe {
                    type_bridge_cancellation_request(
                        cancellation_address as *const TypeBridgeCancellation,
                    )
                },
                TypeBridgeStatus::Ok
            );
        });
        let limits = limits_descriptor(QueryExecutionResourceLimits::default());
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &limits,
                    cancellation,
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        requester.join().unwrap();
        assert!(read.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_cancelled"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let timeout_limits = QueryExecutionResourceLimits {
            timeout_milliseconds: 1,
            ..QueryExecutionResourceLimits::default()
        };
        let timeout_descriptor = limits_descriptor(timeout_limits);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &timeout_descriptor,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(read.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        state.pending_open.store(false, Ordering::Release);

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        state.supports_given_rows.store(true, Ordering::Release);
        let inherited_timeout = DatabaseConfigV2Fixture::plaintext(timeout_limits);
        let (mut database, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &inherited_timeout.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        state.pending_open.store(true, Ordering::Release);
        let opens_before = state.opens.load(Ordering::Acquire);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(read.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(write.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        assert_eq!(state.opens.load(Ordering::Acquire), opens_before + 2);
        unsafe { close_diagnostics(&mut diagnostics) };
        state.pending_open.store(false, Ordering::Release);
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn rollback_only_commit_is_exact_pre_dispatch_and_retains_legacy_and_v2_owners() {
        let state = Arc::new(FakeState::default());
        state.supports_given_rows.store(true, Ordering::Release);
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "rollbackonly")));
        let fixture = DatabaseConfigV2Fixture::plaintext(QueryExecutionResourceLimits::default());
        let (mut database, mut diagnostics, status) =
            unsafe { open_database_v2(runtime, package, &fixture.config, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        let cause = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write);

        let legacy_context = unsafe { &*database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        unsafe { &*database }.block_on(legacy_context.latch_rollback_only(&cause));
        let mut legacy = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*database },
                legacy_context,
                unsafe { &*database }.policy_answer_ceiling(),
            ),
        ));
        let legacy_before = legacy;
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut legacy, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(legacy, legacy_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_rollback_only"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                (&*legacy).block_on(
                    (&*legacy)
                        .context()
                        .expect("the rejected owner retains its context")
                        .lifecycle_state(),
                )
            },
            TransactionContextState::RollbackOnly
        );

        let v2_context = unsafe { &*database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        unsafe { &*database }.block_on(v2_context.latch_rollback_only(&cause));
        let mut v2 = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*database },
                v2_context,
                unsafe { &*database }.policy_answer_ceiling(),
            ),
        ));
        let v2_before = v2;
        let descriptor = limits_descriptor(QueryExecutionResourceLimits::default());
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut v2,
                    &descriptor,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(v2, v2_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_rollback_only"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe {
                (&*v2).block_on(
                    (&*v2)
                        .context()
                        .expect("the rejected owner retains its context")
                        .lifecycle_state(),
                )
            },
            TransactionContextState::RollbackOnly
        );

        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut legacy, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut v2, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 2);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn v2_and_inherited_legacy_commits_reject_before_dispatch_but_actual_outcome_wins() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "v2commit")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);

        let context = unsafe { &*database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        let zero_statement = QueryExecutionResourceLimits {
            statements: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let mut inherited = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*database },
                context,
                Some(zero_statement),
            ),
        ));
        let inherited_before = inherited;
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut inherited, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(inherited, inherited_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_statement_limit"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut inherited, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let context = unsafe { &*database }
            .open_transaction_context(TxType::Write)
            .unwrap();
        let zero_timeout = QueryExecutionResourceLimits {
            timeout_milliseconds: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let mut inherited = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                unsafe { &*database },
                context,
                Some(zero_timeout),
            ),
        ));
        let inherited_before = inherited;
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut inherited, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(inherited, inherited_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut inherited, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let zero_statement_descriptor = limits_descriptor(zero_statement);
        let write_before = write;
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut write,
                    &zero_statement_descriptor,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), 0);
        unsafe { close_diagnostics(&mut diagnostics) };

        let zero_timeout_descriptor = limits_descriptor(zero_timeout);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut write,
                    &zero_timeout_descriptor,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(write, write_before);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "transaction_deadline_exceeded"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        state.commit_delay_milliseconds.store(50, Ordering::Release);
        state.commit_started.store(false, Ordering::Release);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let cancellation_address = cancellation as usize;
        let observed = Arc::clone(&state);
        let requester = std::thread::spawn(move || {
            while !observed.commit_started.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            assert_eq!(
                unsafe {
                    type_bridge_cancellation_request(
                        cancellation_address as *const TypeBridgeCancellation,
                    )
                },
                TypeBridgeStatus::Ok
            );
        });
        let default_descriptor = limits_descriptor(QueryExecutionResourceLimits::default());
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut write,
                    &default_descriptor,
                    cancellation,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        requester.join().unwrap();
        assert!(write.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(state.commits.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn v2_commit_deadline_awaits_actual_outcome_without_cancelling_reusable_token() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "commitdeadline")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        state.commit_delay_milliseconds.store(75, Ordering::Release);
        state.commit_started.store(false, Ordering::Release);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let answer = unsafe { &*cancellation }.answer_cancellation();
        let observed = Arc::clone(&state);
        let observer = std::thread::spawn(move || {
            while !observed.commit_started.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            answer.is_cancelled()
        });
        let deadline_limits = QueryExecutionResourceLimits {
            timeout_milliseconds: 5,
            ..QueryExecutionResourceLimits::default()
        };
        let deadline_descriptor = limits_descriptor(deadline_limits);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit_v2_impl(
                    &mut write,
                    &deadline_descriptor,
                    cancellation,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!observer.join().unwrap());
        assert!(write.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(state.commits.load(Ordering::Acquire), 1);
        let mut requested = 1;
        assert_eq!(
            unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(requested, 0);
        assert!(
            !unsafe { &*cancellation }
                .answer_cancellation()
                .is_cancelled()
        );

        let mut read = ptr::null_mut();
        let default_descriptor = limits_descriptor(QueryExecutionResourceLimits::default());
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open_v2_impl(
                    database,
                    &default_descriptor,
                    cancellation,
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn parent_lifecycle_retains_in_use_slots_and_closes_children() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "lifecycle")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert!(diagnostics.is_null());

        let mut version = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            unsafe { type_bridge_database_server_version(database, &mut version) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(version.data, version.length) },
            b"3.12.1"
        );

        let runtime_before = runtime;
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::InUse
        );
        assert_eq!(runtime, runtime_before);
        assert!(!diagnostics.is_null());
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
        assert!(package.is_null());

        let mut transaction = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let database_before = database;
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::InUse
        );
        assert_eq!(database, database_before);
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(transaction.is_null());
        assert_eq!(state.transaction_closes.load(Ordering::Acquire), 1);

        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(write.is_null());
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(database.is_null());
        assert_eq!(state.database_closes.load(Ordering::Acquire), 1);
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(runtime.is_null());
    }

    #[test]
    fn cancellation_and_invalid_layout_reject_before_provider_io() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package_a = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "packagea")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package_a, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);
        assert_eq!(state.connects.load(Ordering::Acquire), 1);

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let mut requested = 0;
        assert_eq!(
            unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(requested, 1);

        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    cancellation,
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert!(read.is_null());
        assert_eq!(state.opens.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "cancelled_before_dispatch"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        let (_address, _database, _username, _password, mut invalid) = database_config();
        invalid.reserved[0] = 1;
        let mut rejected = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_open_v1(
                    runtime,
                    package_a,
                    &invalid,
                    ptr::null(),
                    &mut rejected,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(rejected.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 1);
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package_a) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn cancellation_reader_rejects_interior_output_without_corrupting_handle() {
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
        let interior = unsafe { cancellation.cast::<u8>().add(1) };
        assert_eq!(
            unsafe { type_bridge_cancellation_is_requested(cancellation, interior) },
            TypeBridgeStatus::InvalidArgument,
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok,
        );
        let mut requested = 0;
        assert_eq!(
            unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(requested, 1);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok,
        );
    }

    #[test]
    fn database_configuration_is_copied_bounded_and_utf8_checked_before_dispatch() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "config")));
        let (address, database_name, username, _password, mut config) = database_config();
        let mut database = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();

        let oversized = vec![b'a'; MAX_ADDRESS_BYTES + 1];
        config.address = view(&oversized);
        assert_eq!(
            unsafe {
                type_bridge_database_open_v1(
                    runtime,
                    package,
                    &config,
                    ptr::null(),
                    &mut database,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        unsafe { close_diagnostics(&mut diagnostics) };

        let invalid_utf8 = [0xff];
        config.address = view(&invalid_utf8);
        assert_eq!(
            unsafe {
                type_bridge_database_open_v1(
                    runtime,
                    package,
                    &config,
                    ptr::null(),
                    &mut database,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        unsafe { close_diagnostics(&mut diagnostics) };

        for (candidate, expected_status, expected_code) in [
            (
                "http://localhost:1729",
                TypeBridgeStatus::InvalidArgument,
                "c_database_address_invalid",
            ),
            (
                "secret@localhost:1729",
                TypeBridgeStatus::InvalidArgument,
                "c_database_address_invalid",
            ),
            (
                " localhost:1729",
                TypeBridgeStatus::InvalidArgument,
                "c_database_address_invalid",
            ),
            (
                "[::1:1729",
                TypeBridgeStatus::InvalidArgument,
                "c_database_address_invalid",
            ),
            (
                "localhost:0",
                TypeBridgeStatus::InvalidArgument,
                "c_database_address_invalid",
            ),
            (
                "localhost:1729,replica.example:1729",
                TypeBridgeStatus::Unsupported,
                "c_database_multi_endpoint_unsupported",
            ),
        ] {
            config.address = view(candidate.as_bytes());
            assert_eq!(
                unsafe {
                    type_bridge_database_open_v1(
                        runtime,
                        package,
                        &config,
                        ptr::null(),
                        &mut database,
                        &mut diagnostics,
                    )
                },
                expected_status,
                "unexpected status for hostile address spelling"
            );
            assert!(database.is_null());
            assert_eq!(state.connects.load(Ordering::Acquire), 0);
            assert_eq!(unsafe { diagnostic_code(diagnostics) }, expected_code);
            let exposed = String::from_utf8(unsafe { diagnostic_components(diagnostics) }).unwrap();
            assert!(
                !exposed.contains("secret@"),
                "credential-like address text reached diagnostics: {exposed}"
            );
            unsafe { close_diagnostics(&mut diagnostics) };
        }

        config.address = view(&address);
        config.http_port = 0;
        assert_eq!(
            unsafe {
                type_bridge_database_open_v1(
                    runtime,
                    package,
                    &config,
                    ptr::null(),
                    &mut database,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_database_config_http_port_invalid"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        config.address = view(&address);
        config.http_port = 8000;
        config.database = view(&database_name);
        config.username = view(&username);
        config.password = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        config.tls_mode = TLS_NATIVE_ROOTS;
        assert_eq!(
            unsafe {
                type_bridge_database_open_v1(
                    runtime,
                    package,
                    &config,
                    ptr::null(),
                    &mut database,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let copied = state.inputs.lock().unwrap().pop_back().unwrap();
        assert_eq!(copied.address, "localhost:1729");
        assert_eq!(copied.database, "workforce");
        assert_eq!(copied.username, "admin");
        assert_eq!(copied.password, "");
        assert_eq!(copied.http_port, 8000);
        assert!(copied.tls_native_roots);

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn write_terminals_preserve_commit_certainty_and_cancel_before_dispatch() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "commit")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);

        state.commit.store(COMMIT_ABORTED, Ordering::Release);
        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut write, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(write.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "commit_definitely_aborted"
        );
        assert!(
            !String::from_utf8_lossy(&unsafe { diagnostic_components(diagnostics) })
                .contains("secret")
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        state.commit.store(COMMIT_UNKNOWN, Ordering::Release);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut write, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::CommitOutcomeUnknown
        );
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "commit_outcome_unknown"
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        state.commit.store(COMMIT_SUCCESS, Ordering::Release);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let write_before = write;
        let commit_count = state.commits.load(Ordering::Acquire);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut write, cancellation, &mut diagnostics)
            },
            TypeBridgeStatus::Cancelled
        );
        assert_eq!(write, write_before);
        assert_eq!(state.commits.load(Ordering::Acquire), commit_count);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_write_transaction_close(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(state.rollbacks.load(Ordering::Acquire), 1);

        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn panicking_transaction_terminals_consume_slots_and_release_parent_children() {
        let state = Arc::new(FakeState::default());
        let mut runtime = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut package = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "panic")));
        let (mut database, mut diagnostics, status) =
            unsafe { open_database(runtime, package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Ok);

        state.panic_transaction_close.store(true, Ordering::Release);
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Panic
        );
        assert!(read.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { &*database }
                .state
                .transaction_children
                .load(Ordering::Acquire),
            0
        );
        state
            .panic_transaction_close
            .store(false, Ordering::Release);

        state.panic_rollback.store(true, Ordering::Release);
        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_write_transaction_rollback(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Panic
        );
        assert!(write.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(
            unsafe { &*database }
                .state
                .transaction_children
                .load(Ordering::Acquire),
            0
        );
        state.panic_rollback.store(false, Ordering::Release);

        state.commit.store(COMMIT_PANIC, Ordering::Release);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_commit(&mut write, ptr::null(), &mut diagnostics)
            },
            TypeBridgeStatus::CommitOutcomeUnknown
        );
        assert!(write.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "commit_outcome_unknown"
        );
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { &*database }
                .state
                .transaction_children
                .load(Ordering::Acquire),
            0
        );

        assert_eq!(
            unsafe { type_bridge_database_close(&mut database, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn exact_version_profile_and_provider_errors_are_redacted() {
        let state = Arc::new(FakeState::default());
        let mut runtime_handle = runtime(Arc::clone(&state), Version::new(3, 12, 2));
        let mut package_handle = Box::into_raw(Box::new(package("typedb-3.12.1/v1", "version")));
        let (database, mut diagnostics, status) =
            unsafe { open_database(runtime_handle, package_handle, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Unsupported);
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 1);
        assert_eq!(state.database_closes.load(Ordering::Acquire), 1);
        unsafe { close_diagnostics(&mut diagnostics) };
        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime_handle, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let state = Arc::new(FakeState::default());
        let mut runtime_handle = runtime(Arc::clone(&state), Version::new(3, 12, 1));
        let mut old_package = Box::into_raw(Box::new(package("typedb-3.11.5/v1", "oldprofile")));
        let (database, mut diagnostics, status) =
            unsafe { open_database(runtime_handle, old_package, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::Unsupported);
        assert!(database.is_null());
        assert_eq!(state.connects.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "c_database_semantic_profile_unsupported"
        );
        assert_eq!(
            unsafe { diagnostic_category(diagnostics) },
            TypeBridgeExecutionDiagnosticCategory::UnsupportedCapability
        );
        unsafe { close_diagnostics(&mut diagnostics) };

        state.fail_version_probe.store(true, Ordering::Release);
        let (database, mut diagnostics, status) =
            unsafe { open_database(runtime_handle, package_handle, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::ExecutionFailed);
        assert!(database.is_null());
        assert_eq!(
            unsafe { diagnostic_code(diagnostics) },
            "provider_operation_failed"
        );
        assert_eq!(
            unsafe { diagnostic_category(diagnostics) },
            TypeBridgeExecutionDiagnosticCategory::Provider
        );
        let components = unsafe { diagnostic_components(diagnostics) };
        let exposed = String::from_utf8_lossy(&components);
        assert!(!exposed.contains("private"));
        assert!(!exposed.contains("secret"));
        assert!(!exposed.contains("address"));
        unsafe { close_diagnostics(&mut diagnostics) };
        state.fail_version_probe.store(false, Ordering::Release);

        state.fail_connect.store(true, Ordering::Release);
        let (database, mut diagnostics, status) =
            unsafe { open_database(runtime_handle, package_handle, ptr::null()) };
        assert_eq!(status, TypeBridgeStatus::ExecutionFailed);
        assert!(database.is_null());
        let components = unsafe { diagnostic_components(diagnostics) };
        let exposed = String::from_utf8_lossy(&components);
        assert!(!exposed.contains("private"));
        assert!(!exposed.contains("secret"));
        assert!(!exposed.contains("address"));
        unsafe { close_diagnostics(&mut diagnostics) };

        assert_eq!(
            unsafe { type_bridge_runtime_close(&mut runtime_handle, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut old_package) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_schema_package_close(&mut package_handle) },
            TypeBridgeStatus::Ok
        );
    }
}
