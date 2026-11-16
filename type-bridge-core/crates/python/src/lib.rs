//! # type-bridge-core (Python)
//!
//! PyO3 bindings that expose the **type-bridge** Rust core to Python.
//!
//! This crate provides native plan execution, generated projections and private
//! archived-schema seams over the shared Rust engine.
//!
//! All complex types cross the Python ↔ Rust boundary via
//! `pythonize` / `depythonize` using serde-tagged-enum dicts.

// Database one-shot calls await the deeper staged::Transaction::commit future.
#![recursion_limit = "256"]
#![warn(missing_docs)]

#[cfg(not(feature = "perf-stages"))]
mod debug;
#[allow(deprecated)] // PyO3 automatic FromPyObject for Clone pyclasses.
mod iid;
pub mod match_runtime;
mod migration_catalog_runtime;
pub mod migration_runtime;
pub mod orm_runtime;
mod perf_stages;
mod plan_builder_runtime;
pub mod plan_model_remote_runtime;
pub mod plan_runtime;
mod proxy_connection_runtime;
#[cfg(test)]
#[path = "../../rust/tests/support/purchase_plan.rs"]
mod purchase_plan;
mod query_plan_codec;
mod raw_runtime;
pub mod runtime_projection;
pub mod schema;
#[cfg(test)]
#[path = "../../../../tests/support/sdk_profile_matrix.rs"]
#[allow(dead_code)]
mod sdk_profile_matrix;
#[allow(deprecated)] // PyO3 tuple syntax for superclass initialization.
mod staged_transaction_runtime;
#[cfg(test)]
mod test_isolation;
mod validated_result_runtime;
pub mod version;

use type_bridge_core_lib as core;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pythonize::pythonize;

/// Python-facing wrapper around the Rust [`type_bridge_core_lib::_schema::TypeSchema`].
#[pyclass(name = "_ArchivedTypeSchema", module = "type_bridge_core")]
pub struct TypeSchema {
    inner: core::_schema::TypeSchema,
}

#[pymethods]
impl TypeSchema {
    /// Deserialize from a JSON string.
    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        let schema = core::_schema::TypeSchema::from_json(json)
            .map_err(pyo3::exceptions::PyValueError::new_err)?;
        Ok(TypeSchema { inner: schema })
    }

    /// Serialize to a JSON string.
    fn to_json(&self) -> PyResult<String> {
        self.inner
            .to_json()
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Check if a type (entity, relation, or attribute) is abstract.
    fn is_abstract(&self, type_name: &str) -> bool {
        self.inner.is_abstract(type_name)
    }

    /// Get all owned attributes for an entity or relation (as list of dicts).
    fn get_all_owned_attributes(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let attrs = self.inner.get_all_owned_attributes(name);
        pythonize(py, &attrs)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get all played roles for an entity or relation (as list of dicts).
    fn get_all_plays_roles(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let roles = self.inner.get_all_plays_roles(name);
        pythonize(py, &roles)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get all relates (role specs) for a relation (as list of dicts).
    fn get_all_relates(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let roles = self.inner.get_all_relates(name);
        pythonize(py, &roles)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the entities map as a Python dict.
    #[getter]
    fn entities(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        pythonize(py, &self.inner.entities)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the relations map as a Python dict.
    #[getter]
    fn relations(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        pythonize(py, &self.inner.relations)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the attributes map as a Python dict.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        pythonize(py, &self.inner.attributes)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the functions map as a Python dict.
    #[getter]
    fn functions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        pythonize(py, &self.inner.functions)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the structs map as a Python dict.
    #[getter]
    fn structs(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        pythonize(py, &self.inner.structs)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }
}

fn pythonize_serde_value<T>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>>
where
    T: serde::Serialize + ?Sized,
{
    pythonize(py, value)
        .map(|obj| obj.unbind())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Convert JSON without exposing serde_json's private arbitrary-precision
/// number representation to Python.
fn json_value_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(value) => pythonize_serde_value(py, value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                return pythonize_serde_value(py, &value);
            }
            if let Some(value) = value.as_u64() {
                return pythonize_serde_value(py, &value);
            }

            let token = value.to_string();
            if token.contains(['.', 'e', 'E']) {
                if let Some(value) = value.as_f64() {
                    return pythonize_serde_value(py, &value);
                }
                return Ok(py
                    .import("builtins")?
                    .getattr("float")?
                    .call1((token,))?
                    .unbind());
            }

            // With serde_json's `arbitrary_precision` feature, integers outside
            // u64/i64 have no primitive accessor. Let Python preserve them as an
            // arbitrary-precision int instead of silently rounding through f64.
            Ok(py
                .import("builtins")?
                .getattr("int")?
                .call1((token,))?
                .unbind())
        }
        serde_json::Value::String(value) => pythonize_serde_value(py, value),
        serde_json::Value::Array(values) => {
            let values = values
                .iter()
                .map(|value| json_value_to_python(py, value))
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, values)?.into_any().unbind())
        }
        serde_json::Value::Object(values) => {
            let result = PyDict::new(py);
            for (key, value) in values {
                result.set_item(key, json_value_to_python(py, value)?)?;
            }
            Ok(result.into_any().unbind())
        }
    }
}

/// Internal contract comparison without exposing upstream AST types.
#[pyfunction]
fn _typeql_queries_equivalent(left: &str, right: &str) -> PyResult<bool> {
    core::__internal::equivalent(left, right)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))
}

/// Run the workspace CLI in-process over process-style arguments.
///
/// `arguments` must include `argv[0]`. Output goes to the process stdout
/// and stderr exactly as the standalone `type-bridge` binary prints it;
/// the returned value is the process exit code. The GIL is released for
/// the whole run so connected commands do not block other Python threads.
#[pyfunction]
fn run_cli(py: Python<'_>, arguments: Vec<String>) -> i32 {
    py.detach(|| type_bridge_cli::run_cli(arguments))
}

// Preserve the GIL-required module contract across the dependency migration.
#[pymodule(gil_used = true)]
fn type_bridge_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    #[cfg(not(feature = "perf-stages"))]
    debug::install();
    m.add_class::<iid::PyIid>()?;
    m.add_class::<TypeSchema>()?;
    m.add_function(wrap_pyfunction!(_typeql_queries_equivalent, m)?)?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;

    // ORM CRUD query builder
    orm_runtime::register(m)?;
    runtime_projection::register(m)?;
    match_runtime::register(m)?;
    migration_runtime::register(m)?;
    migration_catalog_runtime::register(m)?;
    query_plan_codec::register(m)?;
    plan_runtime::register(m)?;
    plan_model_remote_runtime::register(m)?;
    proxy_connection_runtime::register(m)?;
    staged_transaction_runtime::register(m)?;
    raw_runtime::register(m)?;
    plan_builder_runtime::register(m)?;
    perf_stages::register(m)?;
    schema::register(m)?;
    version::register(m)?;

    let public = m
        .getattr("__all__")?
        .extract::<Vec<String>>()?
        .into_iter()
        .filter(|name| !name.starts_with('_'))
        .collect::<Vec<_>>();
    m.setattr("__all__", public)?;
    Ok(())
}
