//! TypeDB implementation of the provider-neutral fenced execution stores.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::reserved::TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX;
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{Database, OrmError, Transaction};
use type_bridge_schema_compat::typeql_to_declared;
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionFence, ExecutionFuture, ExecutionScope, GroupEventRecord,
    GroupJournalEventKind, JournalEntry, JournalSequence, LeaseHolderId, MigrationExecutionJournal,
    MigrationLease, MigrationLeaseStore, OpenPlanRecord, OpenRollbackPlanRecord, PlanRecord,
    RollbackPlanRecord, RollbackStepEventRecord, RolledBackRecord, VerifiedMigrationApplyPlan,
    VerifiedMigrationRollbackPlan, VerifiedMigrationTransactionGroup,
    VerifiedSchemaMigrationManifest, active_applied_entries, verified_manifest_digest,
};

use crate::control_schema::{
    APPLIED_RECORD_KIND, CONTROL_ENTITY, CONTROL_SCOPE, EVENT_RECORD_KIND,
    JOURNAL_CONTROL_SCHEMA_TYPEQL, JOURNAL_ENTITY, JOURNAL_OWNER_ENTITY, JOURNAL_OWNER_KEY,
    JOURNAL_OWNER_MANAGED_DATABASE, JOURNAL_OWNER_MANAGED_SCOPE, JOURNAL_OWNER_SINGLETON_KEY,
    LEASE_FENCE, LEASE_FREE, LEASE_HELD, LEASE_HOLDER, LEASE_STATE, MANAGED_FENCE_SCHEMA_TYPEQL,
    NEXT_SEQUENCE, PLAN_RECORD_KIND, RECORD_KEY, RECORD_KIND, RECORD_PAYLOAD,
    RECORD_PAYLOAD_DIGEST, RECORD_SEQUENCE, ROLLBACK_EVENT_RECORD_KIND, ROLLBACK_PLAN_RECORD_KIND,
    ROLLED_BACK_RECORD_KIND,
};
use crate::observation::{partition_typeql_export, partition_typeql_export_lossless};
use crate::wire::{
    decode_applied, decode_event, decode_plan, decode_rollback_event, decode_rollback_plan,
    decode_rolled_back, encode_applied, encode_event, encode_plan, encode_rollback_event,
    encode_rollback_plan, encode_rolled_back, persisted_fence,
};

/// Derive the one-to-one companion journal database name.
///
/// The managed and journal databases are one recovery unit. Operators must
/// back up, restore, clone, and delete the derived pair together; restoring
/// either member alone is unsupported and fails closed through fence or
/// verified-record identity mismatch. The suffix is reserved within a TypeDB
/// deployment: no managed database may use a name that is another managed
/// database's derived journal name.
#[must_use]
pub fn derived_journal_database_name(managed_database_name: &str) -> String {
    format!("{managed_database_name}{TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX}")
}

/// Verified historical manifest catalog used to rederive applied records.
pub struct VerifiedMigrationCatalog<'a> {
    manifests: BTreeMap<MigrationId, &'a VerifiedSchemaMigrationManifest>,
}

impl<'a> VerifiedMigrationCatalog<'a> {
    /// Build a duplicate-free catalog of independently verified manifests.
    pub fn new(
        manifests: impl IntoIterator<Item = &'a VerifiedSchemaMigrationManifest>,
    ) -> Result<Self, Diagnostic> {
        let mut indexed = BTreeMap::new();
        for manifest in manifests {
            verified_manifest_digest(manifest)?;
            if indexed.insert(manifest.id().clone(), manifest).is_some() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_typedb_duplicate_catalog_id",
                    "verified migration catalog contains a duplicate migration identity",
                ));
            }
        }
        Ok(Self { manifests: indexed })
    }

    fn get(&self, id: &MigrationId) -> Option<&'a VerifiedSchemaMigrationManifest> {
        self.manifests.get(id).copied()
    }

    fn values(&self) -> impl ExactSizeIterator<Item = &'a VerifiedSchemaMigrationManifest> + '_ {
        self.manifests.values().copied()
    }
}

/// TypeDB-backed lease and journal store bound to verified history evidence.
///
/// The authoritative lease and write-ahead journal live in a deterministically
/// named companion database. The managed database contains only a fence mirror
/// read by prepared schema transactions. Acquisition advances authority first,
/// then waits to publish the mirror; release clears authority first, then the
/// mirror. The pair is a single backup and recovery unit.
pub struct TypeDbMigrationStore<'a> {
    managed_database: Arc<Database>,
    journal_database: Arc<Database>,
    managed_scope_id: ManagedScopeId,
    catalog: VerifiedMigrationCatalog<'a>,
    plan: Option<&'a VerifiedMigrationApplyPlan>,
    rollback_plan: Option<&'a VerifiedMigrationRollbackPlan>,
    managed_schema_verified: AtomicBool,
    journal_schema_verified: AtomicBool,
}

