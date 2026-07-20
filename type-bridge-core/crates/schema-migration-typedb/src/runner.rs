//! Directory-to-database apply orchestration over one TypeDB pair.
//!
//! The runner is the first non-test caller of the verified execution stack:
//! it discovers and replay-verifies the canonical migration chain on disk,
//! reads the applied basis from the authoritative journal database under a
//! lease, derives the apply plan offline, and executes it through the fenced
//! store and provider. It adds no semantics of its own — every decision is
//! delegated to the discovery, planning, and coordination layers it wires.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_orm::Database;
use type_bridge_schema::ManagedDeltaContext;
use type_bridge_schema_migration::{
    ExecutionScope, LeaseHolderId, MigrationApplyApproval, MigrationApplyPlanError,
    MigrationApplyTarget, MigrationExecutionJournal, MigrationExecutionOutcome,
    MigrationHistoryGraph, MigrationLeaseStore, MigrationRollbackOutcome, MigrationSafetyPolicy,
    MigrationVerifyReport, SchemaLoweringBinding, build_verified_migration_apply_plan,
    build_verified_migration_rollback_plan, discover_verified_migration_chain,
    execute_verified_migration_apply_plan, execute_verified_migration_rollback_plan,
    verify_migration_state,
};

use type_bridge_migration::{MigrationStateStore, TypeDbStateStore, load_dir_checked};

use crate::legacy_import::{extract_legacy_frontier, verify_legacy_continuity};
use crate::observation::rebuild_live_managed_state;
use crate::provider::TypeDbMigrationProvider;
use crate::store::{TypeDbMigrationStore, VerifiedMigrationCatalog};

/// Result of one directory apply pass.
#[derive(Debug)]
pub enum MigrationDirectoryApplyOutcome {
    /// The applied ledger already covers the requested target frontier.
    UpToDate,
    /// The coordinator executed the derived plan and reported this outcome.
    Executed(MigrationExecutionOutcome),
}

/// Result of one directory rollback pass.
#[derive(Debug)]
pub enum MigrationDirectoryRollbackOutcome {
    /// None of the requested removals is active in the applied ledger.
    UpToDate,
    /// The coordinator executed the derived rollback and reported this outcome.
    Executed(MigrationRollbackOutcome),
}

/// Failure while orchestrating one directory apply pass.
#[derive(Debug)]
pub enum MigrationDirectoryApplyError {
    /// Discovery, storage, provider, or execution failed with a diagnostic.
    Diagnostic(Diagnostic),
    /// Offline plan derivation failed before any provider write.
    Plan(MigrationApplyPlanError),
}

impl fmt::Display for MigrationDirectoryApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(error) => write!(formatter, "{error}"),
            Self::Plan(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for MigrationDirectoryApplyError {}

impl From<Diagnostic> for MigrationDirectoryApplyError {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(value)
    }
}

impl From<MigrationApplyPlanError> for MigrationDirectoryApplyError {
    fn from(value: MigrationApplyPlanError) -> Self {
        Self::Plan(value)
    }
}

/// Apply orchestrator bound to one managed/journal database pair.
pub struct TypeDbMigrationRunner {
    managed_database: Arc<Database>,
    journal_database: Arc<Database>,
    genesis_source: DeclaredSchema,
    context: ManagedDeltaContext,
    lowering_binding: SchemaLoweringBinding,
    policy: MigrationSafetyPolicy,
}

impl TypeDbMigrationRunner {
    /// Bind the runner to one database pair and one explicit execution policy.
    ///
    /// `genesis_source` is the schema every parentless manifest verifies
    /// against — the empty declared schema unless the scope was adopted with
    /// pre-managed content. Pair-name derivation, capability coherence, and
    /// server-version gates are enforced by the store, planner, and provider
    /// this runner composes.
    pub fn new(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        genesis_source: DeclaredSchema,
        context: ManagedDeltaContext,
        lowering_binding: SchemaLoweringBinding,
        policy: MigrationSafetyPolicy,
    ) -> Self {
        Self {
            managed_database,
            journal_database,
            genesis_source,
            context,
            lowering_binding,
            policy,
        }
    }

    /// Discover and replay-verify the canonical chain in one directory.
    pub fn discover(&self, directory: &Path) -> Result<MigrationHistoryGraph, Diagnostic> {
        discover_verified_migration_chain(directory, &self.genesis_source, &self.context)
    }

    /// Apply the discovered chain up to `target` through the live pair.
    ///
    /// The applied basis is read under a short-lived inspection lease that is
    /// released before execution; the coordinator re-acquires the lease and
    /// its stale-plan gates reject the derived plan if the ledger moved in
    /// between, so the read is rendezvous only, never authority.
    pub async fn apply(
        &self,
        directory: &Path,
        target: &MigrationApplyTarget,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let graph = self.discover(directory)?;
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            catalog,
        )?;
        store.ensure_control_schema().await?;

        let basis = self.load_applied_basis(&store, holder).await?;
        let pending = match target {
            MigrationApplyTarget::DefaultHead => graph.plan_apply_to_default_head(&basis)?,
            MigrationApplyTarget::Explicit(targets) => graph.plan_apply(&basis, targets)?,
        };
        if pending.is_empty() {
            return Ok(MigrationDirectoryApplyOutcome::UpToDate);
        }

