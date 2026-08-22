//! Immutable generated migration-catalog inspection for Node.

use std::collections::BTreeSet;
use std::sync::Arc;

use napi::bindgen_prelude::{Buffer, Error, Result};
use napi_derive::napi;
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, decode_schema_authority};
use type_bridge_schema_migration::{
    MigrationApplyApproval, MigrationApplyPlanError, MigrationApplyTarget, MigrationCatalog,
    MigrationSafetyPolicy, SafetyPolicyDecision, VerifiedMigrationApplyPlan,
    VerifiedMigrationRollbackPlan, migration_runtime_capability_vocabulary,
};

use crate::NodeRustDatabase;

#[napi(object)]
pub struct NodeMigrationIdentity {
    pub app_label: String,
    pub name: String,
}

#[napi(object)]
pub struct NodeMigrationHistoryEntry {
    pub id: NodeMigrationIdentity,
    pub parents: Vec<NodeMigrationIdentity>,
    pub manifest_digest: String,
    pub step_count: u32,
    pub safety: String,
    pub reversible: bool,
}

#[napi(object)]
pub struct NodeMigrationVerificationFinding {
    pub kind: String,
    pub pending: Vec<NodeMigrationIdentity>,
    pub diagnostic_code: Option<String>,
}

#[napi(object)]
pub struct NodeMigrationVerificationReport {
    pub clean: bool,
    pub findings: Vec<NodeMigrationVerificationFinding>,
    pub applied_frontier: Vec<NodeMigrationIdentity>,
}

#[napi(object)]
pub struct NodeMigrationExecutionReport {
    pub direction: String,
    pub status: String,
    pub migration_id: Option<NodeMigrationIdentity>,
    pub position_kind: Option<String>,
    pub position_ordinal: Option<u32>,
    pub diagnostic_category: Option<String>,
    pub diagnostic_code: Option<String>,
}

/// Generated-package-owned, source-free migration catalog.
#[napi]
pub struct NodeMigrationCatalog {
    inner: MigrationCatalog,
}

#[napi]
impl NodeMigrationCatalog {
    #[napi]
    pub fn fingerprint_json(&self) -> Result<String> {
        serde_json::to_string(self.inner.fingerprint())
            .map_err(|_| catalog_error("migration_catalog_encoding_failed"))
    }

    #[napi(getter)]
    pub fn length(&self) -> u32 {
        u32::try_from(self.inner.entries().len()).expect("bundle entry limit fits u32")
    }

    #[napi]
    pub fn is_empty(&self) -> bool {
        self.inner.entries().is_empty()
    }

    #[napi]
    pub fn heads(&self) -> Vec<NodeMigrationIdentity> {
        self.inner.heads().iter().map(migration_identity).collect()
    }

    #[napi]
    pub fn entry(&self, index: u32) -> Option<NodeMigrationHistoryEntry> {
        self.inner.entries().get(index as usize).map(|entry| {
            let manifest = entry.manifest();
            NodeMigrationHistoryEntry {
                id: migration_identity(manifest.id()),
                parents: manifest.parents().iter().map(migration_identity).collect(),
                manifest_digest: entry.manifest_digest().to_hex(),
                step_count: u32::try_from(manifest.steps().len())
                    .expect("manifest step limit fits u32"),
                safety: safety_name(manifest.safety()).to_owned(),
                reversible: manifest.reversible(),
            }
        })
    }

    #[napi]
    pub fn preview_apply(
        &self,
        applied: Vec<NodeMigrationIdentity>,
        targets: Option<Vec<NodeMigrationIdentity>>,
    ) -> Result<NodeMigrationPreview> {
        let target = match targets {
            None => MigrationApplyTarget::DefaultHead,
            Some(targets) => MigrationApplyTarget::Explicit(migration_set(targets)?),
        };
        self.inner
            .preview_apply(&migration_set(applied)?, &target)
            .map(|plan| NodeMigrationPreview {
                inner: Arc::new(NodeMigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: NodeMigrationPreviewInner::Apply(plan),
                }),
            })
            .map_err(plan_error)
    }

    #[napi]
    pub fn preview_rollback(
        &self,
        applied: Vec<NodeMigrationIdentity>,
        removals: Vec<NodeMigrationIdentity>,
    ) -> Result<NodeMigrationPreview> {
        self.inner
            .preview_rollback(&migration_set(applied)?, &migration_set(removals)?)
            .map(|plan| NodeMigrationPreview {
                inner: Arc::new(NodeMigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: NodeMigrationPreviewInner::Rollback(plan),
                }),
            })
            .map_err(plan_error)
    }

    #[napi]
    pub fn verify(&self, database: &NodeRustDatabase) -> Result<NodeMigrationVerificationReport> {
        let (database, runtime) = database.handles();
        runtime
            .block_on(type_bridge_schema_migration_typedb::verify_catalog_state(
                database,
                &self.inner,
            ))
            .map(verification_report)
            .map_err(|error| catalog_error(error.code().as_str()))
    }
}

