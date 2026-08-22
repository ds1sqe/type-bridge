//! Immutable generated migration-catalog inspection for Python.

use std::collections::BTreeSet;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use type_bridge_schema::{ManagedDeltaContext, decode_schema_authority};
use type_bridge_schema_migration::{
    MigrationApplyApproval, MigrationApplyPlanError, MigrationApplyTarget, MigrationSafetyPolicy,
    SafetyPolicyDecision, VerifiedMigrationApplyPlan, VerifiedMigrationRollbackPlan,
};
use type_bridge_schema_migration::{MigrationCatalog, migration_runtime_capability_vocabulary};

use crate::orm_runtime::{PyRustDatabase, provider_block_on};

/// One canonical compound migration identity.
#[pyclass(name = "MigrationIdentity", frozen)]
#[derive(Clone)]
pub struct PyMigrationIdentity {
    app_label: String,
    name: String,
}

#[pymethods]
impl PyMigrationIdentity {
    #[new]
    fn new(app_label: String, name: String) -> Self {
        Self { app_label, name }
    }

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
                inner: Arc::new(PyMigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: PyMigrationPreviewInner::Apply(plan),
                }),
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
                inner: Arc::new(PyMigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: PyMigrationPreviewInner::Rollback(plan),
                }),
            })
            .map_err(py_plan_error)
    }

    fn verify(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
    ) -> PyResult<PyMigrationVerificationReport> {
        let (database, runtime) = database.handles();
        provider_block_on(
            py,
            runtime.as_ref(),
            type_bridge_schema_migration_typedb::verify_catalog_state(database, &self.inner),
        )
        .map(verification_report)
        .map_err(py_catalog_diagnostic)
    }
}

enum PyMigrationPreviewInner {
    Apply(VerifiedMigrationApplyPlan),
    Rollback(VerifiedMigrationRollbackPlan),
}

struct PyMigrationPreviewState {
    catalog: MigrationCatalog,
    plan: PyMigrationPreviewInner,
}

/// Immutable provider-free forward or rollback preview.
#[pyclass(name = "MigrationPreview", frozen)]
pub struct PyMigrationPreview {
    inner: Arc<PyMigrationPreviewState>,
}

