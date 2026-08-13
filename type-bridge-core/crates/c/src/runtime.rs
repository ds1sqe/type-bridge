//! Synchronous C runtime, database, transaction, and cancellation boundary.

use std::future::Future;
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
    ConnectOptions, Database, OrmError, TransactionContext, TxType,
    is_identity_safe_provider_address, lower_classified_commit_error, lower_execution_error,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded,
    initialize_view,
};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, execution_diagnostics_handle, return_execution_error,
};
use crate::generated_preflight::direct_output_preflight;

const RUNTIME_CONFIG_VERSION: u32 = 1;
const DATABASE_CONFIG_VERSION: u32 = 1;
const MIN_RUNTIME_WORKER_THREADS: u32 = 1;
const MAX_RUNTIME_WORKER_THREADS: u32 = 64;
const MAX_ADDRESS_BYTES: usize = 4 * 1024;
const MAX_DATABASE_NAME_BYTES: usize = 256;
const MAX_USERNAME_BYTES: usize = 4 * 1024;
const MAX_PASSWORD_BYTES: usize = 64 * 1024;
const TLS_DISABLED: u32 = 0;
const TLS_NATIVE_ROOTS: u32 = 1;
const REQUIRED_SEMANTIC_PROFILE: &[u8] = b"typedb-3.12.1/v1";

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Database, OrmError>> + Send + 'static>>;

trait DatabaseConnector: Send + Sync {
    fn connect(&self, input: DatabaseConnectInput) -> ConnectFuture;
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
        commit_context_classified(&self.state.runtime.runtime, context)
    }

    #[cfg(test)]
    pub(crate) fn from_test_database(package: Arc<SchemaPackageState>, database: Database) -> Self {
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
                transaction_children: AtomicUsize::new(0),
            }),
        }
    }
}

impl TypeBridgeReadTransaction {
    pub(crate) fn package_state(&self) -> &Arc<SchemaPackageState> {
        &self.state.database._package
    }

    pub(crate) fn context(&self) -> Option<&TransactionContext> {
        self.state.context.as_ref()
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
    pub(crate) fn package_state(&self) -> &Arc<SchemaPackageState> {
        &self.state.database._package
    }

    pub(crate) fn context(&self) -> Option<&TransactionContext> {
        self.state.context.as_ref()
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

fn commit_context_classified(
    runtime: &Runtime,
    context: &TransactionContext,
) -> Result<(), SdkExecutionDiagnostic> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(context.commit_classified())
    })) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(lower_classified_commit_error(error)),
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

/// Open one package-fenced read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_open(
    database: *const TypeBridgeDatabase,
    cancellation: *const TypeBridgeCancellation,
    out_transaction: *mut *mut TypeBridgeReadTransaction,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    if aliases(database, out_transaction)
        || aliases(database, out_diagnostics)
        || aliases(cancellation, out_transaction)
        || aliases(cancellation, out_diagnostics)
    {
        return TypeBridgeStatus::InvalidArgument;
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
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.state.runtime.runtime.block_on(context.close());
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeReadTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                poisoned: AtomicBool::new(false),
            },
        };
        unsafe { out_transaction.write(Box::into_raw(Box::new(transaction))) };
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
    if aliases(database, out_transaction)
        || aliases(database, out_diagnostics)
        || aliases(cancellation, out_transaction)
        || aliases(cancellation, out_diagnostics)
    {
        return TypeBridgeStatus::InvalidArgument;
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
        if let Err(diagnostic) = increment_child(&database.state.transaction_children) {
            let _ = database.state.runtime.runtime.block_on(context.close());
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let transaction = TypeBridgeWriteTransaction {
            state: TransactionState {
                database: Arc::clone(&database.state),
                context: Some(context),
                poisoned: AtomicBool::new(false),
            },
        };
        unsafe { out_transaction.write(Box::into_raw(Box::new(transaction))) };
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
    if transaction.is_null() || out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    if transaction.cast::<()>() == out_diagnostics.cast::<()>()
        || (!cancellation.is_null()
            && (cancellation.cast::<()>() == transaction.cast::<()>()
                || cancellation.cast::<()>() == out_diagnostics.cast::<()>()))
    {
        return TypeBridgeStatus::InvalidArgument;
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
        // SAFETY: the live handle remains caller-owned on this pre-dispatch rejection.
        if unsafe { &*value }.state.poisoned.load(Ordering::Acquire) {
            return return_execution_error(poisoned_transaction_diagnostic(), out_diagnostics);
        }
        if is_cancelled(cancellation) {
            return cancellation_error(out_diagnostics);
        }
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
        // Cancellation is deliberately not observed after this dispatch point.
        let result = commit_context_classified(&handle.state.database.runtime.runtime, &context);
        drop(handle);
        match result {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        }
    })
}

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
    use std::sync::atomic::{AtomicBool, AtomicU8};

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
        panic_transaction_close: AtomicBool,
        panic_rollback: AtomicBool,
        commit: AtomicU8,
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
