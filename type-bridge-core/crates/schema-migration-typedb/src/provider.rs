//! TypeDB implementation of the provider-neutral migration execution seam.
//!
//! Fencing model: every provider schema transaction reads the exact managed
//! fence-mirror row first. The pinned TypeDB 3.12.1 exclusion semantics then
//! block a competing takeover until this transaction finishes, so the
//! in-transaction fence recheck immediately before commit satisfies the
//! same-transaction fencing contract rather than an advisory check.

use std::sync::Arc;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration::CONDITIONAL_RESOLUTION_CAPABILITY;
use type_bridge_contract::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{DocumentId, ManagedSchemaState};
use type_bridge_contract::schema_delta::SCHEMA_REDEFINE_CAPABILITY;
use type_bridge_orm::migration_assertion::{
    MigrationAssertionExecutionContext, MigrationAssertionExecutionError,
    execute_migration_assertion,
};
use type_bridge_orm::{CommitFailureCertainty, Database, OrmError, Transaction};
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema::BUILTIN_SCHEMA_CAPABILITY_IDS;
use type_bridge_schema_migration::{
    ExecutionFuture, GroupCommitCertainty, GroupCommitFailure, GroupCommitFuture,
    MigrationExecutionProvider, MigrationLease, PreparedMigrationGroup, StatementUnit,
    typedb_3_12_1_profile,
};

use crate::observation::observe_managed_state_from_export;
use crate::store::require_active_managed_fence;

const SUPPORTED_SERVER: (u32, u32, u32) = (3, 12, 1);

/// TypeDB 3.12.1 execution provider over one managed database.
pub struct TypeDbMigrationProvider {
    database: Arc<Database>,
    capabilities: CapabilitySet,
}

impl TypeDbMigrationProvider {
    /// Bind the provider to one managed database after an exact version gate.
    pub fn new(database: Arc<Database>) -> Result<Self, Diagnostic> {
        require_supported_server(&database)?;
        Ok(Self {
            database,
            capabilities: execution_capability_vocabulary()?,
        })
    }

    async fn observe_in_transaction(
        &self,
        transaction: &mut Transaction,
        lease: &MigrationLease,
        source_candidate: &ManagedSchemaState,
        target_candidate: &ManagedSchemaState,
    ) -> Result<ManagedSchemaState, Diagnostic> {
        require_active_managed_fence(transaction, lease).await?;
        let export = self.database.schema_text().await.map_err(map_orm_error)?;
        require_active_managed_fence(transaction, lease).await?;
        observe_managed_state_from_export(
            observation_document()?,
            &export,
            &self.capabilities,
            source_candidate,
            target_candidate,
        )
    }
}

impl MigrationExecutionProvider for TypeDbMigrationProvider {
    fn available_capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    fn observe_managed_state<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source_candidate: &'a ManagedSchemaState,
        target_candidate: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState> {
        Box::pin(async move {
            let mut transaction = self
                .database
                .schema_transaction()
                .await
                .map_err(map_orm_error)?;
            let observed = self
                .observe_in_transaction(&mut transaction, lease, source_candidate, target_candidate)
                .await;
            let _ = transaction.rollback().await;
            observed
        })
    }

    fn prepare_group<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, Box<dyn PreparedMigrationGroup + 'a>> {
        Box::pin(async move {
            let mut transaction = self
                .database
                .schema_transaction()
                .await
                .map_err(map_orm_error)?;
            let observed = self
                .observe_in_transaction(&mut transaction, lease, source, target)
                .await;
            let observed = match observed {
                Ok(observed) => observed,
                Err(error) => {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            };
            if observed != *source {
                let _ = transaction.rollback().await;
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_prepare_source_mismatch",
                    "live managed state is not the exact transaction-group source",
                ));
            }
            Ok(Box::new(TypeDbPreparedGroup {
                transaction,
                capabilities: &self.capabilities,
                source: observed,
            }) as Box<dyn PreparedMigrationGroup + 'a>)
        })
    }
}

struct TypeDbPreparedGroup<'a> {
    transaction: Transaction,
    capabilities: &'a CapabilitySet,
    source: ManagedSchemaState,
}