        let plan = build_verified_migration_apply_plan(
            &graph,
            &basis,
            target,
            &self.context,
            &self.lowering_binding,
            &self.policy,
            approvals,
        )?;
        let store = store.bind_plan(&plan)?;
        let provider = TypeDbMigrationProvider::new(Arc::clone(&self.managed_database))?;
        let outcome =
            execute_verified_migration_apply_plan(&store, &provider, holder, &plan).await?;
        Ok(MigrationDirectoryApplyOutcome::Executed(outcome))
    }

    /// Verify the migration state triad and report every drift finding.
    ///
    /// The check is strictly read-only: the applied basis is loaded through
    /// the store's lease-free snapshot read — no lease is acquired, no
    /// control schema is installed, and no database is created or touched
    /// beyond schema exports and read transactions. The live schema is
    /// rebuilt for reporting without candidate matching, and nothing is
    /// repaired, generated, or applied. A schema mismatch is drift, not an
    /// invitation to reconcile production automatically.
    pub async fn verify(
        &self,
        directory: &Path,
        desired: Option<&DeclaredSchema>,
    ) -> Result<MigrationVerifyReport, MigrationDirectoryApplyError> {
        let graph = self.discover(directory)?;
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            catalog,
        )?;
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let basis: BTreeSet<MigrationId> = store
            .load_applied_read_only(&scope)
            .await?
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect();

        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_typedb_export_unreadable",
                "managed database schema export failed during verification",
            )
            .with_detail("provider", error.to_string())
        })?;
        let document = DocumentId::new("typebridge-verify-live-export.typeql")?;
        let live =
            rebuild_live_managed_state(document, &export, &self.genesis_source, &self.context)?;

        Ok(verify_migration_state(
            &graph,
            &basis,
            &self.genesis_source,
            desired,
            Some(&live),
            &self.context,
        )?)
    }

    /// Import a completed legacy (v1) history as the canonical bridge basis.
    ///
    /// The legacy directory is loaded through the checked v1 reader (file
    /// checksums verified byte-for-byte under the released rule), the legacy
    /// applied ledger in the managed database must cover the complete loaded
    /// history without checksum drift, and the canonical directory's
    /// legacy-frontier bridge must record exactly the loaded frontier. The
    /// bridge then applies as a zero-operation checkpoint: the coordinator's
    /// live-state gate proves the database sits at the reconstructed legacy
    /// head, and no legacy step is marked as newly executed.
    pub async fn import_legacy_frontier(
        &self,
        legacy_directory: &Path,
        canonical_directory: &Path,
        holder: &LeaseHolderId,
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let legacy_graph = load_dir_checked(legacy_directory).map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_load_failed",
                "legacy migration directory failed the checked v1 loader",
            )
            .with_detail("legacy", error.to_string())
        })?;
        let state_store = TypeDbStateStore::new(Arc::clone(&self.managed_database));
        let applied = state_store.load_applied().await.map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_ledger_unreadable",
                "legacy applied ledger cannot be read from the managed database",
            )
            .with_detail("legacy", error.to_string())
        })?;
        verify_legacy_continuity(&legacy_graph, &applied)?;
        let frontier = extract_legacy_frontier(&legacy_graph)?;

        let graph = self.discover(canonical_directory)?;
        let bridge = graph
            .manifests()
            .map(|(_, manifest)| manifest)
            .find(|manifest| manifest.is_legacy_bridge())
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_import_bridge_missing",
                    "canonical directory carries no legacy-frontier bridge manifest",
                )
            })?;
        if bridge.legacy_parents() != frontier.as_slice() {
            return Err(legacy_import_failure(
                "migration_legacy_import_frontier_mismatch",
                "bridge manifest records a different legacy frontier than the loaded files",
            )
            .into());
        }

        let target = MigrationApplyTarget::Explicit(BTreeSet::from([bridge.id().clone()]));
        self.apply(canonical_directory, &target, holder, &[]).await
    }

    /// Roll the requested applied migrations back through the live pair.
    ///
    /// `removals` must be downward-closed over the applied set: an identity
    /// with a remaining applied descendant is rejected by the planner. The
    /// applied basis is read under the same rendezvous-only inspection lease
    /// as [`Self::apply`]; the coordinator's stale-ledger gate rejects the
    /// derived plan if the ledger moved in between.
    pub async fn rollback(
        &self,
        directory: &Path,
        removals: &BTreeSet<MigrationId>,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryRollbackOutcome, MigrationDirectoryApplyError> {
        let graph = self.discover(directory)?;
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            catalog,
        )?;
        store.ensure_control_schema().await?;

        let basis = self.load_applied_basis(&store, holder).await?;
        if removals.iter().all(|id| !basis.contains(id)) {
            return Ok(MigrationDirectoryRollbackOutcome::UpToDate);
        }

        let plan = build_verified_migration_rollback_plan(
            &graph,
            &basis,
            removals,
            &self.context,
            &self.lowering_binding,
            &self.policy,
            approvals,
        )?;
        let store = store.bind_rollback_plan(&plan)?;
        let provider = TypeDbMigrationProvider::new(Arc::clone(&self.managed_database))?;
        let outcome =
            execute_verified_migration_rollback_plan(&store, &provider, holder, &plan).await?;
        Ok(MigrationDirectoryRollbackOutcome::Executed(outcome))
    }

    async fn load_applied_basis(
        &self,
        store: &TypeDbMigrationStore<'_>,
        holder: &LeaseHolderId,
    ) -> Result<BTreeSet<MigrationId>, Diagnostic> {
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let lease = store.acquire(&scope, holder).await?;
        let loaded = store.load_applied(&lease).await;
        let release = store.release(&lease).await;
        let entries = loaded?;
        release?;
        Ok(entries
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect())
    }
}

fn legacy_import_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
        type_bridge_contract::diagnostic::DiagnosticCode::new(code)
            .expect("static legacy import diagnostic code"),
        message,
    )
}
