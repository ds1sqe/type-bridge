//! Built-in proxy connection: terminals run against a TypeBridge server
//! through the same execute call shape as a database or read transaction.

use std::path::PathBuf;

use type_bridge_orm::{ProxyEndpoint, ProxyQueryConnection, QueryTarget, TlsPolicy};

use super::*;
use crate::policy::parse_common_limits;
use crate::policy::{TLS_CUSTOM_ROOT_CA, TLS_DISABLED, TLS_NATIVE_ROOTS};

const PROXY_CONNECTION_OPTIONS_VERSION: u32 = 1;
const PROXY_ENDPOINT_BYTES_MAX: usize = 4096;
const PROXY_CUSTOM_ROOT_PATH_BYTES_MAX: usize = 4096;
const PROXY_HEADER_COUNT_MAX: usize = 64;
const PROXY_HEADER_BYTES_MAX: usize = 8192;

/// One request header sent with every proxy request.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeProxyHeaderV1 {
    /// Header name bytes.
    pub name: TypeBridgeByteView,
    /// Header value bytes.
    pub value: TypeBridgeByteView,
}

/// Version-1 proxy connection options; every view is copied during open.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeProxyConnectionOptionsV1 {
    /// Exact `size_of` this structure.
    pub struct_size: u32,
    /// Exact options version.
    pub version: u32,
    /// Server base URL such as `http://localhost:8080`.
    pub endpoint: TypeBridgeByteView,
    /// One of the `TYPE_BRIDGE_TLS_*` modes.
    pub tls_mode: u32,
    /// Must be zero.
    pub reserved0: u32,
    /// Custom-root PEM path; empty outside custom-root mode.
    pub custom_root_ca_path: TypeBridgeByteView,
    /// Request headers, or null when `header_count` is zero.
    pub headers: *const TypeBridgeProxyHeaderV1,
    /// Number of request headers.
    pub header_count: usize,
    /// Pinned advertisement; empty fetches it from the server.
    pub pinned_advertisement: TypeBridgeByteView,
    /// Limits bounding the open itself; null selects defaults.
    pub connection_limits: *const TypeBridgeQueryExecutionLimitsV1,
    /// Ceiling for every terminal on the connection; null selects defaults.
    pub answer_limits: *const TypeBridgeQueryExecutionLimitsV1,
    /// Must be zero.
    pub reserved: [u64; 4],
}

/// Opaque connection to a TypeBridge server bound to one schema package.
pub struct TypeBridgeProxyConnection {
    package: Arc<SchemaPackageState>,
    connection: ProxyQueryConnection,
    runtime: tokio::runtime::Runtime,
}

struct CopiedOptions {
    endpoint: ProxyEndpoint,
    advertisement: Option<Vec<u8>>,
    connection_limits: QueryExecutionResourceLimits,
    answer_limits: QueryExecutionResourceLimits,
}

fn options_invalid(code_text: &'static str, text: &'static str) -> SdkExecutionDiagnostic {
    invalid(code_text, text)
}

unsafe fn copied_text(
    view: TypeBridgeByteView,
    max: usize,
    code_text: &'static str,
    text: &'static str,
) -> Result<String, SdkExecutionDiagnostic> {
    if view.length > max || (view.length != 0 && view.data.is_null()) {
        return Err(options_invalid(code_text, text));
    }
    // SAFETY: preflighted caller range, retained immutable for the call.
    let bytes = unsafe { copy_query_bytes(view, AllocationSite::ProxyConnectionBytes) }
        .map_err(|_| allocation_exhausted())?;
    String::from_utf8(bytes).map_err(|_| options_invalid(code_text, text))
}

unsafe fn read_limits(
    limits: *const TypeBridgeQueryExecutionLimitsV1,
) -> Result<QueryExecutionResourceLimits, SdkExecutionDiagnostic> {
    let snapshot = if limits.is_null() {
        None
    } else {
        // SAFETY: the complete descriptor was preflighted; unaligned read.
        Some(unsafe { limits.read_unaligned() })
    };
    parse_common_limits(snapshot)
}

