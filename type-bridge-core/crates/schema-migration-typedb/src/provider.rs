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
use type_bridge_contract::schema_delta::{
    SCHEMA_REDEFINE_CAPABILITY, schema_transition_capability_vocabulary,
};
use type_bridge_orm::migration_assertion::{
    MigrationAssertionExecutionContext, MigrationAssertionExecutionError,
    execute_migration_assertion,
};
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, OrmError, Transaction,
};
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema::BUILTIN_SCHEMA_CAPABILITY_IDS;
use type_bridge_schema_migration::{
    ExecutionFuture, GroupCommitCertainty, GroupCommitFailure, GroupCommitFuture,
    MigrationExecutionProvider, MigrationLease, PreparedMigrationGroup, StatementUnit,
};

use crate::observation::{
    ManagedObservationAuthority, observe_managed_state_from_export_with_authority,
};
use crate::runner::LegacyExecutionBinding;
use crate::store::require_active_managed_fence;

const SUPPORTED_SERVER: (u32, u32, u32) = (3, 12, 1);

/// TypeDB 3.12.1 execution provider over one managed database.
pub struct TypeDbMigrationProvider {
    database: Arc<Database>,
    capabilities: CapabilitySet,
    legacy_binding: Option<(LegacyExecutionBinding, String)>,
    observation_authority: ManagedObservationAuthority,
}

impl TypeDbMigrationProvider {
    /// Bind the provider to one managed database after an exact version gate.
    pub fn new(database: Arc<Database>) -> Result<Self, Diagnostic> {
        require_supported_server(&database)?;
        Ok(Self {
            database,
            capabilities: execution_capability_vocabulary()?,
            legacy_binding: None,
            observation_authority: ManagedObservationAuthority::ExactPortable,
        })
    }

    /// Bind the runner-validated legacy pair state into every managed SCHEMA
    /// transaction opened by this provider.
    pub(crate) fn new_with_legacy_binding(
        database: Arc<Database>,
        legacy_binding: LegacyExecutionBinding,
        managed_scope: String,
        observation_authority: ManagedObservationAuthority,
    ) -> Result<Self, Diagnostic> {
        let mut provider = Self::new(database)?;
        provider.legacy_binding = Some((legacy_binding, managed_scope));
        provider.observation_authority = observation_authority;
        Ok(provider)
    }

    pub(crate) fn new_with_observation_authority(
        database: Arc<Database>,
        observation_authority: ManagedObservationAuthority,
    ) -> Result<Self, Diagnostic> {
        let mut provider = Self::new(database)?;
        provider.observation_authority = observation_authority;
        Ok(provider)
    }

    async fn observe_in_transaction(
        &self,
        transaction: &mut Transaction,
        lease: &MigrationLease,
        source_candidate: &ManagedSchemaState,
        target_candidate: &ManagedSchemaState,
    ) -> Result<ManagedSchemaState, Diagnostic> {
        require_active_managed_fence(transaction, lease).await?;
        if let Some((binding, managed_scope)) = &self.legacy_binding {
            binding
                .validate_contents(transaction, managed_scope)
                .await?;
        }
        let export = self.database.schema_text().await.map_err(map_orm_error)?;
        require_active_managed_fence(transaction, lease).await?;
        if let Some((binding, managed_scope)) = &self.legacy_binding {
            binding
                .validate_contents(transaction, managed_scope)
                .await?;
        }
        observe_managed_state_from_export_with_authority(
            observation_document()?,
            &export,
            &self.capabilities,
            source_candidate,
            target_candidate,
            &self.observation_authority,
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
            finish_provider_schema_guard(&mut transaction, observed, "managed-state observation")
                .await
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
                    return finish_provider_schema_guard(
                        &mut transaction,
                        Err(error),
                        "transaction-group source observation",
                    )
                    .await;
                }
            };
            if observed != *source {
                let primary = failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_prepare_source_mismatch",
                    "live managed state is not the exact transaction-group source",
                );
                return finish_provider_schema_guard(
                    &mut transaction,
                    Err(primary),
                    "transaction-group source mismatch",
                )
                .await;
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
                let error = finish_provider_schema_guard::<()>(
                    &mut this.transaction,
                    Err(error),
                    "transaction-group pre-commit fence",
                )
                .await
                .expect_err("an error result remains an error after cleanup");
                return Err(GroupCommitFailure::new(
                    GroupCommitCertainty::DefinitelyAborted,
                    error,
                ));
            }
            this.transaction.commit_classified().await.map_err(|error| {
                let certainty = group_commit_certainty(&error);
                GroupCommitFailure::new(certainty, map_orm_error(error.into_orm_error()))
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

fn group_commit_certainty(error: &ClassifiedCommitError) -> GroupCommitCertainty {
    match error.commit_failure_certainty() {
        Some(CommitFailureCertainty::DefinitelyAborted) => GroupCommitCertainty::DefinitelyAborted,
        Some(CommitFailureCertainty::Unknown) | None => GroupCommitCertainty::Unknown,
    }
}

/// Compose the exact execution capability vocabulary for TypeDB 3.12.1.
pub fn execution_capability_vocabulary() -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = schema_transition_capability_vocabulary();
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

async fn finish_provider_schema_guard<T>(
    transaction: &mut Transaction,
    primary: Result<T, Diagnostic>,
    operation: &'static str,
) -> Result<T, Diagnostic> {
    match (primary, transaction.rollback().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(provider_schema_cleanup_failure(cleanup, None, operation)),
        (Err(primary), Err(cleanup)) => Err(provider_schema_cleanup_failure(
            cleanup,
            Some(&primary),
            operation,
        )),
    }
}

fn provider_schema_cleanup_failure(
    cleanup: OrmError,
    primary: Option<&Diagnostic>,
    operation: &'static str,
) -> Diagnostic {
    let mut diagnostic = failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_schema_guard_cleanup_uncertain",
        "provider schema transaction termination was not acknowledged",
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
    use type_bridge_schema_migration::typedb_3_12_1_profile;

    #[test]
    fn execution_capability_vocabulary_is_a_closed_superset() {
        let capabilities = execution_capability_vocabulary().expect("capability vocabulary");
        let authority_capabilities = type_bridge_schema::schema_authority_capability_vocabulary();
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
        capabilities
            .ensure_supported_by(&authority_capabilities)
            .expect("schema authority consumers understand every workspace execution requirement");
    }

    #[test]
    fn classified_commit_evidence_maps_without_widening_legacy_orm_errors() {
        let aborted = ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::DefinitelyAborted,
            message: "server rejected commit".to_owned(),
        };
        assert_eq!(
            group_commit_certainty(&aborted),
            GroupCommitCertainty::DefinitelyAborted,
        );
        assert!(matches!(
            aborted.into_orm_error(),
            OrmError::Transaction(message) if message == "Commit failed: server rejected commit"
        ));

        let unknown = ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::Unknown,
            message: "connection dropped".to_owned(),
        };
        assert_eq!(
            group_commit_certainty(&unknown),
            GroupCommitCertainty::Unknown,
        );
        let lifecycle = ClassifiedCommitError::from(OrmError::Transaction("consumed".to_owned()));
        assert_eq!(
            group_commit_certainty(&lifecycle),
            GroupCommitCertainty::Unknown,
        );
    }
}
