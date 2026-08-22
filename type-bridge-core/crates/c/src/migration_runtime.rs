//! Rust-owned migration runtime resources prepared for the additive C ABI.

// The complete owner graph is intentionally assembled before any symbol is
// exported: ABI 1.4's exact export ledger must remain frozen until the whole
// ABI 1.5 migration surface can be activated atomically.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration::MigrationId;
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, VerifiedSchemaAuthority};
use type_bridge_schema_migration::{
    LeaseHolderId, MigrationApplyApproval, MigrationApplyPlanError, MigrationApplyTarget,
    MigrationCatalog, MigrationExecutionReport, MigrationSafetyPolicy, SafetyPolicyDecision,
    VerifiedMigrationApplyPlan, VerifiedMigrationRollbackPlan,
    migration_runtime_capability_vocabulary, require_authorized_apply_plan,
    require_authorized_rollback_plan,
};

use crate::diagnostic::stable;
use crate::runtime::TypeBridgeDatabase;

/// Bound administration owner retaining the exact managed database and scope.
#[derive(Clone)]
pub(crate) struct DatabaseAdministrationState {
    administrator: type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator,
    runtime: tokio::runtime::Handle,
}

impl DatabaseAdministrationState {
    pub(crate) fn open(database: &TypeBridgeDatabase) -> Result<Arc<Self>, Diagnostic> {
        let administrator = type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator::from_managed_database(
            database.orm_database_arc(),
            database.package_state()._authority.managed_scope().id().clone(),
        )?;
        Ok(Arc::new(Self {
            administrator,
            runtime: database.runtime_handle(),
        }))
    }

    #[cfg(test)]
    fn from_administrator(
        administrator: type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator,
    ) -> Arc<Self> {
        Arc::new(Self {
            administrator,
            runtime: tokio::runtime::Handle::current(),
        })
    }

    pub(crate) async fn database_exists(&self) -> Result<bool, Diagnostic> {
        self.administrator.database_exists().await
    }

    pub(crate) fn database_exists_blocking(&self) -> Result<bool, Diagnostic> {
        self.runtime.block_on(self.database_exists())
    }