/// Validate and copy every option before any I/O.
unsafe fn copy_options(
    options: &TypeBridgeProxyConnectionOptionsV1,
) -> Result<CopiedOptions, SdkExecutionDiagnostic> {
    if options.struct_size as usize != size_of::<TypeBridgeProxyConnectionOptionsV1>()
        || options.version != PROXY_CONNECTION_OPTIONS_VERSION
        || options.reserved0 != 0
        || options.reserved != [0; 4]
    {
        return Err(options_invalid(
            "c_proxy_options_layout_invalid",
            "The proxy connection options layout is invalid",
        ));
    }
    // SAFETY: limits descriptors were preflighted.
    let connection_limits = unsafe { read_limits(options.connection_limits) }?;
    // SAFETY: limits descriptors were preflighted.
    let answer_limits = unsafe { read_limits(options.answer_limits) }?;
    // SAFETY: preflighted caller range.
    let url = unsafe {
        copied_text(
            options.endpoint,
            PROXY_ENDPOINT_BYTES_MAX,
            "c_proxy_endpoint_invalid",
            "The proxy endpoint must be bounded UTF-8",
        )
    }?;
    if url.is_empty() {
        return Err(options_invalid(
            "c_proxy_endpoint_invalid",
            "The proxy endpoint must be bounded UTF-8",
        ));
    }
    let path_view = options.custom_root_ca_path;
    let tls = match options.tls_mode {
        TLS_DISABLED | TLS_NATIVE_ROOTS if path_view.length != 0 || !path_view.data.is_null() => {
            return Err(options_invalid(
                "c_proxy_custom_root_must_be_empty",
                "The custom-root path must be empty outside custom-root TLS mode",
            ));
        }
        TLS_DISABLED => TlsPolicy::disabled(),
        TLS_NATIVE_ROOTS => TlsPolicy::native_roots(),
        TLS_CUSTOM_ROOT_CA => {
            // SAFETY: preflighted caller range.
            let path = unsafe {
                copied_text(
                    path_view,
                    PROXY_CUSTOM_ROOT_PATH_BYTES_MAX,
                    "c_proxy_custom_root_path_invalid",
                    "The custom-root path must be bounded UTF-8",
                )
            }?;
            TlsPolicy::custom_root(PathBuf::from(path))?
        }
        _ => {
            return Err(options_invalid(
                "c_proxy_tls_mode_invalid",
                "The proxy TLS mode is invalid",
            ));
        }
    };
    let mut endpoint = ProxyEndpoint::new(url).tls(tls);
    if options.header_count > PROXY_HEADER_COUNT_MAX
        || (options.header_count != 0 && options.headers.is_null())
    {
        return Err(options_invalid(
            "c_proxy_headers_invalid",
            "The proxy header array is invalid",
        ));
    }
    for index in 0..options.header_count {
        // SAFETY: the complete header array was preflighted; unaligned read.
        let header = unsafe { options.headers.add(index).read_unaligned() };
        // SAFETY: preflighted caller ranges.
        let (name, value) = unsafe {
            (
                copied_text(
                    header.name,
                    PROXY_HEADER_BYTES_MAX,
                    "c_proxy_headers_invalid",
                    "The proxy header array is invalid",
                )?,
                copied_text(
                    header.value,
                    PROXY_HEADER_BYTES_MAX,
                    "c_proxy_headers_invalid",
                    "The proxy header array is invalid",
                )?,
            )
        };
        endpoint = endpoint.header(name, value);
    }
    let pinned = options.pinned_advertisement;
    let advertisement = if pinned.length == 0 && pinned.data.is_null() {
        None
    } else if pinned.length == 0 || pinned.data.is_null() {
        return Err(remote_advertisement_invalid());
    } else if pinned.length > MAX_REMOTE_ENVELOPE_BYTES {
        return Err(remote_advertisement_limit());
    } else {
        // SAFETY: preflighted caller range.
        Some(
            unsafe { copy_query_bytes(pinned, AllocationSite::QueryRemoteAdvertisementBytes) }
                .map_err(|_| allocation_exhausted())?,
        )
    };
    Ok(CopiedOptions {
        endpoint,
        advertisement,
        connection_limits,
        answer_limits,
    })
}

/// Reject option ranges that overlap the caller's output slots.
unsafe fn preflight_options(
    preflight: &DirectOutputPreflight,
    options: &TypeBridgeProxyConnectionOptionsV1,
) -> Result<(), TypeBridgeStatus> {
    let mut views = vec![
        options.endpoint,
        options.custom_root_ca_path,
        options.pinned_advertisement,
    ];
    // SAFETY: complete limits descriptors only.
    unsafe { preflight_handle(preflight, options.connection_limits) }?;
    // SAFETY: complete limits descriptors only.
    unsafe { preflight_handle(preflight, options.answer_limits) }?;
    if !options.headers.is_null() && options.header_count <= PROXY_HEADER_COUNT_MAX {
        preflight.check_bytes(
            options.headers.cast(),
            options.header_count * size_of::<TypeBridgeProxyHeaderV1>(),
        )?;
        for index in 0..options.header_count {
            // SAFETY: the header array was preflighted above.
            let header = unsafe { options.headers.add(index).read_unaligned() };
            views.push(header.name);
            views.push(header.value);
        }
    }
    for view in views {
        if !view.data.is_null() && view.length != 0 {
            preflight.check_bytes(view.data.cast(), view.length)?;
        }
    }
    Ok(())
}

fn proxy_runtime() -> Result<tokio::runtime::Runtime, SdkExecutionDiagnostic> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            SdkExecutionDiagnostic::resource_limit(
                code("c_proxy_runtime_unavailable"),
                message("The proxy connection could not start its I/O runtime"),
            )
        })
}