impl<'a> TypeDbMigrationStore<'a> {
    /// Construct a history-bound store over one exact managed/journal pair.
    pub fn new(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        managed_scope_id: ManagedScopeId,
        catalog: VerifiedMigrationCatalog<'a>,
    ) -> Result<Self, Diagnostic> {
        if !managed_database.shares_connection_authority_with(&journal_database) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_database_authority_mismatch",
                "managed and journal databases must share one provider connection authority",
            ));
        }
        if managed_database.database_name() == journal_database.database_name() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_database_pair_alias",
                "managed and journal databases must be distinct",
            ));
        }
        let expected = derived_journal_database_name(managed_database.database_name());
        if journal_database.database_name() != expected {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_journal_database_name_mismatch",
                "journal database name is not the one-to-one derivative of the managed database",
            )
            .with_detail(
                "managed_database",
                managed_database.database_name().to_owned(),
            )
            .with_detail("expected_journal_database", expected)
            .with_detail(
                "actual_journal_database",
                journal_database.database_name().to_owned(),
            ));
        }
        Ok(Self {
            managed_database,
            journal_database,
            managed_scope_id,
            catalog,
            plan: None,
            rollback_plan: None,
            managed_schema_verified: AtomicBool::new(false),
            journal_schema_verified: AtomicBool::new(false),
        })
    }

    /// Bind the store to the exact apply plan used for open-plan recovery.
    pub fn bind_plan(self, plan: &'a VerifiedMigrationApplyPlan) -> Result<Self, Diagnostic> {
        self.bind_plan_inner(plan, false)
    }

    /// Bind the one guarded legacy-import plan.
    ///
    /// This is intentionally crate-private: public/generic plan binding must
    /// never establish the permanent bridge without the runner's managed-side
    /// recovery anchor and V1 ledger guard.
    pub(crate) fn bind_legacy_import_plan(
        self,
        plan: &'a VerifiedMigrationApplyPlan,
    ) -> Result<Self, Diagnostic> {
        self.bind_plan_inner(plan, true)
    }

    /// Bind a descendant apply plan after the runner validated the permanent
    /// legacy cutover binding. Public callers cannot bypass that proof.
    pub(crate) fn bind_guarded_bridge_plan(
        self,
        plan: &'a VerifiedMigrationApplyPlan,
    ) -> Result<Self, Diagnostic> {
        self.bind_plan_inner(plan, true)
    }

    fn bind_plan_inner(
        mut self,
        plan: &'a VerifiedMigrationApplyPlan,
        allow_legacy_bridge: bool,
    ) -> Result<Self, Diagnostic> {
        if !allow_legacy_bridge
            && self
                .catalog
                .values()
                .any(|manifest| manifest.is_legacy_bridge())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_legacy_import_required",
                "a bridge-rooted catalog requires the runner's validated cutover binding",
            ));
        }
        for migration in plan.migrations() {
            if migration.manifest().is_legacy_bridge() && !allow_legacy_bridge {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_legacy_import_required",
                    "a legacy bridge can be bound only by the guarded legacy import runner",
                ));
            }
            let Some(catalog_manifest) = self.catalog.get(migration.manifest().id()) else {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_plan_manifest_missing",
                    "apply plan references a manifest absent from the verified catalog",
                ));
            };
            if verified_manifest_digest(catalog_manifest)? != migration.digest() {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_plan_manifest_mismatch",
                    "apply plan and verified catalog disagree on manifest identity",
                ));
            }
        }
        self.plan = Some(plan);
        Ok(self)
    }

    /// Bind the store to the exact rollback plan used for open-plan recovery.
    pub fn bind_rollback_plan(
        self,
        plan: &'a VerifiedMigrationRollbackPlan,
    ) -> Result<Self, Diagnostic> {
        self.bind_rollback_plan_inner(plan, false)
    }

    /// Bind a descendant rollback after the runner validated the permanent
    /// legacy cutover binding.
    pub(crate) fn bind_guarded_bridge_rollback_plan(
        self,
        plan: &'a VerifiedMigrationRollbackPlan,
    ) -> Result<Self, Diagnostic> {
        self.bind_rollback_plan_inner(plan, true)
    }

    fn bind_rollback_plan_inner(
        mut self,
        plan: &'a VerifiedMigrationRollbackPlan,
        allow_bridged_catalog: bool,
    ) -> Result<Self, Diagnostic> {
        if !allow_bridged_catalog
            && self
                .catalog
                .values()
                .any(|manifest| manifest.is_legacy_bridge())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_legacy_binding_required",
                "a bridge-rooted rollback requires the runner's validated cutover binding",
            ));
        }
        for rollback in plan.rollbacks() {
            let Some(catalog_manifest) = self.catalog.get(rollback.manifest().id()) else {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_plan_manifest_missing",
                    "rollback plan references a manifest absent from the verified catalog",
                ));
            };
            if verified_manifest_digest(catalog_manifest)? != *rollback.digest() {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_plan_manifest_mismatch",
                    "rollback plan and verified catalog disagree on manifest identity",
                ));
            }
        }
        self.rollback_plan = Some(plan);
        Ok(self)
    }

    /// Install or verify the frozen control schema.
    pub fn ensure_control_schema(&self) -> ExecutionFuture<'_, ()> {
        Box::pin(async move { self.ensure_schema().await })
    }

    async fn ensure_schema(&self) -> Result<(), Diagnostic> {
        self.ensure_journal_schema().await?;
        self.ensure_schema_contract(
            &self.managed_database,
            &self.managed_schema_verified,
            MANAGED_FENCE_SCHEMA_TYPEQL,
            "managed-fence",
        )
        .await
    }

    async fn ensure_schema_for_scope(&self, scope: &ExecutionScope) -> Result<(), Diagnostic> {
        self.require_owned_scope(scope)?;
        self.ensure_schema().await
    }

    fn require_owned_scope(&self, scope: &ExecutionScope) -> Result<(), Diagnostic> {
        if scope.managed_scope_id() == &self.managed_scope_id {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_journal_owner_scope_mismatch",
                "migration operation scope differs from the journal database owner identity",
            ))
        }
    }

    async fn ensure_journal_schema(&self) -> Result<(), Diagnostic> {
        if self.journal_schema_verified.load(Ordering::Acquire) {
            return Ok(());
        }

        // A schema transaction is exclusive with both schema and write
        // transactions. Inspecting the committed export while this handle is
        // retained therefore serializes concurrent bootstrap attempts without
        // a racy preflight-to-install window.
        let mut transaction = self
            .journal_database
            .schema_transaction()
            .await
            .map_err(map_orm_error)?;
        let export = match self.journal_database.schema_text().await {
            Ok(export) => export,
            Err(error) => {
                let primary = map_orm_error(error);
                return Err(rollback_schema_error(
                    &mut transaction,
                    primary,
                    "journal schema export",
                )
                .await);
            }
        };
        let state = match journal_schema_state(&export) {
            Ok(state) => state,
            Err(error) => {
                return Err(rollback_schema_error(
                    &mut transaction,
                    error,
                    "journal schema validation",
                )
                .await);
            }
        };

        match state {
            JournalSchemaState::Exact => {
                if let Err(error) = self.require_journal_owner(&mut transaction).await {
                    return Err(rollback_schema_error(
                        &mut transaction,
                        error,
                        "journal owner validation",
                    )
                    .await);
                }
                transaction.rollback().await.map_err(|error| {
                    schema_cleanup_failure(error, None, "exact journal schema inspection")
                })?;
            }
            JournalSchemaState::Empty => {
                if let Err(error) = transaction.query(JOURNAL_CONTROL_SCHEMA_TYPEQL).await {
                    let primary = map_orm_error(error);
                    return Err(rollback_schema_error(
                        &mut transaction,
                        primary,
                        "journal schema installation",
                    )
                    .await);
                }
                let owner = insert_journal_owner_query(
                    self.managed_database.database_name(),
                    &self.managed_scope_id,
                );
                if let Err(error) = transaction.query(&owner).await {
                    let primary = map_orm_error(error);
                    return Err(rollback_schema_error(
                        &mut transaction,
                        primary,
                        "journal owner installation",
                    )
                    .await);
                }
                if let Err(error) = self.require_journal_owner(&mut transaction).await {
                    return Err(rollback_schema_error(
                        &mut transaction,
                        error,
                        "installed journal owner validation",
                    )
                    .await);
                }
                transaction.commit().await.map_err(map_orm_error)?;

                let installed = self
                    .journal_database
                    .schema_text()
                    .await
                    .map_err(map_orm_error)?;
                if journal_schema_state(&installed)? != JournalSchemaState::Exact {
                    return Err(failure(
                        DiagnosticCategory::Integrity,
                        "migration_typedb_control_schema_mismatch",
                        "installed TypeDB migration journal schema differs from the frozen contract",
                    )
                    .with_detail("control_contract", "journal-control"));
                }
            }
        }

        self.journal_schema_verified.store(true, Ordering::Release);
        Ok(())
    }

    async fn require_journal_owner(&self, transaction: &mut Transaction) -> Result<(), Diagnostic> {
        require_journal_owner(
            transaction,
            self.managed_database.database_name(),
            &self.managed_scope_id,
        )
        .await
    }

    async fn ensure_schema_contract(
        &self,
        database: &Database,
        verified: &AtomicBool,
        schema: &'static str,
        contract: &'static str,
    ) -> Result<(), Diagnostic> {
        if verified.load(Ordering::Acquire) {
            return Ok(());
        }
        let export = database.schema_text().await.map_err(map_orm_error)?;
        if control_schema_matches(&export, schema, contract)? {
            verified.store(true, Ordering::Release);
            return Ok(());
        }

        let mut transaction = database.schema_transaction().await.map_err(map_orm_error)?;
        if let Err(error) = transaction.query(schema).await {
            let primary = map_orm_error(error);
            transaction.rollback().await.map_err(|cleanup| {
                schema_cleanup_failure(cleanup, Some(&primary), "raced control-schema install")
            })?;
            let raced_export = database.schema_text().await.map_err(map_orm_error)?;
            if !control_schema_matches(&raced_export, schema, contract)? {
                return Err(primary);
            }
        } else {
            transaction.commit().await.map_err(map_orm_error)?;
        }

        let installed = database.schema_text().await.map_err(map_orm_error)?;
        if !control_schema_matches(&installed, schema, contract)? {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_control_schema_mismatch",
                "installed TypeDB migration control schema differs from the frozen contract",
            )
            .with_detail("control_contract", contract));
        }
        verified.store(true, Ordering::Release);
        Ok(())
    }

    fn require_plan(&self) -> Result<&VerifiedMigrationApplyPlan, Diagnostic> {
        self.plan.ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_plan_context_missing",
                "open-plan journal access requires an exact verified apply plan",
            )
        })
    }

    fn require_rollback_plan(&self) -> Result<&VerifiedMigrationRollbackPlan, Diagnostic> {
        self.rollback_plan.ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_plan_context_missing",
                "open-plan journal access requires an exact verified rollback plan",
            )
        })
    }

    async fn acquire_inner(
        &self,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<MigrationLease, Diagnostic> {
        self.ensure_schema_for_scope(scope).await?;
        let lease = self.acquire_authoritative(scope, holder).await?;
        self.publish_managed_fence(&lease).await?;
        Ok(lease)
    }

    async fn acquire_authoritative(
        &self,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<MigrationLease, Diagnostic> {
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_control(&mut transaction, scope).await?;
        let (fence, next_sequence, query) = match current {
            Some(current) => {
                let fence = ExecutionFence::new(current.fence)?.checked_successor()?;
                let query = replace_control_query(
                    scope,
                    current.fence,
                    current.next_sequence,
                    holder,
                    fence,
                    current.next_sequence,
                    LEASE_HELD,
                );
                (fence, current.next_sequence, query)
            }
            None => {
                let fence = ExecutionFence::new(1)?;
                (
                    fence,
                    0,
                    insert_control_query(scope, Some(holder), fence, 0, LEASE_HELD),
                )
            }
        };
        transaction.query(&query).await.map_err(map_orm_error)?;
        let lease = MigrationLease::new(scope.clone(), holder.clone(), fence);
        let observed = load_active_control(&mut transaction, &lease).await?;
        if observed.next_sequence != next_sequence {
            let _ = transaction.rollback().await;
            return Err(stale_fence());
        }
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(lease)
    }

    async fn publish_managed_fence(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        let mut transaction = self
            .managed_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_managed_fence(&mut transaction, lease.scope()).await?;
        let query = match current {
            None => insert_managed_fence_query(lease),
            Some(current) if current < lease.fence().get() => {
                replace_managed_fence_query(lease, current)
            }
            Some(_) => {
                let _ = transaction.rollback().await;
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_managed_fence_not_monotonic",
                    "managed fence mirror is not older than the acquired authoritative fence",
                ));
            }
        };
        transaction.query(&query).await.map_err(map_orm_error)?;
        require_active_managed_fence(&mut transaction, lease).await?;
        transaction.commit().await.map_err(map_orm_error)
    }

    async fn release_inner(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        self.ensure_schema_for_scope(lease.scope()).await?;
        self.release_authoritative(lease).await?;
        self.release_managed_fence(lease).await
    }

    async fn release_authoritative(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let query = release_control_query(lease, current.next_sequence);
        transaction.query(&query).await.map_err(map_orm_error)?;
        if !load_free_control(&mut transaction, lease, current.next_sequence).await? {
            let _ = transaction.rollback().await;
            return Err(stale_fence());
        }
        transaction.commit().await.map_err(map_orm_error)
    }

    async fn release_managed_fence(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        let mut transaction = self
            .managed_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        require_active_managed_fence(&mut transaction, lease).await?;
        transaction
            .query(&release_managed_fence_query(lease))
            .await
            .map_err(map_orm_error)?;
        if !load_free_managed_fence(&mut transaction, lease).await? {
            let _ = transaction.rollback().await;
            return Err(stale_fence());
        }
        transaction.commit().await.map_err(map_orm_error)
    }

    async fn begin_plan_inner(
        &self,
        lease: &MigrationLease,
        record: PlanRecord,
    ) -> Result<JournalEntry<PlanRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let plan = self.require_plan()?;
        let expected = expected_plan_record(plan, lease, record.fence())?;
        if record != expected {
            return Err(record_identity_mismatch());
        }
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        if open_plan_exists(&mut transaction, lease.scope()).await? {
            let _ = transaction.rollback().await;
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_open_plan_exists",
                "migration scope already has an open execution plan",
            ));
        }
        let payload = encode_plan(&record)?;
        let sequence =
            append_record(&mut transaction, lease, current, PLAN_RECORD_KIND, &payload).await?;
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    async fn record_event_inner(
        &self,
        lease: &MigrationLease,
        record: GroupEventRecord,
    ) -> Result<JournalEntry<GroupEventRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let plan = self.require_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let open = self
            .load_open_plan_in_transaction(&mut transaction, lease, plan)
            .await?
            .ok_or_else(no_open_plan)?;
        ensure_plan_membership(open.plan().record(), &record)?;
        let payload = encode_event(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            EVENT_RECORD_KIND,
            &payload,
        )
        .await?;
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    async fn record_applied_inner(
        &self,
        lease: &MigrationLease,
        record: AppliedRecord,
    ) -> Result<JournalEntry<AppliedRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let expected_manifest = self.catalog.get(record.migration_id()).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_applied_manifest_unknown",
                "applied record references a manifest absent from verified history",
            )
        })?;
        let expected = AppliedRecord::from_verified_manifest_contract(lease, expected_manifest)?;
        if record != expected {
            return Err(record_identity_mismatch());
        }
        let plan = self.require_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let existing = self
            .load_applied_in_transaction(&mut transaction, lease.scope(), lease.holder())
            .await?;
        if let Some(existing) = existing
            .iter()
            .find(|entry| entry.record().migration_id() == record.migration_id())
        {
            let exact = existing.record() == &record && existing.record().fence() == lease.fence();
            let result = if exact {
                Ok(JournalEntry::from_store(
                    existing.sequence(),
                    existing.record().clone(),
                ))
            } else {
                Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_applied_record_conflict",
                    "applied migration identity already exists with different evidence",
                ))
            };
            let _ = transaction.rollback().await;
            return result;
        }

        let open = self
            .load_open_plan_in_transaction(&mut transaction, lease, plan)
            .await?
            .ok_or_else(no_open_plan)?;
        ensure_applied_membership(open.plan().record(), &record)?;
        let payload = encode_applied(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            APPLIED_RECORD_KIND,
            &payload,
        )
        .await?;

        let mut applied_ids: BTreeSet<MigrationId> = existing
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect();
        applied_ids.insert(record.migration_id().clone());
        // Completion deliberately removes the plan and its transient event
        // journal while retaining applied checkpoints and the monotonic
        // sequence counter. Recovery needs events only while a plan is open.
        if open
            .plan()
            .record()
            .migration_ids()
            .iter()
            .all(|id| applied_ids.contains(id))
        {
            delete_open_plan(&mut transaction, lease.scope()).await?;
        }
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    /// Read the applied ledger without a lease, a write, or a schema install.
    ///
    /// This is the inspection path for read-only commands such as verify:
    /// the frozen control schema is checked against the journal export and
    /// never installed, no control row is consulted, and the whole read is
    /// one snapshot transaction. A journal database without the control
    /// schema has never recorded history through the migration path, so the
    /// load fails closed instead of bootstrapping control state into an
    /// environment the caller promised not to mutate. The result is
    /// reporting-grade data, never execution authority.
    pub async fn load_applied_read_only(
        &self,
        scope: &ExecutionScope,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        self.require_owned_scope(scope)?;
        let export = self
            .journal_database
            .schema_text()
            .await
            .map_err(map_orm_error)?;
        if journal_schema_state(&export)? == JournalSchemaState::Empty {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_journal_control_schema_absent",
                "journal database carries no migration control schema; no \
                 history was recorded through the migration path and a \
                 read-only load will not install one",
            )
            .with_detail(
                "journal_database",
                self.journal_database.database_name().to_owned(),
            ));
        }
        // The persisted wire contract binds scope and fence; the holder on
        // the reconstructed decode lease is inert plumbing, so an
        // inspection-local identity keeps this path honest about never
        // having acquired anything.
        let holder = LeaseHolderId::new("typebridge-read-only-inspection")?;
        let mut transaction = self
            .journal_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        let result = async {
            self.require_journal_owner(&mut transaction).await?;
            self.load_applied_in_transaction(&mut transaction, scope, &holder)
                .await
        }
        .await;
        let close = transaction.close().await.map_err(map_orm_error);
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(records), Ok(())) => Ok(records),
        }
    }

    async fn load_applied_inner(
        &self,
        lease: &MigrationLease,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        load_active_control(&mut transaction, lease).await?;
        let result = self
            .load_applied_in_transaction(&mut transaction, lease.scope(), lease.holder())
            .await;
        let close = transaction.close().await.map_err(map_orm_error);
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(records), Ok(())) => Ok(records),
        }
    }

    async fn load_open_plan_inner(
        &self,
        lease: &MigrationLease,
    ) -> Result<Option<OpenPlanRecord>, Diagnostic> {
        let plan = self.require_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        load_active_control(&mut transaction, lease).await?;
        let result = self
            .load_open_plan_in_transaction(&mut transaction, lease, plan)
            .await;
        let close = transaction.close().await.map_err(map_orm_error);
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(record), Ok(())) => Ok(record),
        }
    }

    async fn load_applied_in_transaction(
        &self,
        transaction: &mut Transaction,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        let raw = self
            .load_applied_rows_in_transaction(transaction, scope, holder)
            .await?;
        let rolled_back = self
            .load_rolled_back_in_transaction(transaction, scope, holder)
            .await?;
        active_applied_entries(raw, &rolled_back)
    }

    async fn load_applied_rows_in_transaction(
        &self,
        transaction: &mut Transaction,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        let rows = load_rows(transaction, scope, Some(APPLIED_RECORD_KIND)).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let fence = persisted_fence(&row.payload, APPLIED_RECORD_KIND)?;
            let historical = MigrationLease::new(scope.clone(), holder.clone(), fence);
            let mut decoded = None;
            for manifest in self.catalog.values() {
                let expected =
                    AppliedRecord::from_verified_manifest_contract(&historical, manifest)?;
                if let Ok(record) = decode_applied(&row.payload, expected)
                    && decoded.replace(record).is_some()
                {
                    return Err(record_identity_mismatch());
                }
            }
            let record = decoded.ok_or_else(record_identity_mismatch)?;
            result.push(JournalEntry::from_store(row.sequence, record));
        }
        Ok(result)
    }

    async fn load_rolled_back_in_transaction(
        &self,
        transaction: &mut Transaction,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<Vec<JournalEntry<RolledBackRecord>>, Diagnostic> {
        let rows = load_rows(transaction, scope, Some(ROLLED_BACK_RECORD_KIND)).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let fence = persisted_fence(&row.payload, ROLLED_BACK_RECORD_KIND)?;
            let historical = MigrationLease::new(scope.clone(), holder.clone(), fence);
            let mut decoded = None;
            for manifest in self.catalog.values() {
                let expected =
                    RolledBackRecord::from_verified_manifest_contract(&historical, manifest)?;
                if let Ok(record) = decode_rolled_back(&row.payload, expected)
                    && decoded.replace(record).is_some()
                {
                    return Err(record_identity_mismatch());
                }
            }
            let record = decoded.ok_or_else(record_identity_mismatch)?;
            result.push(JournalEntry::from_store(row.sequence, record));
        }
        Ok(result)
    }

    async fn load_open_plan_in_transaction(
        &self,
        transaction: &mut Transaction,
        lease: &MigrationLease,
        plan: &VerifiedMigrationApplyPlan,
    ) -> Result<Option<OpenPlanRecord>, Diagnostic> {
        let plan_rows = load_rows(transaction, lease.scope(), Some(PLAN_RECORD_KIND)).await?;
        if plan_rows.is_empty() {
            return Ok(None);
        }
        if plan_rows.len() != 1 {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_multiple_open_plans",
                "migration scope contains more than one open plan record",
            ));
        }
        let row = &plan_rows[0];
        let fence = persisted_fence(&row.payload, PLAN_RECORD_KIND)?;
        let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
        let expected = expected_plan_record(plan, &historical, fence)?;
        let plan_record = decode_plan(&row.payload, expected)?;
        let plan_entry = JournalEntry::from_store(row.sequence, plan_record);

        let event_rows = load_rows(transaction, lease.scope(), Some(EVENT_RECORD_KIND)).await?;
        let mut events = Vec::with_capacity(event_rows.len());
        for event_row in event_rows {
            let record = decode_event_against_plan(&event_row.payload, lease, plan)?;
            events.push(JournalEntry::from_store(event_row.sequence, record));
        }
        OpenPlanRecord::from_store(plan_entry, events).map(Some)
    }

    async fn begin_rollback_plan_inner(
        &self,
        lease: &MigrationLease,
        record: RollbackPlanRecord,
    ) -> Result<JournalEntry<RollbackPlanRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let plan = self.require_rollback_plan()?;
        let expected = expected_rollback_plan_record(plan, lease, record.fence())?;
        if record != expected {
            return Err(record_identity_mismatch());
        }
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        if open_plan_exists(&mut transaction, lease.scope()).await? {
            let _ = transaction.rollback().await;
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_open_plan_exists",
                "migration scope already has an open execution plan",
            ));
        }
        let payload = encode_rollback_plan(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            ROLLBACK_PLAN_RECORD_KIND,
            &payload,
        )
        .await?;
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    async fn record_rollback_event_inner(
        &self,
        lease: &MigrationLease,
        record: RollbackStepEventRecord,
    ) -> Result<JournalEntry<RollbackStepEventRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let plan = self.require_rollback_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let open = self
            .load_open_rollback_plan_in_transaction(&mut transaction, lease, plan)
            .await?
            .ok_or_else(no_open_plan)?;
        let member = open
            .plan()
            .record()
            .rollback_ids()
            .iter()
            .zip(open.plan().record().manifest_digests())
            .any(|(id, digest)| id == record.migration_id() && *digest == record.manifest_digest());
        if !member {
            let _ = transaction.rollback().await;
            return Err(record_identity_mismatch());
        }
        let payload = encode_rollback_event(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            ROLLBACK_EVENT_RECORD_KIND,
            &payload,
        )
        .await?;
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    async fn record_rolled_back_inner(
        &self,
        lease: &MigrationLease,
        record: RolledBackRecord,
    ) -> Result<JournalEntry<RolledBackRecord>, Diagnostic> {
        ensure_record_lease(lease, record.scope(), record.fence())?;
        let expected_manifest = self.catalog.get(record.migration_id()).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_applied_manifest_unknown",
                "retirement record references a manifest absent from verified history",
            )
        })?;
        let expected = RolledBackRecord::from_verified_manifest_contract(lease, expected_manifest)?;
        if record != expected {
            return Err(record_identity_mismatch());
        }
        let plan = self.require_rollback_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .write_transaction()
            .await
            .map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let raw = self
            .load_applied_rows_in_transaction(&mut transaction, lease.scope(), lease.holder())
            .await?;
        let rolled_back = self
            .load_rolled_back_in_transaction(&mut transaction, lease.scope(), lease.holder())
            .await?;
        let active = active_applied_entries(raw, &rolled_back)?;
        let is_active = active.iter().any(|entry| {
            entry.record().migration_id() == record.migration_id()
                && entry.record().manifest_digest() == record.manifest_digest()
        });
        if !is_active {
            let result = if let Some(existing) = rolled_back
                .iter()
                .find(|entry| entry.record() == &record && entry.record().fence() == lease.fence())
            {
                Ok(existing.clone())
            } else {
                Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_retirement_conflict",
                    "retirement target is not active in the applied ledger",
                ))
            };
            let _ = transaction.rollback().await;
            return result;
        }

        let open = self
            .load_open_rollback_plan_in_transaction(&mut transaction, lease, plan)
            .await?
            .ok_or_else(no_open_plan)?;
        let manifest_index = open
            .plan()
            .record()
            .rollback_ids()
            .iter()
            .position(|id| id == record.migration_id());
        if manifest_index.is_none_or(|index| {
            open.plan().record().manifest_digests().get(index) != Some(&record.manifest_digest())
        }) {
            let _ = transaction.rollback().await;
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_foreign_retirement",
                "retirement identity and digest are absent from the open rollback plan",
            ));
        }
        let payload = encode_rolled_back(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            ROLLED_BACK_RECORD_KIND,
            &payload,
        )
        .await?;

        // Completion removes the rollback plan and its transient event
        // journal while retaining both the applied history and the
        // retirement records themselves.
        let mut remaining_active: BTreeSet<MigrationId> = active
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect();
        remaining_active.remove(record.migration_id());
        if open
            .plan()
            .record()
            .rollback_ids()
            .iter()
            .all(|id| !remaining_active.contains(id))
        {
            delete_open_rollback_plan(&mut transaction, lease.scope()).await?;
        }
        transaction.commit().await.map_err(map_orm_error)?;
        Ok(JournalEntry::from_store(sequence, record))
    }

    async fn load_rolled_back_inner(
        &self,
        lease: &MigrationLease,
    ) -> Result<Vec<JournalEntry<RolledBackRecord>>, Diagnostic> {
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        load_active_control(&mut transaction, lease).await?;
        let result = self
            .load_rolled_back_in_transaction(&mut transaction, lease.scope(), lease.holder())
            .await;
        let close = transaction.close().await.map_err(map_orm_error);
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(records), Ok(())) => Ok(records),
        }
    }

    async fn load_open_rollback_plan_inner(
        &self,
        lease: &MigrationLease,
    ) -> Result<Option<OpenRollbackPlanRecord>, Diagnostic> {
        let plan = self.require_rollback_plan()?;
        self.ensure_schema_for_scope(lease.scope()).await?;
        let mut transaction = self
            .journal_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        load_active_control(&mut transaction, lease).await?;
        let result = self
            .load_open_rollback_plan_in_transaction(&mut transaction, lease, plan)
            .await;
        let close = transaction.close().await.map_err(map_orm_error);
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(record), Ok(())) => Ok(record),
        }
    }

    async fn load_open_rollback_plan_in_transaction(
        &self,
        transaction: &mut Transaction,
        lease: &MigrationLease,
        plan: &VerifiedMigrationRollbackPlan,
    ) -> Result<Option<OpenRollbackPlanRecord>, Diagnostic> {
        let plan_rows =
            load_rows(transaction, lease.scope(), Some(ROLLBACK_PLAN_RECORD_KIND)).await?;
        if plan_rows.is_empty() {
            return Ok(None);
        }
        if plan_rows.len() != 1 {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_multiple_open_plans",
                "migration scope contains more than one open rollback plan record",
            ));
        }
        let row = &plan_rows[0];
        let fence = persisted_fence(&row.payload, ROLLBACK_PLAN_RECORD_KIND)?;
        let expected = expected_rollback_plan_record(plan, lease, fence)?;
        let plan_record = decode_rollback_plan(&row.payload, expected)?;
        let plan_entry = JournalEntry::from_store(row.sequence, plan_record);

        let event_rows =
            load_rows(transaction, lease.scope(), Some(ROLLBACK_EVENT_RECORD_KIND)).await?;
        let mut events = Vec::with_capacity(event_rows.len());
        for event_row in event_rows {
            let record = decode_rollback_event_against_plan(&event_row.payload, lease, plan)?;
            events.push(JournalEntry::from_store(event_row.sequence, record));
        }
        OpenRollbackPlanRecord::from_store(plan_entry, events).map(Some)
    }
}

