//! Immutable generated migration-catalog inspection for Python.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use type_bridge_schema::{ManagedDeltaContext, decode_schema_authority};
use type_bridge_schema_migration::{MigrationCatalog, migration_runtime_capability_vocabulary};

/// One canonical compound migration identity.
#[pyclass(name = "MigrationIdentity", frozen)]
#[derive(Clone)]
pub struct PyMigrationIdentity {
    app_label: String,
    name: String,
}

#[pymethods]
impl PyMigrationIdentity {
    #[getter]
    fn app_label(&self) -> &str {
        &self.app_label
    }

    #[getter]
    fn name(&self) -> &str {
        &self.name
    }
}

/// Bounded immutable snapshot of one replay-verified history entry.
#[pyclass(name = "MigrationHistoryEntry", frozen)]
#[derive(Clone)]
pub struct PyMigrationHistoryEntry {
    id: PyMigrationIdentity,
    parents: Vec<PyMigrationIdentity>,
    manifest_digest: String,
    step_count: usize,
    safety: String,
    reversible: bool,
}

#[pymethods]
impl PyMigrationHistoryEntry {
    #[getter]
    fn id(&self) -> PyMigrationIdentity {
        self.id.clone()
    }

    #[getter]
    fn parents(&self) -> Vec<PyMigrationIdentity> {
        self.parents.clone()
    }

    #[getter]
    fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    #[getter]
    fn step_count(&self) -> usize {
        self.step_count
    }

    #[getter]
    fn safety(&self) -> &str {
        &self.safety
    }

    #[getter]
    fn reversible(&self) -> bool {
        self.reversible
    }
}

/// Generated-package-owned, source-free migration catalog.
#[pyclass(name = "MigrationCatalog", frozen)]
pub struct PyMigrationCatalog {
    inner: MigrationCatalog,
}

#[pymethods]
impl PyMigrationCatalog {
    fn fingerprint_json(&self) -> PyResult<String> {
        serde_json::to_string(self.inner.fingerprint()).map_err(py_catalog_error)
    }

    fn __len__(&self) -> usize {
        self.inner.entries().len()
    }

    fn is_empty(&self) -> bool {
        self.inner.entries().is_empty()
    }

    fn heads(&self) -> Vec<PyMigrationIdentity> {
        self.inner.heads().iter().map(migration_identity).collect()
    }

    fn entry(&self, index: usize) -> Option<PyMigrationHistoryEntry> {
        self.inner.entries().get(index).map(|entry| {
            let manifest = entry.manifest();
            PyMigrationHistoryEntry {
                id: migration_identity(manifest.id()),
                parents: manifest.parents().iter().map(migration_identity).collect(),
                manifest_digest: entry.manifest_digest().to_hex(),
                step_count: manifest.steps().len(),
                safety: safety_name(manifest.safety()).to_owned(),
                reversible: manifest.reversible(),
            }
        })
    }
}

/// Open canonical generated history bytes under their generated schema authority.
#[pyfunction]
fn open_migration_catalog(
    schema_authority: &Bound<'_, PyBytes>,
    history_bundle: &Bound<'_, PyBytes>,
) -> PyResult<PyMigrationCatalog> {
    let capabilities = migration_runtime_capability_vocabulary().map_err(py_catalog_diagnostic)?;
    let authority =
        decode_schema_authority(schema_authority.as_bytes(), &capabilities).map_err(|error| {
            py_catalog_code("generated_schema_authority_invalid", error.to_string())
        })?;
    let context = ManagedDeltaContext::new(
        authority.managed_scope().id().clone(),
        authority.semantic_profile().id().clone(),
        capabilities,
    );
    MigrationCatalog::open(history_bundle.as_bytes(), &context)
        .map(|inner| PyMigrationCatalog { inner })
        .map_err(py_catalog_diagnostic)
}

fn migration_identity(id: &type_bridge_contract::migration::MigrationId) -> PyMigrationIdentity {
    PyMigrationIdentity {
        app_label: id.app_label().as_str().to_owned(),
        name: id.name().as_str().to_owned(),
    }
}

fn safety_name(safety: type_bridge_schema::SafetyClass) -> &'static str {
    use type_bridge_schema::SafetyClass::*;
    match safety {
        FormalOnly => "formal_only",
        SchemaMetadata => "schema_metadata",
        Additive => "additive",
        Conditional => "conditional",
        BackfillRequired => "backfill_required",
        Destructive => "destructive",
        Opaque => "opaque",
        Unsupported => "unsupported",
    }
}

fn py_catalog_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> PyErr {
    py_catalog_code(error.code().as_str(), error.to_string())
}

fn py_catalog_error(error: serde_json::Error) -> PyErr {
    py_catalog_code("migration_catalog_encoding_failed", error.to_string())
}

fn py_catalog_code(code: &str, _detail: String) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(format!("migration catalog rejected [{code}]"))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(open_migration_catalog, m)?)?;
    m.add_class::<PyMigrationIdentity>()?;
    m.add_class::<PyMigrationHistoryEntry>()?;
    m.add_class::<PyMigrationCatalog>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use type_bridge_schema_migration::{MigrationHistoryGraph, VerifiedMigrationHistoryBundle};

    use super::PyMigrationCatalog;

    #[test]
    fn empty_catalog_has_bounded_immutable_python_shape() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog = PyMigrationCatalog {
            inner: type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle)
                .unwrap(),
        };

        assert_eq!(catalog.__len__(), 0);
        assert!(catalog.is_empty());
        assert!(catalog.heads().is_empty());
        assert!(catalog.entry(0).is_none());
        assert!(catalog.fingerprint_json().unwrap().contains("sha256"));
    }
}