/// Open one blocking connection to a TypeBridge server for `package`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_open_v1(
    package: *const TypeBridgeSchemaPackage,
    options: *const TypeBridgeProxyConnectionOptionsV1,
    cancellation: *const TypeBridgeCancellation,
    out_connection: *mut *mut TypeBridgeProxyConnection,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(out_connection, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    for result in [
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, package) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, options) },
        // SAFETY: only complete outer objects are inspected before initialization.
        unsafe { preflight_handle(&preflight, cancellation) },
    ] {
        if let Err(status) = result {
            return status;
        }
    }
    let options = if options.is_null() {
        None
    } else {
        // SAFETY: the complete options descriptor was preflighted; unaligned read.
        Some(unsafe { options.read_unaligned() })
    };
    if let Some(options) = &options
        // SAFETY: every option range is checked against the output slots.
        && let Err(status) = unsafe { preflight_options(&preflight, options) }
    {
        return status;
    }
    if !package.is_null()
        // SAFETY: caller retains one complete immutable package handle.
        && let Err(status) = preflight.check_package_borrowed_ranges(unsafe { &*package }.state())
    {
        return status;
    }
    // SAFETY: complete graph preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_connection, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(options) = options else {
            return return_execution_error(
                options_invalid(
                    "c_proxy_options_missing",
                    "The proxy connection options are required",
                ),
                out_diagnostics,
            );
        };
        let answer = if cancellation.is_null() {
            type_bridge_orm::AnswerCancellation::default()
        } else {
            // SAFETY: the complete cancellation handle was preflighted.
            unsafe { &*cancellation }.answer_cancellation()
        };
        // SAFETY: every option range was preflighted and is retained for the call.
        let copied = match unsafe { copy_options(&options) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        // SAFETY: caller retains the immutable package for the call.
        let package = unsafe { &*package }.state();
        let (authority, _registry) = match remote_authority(package) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::ProxyConnectionHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let runtime = match proxy_runtime() {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(ProxyQueryConnection::connect(
                &copied.endpoint,
                copied.advertisement,
                authority,
                copied.connection_limits,
                copied.answer_limits,
                &answer,
            ))
        }));
        let connection = match outcome {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return return_execution_error(error, out_diagnostics),
            Err(_) => return TypeBridgeStatus::Panic,
        };
        let handle = reservation.initialize(TypeBridgeProxyConnection {
            package: Arc::clone(package),
            connection,
            runtime,
        });
        // SAFETY: the output slot was preflighted and initialized.
        unsafe { out_connection.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one proxy connection; no execute call may be running on it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_connection_close(
    connection: *mut *mut TypeBridgeProxyConnection,
) -> TypeBridgeStatus {
    // SAFETY: ownership follows the public pointer-to-pointer close contract.
    unsafe { close_box(connection) }
}

/// Execute one reusable terminal on a TypeBridge server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_proxy_query_execute_v1(
    proxy: *const TypeBridgeProxyConnection,
    terminal: *const TypeBridgeQueryTerminal,
    expected_terminal_kind: u32,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeQueryResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: shared execution preflight checks every complete active input.
    let invocation = match unsafe {
        execute_preflight(
            proxy,
            |preflight, proxy: &TypeBridgeProxyConnection| {
                preflight.check_package_borrowed_ranges(&proxy.package)
            },
            terminal,
            limits,
            cancellation,
            out_result,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    let ExecuteInvocationPreflight {
        outputs: _preflight,
        answer,
        parsed,
    } = invocation;
    // SAFETY: full alias preflight precedes independent output initialization.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if proxy.is_null() || terminal.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains both immutable handles through the call.
        let proxy = unsafe { &*proxy };
        // SAFETY: caller retains the immutable terminal.
        let terminal = unsafe { &*terminal };
        let parsed = match parsed {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, out_diagnostics),
        };
        if let Err(error) = parsed.deadline.check(&answer) {
            return return_execution_error(error, out_diagnostics);
        }
        if terminal.kind != expected_terminal_kind {
            return return_execution_error(nominal_mismatch(), out_diagnostics);
        }
        if !Arc::ptr_eq(&proxy.package, &terminal.state.package) {
            return return_execution_error(package_mismatch(), out_diagnostics);
        }
        if matches!(terminal.spec, TerminalSpec::Manager { .. }) {
            return return_execution_error(manager_remote_unsupported(), out_diagnostics);
        }
        let request = match validate_terminal(&terminal.query, &terminal.spec) {
            Ok(value) => value,
            Err(error) => return return_execution_error(lower(error), out_diagnostics),
        };
        let reservation = match ReservedBox::try_new(AllocationSite::QueryResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            proxy.runtime.block_on(execute_validated_on_target(
                QueryTarget::Proxy(&proxy.connection),
                terminal,
                request,
                ProjectedQueryOrigin::remote_unbound(),
                parsed.resources,
                parsed.deadline,
                &answer,
            ))
        }));
        let value = match outcome {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                return return_query_failure(
                    QueryExecutionFailure::Diagnostic(error),
                    out_diagnostics,
                );
            }
            Err(_) => return return_query_failure(QueryExecutionFailure::Panic, out_diagnostics),
        };
        let result = reservation.initialize(value);
        // SAFETY: output slot was initialized and remains caller-writable.
        unsafe { out_result.write_unaligned(Box::into_raw(result)) };
        TypeBridgeStatus::Ok
    })
}
