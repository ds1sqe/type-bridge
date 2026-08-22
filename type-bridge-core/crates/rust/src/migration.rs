//! Generated-package-owned immutable migration catalog inspection.

use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::migration::{MigrationId, MigrationStep};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass};

use crate::error::{Error, Result};
use crate::schema::{Schema, SchemaPackage};
#[cfg(feature = "typedb")]
use crate::session::Database;

/// Immutable replay-verified migration catalog branded by one generated schema.
#[derive(Clone, Debug)]
pub struct MigrationCatalog<S: Schema> {
    inner: type_bridge_schema_migration::MigrationCatalog,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationCatalog<S> {
    /// Return the canonical history-bundle fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &Fingerprint {
        self.inner.fingerprint()
    }

    /// Return the number of replay-verified history entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.entries().len()
    }

    /// Report whether the catalog contains no committed migrations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.entries().is_empty()
    }

    /// Return graph heads in canonical compound-identity order.
    #[must_use]
    pub fn heads(&self) -> &[MigrationId] {
        self.inner.heads()
    }

    /// Inspect one history entry by deterministic topological ordinal.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<MigrationHistoryEntry<'_>> {
        self.inner
            .entries()
            .get(index)
            .map(|entry| MigrationHistoryEntry { inner: entry })
    }

    /// Build a provider-free forward preview under this generated authority.
    pub fn preview_apply(
        &self,
        applied: impl IntoIterator<Item = MigrationId>,
        targets: Option<Vec<MigrationId>>,
    ) -> Result<MigrationPreview<S>> {
        let applied = applied.into_iter().collect();
        let target = match targets {
            Some(targets) => type_bridge_schema_migration::MigrationApplyTarget::Explicit(
                targets.into_iter().collect(),
            ),
            None => type_bridge_schema_migration::MigrationApplyTarget::DefaultHead,
        };
        self.inner
            .preview_apply(&applied, &target)
            .map(|plan| MigrationPreview {
                inner: Arc::new(MigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: MigrationPreviewInner::Apply(plan),
                }),
                marker: PhantomData,
            })
            .map_err(plan_error)
    }

    /// Build a provider-free rollback preview for an explicit removal set.
    pub fn preview_rollback(
        &self,
        applied: impl IntoIterator<Item = MigrationId>,
        removals: impl IntoIterator<Item = MigrationId>,
    ) -> Result<MigrationPreview<S>> {
        self.inner
            .preview_rollback(
                &applied.into_iter().collect(),
                &removals.into_iter().collect(),
            )
            .map(|plan| MigrationPreview {
                inner: Arc::new(MigrationPreviewState {
                    catalog: self.inner.clone(),
                    plan: MigrationPreviewInner::Rollback(plan),
                }),
                marker: PhantomData,
            })
            .map_err(plan_error)
    }

    /// Verify the applied ledger and live managed semantics without mutation.
    #[cfg(feature = "typedb")]
    pub async fn verify(
        &self,
        database: &Database<S>,
    ) -> Result<type_bridge_schema_migration::MigrationVerifyReport> {
        type_bridge_schema_migration_typedb::verify_catalog_state(
            Arc::new(database.inner_orm().clone()),
            &self.inner,
        )
        .await
        .map_err(migration_diagnostic)
    }
}

#[derive(Clone, Debug)]
enum MigrationPreviewInner {
    Apply(type_bridge_schema_migration::VerifiedMigrationApplyPlan),
    Rollback(type_bridge_schema_migration::VerifiedMigrationRollbackPlan),
}

#[derive(Clone, Debug)]
struct MigrationPreviewState {
    catalog: type_bridge_schema_migration::MigrationCatalog,
    plan: MigrationPreviewInner,
}

