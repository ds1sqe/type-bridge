//! PyO3 wrappers for migration spec normalization, graph validation, and the
//! checksum-drift gate.
//!
//! These functions serde-normalize migration IR and run pure graph/checksum
//! checks over it. They open no transactions and execute no TypeQL — migration
//! execution is owned by a later sub-plan.

use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};
use type_bridge_migration::{
    AppliedMigrationRecord, MigrationGraph, MigrationSpec, check_checksum_drift,
    migration_file_checksum, validate_graph,
};

/// Normalize a serialized `MigrationSpec` dict through Rust serde.
#[pyfunction]
fn normalize_migration_spec(py: Python<'_>, spec: Bound<'_, PyAny>) -> PyResult<PyObject> {
    let spec: MigrationSpec = depythonize(&spec)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec: {error}")))?;
    pythonize(py, &spec)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Normalize a serialized `MigrationGraph` dict through Rust serde.
#[pyfunction]
fn normalize_migration_graph(py: Python<'_>, graph: Bound<'_, PyAny>) -> PyResult<PyObject> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    pythonize(py, &graph)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Serialize a serialized `MigrationSpec` dict to canonical JSON.
#[pyfunction]
fn migration_spec_to_json(spec: Bound<'_, PyAny>) -> PyResult<String> {
    let spec: MigrationSpec = depythonize(&spec)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec: {error}")))?;
    serde_json::to_string(&spec).map_err(|error| py_value_error(error.to_string()))
}

/// Deserialize a `MigrationSpec` JSON string and return its normalized dict.
#[pyfunction]
fn migration_spec_from_json(py: Python<'_>, json: &str) -> PyResult<PyObject> {
    let spec: MigrationSpec = serde_json::from_str(json)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec JSON: {error}")))?;
    pythonize(py, &spec)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Serialize a serialized `MigrationGraph` dict to canonical JSON.
#[pyfunction]
fn migration_graph_to_json(graph: Bound<'_, PyAny>) -> PyResult<String> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    serde_json::to_string(&graph).map_err(|error| py_value_error(error.to_string()))
}

/// Deserialize a `MigrationGraph` JSON string and return its normalized dict.
#[pyfunction]
fn migration_graph_from_json(py: Python<'_>, json: &str) -> PyResult<PyObject> {
    let graph: MigrationGraph = serde_json::from_str(json)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph JSON: {error}")))?;
    pythonize(py, &graph)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Calculate the migration-file checksum used for drift detection.
#[pyfunction]
fn calculate_migration_file_checksum(content: &str) -> String {
    migration_file_checksum(content)
}

/// Validate a serialized migration graph and return structured errors.
#[pyfunction]
#[pyo3(signature = (graph, applied_records = None))]
fn validate_migration_graph(
    py: Python<'_>,
    graph: Bound<'_, PyAny>,
    applied_records: Option<Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    let applied_records = depythonize_applied_records(applied_records)?;
    let errors = validate_graph(&graph, &applied_records);
    pythonize(py, &errors)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Fail if any applied migration checksum differs from the loaded graph.
#[pyfunction]
fn check_migration_drift(
    graph: Bound<'_, PyAny>,
    applied_records: Bound<'_, PyAny>,
) -> PyResult<()> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    let applied_records: Vec<AppliedMigrationRecord> = depythonize(&applied_records)
        .map_err(|error| py_value_error(format!("Invalid applied migration records: {error}")))?;
    check_checksum_drift(&graph, &applied_records)
        .map_err(|error| py_value_error(error.to_string()))
}

fn depythonize_applied_records(
    applied_records: Option<Bound<'_, PyAny>>,
) -> PyResult<Vec<AppliedMigrationRecord>> {
    let Some(applied_records) = applied_records else {
        return Ok(Vec::new());
    };
    if applied_records.is_none() {
        return Ok(Vec::new());
    }
    depythonize(&applied_records)
        .map_err(|error| py_value_error(format!("Invalid applied migration records: {error}")))
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

/// Register migration-runtime facade functions on the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(normalize_migration_spec, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_migration_graph, m)?)?;
    m.add_function(wrap_pyfunction!(migration_spec_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(migration_spec_from_json, m)?)?;
    m.add_function(wrap_pyfunction!(migration_graph_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(migration_graph_from_json, m)?)?;
    m.add_function(wrap_pyfunction!(calculate_migration_file_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(validate_migration_graph, m)?)?;
    m.add_function(wrap_pyfunction!(check_migration_drift, m)?)?;
    Ok(())
}
