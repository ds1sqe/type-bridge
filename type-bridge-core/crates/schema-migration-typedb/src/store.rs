//! TypeDB implementation of the provider-neutral fenced execution stores.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{Database, OrmError, Transaction};
use type_bridge_schema_compat::typeql_to_declared;
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionFence, ExecutionFuture, ExecutionScope,
    GroupEventRecord, GroupJournalEventKind, JournalEntry, JournalSequence,
    LeaseHolderId, MigrationExecutionJournal, MigrationLease,
    MigrationLeaseStore, OpenPlanRecord, PlanRecord,
    VerifiedMigrationApplyPlan, VerifiedMigrationTransactionGroup,
    VerifiedSchemaMigrationManifest, verified_manifest_digest,
};

use crate::control_schema::{
    APPLIED_RECORD_KIND, CONTROL_ENTITY, CONTROL_SCOPE, EVENT_RECORD_KIND,
    JOURNAL_CONTROL_SCHEMA_TYPEQL, JOURNAL_ENTITY, LEASE_FENCE, LEASE_FREE,
    LEASE_HELD, LEASE_HOLDER, LEASE_STATE, MANAGED_FENCE_SCHEMA_TYPEQL,
    NEXT_SEQUENCE, PLAN_RECORD_KIND, RECORD_KEY, RECORD_KIND, RECORD_PAYLOAD,
    RECORD_PAYLOAD_DIGEST, RECORD_SEQUENCE,
};
use crate::observation::partition_typeql_export;
use crate::wire::{
    decode_applied, decode_event, decode_plan, encode_applied, encode_event,
    encode_plan, persisted_fence,
};

const JOURNAL_DATABASE_SUFFIX: &str = "__tbv2_journal";