/// Generated-schema-branded immutable provider-free migration preview.
#[derive(Clone, Debug)]
pub struct MigrationPreview<S: Schema> {
    inner: Arc<MigrationPreviewState>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationPreview<S> {
    /// Return whether this is a forward apply preview.
    #[must_use]
    pub fn is_apply(&self) -> bool {
        matches!(self.inner.plan, MigrationPreviewInner::Apply(_))
    }

    /// Preview objects never grant provider execution authority.
    #[must_use]
    pub fn execution_authorized(&self) -> bool {
        match &self.inner.plan {
            MigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            MigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    /// Return the number of migrations in execution order.
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.inner.plan {
            MigrationPreviewInner::Apply(plan) => plan.migrations().len(),
            MigrationPreviewInner::Rollback(plan) => plan.rollbacks().len(),
        }
    }

    /// Report whether the preview contains no migration work.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Inspect one migration by deterministic execution ordinal.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<MigrationPreviewEntry<'_>> {
        match &self.inner.plan {
            MigrationPreviewInner::Apply(plan) => {
                plan.migrations()
                    .get(index)
                    .map(|entry| MigrationPreviewEntry {
                        id: entry.manifest().id(),
                        safety: entry.manifest().safety(),
                        step_count: entry.steps().len(),
                        transaction_group_count: entry.transaction_groups().len(),
                        backfill_count: entry.backfill_step_indices().len(),
                        reversible: entry.manifest().reversible(),
                    })
            }
            MigrationPreviewInner::Rollback(plan) => {
                plan.rollbacks()
                    .get(index)
                    .map(|entry| MigrationPreviewEntry {
                        id: entry.manifest().id(),
                        safety: entry.rollback_safety(),
                        step_count: entry.operations().len(),
                        transaction_group_count: entry.steps().len(),
                        backfill_count: entry.backfills().len(),
                        reversible: true,
                    })
            }
        }
    }

    /// Begin selecting exact transitions that require explicit approval.
    #[must_use]
    pub fn approval_builder(&self) -> MigrationApprovalBuilder<S> {
        MigrationApprovalBuilder {
            preview: Arc::clone(&self.inner),
            selected: BTreeSet::new(),
            marker: PhantomData,
        }
    }

    /// Rebuild a fresh executable plan from approvals owned by this preview.
    pub fn authorize(&self, approvals: &MigrationApprovalSet<S>) -> Result<MigrationPlan<S>> {
        if !Arc::ptr_eq(&self.inner, &approvals.preview) {
            return Err(migration_error("migration_approval_plan_mismatch"));
        }
        let policy = type_bridge_schema_migration::MigrationSafetyPolicy::default_policy();
        let plan = match &self.inner.plan {
            MigrationPreviewInner::Apply(plan) => self
                .inner
                .catalog
                .authorize_apply(
                    &plan.applied_migrations().iter().cloned().collect(),
                    &type_bridge_schema_migration::MigrationApplyTarget::Explicit(
                        plan.target_frontier().iter().cloned().collect(),
                    ),
                    &policy,
                    &approvals.approvals,
                )
                .map(MigrationPreviewInner::Apply),
            MigrationPreviewInner::Rollback(plan) => self
                .inner
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
                .map(MigrationPreviewInner::Rollback),
        }
        .map_err(plan_error)?;
        Ok(MigrationPlan {
            catalog: self.inner.catalog.clone(),
            plan,
            marker: PhantomData,
        })
    }
}

/// Mutable exact-approval selection owned by one provider-free preview.
#[derive(Debug)]
pub struct MigrationApprovalBuilder<S: Schema> {
    preview: Arc<MigrationPreviewState>,
    selected: BTreeSet<usize>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationApprovalBuilder<S> {
    /// Approve one preview entry when the default policy requires approval.
    pub fn approve(&mut self, index: usize) -> Result<()> {
        let safety = preview_entry(&self.preview.plan, index)
            .ok_or_else(|| migration_error("migration_approval_index_invalid"))?
            .safety();
        match type_bridge_schema_migration::MigrationSafetyPolicy::default_policy().decision(safety)
        {
            type_bridge_schema_migration::SafetyPolicyDecision::RequireApproval => {
                self.selected.insert(index);
                Ok(())
            }
            type_bridge_schema_migration::SafetyPolicyDecision::Allow => {
                Err(migration_error("migration_approval_not_required"))
            }
            type_bridge_schema_migration::SafetyPolicyDecision::Reject => {
                Err(migration_error("migration_approval_policy_rejected"))
            }
        }
    }

    /// Freeze the selected exact transitions into an immutable approval set.
    pub fn finish(self) -> Result<MigrationApprovalSet<S>> {
        let approvals = self
            .selected
            .iter()
            .map(|index| match &self.preview.plan {
                MigrationPreviewInner::Apply(plan) => {
                    type_bridge_schema_migration::MigrationApplyApproval::for_manifest(
                        plan.migrations()[*index].manifest(),
                    )
                }
                MigrationPreviewInner::Rollback(plan) => {
                    type_bridge_schema_migration::MigrationApplyApproval::for_rollback(
                        plan.rollbacks()[*index].manifest(),
                        plan.rollbacks()[*index].rollback_safety(),
                    )
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(migration_diagnostic)?;
        Ok(MigrationApprovalSet {
            preview: self.preview,
            approvals,
            marker: PhantomData,
        })
    }
}

/// Immutable exact approvals retained with their originating preview owner.
#[derive(Clone, Debug)]
pub struct MigrationApprovalSet<S: Schema> {
    preview: Arc<MigrationPreviewState>,
    approvals: Vec<type_bridge_schema_migration::MigrationApplyApproval>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationApprovalSet<S> {
    /// Return the number of exact approved transitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.approvals.len()
    }

    /// Report whether no transitions were explicitly approved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.approvals.is_empty()
    }
}

/// Generated-schema-branded executable migration plan.
#[derive(Clone, Debug)]
pub struct MigrationPlan<S: Schema> {
    #[cfg_attr(not(feature = "typedb"), allow(dead_code))]
    catalog: type_bridge_schema_migration::MigrationCatalog,
    plan: MigrationPreviewInner,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationPlan<S> {
    /// Confirm that policy and exact approvals granted execution authority.
    #[must_use]
    pub fn execution_authorized(&self) -> bool {
        match &self.plan {
            MigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            MigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    /// Execute this plan through the exact managed database/journal pair.
    #[cfg(feature = "typedb")]
    pub async fn execute(
        &self,
        database: &Database<S>,
        holder: &str,
    ) -> Result<type_bridge_schema_migration::MigrationExecutionReport> {
        self.execute_controlled(
            database,
            holder,
            &type_bridge_schema_migration::MigrationExecutionControl::default(),
        )
        .await
    }

    /// Execute with shared cancellation, absolute deadline, and tightened limits.
    #[cfg(feature = "typedb")]
    pub async fn execute_controlled(
        &self,
        database: &Database<S>,
        holder: &str,
        control: &type_bridge_schema_migration::MigrationExecutionControl,
    ) -> Result<type_bridge_schema_migration::MigrationExecutionReport> {
        let holder = type_bridge_schema_migration::LeaseHolderId::new(holder)
            .map_err(migration_diagnostic)?;
        let database = Arc::new(database.inner_orm().clone());
        match &self.plan {
            MigrationPreviewInner::Apply(plan) => {
                type_bridge_schema_migration_typedb::execute_catalog_apply_plan_controlled(
                    database,
                    &self.catalog,
                    &holder,
                    plan,
                    control,
                )
                .await
                .map(type_bridge_schema_migration::MigrationExecutionReport::from_apply)
                .map_err(migration_diagnostic)
            }
            MigrationPreviewInner::Rollback(plan) => {
                type_bridge_schema_migration_typedb::execute_catalog_rollback_plan_controlled(
                    database,
                    &self.catalog,
                    &holder,
                    plan,
                    control,
                )
                .await
                .map(type_bridge_schema_migration::MigrationExecutionReport::from_rollback)
                .map_err(migration_diagnostic)
            }
        }
    }
}

/// Borrowed bounded inspection of one migration preview entry.
#[derive(Clone, Copy, Debug)]
pub struct MigrationPreviewEntry<'a> {
    id: &'a MigrationId,
    safety: SafetyClass,
    step_count: usize,
    transaction_group_count: usize,
    backfill_count: usize,
    reversible: bool,
}

impl MigrationPreviewEntry<'_> {
    /// Return the compound migration identity.
    pub const fn id(&self) -> &MigrationId {
        self.id
    }
    /// Return the complete forward or reverse safety class.
    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }
    /// Return the complete ordered operation count.
    pub const fn step_count(&self) -> usize {
        self.step_count
    }
    /// Return the schema transaction-group count.
    pub const fn transaction_group_count(&self) -> usize {
        self.transaction_group_count
    }
    /// Return the closed backfill-group count.
    pub const fn backfill_count(&self) -> usize {
        self.backfill_count
    }
    /// Report verified reversibility.
    pub const fn is_reversible(&self) -> bool {
        self.reversible
    }
}

fn preview_entry(plan: &MigrationPreviewInner, index: usize) -> Option<MigrationPreviewEntry<'_>> {
    match plan {
        MigrationPreviewInner::Apply(plan) => {
            plan.migrations()
                .get(index)
                .map(|entry| MigrationPreviewEntry {
                    id: entry.manifest().id(),
                    safety: entry.manifest().safety(),
                    step_count: entry.steps().len(),
                    transaction_group_count: entry.transaction_groups().len(),
                    backfill_count: entry.backfill_step_indices().len(),
                    reversible: entry.manifest().reversible(),
                })
        }
        MigrationPreviewInner::Rollback(plan) => {
            plan.rollbacks()
                .get(index)
                .map(|entry| MigrationPreviewEntry {
                    id: entry.manifest().id(),
                    safety: entry.rollback_safety(),
                    step_count: entry.operations().len(),
                    transaction_group_count: entry.steps().len(),
                    backfill_count: entry.backfills().len(),
                    reversible: true,
                })
        }
    }
}

/// Borrowed immutable view of one catalog history entry.
#[derive(Clone, Copy, Debug)]
pub struct MigrationHistoryEntry<'a> {
    inner: &'a type_bridge_schema_migration::VerifiedMigrationHistoryBundleEntry,
}

impl MigrationHistoryEntry<'_> {
    /// Return the canonical compound migration identity.
    #[must_use]
    pub const fn id(&self) -> &MigrationId {
        self.inner.manifest().id()
    }

