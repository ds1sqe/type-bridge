//! Migration-state schema label projections for the Python facade.

use pyo3::prelude::*;
use pythonize::pythonize;
use type_bridge_schema_migration_typedb::{
    MigrationStateSchemaKind,
    applied_migration_entity_label as rust_applied_migration_entity_label,
    is_migration_state_type as rust_is_migration_state_type,
    migration_state_schema as rust_migration_state_schema,
};

#[pyfunction]
fn migration_state_schema(py: Python<'_>) -> PyResult<Py<PyAny>> {
    pythonize(py, rust_migration_state_schema())
        .map(|object| object.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Return the canonical applied-migration entity label.
#[pyfunction]
fn applied_migration_entity_label() -> &'static str {
    rust_applied_migration_entity_label()
}

/// Return whether a kind/label pair belongs to the migration-state schema.
#[pyfunction]
fn is_migration_state_type(kind: &str, label: &str) -> PyResult<bool> {
    let kind = match kind {
        "entity" => MigrationStateSchemaKind::Entity,
        "relation" => MigrationStateSchemaKind::Relation,
        "attribute" => MigrationStateSchemaKind::Attribute,
        "role" => MigrationStateSchemaKind::Role,
        _ => {
            return Err(py_value_error(format!(
                "Invalid migration state schema kind {kind:?}; expected entity, relation, attribute, or role"
            )));
        }
    };
    Ok(rust_is_migration_state_type(kind, label))
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

/// Register migration-state schema helpers.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(migration_state_schema, m)?)?;
    m.add_function(wrap_pyfunction!(applied_migration_entity_label, m)?)?;
    m.add_function(wrap_pyfunction!(is_migration_state_type, m)?)?;
    Ok(())
}