#[pymethods]
impl PyMigrationPreview {
    #[getter]
    fn direction(&self) -> &'static str {
        match self.inner.plan {
            PyMigrationPreviewInner::Apply(_) => "apply",
            PyMigrationPreviewInner::Rollback(_) => "rollback",
        }
    }

    #[getter]
    fn execution_authorized(&self) -> bool {
        match &self.inner.plan {
            PyMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            PyMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    fn migration_count(&self) -> usize {
        match &self.inner.plan {
            PyMigrationPreviewInner::Apply(plan) => plan.migrations().len(),
            PyMigrationPreviewInner::Rollback(plan) => plan.rollbacks().len(),
        }
    }

    fn migration(&self, index: usize) -> Option<PyMigrationPreviewEntry> {
        match &self.inner.plan {
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

    fn approval_builder(&self) -> PyMigrationApprovalBuilder {
        PyMigrationApprovalBuilder {
            preview: Arc::clone(&self.inner),
            selected: BTreeSet::new(),
        }
    }

    fn authorize(&self, approvals: &PyMigrationApprovalSet) -> PyResult<PyMigrationPlan> {
        if !Arc::ptr_eq(&self.inner, &approvals.preview) {
            return Err(py_catalog_code(
                "migration_approval_plan_mismatch",
                String::new(),
            ));
        }
        Ok(PyMigrationPlan {
            catalog: self.inner.catalog.clone(),
            plan: authorize_plan(&self.inner, &approvals.approvals)?,
        })
    }
}

#[pyclass(name = "MigrationApprovalBuilder")]
pub struct PyMigrationApprovalBuilder {
    preview: Arc<PyMigrationPreviewState>,
    selected: BTreeSet<usize>,
}

#[pymethods]
impl PyMigrationApprovalBuilder {
    fn approve(&mut self, index: usize) -> PyResult<()> {
        let safety = preview_safety(&self.preview.plan, index)
            .ok_or_else(|| py_catalog_code("migration_approval_index_invalid", String::new()))?;
        match MigrationSafetyPolicy::default_policy().decision(safety) {
            SafetyPolicyDecision::RequireApproval => {
                self.selected.insert(index);
                Ok(())
            }
            SafetyPolicyDecision::Allow => Err(py_catalog_code(
                "migration_approval_not_required",
                String::new(),
            )),
            SafetyPolicyDecision::Reject => Err(py_catalog_code(
                "migration_approval_policy_rejected",
                String::new(),
            )),
        }
    }

    fn finish(&self) -> PyResult<PyMigrationApprovalSet> {
        Ok(PyMigrationApprovalSet {
            preview: Arc::clone(&self.preview),
            approvals: exact_approvals(&self.preview.plan, &self.selected)?,
        })
    }
}

#[pyclass(name = "MigrationApprovalSet", frozen)]
pub struct PyMigrationApprovalSet {
    preview: Arc<PyMigrationPreviewState>,
    approvals: Vec<MigrationApplyApproval>,
}

#[pymethods]
impl PyMigrationApprovalSet {
    fn __len__(&self) -> usize {
        self.approvals.len()
    }
}

#[pyclass(name = "MigrationPlan", frozen)]
pub struct PyMigrationPlan {
    #[allow(dead_code)]
    catalog: MigrationCatalog,
    plan: PyMigrationPreviewInner,
}

#[pyclass(name = "MigrationExecutionReport", frozen)]
pub struct PyMigrationExecutionReport {
    direction: String,
    status: String,
    migration_id: Option<PyMigrationIdentity>,
    position_kind: Option<String>,
    position_ordinal: Option<usize>,
    diagnostic_category: Option<String>,
    diagnostic_code: Option<String>,
}

#[pymethods]
impl PyMigrationExecutionReport {
    #[getter]
    fn direction(&self) -> &str {
        &self.direction
    }
    #[getter]
    fn status(&self) -> &str {
        &self.status
    }
    #[getter]
    fn migration_id(&self) -> Option<PyMigrationIdentity> {
        self.migration_id.clone()
    }
    #[getter]
    fn position_kind(&self) -> Option<&str> {
        self.position_kind.as_deref()
    }
    #[getter]
    fn position_ordinal(&self) -> Option<usize> {
        self.position_ordinal
    }
    #[getter]
    fn diagnostic_category(&self) -> Option<&str> {
        self.diagnostic_category.as_deref()
    }
    #[getter]
    fn diagnostic_code(&self) -> Option<&str> {
        self.diagnostic_code.as_deref()
    }
}

#[pymethods]
impl PyMigrationPlan {
    #[getter]
    fn execution_authorized(&self) -> bool {
        match &self.plan {
            PyMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            PyMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    fn execute(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        holder: String,
    ) -> PyResult<PyMigrationExecutionReport> {
        let holder = type_bridge_schema_migration::LeaseHolderId::new(holder)
            .map_err(py_catalog_diagnostic)?;
        let (database, runtime) = database.handles();
        match &self.plan {
            PyMigrationPreviewInner::Apply(plan) => provider_block_on(
                py,
                runtime.as_ref(),
                type_bridge_schema_migration_typedb::execute_catalog_apply_plan(
                    database,
                    &self.catalog,
                    &holder,
                    plan,
                ),
            )
            .map(type_bridge_schema_migration::MigrationExecutionReport::from_apply)
            .map(python_execution_report)
            .map_err(py_catalog_diagnostic),
            PyMigrationPreviewInner::Rollback(plan) => provider_block_on(
                py,
                runtime.as_ref(),
                type_bridge_schema_migration_typedb::execute_catalog_rollback_plan(
                    database,
                    &self.catalog,
                    &holder,
                    plan,
                ),
            )
            .map(type_bridge_schema_migration::MigrationExecutionReport::from_rollback)
            .map(python_execution_report)
            .map_err(py_catalog_diagnostic),
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

#[pyclass(name = "MigrationVerificationFinding", frozen)]
#[derive(Clone)]
pub struct PyMigrationVerificationFinding {
    kind: String,
    pending: Vec<PyMigrationIdentity>,
    diagnostic_code: Option<String>,
}

#[pymethods]
impl PyMigrationVerificationFinding {
    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }
    #[getter]
    fn pending(&self) -> Vec<PyMigrationIdentity> {
        self.pending.clone()
    }
    #[getter]
    fn diagnostic_code(&self) -> Option<&str> {
        self.diagnostic_code.as_deref()
    }
}

#[pyclass(name = "MigrationVerificationReport", frozen)]
pub struct PyMigrationVerificationReport {
    clean: bool,
    findings: Vec<PyMigrationVerificationFinding>,
    applied_frontier: Vec<PyMigrationIdentity>,
}

#[pymethods]
impl PyMigrationVerificationReport {
    #[getter]
    fn clean(&self) -> bool {
        self.clean
    }
    #[getter]
    fn findings(&self) -> Vec<PyMigrationVerificationFinding> {
        self.findings.clone()
    }
    #[getter]
    fn applied_frontier(&self) -> Vec<PyMigrationIdentity> {
        self.applied_frontier.clone()
    }
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

fn preview_safety(
    plan: &PyMigrationPreviewInner,
    index: usize,
) -> Option<type_bridge_schema::SafetyClass> {
    match plan {
        PyMigrationPreviewInner::Apply(plan) => plan
            .migrations()
            .get(index)
            .map(|entry| entry.manifest().safety()),
        PyMigrationPreviewInner::Rollback(plan) => plan
            .rollbacks()
            .get(index)
            .map(|entry| entry.rollback_safety()),
    }
}

fn exact_approvals(
    plan: &PyMigrationPreviewInner,
    selected: &BTreeSet<usize>,
) -> PyResult<Vec<MigrationApplyApproval>> {
    selected
        .iter()
        .map(|index| match plan {
            PyMigrationPreviewInner::Apply(plan) => {
                MigrationApplyApproval::for_manifest(plan.migrations()[*index].manifest())
            }
            PyMigrationPreviewInner::Rollback(plan) => MigrationApplyApproval::for_rollback(
                plan.rollbacks()[*index].manifest(),
                plan.rollbacks()[*index].rollback_safety(),
            ),
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(py_catalog_diagnostic)
}

fn authorize_plan(
    state: &PyMigrationPreviewState,
    approvals: &[MigrationApplyApproval],
) -> PyResult<PyMigrationPreviewInner> {
    let policy = MigrationSafetyPolicy::default_policy();
    match &state.plan {
        PyMigrationPreviewInner::Apply(plan) => state
            .catalog
            .authorize_apply(
                &plan.applied_migrations().iter().cloned().collect(),
                &MigrationApplyTarget::Explicit(plan.target_frontier().iter().cloned().collect()),
                &policy,
                approvals,
            )
            .map(PyMigrationPreviewInner::Apply),
        PyMigrationPreviewInner::Rollback(plan) => state
            .catalog
            .authorize_rollback(
                &plan.applied_basis(),
                &plan
                    .rollbacks()
                    .iter()
                    .map(|entry| entry.manifest().id().clone())
                    .collect(),
                &policy,
                approvals,
            )
            .map(PyMigrationPreviewInner::Rollback),
    }
    .map_err(py_plan_error)
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

fn python_execution_report(
    report: type_bridge_schema_migration::MigrationExecutionReport,
) -> PyMigrationExecutionReport {
    use type_bridge_schema_migration::{
        MigrationExecutionDirection, MigrationExecutionReportPosition, MigrationExecutionStatus,
    };
    let direction = match report.direction() {
        MigrationExecutionDirection::Apply => "apply",
        MigrationExecutionDirection::Rollback => "rollback",
    };
    let status = match report.status() {
        MigrationExecutionStatus::Applied => "applied",
        MigrationExecutionStatus::RolledBack => "rolled_back",
        MigrationExecutionStatus::RetrySafe => "retry_safe",
        MigrationExecutionStatus::RequiresExplicitRecovery => "requires_explicit_recovery",
    };
    let (position_kind, position_ordinal) = match report.position() {
        None => (None, None),
        Some(MigrationExecutionReportPosition::TransactionGroup(value)) => {
            (Some("transaction_group".to_owned()), Some(value))
        }
        Some(MigrationExecutionReportPosition::BackfillStep(value)) => {
            (Some("backfill_step".to_owned()), Some(value))
        }
        Some(MigrationExecutionReportPosition::ManifestCheckpoint) => {
            (Some("manifest_checkpoint".to_owned()), None)
        }
        Some(MigrationExecutionReportPosition::RollbackStep(value)) => {
            (Some("rollback_step".to_owned()), Some(value))
        }
    };
    PyMigrationExecutionReport {
        direction: direction.to_owned(),
        status: status.to_owned(),
        migration_id: report.migration_id().map(migration_identity),
        position_kind,
        position_ordinal,
        diagnostic_category: report
            .diagnostic()
            .map(|value| value.category().as_str().to_owned()),
        diagnostic_code: report
            .diagnostic()
            .map(|value| value.code().as_str().to_owned()),
    }
}

fn verification_report(
    report: type_bridge_schema_migration::MigrationVerifyReport,
) -> PyMigrationVerificationReport {
    let findings = report
        .findings()
        .iter()
        .map(|finding| {
            use type_bridge_schema_migration::MigrationDriftFinding;
            let (kind, pending, diagnostic_code) = match finding {
                MigrationDriftFinding::AppliedLedger { diagnostic } => (
                    "applied_ledger",
                    Vec::new(),
                    Some(diagnostic.code().as_str().to_owned()),
                ),
                MigrationDriftFinding::LiveSemantics { .. } => ("live_semantics", Vec::new(), None),
                MigrationDriftFinding::DesiredDivergence { .. } => {
                    ("desired_divergence", Vec::new(), None)
                }
                MigrationDriftFinding::PendingMigrations { pending } => (
                    "pending_migrations",
                    pending.iter().map(migration_identity).collect(),
                    None,
                ),
                MigrationDriftFinding::Capabilities { diagnostic } => (
                    "capabilities",
                    Vec::new(),
                    Some(diagnostic.code().as_str().to_owned()),
                ),
            };
            PyMigrationVerificationFinding {
                kind: kind.to_owned(),
                pending,
                diagnostic_code,
            }
        })
        .collect();
    PyMigrationVerificationReport {
        clean: report.is_clean(),
        findings,
        applied_frontier: report
            .applied_frontier()
            .iter()
            .map(migration_identity)
            .collect(),
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(open_migration_catalog, m)?)?;
    m.add_class::<PyMigrationIdentity>()?;
    m.add_class::<PyMigrationHistoryEntry>()?;
    m.add_class::<PyMigrationCatalog>()?;
    m.add_class::<PyMigrationPreview>()?;
    m.add_class::<PyMigrationPreviewEntry>()?;
    m.add_class::<PyMigrationApprovalBuilder>()?;
    m.add_class::<PyMigrationApprovalSet>()?;
    m.add_class::<PyMigrationPlan>()?;
    m.add_class::<PyMigrationExecutionReport>()?;
    m.add_class::<PyMigrationVerificationFinding>()?;
    m.add_class::<PyMigrationVerificationReport>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_schema::ManagedDeltaContext;
    use type_bridge_schema_migration::{
        MigrationHistoryGraph, VerifiedMigrationHistoryBundle,
        encode_verified_migration_history_bundle, migration_runtime_capability_vocabulary,
    };

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

    #[test]
    fn python_preview_owns_exact_approval_and_authorization_chain() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let bytes = encode_verified_migration_history_bundle(&bundle).unwrap();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("python-migration-approval").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            migration_runtime_capability_vocabulary().unwrap(),
        );
        let catalog = PyMigrationCatalog {
            inner: type_bridge_schema_migration::MigrationCatalog::open(&bytes, &context).unwrap(),
        };
        let preview = catalog.preview_apply(Vec::new(), None).unwrap();
        let approvals = preview.approval_builder().finish().unwrap();
        assert_eq!(approvals.__len__(), 0);
        assert!(
            preview
                .authorize(&approvals)
                .unwrap()
                .execution_authorized()
        );
        let foreign = catalog.preview_apply(Vec::new(), None).unwrap();
        assert!(foreign.authorize(&approvals).is_err());
    }
}