/// Derive the one-to-one companion journal database name.
///
/// The managed and journal databases are one recovery unit. Operators must
/// back up, restore, clone, and delete the derived pair together; restoring
/// either member alone is unsupported and fails closed through fence or
/// verified-record identity mismatch.
#[must_use]
pub fn derived_journal_database_name(managed_database_name: &str) -> String {
    format!("{managed_database_name}{JOURNAL_DATABASE_SUFFIX}")
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

    fn values(
        &self,
    ) -> impl ExactSizeIterator<Item = &'a VerifiedSchemaMigrationManifest> + '_ {
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
    catalog: VerifiedMigrationCatalog<'a>,
    plan: Option<&'a VerifiedMigrationApplyPlan>,
    managed_schema_verified: AtomicBool,
    journal_schema_verified: AtomicBool,
}

impl<'a> TypeDbMigrationStore<'a> {
    /// Construct a history-bound store over one exact managed/journal pair.
    pub fn new(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        catalog: VerifiedMigrationCatalog<'a>,
    ) -> Result<Self, Diagnostic> {
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
            .with_detail("managed_database", managed_database.database_name().to_owned())
            .with_detail("expected_journal_database", expected)
            .with_detail("actual_journal_database", journal_database.database_name().to_owned()));
        }
        Ok(Self {
            managed_database,
            journal_database,
            catalog,
            plan: None,
            managed_schema_verified: AtomicBool::new(false),
            journal_schema_verified: AtomicBool::new(false),
        })
    }

    /// Bind the store to the exact apply plan used for open-plan recovery.
    pub fn bind_plan(
        mut self,
        plan: &'a VerifiedMigrationApplyPlan,
    ) -> Result<Self, Diagnostic> {
        for migration in plan.migrations() {
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

    /// Install or verify the frozen control schema.
    pub fn ensure_control_schema(&self) -> ExecutionFuture<'_, ()> {
        Box::pin(async move { self.ensure_schema().await })
    }

    async fn ensure_schema(&self) -> Result<(), Diagnostic> {
        self.ensure_schema_contract(
            &self.managed_database,
            &self.managed_schema_verified,
            MANAGED_FENCE_SCHEMA_TYPEQL,
            "managed-fence",
        )
        .await?;
        self.ensure_schema_contract(
            &self.journal_database,
            &self.journal_schema_verified,
            JOURNAL_CONTROL_SCHEMA_TYPEQL,
            "journal-control",
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
            let _ = transaction.rollback().await;
            let raced_export = database.schema_text().await.map_err(map_orm_error)?;
            if !control_schema_matches(&raced_export, schema, contract)? {
                return Err(map_orm_error(error));
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

    async fn acquire_inner(
        &self,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<MigrationLease, Diagnostic> {
        self.ensure_schema().await?;
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

    async fn publish_managed_fence(
        &self,
        lease: &MigrationLease,
    ) -> Result<(), Diagnostic> {
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
        self.ensure_schema().await?;
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
        self.ensure_schema().await?;
        let mut transaction = self.journal_database.write_transaction().await.map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        if !load_rows(&mut transaction, lease.scope(), Some(PLAN_RECORD_KIND))
            .await?
            .is_empty()
        {
            let _ = transaction.rollback().await;
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_open_plan_exists",
                "migration scope already has an open execution plan",
            ));
        }
        let payload = encode_plan(&record)?;
        let sequence = append_record(
            &mut transaction,
            lease,
            current,
            PLAN_RECORD_KIND,
            &payload,
        )
        .await?;
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
        self.ensure_schema().await?;
        let mut transaction = self.journal_database.write_transaction().await.map_err(map_orm_error)?;
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
        self.ensure_schema().await?;
        let mut transaction = self.journal_database.write_transaction().await.map_err(map_orm_error)?;
        let current = load_active_control(&mut transaction, lease).await?;
        let existing = self
            .load_applied_in_transaction(&mut transaction, lease)
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

    async fn load_applied_inner(
        &self,
        lease: &MigrationLease,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        self.ensure_schema().await?;
        let mut transaction = self.journal_database.read_transaction().await.map_err(map_orm_error)?;
        load_active_control(&mut transaction, lease).await?;
        let result = self
            .load_applied_in_transaction(&mut transaction, lease)
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
        self.ensure_schema().await?;
        let mut transaction = self.journal_database.read_transaction().await.map_err(map_orm_error)?;
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
        lease: &MigrationLease,
    ) -> Result<Vec<JournalEntry<AppliedRecord>>, Diagnostic> {
        let rows = load_rows(transaction, lease.scope(), Some(APPLIED_RECORD_KIND)).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let fence = persisted_fence(&row.payload, APPLIED_RECORD_KIND)?;
            let historical = MigrationLease::new(
                lease.scope().clone(),
                lease.holder().clone(),
                fence,
            );
            let mut decoded = None;
            for manifest in self.catalog.values() {
                let expected =
                    AppliedRecord::from_verified_manifest_contract(&historical, manifest)?;
                if let Ok(record) = decode_applied(&row.payload, expected) {
                    if decoded.replace(record).is_some() {
                        return Err(record_identity_mismatch());
                    }
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
        let historical = MigrationLease::new(
            lease.scope().clone(),
            lease.holder().clone(),
            fence,
        );
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
}

#[derive(Clone, Copy)]
struct ControlSnapshot {
    fence: u64,
    next_sequence: u64,
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
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} $fence; fetch {{ \"fence\": $fence }};",
        literal(scope.managed_scope_id().as_str()),
    );
    let documents = query_documents(transaction, &query).await?;
    if documents.is_empty() {
        return Ok(None);
    }
    if documents.len() != 1 {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_duplicate_managed_fence",
            "managed scope has more than one fence mirror row",
        ));
    }
    canonical_u64(&required_scalar(&documents[0], "fence")?).map(Some)
}

async fn load_free_managed_fence(
    transaction: &mut Transaction,
    lease: &MigrationLease,
) -> Result<bool, Diagnostic> {
    let query = format!(
        "match $control isa {CONTROL_ENTITY}, has {CONTROL_SCOPE} {}, has {LEASE_FENCE} {}, has {LEASE_STATE} {}; fetch {{ \"exists\": true }};",
        literal(lease.scope().managed_scope_id().as_str()),
        literal(&lease.fence().get().to_string()),
        literal(LEASE_FREE),
    );
    Ok(query_documents(transaction, &query).await?.len() == 1)
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

fn parse_control_documents(
    documents: Vec<Value>,
) -> Result<Option<ControlSnapshot>, Diagnostic> {
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
        PLAN_RECORD_KIND | EVENT_RECORD_KIND | APPLIED_RECORD_KIND
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
        sequence: JournalSequence::new(canonical_u64(&required_scalar(
            &document,
            "sequence",
        )?)?)?,
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
    let value = document.get(key).ok_or_else(|| malformed_provider_row(key))?;
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
    let parsed = value.parse::<u64>().map_err(|_| malformed_provider_row("u64"))?;
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
    let historical = MigrationLease::new(
        lease.scope().clone(),
        lease.holder().clone(),
        fence,
    );
    PlanRecord::from_verified_plan(
        &historical,
        plan,
        plan.applied_migrations(),
        source,
    )
}

fn decode_event_against_plan(
    bytes: &[u8],
    lease: &MigrationLease,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<GroupEventRecord, Diagnostic> {
    let fence = persisted_fence(bytes, EVENT_RECORD_KIND)?;
    let historical = MigrationLease::new(
        lease.scope().clone(),
        lease.holder().clone(),
        fence,
    );
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
                let Ok(candidate) = GroupEventRecord::new(
                    &historical,
                    migration,
                    group,
                    kind,
                    observed,
                ) else {
                    continue;
                };
                if let Ok(record) = decode_event(bytes, candidate) {
                    if matched.replace(record).is_some() {
                        return Err(record_identity_mismatch());
                    }
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

fn ensure_plan_membership(
    plan: &PlanRecord,
    event: &GroupEventRecord,
) -> Result<(), Diagnostic> {
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

fn ensure_applied_membership(
    plan: &PlanRecord,
    applied: &AppliedRecord,
) -> Result<(), Diagnostic> {
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

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}