impl MigrationLeaseStore for TypeDbMigrationStore<'_> {
    fn acquire<'a>(
        &'a self,
        scope: &'a ExecutionScope,
        holder: &'a LeaseHolderId,
    ) -> ExecutionFuture<'a, MigrationLease> {
        Box::pin(async move { self.acquire_inner(scope, holder).await })
    }

    fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()> {
        Box::pin(async move { self.release_inner(lease).await })
    }
}

impl MigrationExecutionJournal for TypeDbMigrationStore<'_> {
    fn begin_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: PlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>> {
        Box::pin(async move { self.begin_plan_inner(lease, record).await })
    }

    fn record_group_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: GroupEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>> {
        Box::pin(async move { self.record_event_inner(lease, record).await })
    }

    fn record_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: AppliedRecord,
    ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>> {
        Box::pin(async move { self.record_applied_inner(lease, record).await })
    }

    fn load_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>> {
        Box::pin(async move { self.load_applied_inner(lease).await })
    }

    fn load_open_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenPlanRecord>> {
        Box::pin(async move { self.load_open_plan_inner(lease).await })
    }

    fn begin_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackPlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackPlanRecord>> {
        Box::pin(async move { self.begin_rollback_plan_inner(lease, record).await })
    }

    fn record_rollback_step_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackStepEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackStepEventRecord>> {
        Box::pin(async move { self.record_rollback_event_inner(lease, record).await })
    }

    fn record_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RolledBackRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RolledBackRecord>> {
        Box::pin(async move { self.record_rolled_back_inner(lease, record).await })
    }

    fn load_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<RolledBackRecord>>> {
        Box::pin(async move { self.load_rolled_back_inner(lease).await })
    }

    fn load_open_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenRollbackPlanRecord>> {
        Box::pin(async move { self.load_open_rollback_plan_inner(lease).await })
    }
}