enum NodeMigrationPreviewInner {
    Apply(VerifiedMigrationApplyPlan),
    Rollback(VerifiedMigrationRollbackPlan),
}

struct NodeMigrationPreviewState {
    catalog: MigrationCatalog,
    plan: NodeMigrationPreviewInner,
}

/// Immutable provider-free forward or rollback preview.
#[napi]
pub struct NodeMigrationPreview {
    inner: Arc<NodeMigrationPreviewState>,
}

#[napi]
impl NodeMigrationPreview {
    #[napi(getter)]
    pub fn direction(&self) -> &'static str {
        match self.inner.plan {
            NodeMigrationPreviewInner::Apply(_) => "apply",
            NodeMigrationPreviewInner::Rollback(_) => "rollback",
        }
    }

    #[napi(getter)]
    pub fn execution_authorized(&self) -> bool {
        match &self.inner.plan {
            NodeMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            NodeMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    #[napi]
    pub fn migration_count(&self) -> u32 {
        bounded_u32(match &self.inner.plan {
            NodeMigrationPreviewInner::Apply(plan) => plan.migrations().len(),
            NodeMigrationPreviewInner::Rollback(plan) => plan.rollbacks().len(),
        })
    }

    #[napi]
    pub fn migration(&self, index: u32) -> Option<NodeMigrationPreviewEntry> {
        match &self.inner.plan {
            NodeMigrationPreviewInner::Apply(plan) => {
                plan.migrations()
                    .get(index as usize)
                    .map(|entry| NodeMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.manifest().safety()).to_owned(),
                        step_count: bounded_u32(entry.steps().len()),
                        transaction_group_count: bounded_u32(entry.transaction_groups().len()),
                        backfill_count: bounded_u32(entry.backfill_step_indices().len()),
                        reversible: entry.manifest().reversible(),
                    })
            }
            NodeMigrationPreviewInner::Rollback(plan) => {
                plan.rollbacks()
                    .get(index as usize)
                    .map(|entry| NodeMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.rollback_safety()).to_owned(),
                        step_count: bounded_u32(entry.operations().len()),
                        transaction_group_count: bounded_u32(entry.steps().len()),
                        backfill_count: bounded_u32(entry.backfills().len()),
                        reversible: true,
                    })
            }
        }
    }

    #[napi]
    pub fn approval_builder(&self) -> NodeMigrationApprovalBuilder {
        NodeMigrationApprovalBuilder {
            preview: Arc::clone(&self.inner),
            selected: BTreeSet::new(),
        }
    }

    #[napi]
    pub fn authorize(&self, approvals: &NodeMigrationApprovalSet) -> Result<NodeMigrationPlan> {
        if !Arc::ptr_eq(&self.inner, &approvals.preview) {
            return Err(catalog_error("migration_approval_plan_mismatch"));
        }
        let plan = authorize_plan(&self.inner, &approvals.approvals)?;
        Ok(NodeMigrationPlan {
            catalog: self.inner.catalog.clone(),
            plan,
        })
    }
}

#[napi]
pub struct NodeMigrationApprovalBuilder {
    preview: Arc<NodeMigrationPreviewState>,
    selected: BTreeSet<usize>,
}

#[napi]
impl NodeMigrationApprovalBuilder {
    #[napi]
    pub fn approve(&mut self, index: u32) -> Result<()> {
        let index = index as usize;
        let safety = preview_safety(&self.preview.plan, index)
            .ok_or_else(|| catalog_error("migration_approval_index_invalid"))?;
        match MigrationSafetyPolicy::default_policy().decision(safety) {
            SafetyPolicyDecision::RequireApproval => {
                self.selected.insert(index);
                Ok(())
            }
            SafetyPolicyDecision::Allow => Err(catalog_error("migration_approval_not_required")),
            SafetyPolicyDecision::Reject => {
                Err(catalog_error("migration_approval_policy_rejected"))
            }
        }
    }

    #[napi]
    pub fn finish(&self) -> Result<NodeMigrationApprovalSet> {
        Ok(NodeMigrationApprovalSet {
            preview: Arc::clone(&self.preview),
            approvals: exact_approvals(&self.preview.plan, &self.selected)?,
        })
    }
}

