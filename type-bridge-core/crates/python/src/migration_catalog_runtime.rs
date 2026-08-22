//! Immutable generated migration-catalog inspection for Python.

use std::collections::BTreeSet;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use type_bridge_schema::{ManagedDeltaContext, decode_schema_authority};
use type_bridge_schema_migration::{
    MigrationApplyPlanError, MigrationApplyTarget, VerifiedMigrationApplyPlan,
    VerifiedMigrationRollbackPlan,
};
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

    #[pyo3(signature = (applied, targets = None))]
    fn preview_apply(
        &self,
        applied: Vec<PyMigrationIdentity>,
        targets: Option<Vec<PyMigrationIdentity>>,
    ) -> PyResult<PyMigrationPreview> {
        let applied = migration_set(applied)?;
        let target = match targets {
            None => MigrationApplyTarget::DefaultHead,
            Some(targets) => MigrationApplyTarget::Explicit(migration_set(targets)?),
        };
        self.inner
            .preview_apply(&applied, &target)
            .map(|plan| PyMigrationPreview {
                inner: PyMigrationPreviewInner::Apply(plan),
            })
            .map_err(py_plan_error)
    }

    fn preview_rollback(
        &self,
        applied: Vec<PyMigrationIdentity>,
        removals: Vec<PyMigrationIdentity>,
    ) -> PyResult<PyMigrationPreview> {
        self.inner
            .preview_rollback(&migration_set(applied)?, &migration_set(removals)?)
            .map(|plan| PyMigrationPreview {
                inner: PyMigrationPreviewInner::Rollback(plan),
            })
            .map_err(py_plan_error)
    }
}

enum PyMigrationPreviewInner {
    Apply(VerifiedMigrationApplyPlan),
    Rollback(VerifiedMigrationRollbackPlan),
}

/// Immutable provider-free forward or rollback preview.
#[pyclass(name = "MigrationPreview", frozen)]
pub struct PyMigrationPreview {
    inner: PyMigrationPreviewInner,
}

#[pymethods]
impl PyMigrationPreview {
    #[getter]
    fn direction(&self) -> &'static str {
        match self.inner {
            PyMigrationPreviewInner::Apply(_) => "apply",
            PyMigrationPreviewInner::Rollback(_) => "rollback",
        }
    }

    #[getter]
    fn execution_authorized(&self) -> bool {
        match &self.inner {
            PyMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            PyMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    fn migration_count(&self) -> usize {
        match &self.inner {
            PyMigrationPreviewInner::Apply(plan) => plan.migrations().len(),
            PyMigrationPreviewInner::Rollback(plan) => plan.rollbacks().len(),
        }
    }

    fn migration(&self, index: usize) -> Option<PyMigrationPreviewEntry> {
        match &self.inner {
            PyMigrationPreviewInner::Apply(plan) => {
                plan.migrations()
                    .get(index)
                    .map(|entry| PyMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.manifest().safety()).to_owned(),
                        step_count: entry.steps().len(),
                        transaction_group_count: entry.transaction_groups().len(),
                        backfill_count: entry.backfill_step_indices().len(),
                        reversible: entry.manifest().reversible(),
                    })
            }
            PyMigrationPreviewInner::Rollback(plan) => {
                plan.rollbacks()
                    .get(index)
                    .map(|entry| PyMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.rollback_safety()).to_owned(),
                        step_count: entry.operations().len(),
                        transaction_group_count: entry.steps().len(),
                        backfill_count: entry.backfills().len(),
                        reversible: true,
                    })
            }
        }
    }
}

/// Bounded snapshot of one migration inside a preview.
#[pyclass(name = "MigrationPreviewEntry", frozen)]
pub struct PyMigrationPreviewEntry {
    id: PyMigrationIdentity,
    safety: String,
    step_count: usize,
    transaction_group_count: usize,
    backfill_count: usize,
    reversible: bool,
}

#[pymethods]
impl PyMigrationPreviewEntry {
    #[getter]
    fn id(&self) -> PyMigrationIdentity {
        self.id.clone()
    }
    #[getter]
    fn safety(&self) -> &str {
        &self.safety
    }
    #[getter]
    fn step_count(&self) -> usize {
        self.step_count
    }
    #[getter]
    fn transaction_group_count(&self) -> usize {
        self.transaction_group_count
    }
    #[getter]
    fn backfill_count(&self) -> usize {
        self.backfill_count
    }
    #[getter]
    fn reversible(&self) -> bool {
        self.reversible
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

fn migration_set(
    identities: Vec<PyMigrationIdentity>,
) -> PyResult<BTreeSet<type_bridge_contract::migration::MigrationId>> {
    identities
        .into_iter()
        .map(|id| {
            type_bridge_contract::migration::MigrationId::new(id.app_label, id.name)
                .map_err(py_catalog_diagnostic)
        })
        .collect()
}

fn py_plan_error(error: MigrationApplyPlanError) -> PyErr {
    match error {
        MigrationApplyPlanError::Contract(error) => py_catalog_diagnostic(error),
        MigrationApplyPlanError::Schema(_) => {
            py_catalog_code("migration_preview_schema_replay_failed", String::new())
        }
        MigrationApplyPlanError::Lowering(error) => py_catalog_code(error.code(), String::new()),
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
    m.add_class::<PyMigrationPreview>()?;
    m.add_class::<PyMigrationPreviewEntry>()?;
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