#[derive(Clone, Copy)]
struct ControlSnapshot {
    fence: u64,
    next_sequence: u64,
}

#[derive(Debug)]
struct ManagedControlSnapshot {
    fence: u64,
    state: String,
}

struct StoredRow {
    key: String,
    kind: String,
    payload: Vec<u8>,
    sequence: JournalSequence,
}

/// Require an exact active fence mirror inside a managed-database transaction.
///
/// Future prepared migration providers must call this through the same schema
/// transaction that executes statements and commits. An out-of-transaction
/// check is advisory and does not satisfy the fencing contract.
pub async fn require_active_managed_fence(
    transaction: &mut Transaction,
    lease: &MigrationLease,
) -> Result<(), Diagnostic> {
    let snapshot = load_global_managed_control(transaction, lease.scope())
        .await?
        .ok_or_else(stale_fence)?;
    if snapshot.fence != lease.fence().get() || snapshot.state != LEASE_HELD {
        return Err(stale_fence());
    }
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}; fetch {{ \"exists\": true }};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
    );
    if query_documents(transaction, &query).await?.len() == 1 {
        Ok(())
    } else {
        Err(stale_fence())
    }
}

async fn load_managed_fence(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<Option<u64>, Diagnostic> {
    Ok(load_global_managed_control(transaction, scope)
        .await?
        .map(|snapshot| snapshot.fence))
}

async fn load_free_managed_fence(
    transaction: &mut Transaction,
    lease: &MigrationLease,
) -> Result<bool, Diagnostic> {
    let snapshot = load_global_managed_control(transaction, lease.scope()).await?;
    if snapshot.as_ref().is_none_or(|snapshot| {
        snapshot.fence != lease.fence().get() || snapshot.state != LEASE_FREE
    }) {
        return Ok(false);
    }
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}; fetch {{ \"exists\": true }};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_FREE),
    );
    Ok(query_documents(transaction, &query).await?.len() == 1)
}

async fn load_global_managed_control(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<Option<ManagedControlSnapshot>, Diagnostic> {
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} $scope, has {LEASE_FENCE} $fence, has {LEASE_STATE} $state; fetch {{ \"scope\": $scope, \"fence\": $fence, \"state\": $state }};"
    );
    parse_managed_control_documents(
        query_documents(transaction, &query).await?,
        scope.managed_scope_id(),
    )
}

fn parse_managed_control_documents(
    documents: Vec<Value>,
    expected_scope: &ManagedScopeId,
) -> Result<Option<ManagedControlSnapshot>, Diagnostic> {
    if documents.is_empty() {
        return Ok(None);
    }
    if documents.len() != 1 {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_duplicate_managed_fence",
            "managed database has more than one fence mirror row",
        ));
    }
    let document = &documents[0];
    if required_scalar(document, "scope")? != expected_scope.as_str() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_foreign_managed_scope",
            "managed database is already bound to a different migration scope",
        ));
    }
    let state = required_scalar(document, "state")?;
    if state != LEASE_HELD && state != LEASE_FREE {
        return Err(malformed_provider_row("state"));
    }
    Ok(Some(ManagedControlSnapshot {
        fence: canonical_u64(&required_scalar(document, "fence")?)?,
        state,
    }))
}

