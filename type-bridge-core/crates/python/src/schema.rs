//! PyO3 wrappers for Rust schema diff/generation.
//!
//! The schema policy lives in `type_bridge_orm::_schema`; this module only
//! marshals serde-compatible Python dicts across the boundary.

use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};
use type_bridge_orm::_schema::SchemaInfo;
use type_bridge_orm::_schema::diff::{ClassifiedChange, SchemaDiff};
use type_bridge_orm::_schema::generator;

/// Generate a TypeQL `define` block from a serialized `SchemaInfo` dict.
#[pyfunction(name = "_archived_generate_define_block")]
fn generate_define_block(py_info: Bound<'_, PyAny>) -> PyResult<String> {
    let info: SchemaInfo = depythonize(&py_info)
        .map_err(|error| py_value_error(format!("Invalid SchemaInfo: {error}")))?;
    info.validate()
        .map_err(|error| py_value_error(error.to_string()))?;
    Ok(generator::generate_define_block(&info))
}

/// Compute a serialized `SchemaDiff` dict from two serialized `SchemaInfo` dicts.
#[pyfunction(name = "_archived_compute_schema_diff")]
fn compute_schema_diff(
    py: Python<'_>,
    current: Bound<'_, PyAny>,
    target: Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let current: SchemaInfo = depythonize(&current)
        .map_err(|error| py_value_error(format!("Invalid current SchemaInfo: {error}")))?;
    let target: SchemaInfo = depythonize(&target)
        .map_err(|error| py_value_error(format!("Invalid target SchemaInfo: {error}")))?;
    let diff = SchemaDiff::compute(&current, &target);
    pythonize(py, &diff)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Classify a serialized `SchemaDiff` dict.
#[pyfunction(name = "_archived_classify_schema_diff")]
fn classify_schema_diff(py: Python<'_>, diff: Bound<'_, PyAny>) -> PyResult<PyObject> {
    let diff: SchemaDiff = depythonize(&diff)
        .map_err(|error| py_value_error(format!("Invalid SchemaDiff: {error}")))?;
    let classified: Vec<ClassifiedChange> = diff.classify();
    pythonize(py, &classified)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Return whether a serialized `SchemaDiff` has breaking changes.
#[pyfunction(name = "_archived_schema_diff_is_breaking")]
fn schema_diff_is_breaking(diff: Bound<'_, PyAny>) -> PyResult<bool> {
    let diff: SchemaDiff = depythonize(&diff)
        .map_err(|error| py_value_error(format!("Invalid SchemaDiff: {error}")))?;
    Ok(diff.has_breaking_changes())
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

/// Register schema facade functions on the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(generate_define_block, m)?)?;
    m.add_function(wrap_pyfunction!(compute_schema_diff, m)?)?;
    m.add_function(wrap_pyfunction!(classify_schema_diff, m)?)?;
    m.add_function(wrap_pyfunction!(schema_diff_is_breaking, m)?)?;
    Ok(())
}
