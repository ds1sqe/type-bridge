//! PyO3 wrappers for the TypeDB version gate.
//!
//! All policy lives in `type_bridge_core_lib::version` (the SSOT).  This
//! module is a thin client: parse strings, delegate to core, map errors.
//!
//! Exposed surface on the `type_bridge_core` Python module:
//!
//! | Symbol | Kind | Description |
//! |--------|------|-------------|
//! | `VersionError` | exception | Raised for window, band-mismatch, probe, and parse failures |
//! | `min_supported_version` | function | Returns the floor as `"3.8.0"` |
//! | `max_supported_line` | function | Returns the ceiling line as `"3.11"` |
//! | `band` | function | Protocol-band lookup; `None` for unmapped versions |
//! | `check_supported` | function | Window + band gate; raises `VersionError` on failure |
//! | `embedded_driver_version` | function | The typedb-driver version compiled into the Rust runtime |
//! | `server_version` | function | HTTP probe → detected server version string |

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use type_bridge_core_lib::version as core_version;

// ---------------------------------------------------------------------------
// Python exception
// ---------------------------------------------------------------------------

pyo3::create_exception!(
    type_bridge_core,
    VersionError,
    PyException,
    "Raised when a driver/server version pair is unsupported or undetectable."
);

/// Map a core [`core_version::VersionError`] to the Python `VersionError` exception.
fn to_py_err(e: core_version::VersionError) -> PyErr {
    VersionError::new_err(e.to_string())
}

// ---------------------------------------------------------------------------
// Window accessors
// ---------------------------------------------------------------------------

/// Return the minimum supported TypeDB version as a string (`"3.8.0"`).
///
/// This value is the floor of the declared compatibility window.
#[pyfunction]
pub fn min_supported_version() -> String {
    core_version::MIN_SUPPORTED.to_string()
}

/// Return the maximum supported TypeDB line as `"major.minor"` (e.g. `"3.11"`).
///
/// Any patch release on this line is in-window; the next minor is not.
#[pyfunction]
pub fn max_supported_line() -> String {
    let (major, minor) = core_version::MAX_SUPPORTED_LINE;
    format!("{major}.{minor}")
}

// ---------------------------------------------------------------------------
// Band lookup
// ---------------------------------------------------------------------------

/// Return the protocol band for a TypeDB version string, or `None` if unmapped.
///
/// # Errors
///
/// Raises `VersionError` when `version` cannot be parsed as a TypeDB version.
#[pyfunction]
pub fn band(version: &str) -> PyResult<Option<u8>> {
    let v = version
        .parse::<core_version::Version>()
        .map_err(to_py_err)?;
    Ok(core_version::band(&v))
}

// ---------------------------------------------------------------------------
// Combined gate
// ---------------------------------------------------------------------------

/// Assert that `driver` and `server` versions are mutually compatible.
///
/// Both the support window and the protocol band must agree.  See the Rust
/// `check_supported` documentation for the exact rules.
///
/// # Errors
///
/// Raises `VersionError` when either version string is unparseable or when the
/// versions are incompatible (window violation or band mismatch).
#[pyfunction]
pub fn check_supported(driver: &str, server: &str) -> PyResult<()> {
    let d = driver.parse::<core_version::Version>().map_err(to_py_err)?;
    let s = server.parse::<core_version::Version>().map_err(to_py_err)?;
    core_version::check_supported(&d, &s).map_err(to_py_err)
}

// ---------------------------------------------------------------------------
// Embedded runtime driver
// ---------------------------------------------------------------------------

/// Return the `typedb-driver` version compiled into the Rust runtime.
///
/// Every TypeBridge transaction executes through the embedded Rust driver,
/// so its protocol band — not only the installed Python driver's — must
/// match the server. The connect-time gate checks both.
#[pyfunction]
pub fn embedded_driver_version() -> &'static str {
    type_bridge_orm::session::real_driver::PINNED_DRIVER_VERSION
}

// ---------------------------------------------------------------------------
// HTTP probe
// ---------------------------------------------------------------------------

/// Query the TypeDB HTTP API for the server version.
///
/// Returns the detected version as a string (e.g. `"3.10.4"`).
///
/// The `address` is a gRPC-style address (`"host:1729"` or bare `"host"`).
/// `http_port` defaults to `8000`; `tls` defaults to `False`.
///
/// The GIL is released for the blocking HTTP call so other threads are not
/// stalled during the probe.
///
/// # Errors
///
/// Raises `VersionError` when the endpoint is unreachable, returns an
/// unexpected response, or the version string cannot be parsed.
#[pyfunction]
#[pyo3(signature = (address, http_port = 8000, tls = false))]
pub fn server_version(
    py: Python<'_>,
    address: String,
    http_port: u16,
    tls: bool,
) -> PyResult<String> {
    // Release the GIL across the blocking HTTP call.
    let result = py.allow_threads(move || core_version::server_version(&address, http_port, tls));
    result.map(|v| v.to_string()).map_err(to_py_err)
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// Register version gate symbols on the `type_bridge_core` Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("VersionError", m.py().get_type::<VersionError>())?;
    m.add_function(wrap_pyfunction!(min_supported_version, m)?)?;
    m.add_function(wrap_pyfunction!(max_supported_line, m)?)?;
    m.add_function(wrap_pyfunction!(band, m)?)?;
    m.add_function(wrap_pyfunction!(check_supported, m)?)?;
    m.add_function(wrap_pyfunction!(embedded_driver_version, m)?)?;
    m.add_function(wrap_pyfunction!(server_version, m)?)?;
    Ok(())
}