async fn load_control(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<Option<ControlSnapshot>, Diagnostic> {
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} $fence, has {NEXT_SEQUENCE} $next; fetch {{ \"fence\": $fence, \"next\": $next }};",
        literal(scope.managed_scope_id().as_str())
    );
    parse_control_documents(query_documents(transaction, &query).await?)
}

async fn load_active_control(
    transaction: &mut Transaction,
    lease: &MigrationLease,
) -> Result<ControlSnapshot, Diagnostic> {
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} $next; fetch {{ \"next\": $next }};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
    );
    let documents = query_documents(transaction, &query).await?;
    if documents.len() != 1 {
        return Err(stale_fence());
    }
    Ok(ControlSnapshot {
        fence: lease.fence().get(),
        next_sequence: canonical_u64(&required_scalar(&documents[0], "next")?)?,
    })
}

async fn load_free_control(
    transaction: &mut Transaction,
    lease: &MigrationLease,
    next_sequence: u64,
) -> Result<bool, Diagnostic> {
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {}; fetch {{ \"exists\": true }};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_FREE),
        literal(&next_sequence.to_string()),
    );
    Ok(query_documents(transaction, &query).await?.len() == 1)
}

fn parse_control_documents(documents: Vec<Value>) -> Result<Option<ControlSnapshot>, Diagnostic> {
    if documents.is_empty() {
        return Ok(None);
    }
    if documents.len() != 1 {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_duplicate_control_row",
            "migration scope has more than one control row",
        ));
    }
    Ok(Some(ControlSnapshot {
        fence: canonical_u64(&required_scalar(&documents[0], "fence")?)?,
        next_sequence: canonical_u64(&required_scalar(&documents[0], "next")?)?,
    }))
}

async fn append_record(
    transaction: &mut Transaction,
    lease: &MigrationLease,
    current: ControlSnapshot,
    kind: &str,
    payload: &[u8],
) -> Result<JournalSequence, Diagnostic> {
    let next = current.next_sequence.checked_add(1).ok_or_else(|| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "migration_typedb_sequence_overflow",
            "migration journal sequence exhausted the u64 domain",
        )
    })?;
    let sequence = JournalSequence::new(next)?;
    let key = format!(
        "{}#{}",
        lease.scope().managed_scope_id().as_str(),
        sequence.get()
    );
    let payload = std::str::from_utf8(payload).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_not_utf8",
            "canonical migration record bytes are not UTF-8",
        )
    })?;
    let digest = payload_digest(payload.as_bytes());
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {}; delete $control; insert $next-control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {}; $record isa {JOURNAL_ENTITY}, has {RECORD_KEY} {}, has {CONTROL_SCOPE} {}, has {RECORD_SEQUENCE} {}, has {RECORD_KIND} {}, has {RECORD_PAYLOAD} {}, has {RECORD_PAYLOAD_DIGEST} {};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
        literal(&current.next_sequence.to_string()),
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
        literal(&next.to_string()),
        literal(&key),
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&next.to_string()),
        literal(kind),
        literal(payload),
        literal(&digest),
    );
    transaction.query(&query).await.map_err(map_orm_error)?;
    let observed = load_active_control(transaction, lease).await?;
    if observed.next_sequence != next {
        return Err(stale_fence());
    }
    let inserted = load_row_by_key(transaction, &key).await?;
    if inserted.kind != kind
        || inserted.payload != payload.as_bytes()
        || inserted.sequence != sequence
    {
        return Err(record_identity_mismatch());
    }
    Ok(sequence)
}