    /// Return canonical parent identities.
    #[must_use]
    pub fn parents(&self) -> &[MigrationId] {
        self.inner.manifest().parents()
    }

    /// Return the exact canonical manifest digest.
    #[must_use]
    pub const fn manifest_digest(
        &self,
    ) -> type_bridge_contract::migration::MigrationManifestDigest {
        self.inner.manifest_digest()
    }

    /// Return the number of ordered schema, assertion, and backfill steps.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.inner.manifest().steps().len()
    }

    /// Return one ordered step for typed inspection.
    #[must_use]
    pub fn step(&self, index: usize) -> Option<&MigrationStep> {
        self.inner.manifest().steps().get(index)
    }

    /// Return the closed safety classification.
    #[must_use]
    pub const fn safety(&self) -> SafetyClass {
        self.inner.manifest().safety()
    }

    /// Report whether the complete migration has a verified reverse.
    #[must_use]
    pub const fn is_reversible(&self) -> bool {
        self.inner.manifest().reversible()
    }
}

impl<S: Schema> SchemaPackage<S> {
    /// Open the generated package's canonical migration-history resource.
    ///
    /// Package authority and every bundled schema/manifest are verified
    /// offline before the catalog is returned. This operation performs no
    /// connection, database, transaction, journal, or provider I/O.
    pub fn open_migration_catalog(&self, bytes: &[u8]) -> Result<MigrationCatalog<S>> {
        let (_projection, authority) = self.verify_and_install_with_authority()?;
        let authority = authority.ok_or_else(|| Error::SchemaVerification {
            message: "migration catalogs require generated schema authority".to_owned(),
            source: None,
        })?;
        let context = ManagedDeltaContext::new(
            authority.managed_scope().id().clone(),
            authority.semantic_profile().id().clone(),
            type_bridge_schema_migration::migration_runtime_capability_vocabulary().map_err(
                |error| Error::SchemaVerification {
                    message: format!(
                        "migration runtime capability vocabulary is invalid [{}]",
                        error.code().as_str()
                    ),
                    source: Some(Box::new(error)),
                },
            )?,
        );
        let inner = type_bridge_schema_migration::MigrationCatalog::open(bytes, &context).map_err(
            |error| Error::SchemaVerification {
                message: format!(
                    "generated migration catalog was rejected [{}]",
                    error.code().as_str()
                ),
                source: Some(Box::new(error)),
            },
        )?;
        Ok(MigrationCatalog {
            inner,
            marker: PhantomData,
        })
    }
}

fn plan_error(error: type_bridge_schema_migration::MigrationApplyPlanError) -> Error {
    let message = match &error {
        type_bridge_schema_migration::MigrationApplyPlanError::Contract(diagnostic) => format!(
            "generated migration preview was rejected [{}]",
            diagnostic.code().as_str()
        ),
        type_bridge_schema_migration::MigrationApplyPlanError::Schema(_) => {
            "generated migration preview schema replay failed".to_owned()
        }
        type_bridge_schema_migration::MigrationApplyPlanError::Lowering(diagnostic) => format!(
            "generated migration preview lowering was rejected [{}]",
            diagnostic.code()
        ),
    };
    Error::SchemaVerification {
        message,
        source: Some(Box::new(error)),
    }
}

fn migration_error(code: &str) -> Error {
    Error::SchemaVerification {
        message: format!("generated migration operation was rejected [{code}]"),
        source: None,
    }
}

fn migration_diagnostic(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    Error::from_contract_diagnostic(error)
}