    pub(crate) fn database_exists_controlled_blocking(
        &self,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<bool, Diagnostic> {
        self.runtime
            .block_on(self.administrator.database_exists_controlled(control))
    }

    pub(crate) async fn create_database_outcome(
        &self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome, Diagnostic>
    {
        self.administrator.create_database_outcome().await
    }

    pub(crate) fn create_database_outcome_blocking(
        &self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome, Diagnostic>
    {
        self.runtime.block_on(self.create_database_outcome())
    }

    pub(crate) fn create_database_outcome_controlled_blocking(
        &self,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome, Diagnostic>
    {
        self.runtime.block_on(
            self.administrator
                .create_database_outcome_controlled(control),
        )
    }

    pub(crate) async fn inspect(
        &self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairState, Diagnostic> {
        self.administrator.inspect().await
    }

    pub(crate) fn inspect_blocking(
        &self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairState, Diagnostic> {
        self.runtime.block_on(self.inspect())
    }

    pub(crate) fn inspect_controlled_blocking(
        &self,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairState, Diagnostic> {
        self.runtime
            .block_on(self.administrator.inspect_controlled(control))
    }

    pub(crate) async fn plan_delete(
        self: &Arc<Self>,
    ) -> Result<DatabaseDeletionPlanState, Diagnostic> {
        self.administrator
            .plan_delete()
            .await
            .map(|plan| DatabaseDeletionPlanState {
                administration: Arc::clone(self),
                plan: Some(plan),
            })
    }

    pub(crate) fn plan_delete_blocking(
        self: &Arc<Self>,
    ) -> Result<DatabaseDeletionPlanState, Diagnostic> {
        self.runtime.block_on(self.plan_delete())
    }

    pub(crate) fn plan_delete_controlled_blocking(
        self: &Arc<Self>,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<DatabaseDeletionPlanState, Diagnostic> {
        self.runtime.block_on(async {
            self.administrator
                .plan_delete_controlled(control)
                .await
                .map(|plan| DatabaseDeletionPlanState {
                    administration: Arc::clone(self),
                    plan: Some(plan),
                })
        })
    }
}

/// C-owned, single-use pair-aware deletion plan retaining its administration owner.
pub(crate) struct DatabaseDeletionPlanState {
    administration: Arc<DatabaseAdministrationState>,
    plan: Option<type_bridge_schema_migration_typedb::ManagedDatabasePairDeletionPlan>,
}

impl DatabaseDeletionPlanState {
    pub(crate) fn inspected_state(
        &self,
    ) -> Option<type_bridge_schema_migration_typedb::ManagedDatabasePairState> {
        self.plan.as_ref().map(|plan| plan.inspected_state())
    }

    pub(crate) async fn execute(
        &mut self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome, Diagnostic>
    {
        let plan = self.plan.take().ok_or_else(|| {
            stable(
                DiagnosticCategory::InvalidContract,
                "c_database_deletion_plan_closed",
                "The database deletion plan is already closed or consumed",
            )
        })?;
        plan.execute().await
    }

    pub(crate) fn execute_blocking(
        &mut self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome, Diagnostic>
    {
        let runtime = self.administration.runtime.clone();
        runtime.block_on(self.execute())
    }

    pub(crate) fn execute_controlled_blocking(
        &mut self,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome, Diagnostic>
    {
        let plan = self.plan.take().ok_or_else(|| {
            stable(
                DiagnosticCategory::InvalidContract,
                "c_database_deletion_plan_closed",
                "The database deletion plan is already closed or consumed",
            )
        })?;
        let runtime = self.administration.runtime.clone();
        runtime.block_on(plan.execute_controlled(control))
    }

    pub(crate) fn close(&mut self) {
        self.plan = None;
    }

    pub(crate) fn retains_administration(&self, owner: &Arc<DatabaseAdministrationState>) -> bool {
        Arc::ptr_eq(&self.administration, owner)
    }
}

/// Immutable catalog state shared by C catalog and entry handles.
#[derive(Debug)]
pub(crate) struct MigrationCatalogState {
    catalog: MigrationCatalog,
    fingerprint_json: Vec<u8>,
}

impl MigrationCatalogState {
    #[cfg(test)]
    pub(crate) fn empty_for_abi_lifecycle_test() -> Arc<Self> {
        let graph =
            type_bridge_schema_migration::MigrationHistoryGraph::from_verified(std::iter::empty())
                .expect("empty migration graph is valid");
        let bundle =
            type_bridge_schema_migration::VerifiedMigrationHistoryBundle::from_graph(&graph)
                .expect("empty migration bundle is valid");
        let catalog = type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle)
            .expect("empty migration catalog is valid");
        let fingerprint_json =
            serde_json::to_vec(catalog.fingerprint()).expect("catalog fingerprint encodes");
        Arc::new(Self {
            catalog,
            fingerprint_json,
        })
    }

    /// Open exact generated bundle bytes under their verified package authority.
    pub(crate) fn open(
        authority: &VerifiedSchemaAuthority,
        history_bundle: &[u8],
    ) -> Result<Arc<Self>, Diagnostic> {
        let capabilities = migration_runtime_capability_vocabulary()?;
        let context = ManagedDeltaContext::new(
            authority.managed_scope().id().clone(),
            authority.semantic_profile().id().clone(),
            capabilities,
        );
        let catalog = MigrationCatalog::open(history_bundle, &context)?;
        let fingerprint_json = serde_json::to_vec(catalog.fingerprint()).map_err(|_| {
            stable(
                DiagnosticCategory::Integrity,
                "c_migration_catalog_fingerprint_encoding_failed",
                "The verified migration catalog fingerprint could not be encoded",
            )
        })?;
        Ok(Arc::new(Self {
            catalog,
            fingerprint_json,
        }))
    }

    /// Return the canonical fingerprint bytes retained by this owner.
    pub(crate) fn fingerprint_json(&self) -> &[u8] {
        &self.fingerprint_json
    }

    /// Return the bounded number of verified entries.
    pub(crate) fn len(&self) -> usize {
        self.catalog.entries().len()
    }

    /// Create an independently owned snapshot for one topological ordinal.
    pub(crate) fn entry(self: &Arc<Self>, index: usize) -> Option<MigrationHistoryEntryState> {
        self.catalog
            .entries()
            .get(index)
            .map(|_| MigrationHistoryEntryState {
                catalog: Arc::clone(self),
                index,
            })
    }

    /// Create independently owned snapshots for canonical graph heads.
    pub(crate) fn heads(&self) -> Vec<MigrationIdentitySnapshot> {
        self.catalog
            .heads()
            .iter()
            .map(MigrationIdentitySnapshot::from_id)
            .collect()
    }

    /// Build a catalog-bound provider-free apply preview.
    pub(crate) fn preview_apply(
        self: &Arc<Self>,
        applied: &[MigrationIdentitySnapshot],
        targets: Option<&[MigrationIdentitySnapshot]>,
    ) -> Result<Arc<MigrationPlanState>, Diagnostic> {
        let target = match targets {
            None => MigrationApplyTarget::DefaultHead,
            Some(targets) => MigrationApplyTarget::Explicit(identity_set(targets)?),
        };
        self.catalog
            .preview_apply(&identity_set(applied)?, &target)
            .map(|plan| {
                Arc::new(MigrationPlanState {
                    catalog: Arc::clone(self),
                    plan: MigrationPlanKind::Apply(plan),
                })
            })
            .map_err(plan_diagnostic)
    }

    /// Build a catalog-bound provider-free rollback preview.
    pub(crate) fn preview_rollback(
        self: &Arc<Self>,
        applied: &[MigrationIdentitySnapshot],
        removals: &[MigrationIdentitySnapshot],
    ) -> Result<Arc<MigrationPlanState>, Diagnostic> {
        self.catalog
            .preview_rollback(&identity_set(applied)?, &identity_set(removals)?)
            .map(|plan| {
                Arc::new(MigrationPlanState {
                    catalog: Arc::clone(self),
                    plan: MigrationPlanKind::Rollback(plan),
                })
            })
            .map_err(plan_diagnostic)
    }

    /// Verify the exact catalog/ledger/live triad without mutation.
    pub(crate) fn verify(
        &self,
        database: &TypeBridgeDatabase,
    ) -> Result<MigrationVerificationState, Diagnostic> {
        database
            .block_on(type_bridge_schema_migration_typedb::verify_catalog_state(
                database.orm_database_arc(),
                &self.catalog,
            ))
            .map(|report| MigrationVerificationState { report })
    }
}

/// Owned read-only verification report prepared for ABI 1.5 handles.
#[derive(Clone, Debug)]
pub(crate) struct MigrationVerificationState {
    report: type_bridge_schema_migration::MigrationVerifyReport,
}

impl MigrationVerificationState {
    pub(crate) fn is_clean(&self) -> bool {
        self.report.is_clean()
    }

    pub(crate) fn len(&self) -> usize {
        self.report.findings().len()
    }

    pub(crate) fn finding(&self, index: usize) -> Option<MigrationVerificationFindingSnapshot> {
        use type_bridge_schema_migration::MigrationDriftFinding;
        self.report
            .findings()
            .get(index)
            .map(|finding| match finding {
                MigrationDriftFinding::AppliedLedger { diagnostic } => {
                    MigrationVerificationFindingSnapshot {
                        kind: MigrationVerificationFindingKind::AppliedLedger,
                        pending: Vec::new(),
                        diagnostic: Some(diagnostic.clone()),
                    }
                }
                MigrationDriftFinding::LiveSemantics { .. } => {
                    MigrationVerificationFindingSnapshot {
                        kind: MigrationVerificationFindingKind::LiveSemantics,
                        pending: Vec::new(),
                        diagnostic: None,
                    }
                }
                MigrationDriftFinding::DesiredDivergence { .. } => {
                    MigrationVerificationFindingSnapshot {
                        kind: MigrationVerificationFindingKind::DesiredDivergence,
                        pending: Vec::new(),
                        diagnostic: None,
                    }
                }
                MigrationDriftFinding::PendingMigrations { pending } => {
                    MigrationVerificationFindingSnapshot {
                        kind: MigrationVerificationFindingKind::PendingMigrations,
                        pending: pending
                            .iter()
                            .map(MigrationIdentitySnapshot::from_id)
                            .collect(),
                        diagnostic: None,
                    }
                }
                MigrationDriftFinding::Capabilities { diagnostic } => {
                    MigrationVerificationFindingSnapshot {
                        kind: MigrationVerificationFindingKind::Capabilities,
                        pending: Vec::new(),
                        diagnostic: Some(diagnostic.clone()),
                    }
                }
            })
    }

    pub(crate) fn applied_frontier(&self) -> Vec<MigrationIdentitySnapshot> {
        self.report
            .applied_frontier()
            .iter()
            .map(MigrationIdentitySnapshot::from_id)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrationVerificationFindingKind {
    AppliedLedger,
    LiveSemantics,
    DesiredDivergence,
    PendingMigrations,
    Capabilities,
}

#[derive(Clone, Debug)]
pub(crate) struct MigrationVerificationFindingSnapshot {
    pub(crate) kind: MigrationVerificationFindingKind,
    pub(crate) pending: Vec<MigrationIdentitySnapshot>,
    pub(crate) diagnostic: Option<Diagnostic>,
}

/// Copied compound migration identity with no delimiter-joined representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MigrationIdentitySnapshot {
    pub(crate) app_label: Vec<u8>,
    pub(crate) name: Vec<u8>,
}

impl MigrationIdentitySnapshot {
    fn from_id(id: &type_bridge_contract::migration::MigrationId) -> Self {
        Self {
            app_label: id.app_label().as_str().as_bytes().to_vec(),
            name: id.name().as_str().as_bytes().to_vec(),
        }
    }

    fn to_id(&self) -> Result<MigrationId, Diagnostic> {
        MigrationId::new(
            String::from_utf8(self.app_label.clone()).map_err(|_| identity_encoding_failed())?,
            String::from_utf8(self.name.clone()).map_err(|_| identity_encoding_failed())?,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrationPlanDirection {
    Apply,
    Rollback,
}

#[derive(Clone, Debug)]
enum MigrationPlanKind {
    Apply(VerifiedMigrationApplyPlan),
    Rollback(VerifiedMigrationRollbackPlan),
}

/// Immutable preview-plan owner retaining its exact catalog authority.
#[derive(Clone, Debug)]
pub(crate) struct MigrationPlanState {
    catalog: Arc<MigrationCatalogState>,
    plan: MigrationPlanKind,
}

impl MigrationPlanState {
    pub(crate) fn direction(&self) -> MigrationPlanDirection {
        match self.plan {
            MigrationPlanKind::Apply(_) => MigrationPlanDirection::Apply,
            MigrationPlanKind::Rollback(_) => MigrationPlanDirection::Rollback,
        }
    }

    pub(crate) fn execution_authorized(&self) -> bool {
        match &self.plan {
            MigrationPlanKind::Apply(plan) => plan.execution_authorized(),
            MigrationPlanKind::Rollback(plan) => plan.execution_authorized(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.plan {
            MigrationPlanKind::Apply(plan) => plan.migrations().len(),
            MigrationPlanKind::Rollback(plan) => plan.rollbacks().len(),
        }
    }

    pub(crate) fn entry(self: &Arc<Self>, index: usize) -> Option<MigrationPlanEntryState> {
        (index < self.len()).then(|| MigrationPlanEntryState {
            plan: Arc::clone(self),
            index,
        })
    }

    pub(crate) fn catalog_fingerprint_json(&self) -> &[u8] {
        self.catalog.fingerprint_json()
    }

    /// Create one mutable approval builder bound to this exact preview owner.
    pub(crate) fn approval_builder(self: &Arc<Self>) -> MigrationApprovalBuilderState {
        MigrationApprovalBuilderState {
            preview: Arc::clone(self),
            selected: BTreeSet::new(),
        }
    }

    /// Rebuild an executable plan from an approval set bound to this preview.
    pub(crate) fn authorize(
        self: &Arc<Self>,
        approvals: &MigrationApprovalSetState,
    ) -> Result<Arc<Self>, Diagnostic> {
        if !Arc::ptr_eq(self, &approvals.preview) {
            return Err(stable(
                DiagnosticCategory::InvalidContract,
                "c_migration_approval_plan_mismatch",
                "Migration approvals belong to a different preview owner",
            ));
        }
        let policy = MigrationSafetyPolicy::default_policy();
        let plan = match &self.plan {
            MigrationPlanKind::Apply(plan) => self
                .catalog
                .catalog
                .authorize_apply(
                    &plan.applied_migrations().iter().cloned().collect(),
                    &MigrationApplyTarget::Explicit(
                        plan.target_frontier().iter().cloned().collect(),
                    ),
                    &policy,
                    &approvals.approvals,
                )
                .map(MigrationPlanKind::Apply),
            MigrationPlanKind::Rollback(plan) => self
                .catalog
                .catalog
                .authorize_rollback(
                    &plan.applied_basis(),
                    &plan
                        .rollbacks()
                        .iter()
                        .map(|entry| entry.manifest().id().clone())
                        .collect(),
                    &policy,
                    &approvals.approvals,
                )
                .map(MigrationPlanKind::Rollback),
        }
        .map_err(plan_diagnostic)?;
        Ok(Arc::new(Self {
            catalog: Arc::clone(&self.catalog),
            plan,
        }))
    }

    /// Execute an authorized plan through the database's exact provider runtime.
    pub(crate) fn execute(
        &self,
        database: &TypeBridgeDatabase,
        holder: &str,
    ) -> Result<MigrationPlanExecutionState, Diagnostic> {
        self.execute_controlled(
            database,
            holder,
            &type_bridge_schema_migration::MigrationExecutionControl::default(),
        )
    }

    pub(crate) fn execute_controlled(
        &self,
        database: &TypeBridgeDatabase,
        holder: &str,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<MigrationPlanExecutionState, Diagnostic> {
        self.require_execution_authorized()?;
        let holder = LeaseHolderId::new(holder)?;
        let managed_database = database.orm_database_arc();
        match &self.plan {
            MigrationPlanKind::Apply(plan) => database
                .block_on(
                    type_bridge_schema_migration_typedb::execute_catalog_apply_plan_controlled(
                        managed_database,
                        &self.catalog.catalog,
                        &holder,
                        plan,
                        control,
                    ),
                )
                .map(MigrationExecutionReport::from_apply)
                .map(|report| MigrationPlanExecutionState { report }),
            MigrationPlanKind::Rollback(plan) => database
                .block_on(
                    type_bridge_schema_migration_typedb::execute_catalog_rollback_plan_controlled(
                        managed_database,
                        &self.catalog.catalog,
                        &holder,
                        plan,
                        control,
                    ),
                )
                .map(MigrationExecutionReport::from_rollback)
                .map(|report| MigrationPlanExecutionState { report }),
        }
    }

    fn require_execution_authorized(&self) -> Result<(), Diagnostic> {
        match &self.plan {
            MigrationPlanKind::Apply(plan) => require_authorized_apply_plan(plan),
            MigrationPlanKind::Rollback(plan) => require_authorized_rollback_plan(plan),
        }
    }
}

/// Owned terminal result prepared for the additive ABI 1.5 outcome handles.
#[derive(Clone, Debug)]
pub(crate) struct MigrationPlanExecutionState {
    report: MigrationExecutionReport,
}

impl MigrationPlanExecutionState {
    pub(crate) const fn report(&self) -> &MigrationExecutionReport {
        &self.report
    }
}

/// Mutable, single-owner approval selection bound to one exact preview.
#[derive(Debug)]
pub(crate) struct MigrationApprovalBuilderState {
    preview: Arc<MigrationPlanState>,
    selected: BTreeSet<usize>,
}

impl MigrationApprovalBuilderState {
    /// Add one exact preview entry whose safety requires explicit approval.
    pub(crate) fn approve(&mut self, index: usize) -> Result<(), Diagnostic> {
        let entry = self.preview.entry(index).ok_or_else(|| {
            stable(
                DiagnosticCategory::InvalidContract,
                "c_migration_approval_index_invalid",
                "Migration approval index is outside the preview",
            )
        })?;
        match MigrationSafetyPolicy::default_policy().decision(entry.safety()) {
            SafetyPolicyDecision::RequireApproval => {
                self.selected.insert(index);
                Ok(())
            }
            SafetyPolicyDecision::Allow => Err(stable(
                DiagnosticCategory::InvalidContract,
                "c_migration_approval_not_required",
                "Migration preview entry does not require explicit approval",
            )),
            SafetyPolicyDecision::Reject => Err(stable(
                DiagnosticCategory::InvalidContract,
                "c_migration_approval_policy_rejected",
                "Migration preview entry cannot be admitted by approval",
            )),
        }
    }

    /// Freeze selected exact transition approvals into an immutable owner.
    pub(crate) fn finish(self) -> Result<MigrationApprovalSetState, Diagnostic> {
        let approvals = self
            .selected
            .iter()
            .map(|index| match &self.preview.plan {
                MigrationPlanKind::Apply(plan) => {
                    MigrationApplyApproval::for_manifest(plan.migrations()[*index].manifest())
                }
                MigrationPlanKind::Rollback(plan) => MigrationApplyApproval::for_rollback(
                    plan.rollbacks()[*index].manifest(),
                    plan.rollbacks()[*index].rollback_safety(),
                ),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MigrationApprovalSetState {
            preview: self.preview,
            approvals,
        })
    }
}

/// Immutable exact approval set retaining its preview owner.
#[derive(Clone, Debug)]
pub(crate) struct MigrationApprovalSetState {
    preview: Arc<MigrationPlanState>,
    approvals: Vec<MigrationApplyApproval>,
}

impl MigrationApprovalSetState {
    pub(crate) fn len(&self) -> usize {
        self.approvals.len()
    }
}

/// Independently owned immutable preview-entry snapshot.
#[derive(Clone, Debug)]
pub(crate) struct MigrationPlanEntryState {
    plan: Arc<MigrationPlanState>,
    index: usize,
}

impl MigrationPlanEntryState {
    pub(crate) fn identity(&self) -> MigrationIdentitySnapshot {
        let id = match &self.plan.plan {
            MigrationPlanKind::Apply(plan) => plan.migrations()[self.index].manifest().id(),
            MigrationPlanKind::Rollback(plan) => plan.rollbacks()[self.index].manifest().id(),
        };
        MigrationIdentitySnapshot::from_id(id)
    }

    pub(crate) fn safety(&self) -> SafetyClass {
        match &self.plan.plan {
            MigrationPlanKind::Apply(plan) => plan.migrations()[self.index].manifest().safety(),
            MigrationPlanKind::Rollback(plan) => plan.rollbacks()[self.index].rollback_safety(),
        }
    }

    pub(crate) fn operation_count(&self) -> usize {
        match &self.plan.plan {
            MigrationPlanKind::Apply(plan) => plan.migrations()[self.index].steps().len(),
            MigrationPlanKind::Rollback(plan) => plan.rollbacks()[self.index].operations().len(),
        }
    }

    pub(crate) fn transaction_group_count(&self) -> usize {
        match &self.plan.plan {
            MigrationPlanKind::Apply(plan) => {
                plan.migrations()[self.index].transaction_groups().len()
            }
            MigrationPlanKind::Rollback(plan) => plan.rollbacks()[self.index].steps().len(),
        }
    }

    pub(crate) fn backfill_count(&self) -> usize {
        match &self.plan.plan {
            MigrationPlanKind::Apply(plan) => {
                plan.migrations()[self.index].backfill_step_indices().len()
            }
            MigrationPlanKind::Rollback(plan) => plan.rollbacks()[self.index].backfills().len(),
        }
    }
}

fn identity_set(
    identities: &[MigrationIdentitySnapshot],
) -> Result<BTreeSet<MigrationId>, Diagnostic> {
    identities
        .iter()
        .map(MigrationIdentitySnapshot::to_id)
        .collect()
}

fn identity_encoding_failed() -> Diagnostic {
    stable(
        DiagnosticCategory::Integrity,
        "c_migration_identity_encoding_invalid",
        "A retained migration identity is not valid UTF-8",
    )
}

fn plan_diagnostic(error: MigrationApplyPlanError) -> Diagnostic {
    match error {
        MigrationApplyPlanError::Contract(diagnostic) => diagnostic,
        MigrationApplyPlanError::Schema(_) => stable(
            DiagnosticCategory::Integrity,
            "c_migration_preview_schema_replay_failed",
            "Migration preview schema replay failed",
        ),
        MigrationApplyPlanError::Lowering(diagnostic) => Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new(diagnostic.code()).expect("lowering diagnostic code is canonical"),
            "Migration preview lowering was rejected",
        ),
    }
}

/// Independently owned immutable entry handle retaining its catalog owner.
#[derive(Clone, Debug)]
pub(crate) struct MigrationHistoryEntryState {
    catalog: Arc<MigrationCatalogState>,
    index: usize,
}

impl MigrationHistoryEntryState {
    fn entry(&self) -> &type_bridge_schema_migration::VerifiedMigrationHistoryBundleEntry {
        &self.catalog.catalog.entries()[self.index]
    }

    pub(crate) fn identity(&self) -> MigrationIdentitySnapshot {
        MigrationIdentitySnapshot::from_id(self.entry().manifest().id())
    }

    pub(crate) fn parents(&self) -> Vec<MigrationIdentitySnapshot> {
        self.entry()
            .manifest()
            .parents()
            .iter()
            .map(MigrationIdentitySnapshot::from_id)
            .collect()
    }

    pub(crate) fn manifest_digest(&self) -> [u8; 32] {
        self.entry().manifest_digest().bytes()
    }

    pub(crate) fn step_count(&self) -> usize {
        self.entry().manifest().steps().len()
    }

    pub(crate) fn safety(&self) -> SafetyClass {
        self.entry().manifest().safety()
    }

    pub(crate) fn reversible(&self) -> bool {
        self.entry().manifest().reversible()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::migration::{MigrationId, MigrationStepId, SchemaDeltaStep};
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
    };
    use type_bridge_orm::error::OrmError;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
    use type_bridge_orm::{Database, DatabaseConnectionAuthority};
    use type_bridge_schema::decode_schema_authority;
    use type_bridge_schema::{ManagedDeltaContext, diff_managed, inverse_delta};
    use type_bridge_schema_migration::{
        MigrationHistoryGraph, SchemaMigrationDraft, VerifiedMigrationHistoryBundle,
        build_verified_manifest, encode_verified_migration_history_bundle,
        migration_runtime_capability_vocabulary,
    };

    use super::{DatabaseAdministrationState, MigrationCatalogState};

    struct AdministrationBackend {
        databases: Arc<Mutex<BTreeSet<String>>>,
    }

    impl DriverBackend for AdministrationBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Err(OrmError::Connection("unexpected transaction".into())) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
            let exists = self.databases.lock().unwrap().contains(database);
            Box::pin(async move { Ok(exists) })
        }

        fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.databases.lock().unwrap().insert(database.to_owned());
            Box::pin(async { Ok(()) })
        }

        fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.databases.lock().unwrap().remove(database);
            Box::pin(async { Ok(()) })
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    #[tokio::test]
    async fn c_administration_owner_retains_pair_aware_single_use_plan() {
        let databases = Arc::new(Mutex::new(BTreeSet::new()));
        let authority = DatabaseConnectionAuthority::isolated();
        let managed = Arc::new(Database::with_backend_authority(
            Box::new(AdministrationBackend {
                databases: Arc::clone(&databases),
            }),
            "app",
            authority.clone(),
        ));
        let journal = Arc::new(Database::with_backend_authority(
            Box::new(AdministrationBackend {
                databases: Arc::clone(&databases),
            }),
            type_bridge_schema_migration_typedb::derived_journal_database_name("app"),
            authority,
        ));
        let administrator =
            type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator::new(
                managed,
                journal,
                type_bridge_contract::managed_scope::ManagedScopeId::new("generated-scope")
                    .unwrap(),
            )
            .unwrap();
        let owner = DatabaseAdministrationState::from_administrator(administrator);

        assert!(!owner.database_exists().await.unwrap());
        assert_eq!(
            owner.create_database_outcome().await.unwrap(),
            type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created
        );
        assert_eq!(
            owner.inspect().await.unwrap(),
            type_bridge_schema_migration_typedb::ManagedDatabasePairState::StandaloneManaged
        );
        let mut plan = owner.plan_delete().await.unwrap();
        assert!(plan.retains_administration(&owner));
        drop(owner);
        assert_eq!(
            plan.execute().await.unwrap(),
            type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedStandaloneManaged
        );
        assert!(plan.execute().await.is_err());
        assert!(databases.lock().unwrap().is_empty());
    }

    fn declared(labels: &[&str]) -> DeclaredSchema {
        let facts = labels.iter().enumerate().map(|(index, label)| {
            let ordinal = u64::try_from(index).unwrap();
            let line = u32::try_from(index + 1).unwrap();
            SourcedSchemaFact::new(
                SchemaFact::Type(
                    TypeFact::new(TypeId::new(TypeKind::Entity, *label).unwrap()).unwrap(),
                ),
                SourceSpan::new(
                    DocumentId::new("c-migration-owner-fixture").unwrap(),
                    ordinal,
                    ordinal + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .unwrap(),
            )
        });
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), facts).unwrap()
    }

    #[test]
    fn empty_catalog_owner_has_bounded_independent_shape() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog =
            type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle).unwrap();
        let fingerprint_json = serde_json::to_vec(catalog.fingerprint()).unwrap();
        let owner = std::sync::Arc::new(MigrationCatalogState {
            catalog,
            fingerprint_json,
        });

        assert_eq!(owner.len(), 0);
        assert!(owner.heads().is_empty());
        assert!(owner.entry(0).is_none());
        assert!(owner.fingerprint_json().starts_with(b"{"));
    }

    #[test]
    fn authority_opened_catalog_owns_non_executable_preview_resources() {
        let capabilities = migration_runtime_capability_vocabulary().unwrap();
        let authority = decode_schema_authority(
            include_bytes!("../../server/tests/fixtures/schema-authority.json"),
            &capabilities,
        )
        .unwrap();
        let context = ManagedDeltaContext::new(
            authority.managed_scope().id().clone(),
            authority.semantic_profile().id().clone(),
            capabilities,
        );
        let source = declared(&["person", "team"]);
        let target = declared(&["person"]);
        let delta = diff_managed(&source, &target, &context).unwrap();
        let reverse = inverse_delta(&delta).unwrap();
        let step = SchemaDeltaStep::new(
            MigrationStepId::new("step-expand").unwrap(),
            delta,
            Some(reverse),
        )
        .unwrap();
        let manifest = build_verified_manifest(
            SchemaMigrationDraft::new(
                MigrationId::new("example", "0001_expand").unwrap(),
                Vec::new(),
                vec![step],
            )
            .unwrap(),
            (&source, &context),
        )
        .unwrap();
        let graph = MigrationHistoryGraph::from_verified([manifest]).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let bytes = encode_verified_migration_history_bundle(&bundle).unwrap();
        let catalog = MigrationCatalogState::open(&authority, &bytes).unwrap();
        let preview = catalog.preview_apply(&[], None).unwrap();

        assert_eq!(
            preview
                .require_execution_authorized()
                .unwrap_err()
                .code()
                .as_str(),
            "migration_apply_preview_not_executable"
        );

        assert_eq!(preview.direction(), super::MigrationPlanDirection::Apply);
        assert!(!preview.execution_authorized());
        assert_eq!(preview.len(), 1);
        let entry = preview.entry(0).unwrap();
        assert_eq!(entry.identity().app_label, b"example");
        assert_eq!(entry.identity().name, b"0001_expand");
        assert_eq!(entry.operation_count(), 1);
        assert_eq!(entry.transaction_group_count(), 1);
        assert_eq!(entry.backfill_count(), 0);
        assert_eq!(entry.safety(), type_bridge_schema::SafetyClass::Destructive);
        assert!(preview.entry(1).is_none());
        assert_eq!(
            preview.catalog_fingerprint_json(),
            catalog.fingerprint_json()
        );
        let mut builder = preview.approval_builder();
        builder.approve(0).unwrap();
        let approvals = builder.finish().unwrap();
        assert_eq!(approvals.len(), 1);
        let executable = preview.authorize(&approvals).unwrap();
        assert!(executable.execution_authorized());
        assert_eq!(executable.len(), 1);
    }
}