async fn load_rows(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
    kind: Option<&str>,
) -> Result<Vec<StoredRow>, Diagnostic> {
    let kind = kind
        .map(|kind| format!(", has {RECORD_KIND} {}", literal(kind)))
        .unwrap_or_default();
    let query = format!(
        "match $record isa {JOURNAL_ENTITY}, has {CONTROL_SCOPE} {}{kind}, has {RECORD_KEY} $key, has {RECORD_SEQUENCE} $sequence, has {RECORD_KIND} $kind, has {RECORD_PAYLOAD} $payload, has {RECORD_PAYLOAD_DIGEST} $digest; fetch {{ \"key\": $key, \"sequence\": $sequence, \"kind\": $kind, \"payload\": $payload, \"digest\": $digest }};",
        literal(scope.managed_scope_id().as_str()),
    );
    let mut rows = query_documents(transaction, &query)
        .await?
        .into_iter()
        .map(parse_stored_row)
        .collect::<Result<Vec<_>, _>>()?;
    rows.sort_by_key(|row| row.sequence);
    for pair in rows.windows(2) {
        if pair[0].sequence == pair[1].sequence || pair[0].key == pair[1].key {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_duplicate_journal_identity",
                "migration journal contains duplicate sequence or record identities",
            ));
        }
    }
    Ok(rows)
}

async fn load_row_by_key(
    transaction: &mut Transaction,
    key: &str,
) -> Result<StoredRow, Diagnostic> {
    let query = format!(
        "match $record isa {JOURNAL_ENTITY}, has {RECORD_KEY} {}, has {RECORD_SEQUENCE} $sequence, has {RECORD_KIND} $kind, has {RECORD_PAYLOAD} $payload, has {RECORD_PAYLOAD_DIGEST} $digest; fetch {{ \"key\": {}, \"sequence\": $sequence, \"kind\": $kind, \"payload\": $payload, \"digest\": $digest }};",
        literal(key),
        literal(key),
    );
    let documents = query_documents(transaction, &query).await?;
    if documents.len() != 1 {
        return Err(record_identity_mismatch());
    }
    parse_stored_row(documents.into_iter().next().expect("length checked"))
}

fn parse_stored_row(document: Value) -> Result<StoredRow, Diagnostic> {
    let key = required_scalar(&document, "key")?;
    let kind = required_scalar(&document, "kind")?;
    if !matches!(
        kind.as_str(),
        PLAN_RECORD_KIND
            | EVENT_RECORD_KIND
            | APPLIED_RECORD_KIND
            | ROLLBACK_PLAN_RECORD_KIND
            | ROLLBACK_EVENT_RECORD_KIND
            | ROLLED_BACK_RECORD_KIND
    ) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_kind_unknown",
            "migration journal row has an unknown record kind",
        ));
    }
    let payload = required_scalar(&document, "payload")?.into_bytes();
    let digest = required_scalar(&document, "digest")?;
    if payload_digest(&payload) != digest {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_payload_digest_mismatch",
            "migration journal payload digest does not match its bytes",
        ));
    }
    Ok(StoredRow {
        key,
        kind,
        payload,
        sequence: JournalSequence::new(canonical_u64(&required_scalar(&document, "sequence")?)?)?,
    })
}

async fn query_documents(
    transaction: &mut Transaction,
    query: &str,
) -> Result<Vec<Value>, Diagnostic> {
    match transaction.query(query).await.map_err(map_orm_error)? {
        QueryResult::Documents(documents) | QueryResult::Rows(documents) => Ok(documents),
        QueryResult::Ok => Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_query_returned_no_documents",
            "TypeDB control query returned no document result",
        )),
    }
}

fn required_scalar(document: &Value, key: &str) -> Result<String, Diagnostic> {
    let value = document
        .get(key)
        .ok_or_else(|| malformed_provider_row(key))?;
    scalar(value).ok_or_else(|| malformed_provider_row(key))
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(value) => value.get("value").and_then(scalar),
        Value::Null | Value::Array(_) => None,
    }
}

fn canonical_u64(value: &str) -> Result<u64, Diagnostic> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| malformed_provider_row("u64"))?;
    if parsed.to_string() != value {
        return Err(malformed_provider_row("u64"));
    }
    Ok(parsed)
}

fn insert_control_query(
    scope: &ExecutionScope,
    holder: Option<&LeaseHolderId>,
    fence: ExecutionFence,
    next_sequence: u64,
    state: &str,
) -> String {
    let holder = holder
        .map(|holder| format!(", has {LEASE_HOLDER} {}", literal(holder.as_str())))
        .unwrap_or_default();
    format!(
        "insert $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {}{holder};",
        literal(scope.managed_scope_id().as_str()),
        literal(&fence.get().to_string()),
        literal(state),
        literal(&next_sequence.to_string()),
    )
}

fn insert_managed_fence_query(lease: &MigrationLease) -> String {
    format!(
        "insert $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
    )
}

fn replace_managed_fence_query(lease: &MigrationLease, old_fence: u64) -> String {
    format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}; delete $control; insert $next-control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&old_fence.to_string()),
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
    )
}

fn release_managed_fence_query(lease: &MigrationLease) -> String {
    format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}; delete $control; insert $free isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_FREE),
    )
}

fn replace_control_query(
    scope: &ExecutionScope,
    old_fence: u64,
    old_next_sequence: u64,
    holder: &LeaseHolderId,
    fence: ExecutionFence,
    next_sequence: u64,
    state: &str,
) -> String {
    format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {NEXT_SEQUENCE} {}; delete $control; insert $next-control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {};",
        literal(scope.managed_scope_id().as_str()),
        literal(&old_fence.to_string()),
        literal(&old_next_sequence.to_string()),
        literal(scope.managed_scope_id().as_str()),
        literal(holder.as_str()),
        literal(&fence.get().to_string()),
        literal(state),
        literal(&next_sequence.to_string()),
    )
}

fn release_control_query(lease: &MigrationLease, next_sequence: u64) -> String {
    format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_HOLDER} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {}; delete $control; insert $free isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}, has {NEXT_SEQUENCE} {};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(lease.holder().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_HELD),
        literal(&next_sequence.to_string()),
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_FREE),
        literal(&next_sequence.to_string()),
    )
}