#[napi]
pub struct NodeMigrationApprovalSet {
    preview: Arc<NodeMigrationPreviewState>,
    approvals: Vec<MigrationApplyApproval>,
}

#[napi]
impl NodeMigrationApprovalSet {
    #[napi(getter)]
    pub fn length(&self) -> u32 {
        bounded_u32(self.approvals.len())
    }
}

#[napi]
pub struct NodeMigrationPlan {
    #[allow(dead_code)]
    catalog: MigrationCatalog,
    plan: NodeMigrationPreviewInner,
}

#[napi]
impl NodeMigrationPlan {
    #[napi(getter)]
    pub fn execution_authorized(&self) -> bool {
        match &self.plan {
            NodeMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            NodeMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    #[napi]
    pub fn execute(
        &self,
        database: &NodeRustDatabase,
        holder: String,
    ) -> Result<NodeMigrationExecutionReport> {
        let holder = type_bridge_schema_migration::LeaseHolderId::new(holder)
            .map_err(|error| catalog_error(error.code().as_str()))?;
        let (database, runtime) = database.handles();
        match &self.plan {
            NodeMigrationPreviewInner::Apply(plan) => runtime
                .block_on(
                    type_bridge_schema_migration_typedb::execute_catalog_apply_plan(
                        database,
                        &self.catalog,
                        &holder,
                        plan,
                    ),
                )
                .map(type_bridge_schema_migration::MigrationExecutionReport::from_apply)
                .map(node_execution_report)
                .map_err(|error| catalog_error(error.code().as_str())),
            NodeMigrationPreviewInner::Rollback(plan) => runtime
                .block_on(
                    type_bridge_schema_migration_typedb::execute_catalog_rollback_plan(
                        database,
                        &self.catalog,
                        &holder,
                        plan,
                    ),
                )
                .map(type_bridge_schema_migration::MigrationExecutionReport::from_rollback)
                .map(node_execution_report)
                .map_err(|error| catalog_error(error.code().as_str())),
        }
    }
}

#[napi(object)]
pub struct NodeMigrationPreviewEntry {
    pub id: NodeMigrationIdentity,
    pub safety: String,
    pub step_count: u32,
    pub transaction_group_count: u32,
    pub backfill_count: u32,
    pub reversible: bool,
}

/// Open canonical generated history bytes under their generated schema authority.
#[napi]
pub fn open_migration_catalog(
    schema_authority: Buffer,
    history_bundle: Buffer,
) -> Result<NodeMigrationCatalog> {
    let capabilities = migration_runtime_capability_vocabulary()
        .map_err(|error| catalog_error(error.code().as_str()))?;
    let authority = decode_schema_authority(schema_authority.as_ref(), &capabilities)
        .map_err(|_| catalog_error("generated_schema_authority_invalid"))?;
    let context = ManagedDeltaContext::new(
        authority.managed_scope().id().clone(),
        authority.semantic_profile().id().clone(),
        capabilities,
    );
    MigrationCatalog::open(history_bundle.as_ref(), &context)
        .map(|inner| NodeMigrationCatalog { inner })
        .map_err(|error| catalog_error(error.code().as_str()))
}

fn migration_identity(id: &type_bridge_contract::migration::MigrationId) -> NodeMigrationIdentity {
    NodeMigrationIdentity {
        app_label: id.app_label().as_str().to_owned(),
        name: id.name().as_str().to_owned(),
    }
}

fn migration_set(
    identities: Vec<NodeMigrationIdentity>,
) -> Result<BTreeSet<type_bridge_contract::migration::MigrationId>> {
    identities
        .into_iter()
        .map(|id| {
            type_bridge_contract::migration::MigrationId::new(id.app_label, id.name)
                .map_err(|error| catalog_error(error.code().as_str()))
        })
        .collect()
}

fn plan_error(error: MigrationApplyPlanError) -> Error {
    match error {
        MigrationApplyPlanError::Contract(error) => catalog_error(error.code().as_str()),
        MigrationApplyPlanError::Schema(_) => {
            catalog_error("migration_preview_schema_replay_failed")
        }
        MigrationApplyPlanError::Lowering(error) => catalog_error(error.code()),
    }
}

fn preview_safety(plan: &NodeMigrationPreviewInner, index: usize) -> Option<SafetyClass> {
    match plan {
        NodeMigrationPreviewInner::Apply(plan) => plan
            .migrations()
            .get(index)
            .map(|entry| entry.manifest().safety()),
        NodeMigrationPreviewInner::Rollback(plan) => plan
            .rollbacks()
            .get(index)
            .map(|entry| entry.rollback_safety()),
    }
}

fn exact_approvals(
    plan: &NodeMigrationPreviewInner,
    selected: &BTreeSet<usize>,
) -> Result<Vec<MigrationApplyApproval>> {
    selected
        .iter()
        .map(|index| match plan {
            NodeMigrationPreviewInner::Apply(plan) => {
                MigrationApplyApproval::for_manifest(plan.migrations()[*index].manifest())
            }
            NodeMigrationPreviewInner::Rollback(plan) => MigrationApplyApproval::for_rollback(
                plan.rollbacks()[*index].manifest(),
                plan.rollbacks()[*index].rollback_safety(),
            ),
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| catalog_error(error.code().as_str()))
}

fn authorize_plan(
    state: &NodeMigrationPreviewState,
    approvals: &[MigrationApplyApproval],
) -> Result<NodeMigrationPreviewInner> {
    let policy = MigrationSafetyPolicy::default_policy();
    match &state.plan {
        NodeMigrationPreviewInner::Apply(plan) => state
            .catalog
            .authorize_apply(
                &plan.applied_migrations().iter().cloned().collect(),
                &MigrationApplyTarget::Explicit(plan.target_frontier().iter().cloned().collect()),
                &policy,
                approvals,
            )
            .map(NodeMigrationPreviewInner::Apply),
        NodeMigrationPreviewInner::Rollback(plan) => state
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
            .map(NodeMigrationPreviewInner::Rollback),
    }
    .map_err(plan_error)
}

fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).expect("canonical migration limit fits u32")
}

fn safety_name(safety: SafetyClass) -> &'static str {
    match safety {
        SafetyClass::FormalOnly => "formal_only",
        SafetyClass::SchemaMetadata => "schema_metadata",
        SafetyClass::Additive => "additive",
        SafetyClass::Conditional => "conditional",
        SafetyClass::BackfillRequired => "backfill_required",
        SafetyClass::Destructive => "destructive",
        SafetyClass::Opaque => "opaque",
        SafetyClass::Unsupported => "unsupported",
    }
}