impl PreparedMigrationGroup for TypeDbPreparedGroup<'_> {
    fn execute_assertion<'a>(
        &'a mut self,
        plan: &'a ValidatedMigrationAssertionPlan,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            execute_migration_assertion(
                &mut self.transaction,
                plan,
                MigrationAssertionExecutionContext::new(
                    &self.source,
                    self.capabilities,
                    StructuralLimits::CANONICAL,
                ),
            )
            .await
            .map_err(map_assertion_error)
        })
    }

    fn execute_statement_unit<'a>(
        &'a mut self,
        unit: &'a StatementUnit,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            for statement in unit.statements() {
                self.transaction
                    .query(statement.query())
                    .await
                    .map_err(map_orm_error)?;
            }
            Ok(())
        })
    }

    fn commit<'a>(self: Box<Self>, lease: &'a MigrationLease) -> GroupCommitFuture<'a>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let mut this = *self;
            if let Err(error) = require_active_managed_fence(&mut this.transaction, lease).await {
                let _ = this.transaction.rollback().await;
                return Err(GroupCommitFailure::new(
                    GroupCommitCertainty::DefinitelyAborted,
                    error,
                ));
            }
            this.transaction.commit().await.map_err(|error| {
                let certainty = match error.commit_failure_certainty() {
                    Some(CommitFailureCertainty::DefinitelyAborted) => {
                        GroupCommitCertainty::DefinitelyAborted
                    }
                    Some(CommitFailureCertainty::Unknown) | None => GroupCommitCertainty::Unknown,
                };
                GroupCommitFailure::new(certainty, map_orm_error(error))
            })
        })
    }

    fn rollback<'a>(self: Box<Self>) -> ExecutionFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let mut this = *self;
            this.transaction.rollback().await.map_err(map_orm_error)
        })
    }
}

/// Compose the exact execution capability vocabulary for TypeDB 3.12.1.
pub fn execution_capability_vocabulary() -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = typedb_3_12_1_profile().required_capabilities.clone();
    for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
        capabilities.insert(CapabilityId::new(*capability)?);
    }
    for capability in migration_assertion_capability_vocabulary().iter().cloned() {
        capabilities.insert(capability);
    }
    capabilities.insert(CapabilityId::new(SCHEMA_REDEFINE_CAPABILITY)?);
    capabilities.insert(CapabilityId::new(CONDITIONAL_RESOLUTION_CAPABILITY)?);
    Ok(capabilities)
}

fn require_supported_server(database: &Database) -> Result<(), Diagnostic> {
    let version = database.server_version().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unknown",
            "migration execution requires a negotiated TypeDB server version",
        )
    })?;
    if (version.major, version.minor, version.patch) != SUPPORTED_SERVER {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unsupported",
            "migration execution requires exactly TypeDB 3.12.1",
        )
        .with_detail(
            "server_version",
            format!("{}.{}.{}", version.major, version.minor, version.patch),
        ));
    }
    Ok(())
}

fn observation_document() -> Result<DocumentId, Diagnostic> {
    DocumentId::new("typebridge-provider-observation.typeql")
}

fn map_assertion_error(error: MigrationAssertionExecutionError) -> Diagnostic {
    if let Some(diagnostic) = error.diagnostic() {
        return diagnostic.clone();
    }
    failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_assertion_failed",
        "migration assertion execution failed against live provider data",
    )
    .with_detail("assertion", error.to_string())
}

fn map_orm_error(error: OrmError) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_provider_error",
        "TypeDB migration execution provider operation failed",
    )
    .with_detail("provider", error.to_string())
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
    use super::*;

    #[test]
    fn execution_capability_vocabulary_is_a_closed_superset() {
        let capabilities = execution_capability_vocabulary().expect("capability vocabulary");
        for capability in typedb_3_12_1_profile().required_capabilities.iter() {
            assert!(capabilities.contains(capability));
        }
        for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
            let id = CapabilityId::new(*capability).expect("builtin capability");
            assert!(capabilities.contains(&id));
        }
        for capability in migration_assertion_capability_vocabulary().iter() {
            assert!(capabilities.contains(capability));
        }
        for extra in [
            SCHEMA_REDEFINE_CAPABILITY,
            CONDITIONAL_RESOLUTION_CAPABILITY,
        ] {
            let id = CapabilityId::new(extra).expect("extra capability");
            assert!(capabilities.contains(&id));
        }
        let expected: std::collections::BTreeSet<String> = typedb_3_12_1_profile()
            .required_capabilities
            .iter()
            .map(ToString::to_string)
            .chain(
                BUILTIN_SCHEMA_CAPABILITY_IDS
                    .iter()
                    .map(ToString::to_string),
            )
            .chain(
                migration_assertion_capability_vocabulary()
                    .iter()
                    .map(ToString::to_string),
            )
            .chain(
                [
                    SCHEMA_REDEFINE_CAPABILITY,
                    CONDITIONAL_RESOLUTION_CAPABILITY,
                ]
                .iter()
                .map(ToString::to_string),
            )
            .collect();
        let actual: std::collections::BTreeSet<String> =
            capabilities.iter().map(ToString::to_string).collect();
        assert_eq!(actual, expected);
    }
}