async fn delete_open_plan(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<(), Diagnostic> {
    for kind in [PLAN_RECORD_KIND, EVENT_RECORD_KIND] {
        let query = format!(
            "match $record isa {JOURNAL_ENTITY}, has {CONTROL_SCOPE} {}, has {RECORD_KIND} {}; delete $record;",
            literal(scope.managed_scope_id().as_str()),
            literal(kind),
        );
        transaction.query(&query).await.map_err(map_orm_error)?;
    }
    Ok(())
}

async fn delete_open_rollback_plan(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<(), Diagnostic> {
    for kind in [ROLLBACK_PLAN_RECORD_KIND, ROLLBACK_EVENT_RECORD_KIND] {
        let query = format!(
            "match $record isa {JOURNAL_ENTITY}, has {CONTROL_SCOPE} {}, has {RECORD_KIND} {}; delete $record;",
            literal(scope.managed_scope_id().as_str()),
            literal(kind),
        );
        transaction.query(&query).await.map_err(map_orm_error)?;
    }
    Ok(())
}

async fn open_plan_exists(
    transaction: &mut Transaction,
    scope: &ExecutionScope,
) -> Result<bool, Diagnostic> {
    for kind in [PLAN_RECORD_KIND, ROLLBACK_PLAN_RECORD_KIND] {
        if !load_rows(transaction, scope, Some(kind)).await?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn expected_plan_record(
    plan: &VerifiedMigrationApplyPlan,
    lease: &MigrationLease,
    fence: ExecutionFence,
) -> Result<PlanRecord, Diagnostic> {
    let source = plan.source_state().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_empty_plan_unsupported",
            "persistent execution store requires a non-empty apply plan",
        )
    })?;
    let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
    PlanRecord::from_verified_plan(&historical, plan, plan.applied_migrations(), source)
}

fn expected_rollback_plan_record(
    plan: &VerifiedMigrationRollbackPlan,
    lease: &MigrationLease,
    fence: ExecutionFence,
) -> Result<RollbackPlanRecord, Diagnostic> {
    let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
    let basis: Vec<MigrationId> = plan.applied_basis().into_iter().collect();
    RollbackPlanRecord::from_verified_rollback_plan(&historical, plan, &basis, plan.source_state())
}

fn decode_rollback_event_against_plan(
    bytes: &[u8],
    lease: &MigrationLease,
    plan: &VerifiedMigrationRollbackPlan,
) -> Result<RollbackStepEventRecord, Diagnostic> {
    let fence = persisted_fence(bytes, ROLLBACK_EVENT_RECORD_KIND)?;
    let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
    let mut matched = None;
    for rollback in plan.rollbacks() {
        for (step_index, step) in rollback.steps().iter().enumerate() {
            let Ok(reverse) = rollback.reverse_delta(step) else {
                continue;
            };
            for kind in [
                GroupJournalEventKind::BeforeCommit,
                GroupJournalEventKind::Committed,
                GroupJournalEventKind::CommitOutcomeUnknown,
                GroupJournalEventKind::DefinitelyAborted,
                GroupJournalEventKind::FormalOnlyAdvanced,
            ] {
                let observed = (kind == GroupJournalEventKind::Committed)
                    .then(|| reverse.target().managed_semantic_schema().clone());
                let Ok(candidate) =
                    RollbackStepEventRecord::new(&historical, rollback, step_index, kind, observed)
                else {
                    continue;
                };
                if let Ok(record) = decode_rollback_event(bytes, candidate)
                    && matched.replace(record).is_some()
                {
                    return Err(record_identity_mismatch());
                }
            }
        }
    }
    matched.ok_or_else(record_identity_mismatch)
}

fn decode_event_against_plan(
    bytes: &[u8],
    lease: &MigrationLease,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<GroupEventRecord, Diagnostic> {
    let fence = persisted_fence(bytes, EVENT_RECORD_KIND)?;
    let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
    let mut matched = None;
    for migration in plan.migrations() {
        for group in migration.transaction_groups() {
            for kind in [
                GroupJournalEventKind::BeforeCommit,
                GroupJournalEventKind::Committed,
                GroupJournalEventKind::CommitOutcomeUnknown,
                GroupJournalEventKind::DefinitelyAborted,
                GroupJournalEventKind::FormalOnlyAdvanced,
            ] {
                let observed = if kind == GroupJournalEventKind::Committed {
                    Some(group_target(migration, group).clone())
                } else {
                    None
                };
                let Ok(candidate) =
                    GroupEventRecord::new(&historical, migration, group, kind, observed)
                else {
                    continue;
                };
                if let Ok(record) = decode_event(bytes, candidate)
                    && matched.replace(record).is_some()
                {
                    return Err(record_identity_mismatch());
                }
            }
        }
    }
    matched.ok_or_else(record_identity_mismatch)
}

fn group_target<'a>(
    migration: &'a type_bridge_schema_migration::VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
) -> &'a type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint {
    migration
        .steps()
        .get(group.schema_delta_step_index())
        .and_then(|step| step.step().as_schema_delta())
        .expect("verified group terminates in one schema-delta step")
        .delta()
        .target()
        .managed_semantic_schema()
}

fn ensure_record_lease(
    lease: &MigrationLease,
    scope: &ExecutionScope,
    fence: ExecutionFence,
) -> Result<(), Diagnostic> {
    if lease.scope() == scope && lease.fence() == fence {
        Ok(())
    } else {
        Err(stale_fence())
    }
}

fn ensure_plan_membership(plan: &PlanRecord, event: &GroupEventRecord) -> Result<(), Diagnostic> {
    let found = plan
        .migration_ids()
        .iter()
        .zip(plan.manifest_digests())
        .any(|(id, digest)| id == event.migration_id() && *digest == event.manifest_digest());
    if found {
        Ok(())
    } else {
        Err(record_identity_mismatch())
    }
}

fn ensure_applied_membership(plan: &PlanRecord, applied: &AppliedRecord) -> Result<(), Diagnostic> {
    let found = plan
        .migration_ids()
        .iter()
        .zip(plan.manifest_digests())
        .any(|(id, digest)| id == applied.migration_id() && *digest == applied.manifest_digest());
    if found {
        Ok(())
    } else {
        Err(record_identity_mismatch())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JournalSchemaState {
    Empty,
    Exact,
}

fn journal_schema_state(export: &str) -> Result<JournalSchemaState, Diagnostic> {
    if export.trim().is_empty() {
        return Ok(JournalSchemaState::Empty);
    }
    let document = DocumentId::new("typebridge-journal-control-provider-export.typeql")?;
    let partitioned = partition_typeql_export_lossless(document, export)?;
    if partitioned.full().facts().next().is_none() {
        return Ok(JournalSchemaState::Empty);
    }
    if partitioned.user().facts().next().is_some()
        || partitioned.legacy_control().facts().next().is_some()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_journal_database_not_exclusive",
            "journal database contains non-journal schema and will not be claimed or modified",
        ));
    }
    let expected_document = DocumentId::new("typebridge-journal-control-schema.typeql")?;
    let expected =
        typeql_to_declared(expected_document, JOURNAL_CONTROL_SCHEMA_TYPEQL).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_frozen_schema_invalid",
                "frozen TypeDB control schema cannot be normalized",
            )
        })?;
    if partitioned.internal().declared_identity_fingerprint()
        != expected.declared_identity_fingerprint()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_control_schema_mismatch",
            "reserved TypeDB migration control schema differs from the frozen contract",
        )
        .with_detail("control_contract", "journal-control"));
    }
    Ok(JournalSchemaState::Exact)
}

fn insert_journal_owner_query(
    managed_database_name: &str,
    managed_scope_id: &ManagedScopeId,
) -> String {
    format!(
        "insert $owner isa {JOURNAL_OWNER_ENTITY}, has {JOURNAL_OWNER_KEY} {}, has {JOURNAL_OWNER_MANAGED_DATABASE} {}, has {JOURNAL_OWNER_MANAGED_SCOPE} {};",
        literal(JOURNAL_OWNER_SINGLETON_KEY),
        literal(managed_database_name),
        literal(managed_scope_id.as_str()),
    )
}

async fn require_journal_owner(
    transaction: &mut Transaction,
    managed_database_name: &str,
    managed_scope_id: &ManagedScopeId,
) -> Result<(), Diagnostic> {
    let query = format!(
        "match $owner isa {JOURNAL_OWNER_ENTITY}, has {JOURNAL_OWNER_KEY} $key, has {JOURNAL_OWNER_MANAGED_DATABASE} $database, has {JOURNAL_OWNER_MANAGED_SCOPE} $scope; fetch {{ \"key\": $key, \"database\": $database, \"scope\": $scope }};"
    );
    let documents = query_documents(transaction, &query).await?;
    let exact = documents.len() == 1
        && required_scalar(&documents[0], "key")? == JOURNAL_OWNER_SINGLETON_KEY
        && required_scalar(&documents[0], "database")? == managed_database_name
        && required_scalar(&documents[0], "scope")? == managed_scope_id.as_str();
    if exact {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_journal_owner_mismatch",
            "journal database owner identity is missing, duplicated, or bound to another managed database or scope",
        ))
    }
}

fn control_schema_matches(
    export: &str,
    expected_schema: &str,
    contract: &'static str,
) -> Result<bool, Diagnostic> {
    if !export.contains(crate::TYPEBRIDGE_INTERNAL_PREFIX) {
        return Ok(false);
    }
    let document = DocumentId::new(format!("typebridge-{contract}-provider-export.typeql"))?;
    let partitioned = partition_typeql_export(document, export)?;
    let expected_document = DocumentId::new(format!("typebridge-{contract}-schema.typeql"))?;
    let expected = typeql_to_declared(expected_document, expected_schema).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_frozen_schema_invalid",
            "frozen TypeDB control schema cannot be normalized",
        )
    })?;
    if partitioned.internal().declared_identity_fingerprint()
        != expected.declared_identity_fingerprint()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_control_schema_mismatch",
            "reserved TypeDB migration control schema differs from the frozen contract",
        )
        .with_detail("control_contract", contract));
    }
    Ok(true)
}

fn literal(value: &str) -> String {
    serde_json::to_string(value).expect("UTF-8 strings are JSON representable")
}

fn payload_digest(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn map_orm_error(error: OrmError) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_provider_error",
        "TypeDB migration execution store operation failed",
    )
    .with_detail("provider", error.to_string())
}

async fn rollback_schema_error(
    transaction: &mut Transaction,
    primary: Diagnostic,
    operation: &'static str,
) -> Diagnostic {
    match transaction.rollback().await {
        Ok(()) => primary,
        Err(cleanup) => schema_cleanup_failure(cleanup, Some(&primary), operation),
    }
}

