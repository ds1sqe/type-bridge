//! PyO3 wrapper for the TOML schema DSL transpiler.
//!
//! Exposes `toml_to_typeql` from `type_bridge_toml_transpiler` as a Python
//! callable.  The pure-Rust library (`crates/toml-transpiler`) has no PyO3
//! dependency; this module is the sole PyO3 surface.

use pyo3::prelude::*;
use type_bridge_toml_transpiler::toml_to_typeql as rust_toml_to_typeql;

/// Transpile a TOML schema document into a canonical TypeQL `define` block.
///
/// Accepts the full text of a type-bridge `.toml` schema file and returns the
/// equivalent TypeQL string that can be passed directly to `parse_tql_schema`.
///
/// # Errors
///
/// Raises `ValueError` when the input is not valid TOML or does not conform to
/// the type-bridge schema shape (unknown key, missing required field, etc.).
#[pyfunction]
pub fn toml_to_typeql(toml_text: &str) -> PyResult<String> {
    rust_toml_to_typeql(toml_text).map_err(|error| py_value_error(error.to_string()))
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}