fn catalog_error(code: &str) -> Error {
    Error::from_reason(format!("migration catalog rejected [{code}]"))
}

fn node_execution_report(
    report: type_bridge_schema_migration::MigrationExecutionReport,
) -> NodeMigrationExecutionReport {
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
        Some(MigrationExecutionReportPosition::TransactionGroup(value)) => (
            Some("transaction_group".to_owned()),
            Some(bounded_u32(value)),
        ),
        Some(MigrationExecutionReportPosition::BackfillStep(value)) => {
            (Some("backfill_step".to_owned()), Some(bounded_u32(value)))
        }
        Some(MigrationExecutionReportPosition::ManifestCheckpoint) => {
            (Some("manifest_checkpoint".to_owned()), None)
        }
        Some(MigrationExecutionReportPosition::RollbackStep(value)) => {
            (Some("rollback_step".to_owned()), Some(bounded_u32(value)))
        }
    };
    NodeMigrationExecutionReport {
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
) -> NodeMigrationVerificationReport {
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
            NodeMigrationVerificationFinding {
                kind: kind.to_owned(),
                pending,
                diagnostic_code,
            }
        })
        .collect();
    NodeMigrationVerificationReport {
        clean: report.is_clean(),
        findings,
        applied_frontier: report
            .applied_frontier()
            .iter()
            .map(migration_identity)
            .collect(),
    }
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

    use super::NodeMigrationCatalog;

    #[test]
    fn empty_catalog_has_bounded_immutable_node_shape() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog = NodeMigrationCatalog {
            inner: type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle)
                .unwrap(),
        };

        assert_eq!(catalog.length(), 0);
        assert!(catalog.is_empty());
        assert!(catalog.heads().is_empty());
        assert!(catalog.entry(0).is_none());
        assert!(catalog.fingerprint_json().unwrap().contains("sha256"));
    }

    #[test]
    fn node_preview_owns_exact_approval_and_authorization_chain() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let bytes = encode_verified_migration_history_bundle(&bundle).unwrap();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("node-migration-approval").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            migration_runtime_capability_vocabulary().unwrap(),
        );
        let catalog = NodeMigrationCatalog {
            inner: type_bridge_schema_migration::MigrationCatalog::open(&bytes, &context).unwrap(),
        };
        let preview = catalog.preview_apply(Vec::new(), None).unwrap();
        let approvals = preview.approval_builder().finish().unwrap();
        assert_eq!(approvals.length(), 0);
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