fn schema_cleanup_failure(
    cleanup: OrmError,
    primary: Option<&Diagnostic>,
    operation: &'static str,
) -> Diagnostic {
    let mut diagnostic = failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_schema_guard_cleanup_uncertain",
        "TypeDB schema transaction termination was not acknowledged",
    )
    .with_detail("operation", operation)
    .with_detail("cleanup", cleanup.to_string());
    if let Some(primary) = primary {
        diagnostic = diagnostic
            .with_detail("primary_code", primary.code().as_str().to_owned())
            .with_detail("primary", primary.to_string());
    }
    diagnostic
}

fn stale_fence() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_execution_stale_fence",
        "migration execution lease is not the current active holder and fence",
    )
}

fn no_open_plan() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_open_plan_missing",
        "migration journal write requires one open execution plan",
    )
}

fn record_identity_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_record_identity_mismatch",
        "persisted migration record does not match independently verified execution evidence",
    )
}

fn malformed_provider_row(field: &str) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_row_malformed",
        "TypeDB migration control row is missing a canonical scalar field",
    )
    .with_detail("field", field.to_owned())
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
    use type_bridge_orm::{DatabaseConnectionAuthority, OrmError};

    use super::*;

    struct NoIoBackend {
        calls: Arc<AtomicUsize>,
    }

    impl DriverBackend for NoIoBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn is_open(&self) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            true
        }

        fn close_connection(&self) -> Result<(), OrmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(OrmError::Connection("unexpected backend I/O".to_owned()))
        }

        fn database_exists(&self, _database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn create_database(&self, _database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn delete_database(&self, _database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }
    }

    fn no_io_database(
        name: String,
        calls: Arc<AtomicUsize>,
        authority: Option<DatabaseConnectionAuthority>,
    ) -> Arc<Database> {
        let backend = Box::new(NoIoBackend { calls });
        Arc::new(match authority {
            Some(authority) => Database::with_backend_authority(backend, name, authority),
            None => Database::with_backend(backend, name),
        })
    }

    #[test]
    fn database_authority_mismatch_rejects_before_backend_io_without_identity_leakage() {
        const SENTINEL: &str = "TB_PROVIDER_SECRET_89ab";
        let calls = Arc::new(AtomicUsize::new(0));
        let managed_name = format!("managed-{SENTINEL}");
        let journal_name = derived_journal_database_name(&managed_name);
        let managed = no_io_database(managed_name, Arc::clone(&calls), None);
        let journal = no_io_database(journal_name, Arc::clone(&calls), None);
        let scope = ManagedScopeId::new("authority-test-scope").unwrap();
        let catalog =
            VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
                .unwrap();

        let result = TypeDbMigrationStore::new(managed, journal, scope, catalog);
        let Err(error) = result else {
            panic!("independent custom backend authorities must reject");
        };
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_database_authority_mismatch"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let rendered = format!("{error}\n{error:?}");
        assert!(!rendered.contains(SENTINEL), "{rendered}");
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn explicitly_shared_custom_backend_authority_constructs_one_pair() {
        let calls = Arc::new(AtomicUsize::new(0));
        let authority = DatabaseConnectionAuthority::isolated();
        let managed = no_io_database(
            "managed".to_owned(),
            Arc::clone(&calls),
            Some(authority.clone()),
        );
        let journal = no_io_database(
            derived_journal_database_name("managed"),
            Arc::clone(&calls),
            Some(authority),
        );
        let scope = ManagedScopeId::new("authority-test-scope").unwrap();
        let catalog =
            VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
                .unwrap();

        assert!(TypeDbMigrationStore::new(managed, journal, scope, catalog).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn journal_schema_contract_accepts_only_empty_or_exact_exclusive_schema() {
        assert_eq!(journal_schema_state("").unwrap(), JournalSchemaState::Empty);
        assert_eq!(
            journal_schema_state(JOURNAL_CONTROL_SCHEMA_TYPEQL).unwrap(),
            JournalSchemaState::Exact
        );

        let foreign = journal_schema_state("define entity customer;")
            .expect_err("a user database must not be claimed as a journal");
        assert_eq!(
            foreign.code().as_str(),
            "migration_typedb_journal_database_not_exclusive"
        );

        let mixed = format!("{JOURNAL_CONTROL_SCHEMA_TYPEQL}\nentity customer;");
        let mixed = journal_schema_state(&mixed)
            .expect_err("exact control schema plus user schema is not an exclusive journal");
        assert_eq!(
            mixed.code().as_str(),
            "migration_typedb_journal_database_not_exclusive"
        );

        let partial = journal_schema_state(
            "define attribute typebridge-internal-v2-control-scope, value string;",
        )
        .expect_err("partial reserved schema must fail closed");
        assert_eq!(
            partial.code().as_str(),
            "migration_typedb_control_schema_mismatch"
        );
    }

    #[test]
    fn journal_schema_contract_rejects_every_lossy_compatibility_construct() {
        let payload_ownership = "owns typebridge-internal-v2-record-payload @card(1..1),";
        for (case, replacement) in [
            (
                "ordered distinct ownership",
                "owns typebridge-internal-v2-record-payload[] @card(1..1) @distinct,",
            ),
            (
                "cascade ownership",
                "owns typebridge-internal-v2-record-payload @card(1..1) @cascade,",
            ),
            (
                "subkey ownership",
                "owns typebridge-internal-v2-record-payload @card(1..1) @subkey(journal),",
            ),
        ] {
            let schema = JOURNAL_CONTROL_SCHEMA_TYPEQL.replacen(payload_ownership, replacement, 1);
            let error = journal_schema_state(&schema).expect_err(case);
            assert_eq!(
                error.code().as_str(),
                "migration_typedb_export_invalid",
                "{case}: {error}"
            );
        }

        for (case, definition) in [
            (
                "opaque function",
                "fun typebridge-hidden() -> integer: return { 1 };",
            ),
            (
                "released struct",
                "struct typebridge-hidden, value payload string;",
            ),
        ] {
            let schema = format!("{JOURNAL_CONTROL_SCHEMA_TYPEQL}\n{definition}\n");
            journal_schema_state(&schema).expect_err(case);
        }
    }

    #[test]
    fn journal_owner_insert_is_literal_escaped_and_binds_both_identities() {
        let scope = ManagedScopeId::new("scope\"with-quote").unwrap();
        let query = insert_journal_owner_query("managed\"database", &scope);
        assert!(query.contains(JOURNAL_OWNER_SINGLETON_KEY));
        assert!(query.contains("managed\\\"database"));
        assert!(query.contains("scope\\\"with-quote"));
        assert!(query.contains(JOURNAL_OWNER_MANAGED_DATABASE));
        assert!(query.contains(JOURNAL_OWNER_MANAGED_SCOPE));
    }

    #[test]
    fn managed_control_singleton_rejects_foreign_and_multiple_scopes() {
        let expected = ManagedScopeId::new("expected-scope").expect("scope");
        let exact = serde_json::json!({
            "scope": "expected-scope",
            "fence": "7",
            "state": LEASE_FREE,
        });
        let parsed = parse_managed_control_documents(vec![exact.clone()], &expected)
            .expect("exact managed owner")
            .expect("managed row");
        assert_eq!(parsed.fence, 7);
        assert_eq!(parsed.state, LEASE_FREE);

        let foreign = serde_json::json!({
            "scope": "restored-under-another-scope",
            "fence": "7",
            "state": LEASE_FREE,
        });
        assert_eq!(
            parse_managed_control_documents(vec![foreign], &expected)
                .expect_err("one-sided journal replacement cannot rebind the managed database")
                .code()
                .as_str(),
            "migration_typedb_foreign_managed_scope"
        );
        assert_eq!(
            parse_managed_control_documents(vec![exact.clone(), exact], &expected)
                .expect_err("managed control is a global singleton")
                .code()
                .as_str(),
            "migration_typedb_duplicate_managed_fence"
        );
    }

    #[test]
    fn managed_fence_matching_still_ignores_application_schema() {
        let export = format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\nentity customer;");
        assert!(
            control_schema_matches(&export, MANAGED_FENCE_SCHEMA_TYPEQL, "managed-fence").unwrap()
        );
    }

    #[test]
    fn managed_fence_matching_preserves_released_application_extensions() {
        let export = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n\
             attribute tag, value string;\n\
             attribute name, value string;\n\
             entity customer, owns tag[] @card(0..5) @distinct, \
             owns name @cascade @subkey(primary);"
        );
        assert!(
            control_schema_matches(&export, MANAGED_FENCE_SCHEMA_TYPEQL, "managed-fence").unwrap()
        );
    }
}
