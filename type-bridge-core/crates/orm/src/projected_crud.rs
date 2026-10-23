//! Exact single-model CRUD over a verified generated runtime projection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{
    ModelProjection, ProjectedContainer, ProjectedModelForm, ProjectedModelUse, ReadRoleProjection,
    RoleTokenProjection,
};
use type_bridge_contract::schema::{AnnotationKindId, CollectionMode, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage,
    SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_contract::value::CanonicalValue;

use crate::_descriptor::{EntityDescriptor, RelationDescriptor, RoleDescriptor};
use crate::_dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput,
};
use crate::_manager::hydration::{
    coalesce_dynamic_relation_by_iid, extract_count, hydrate_dynamic_entity,
};
use crate::_manager::{DynamicEntityManager, DynamicRelationManager, query_builder};
use crate::execution_diagnostic::{lower_commit_error, lower_orm_error};
use crate::match_request::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use crate::plan_builders::authority::PlanAuthority;
use crate::plan_builders::insert::relation_insert;
use crate::plan_builders::put::{KeyedPut, keyed_entity_put};
use crate::plan_builders::reference::{Reference, ReferenceIdentity};
use crate::plan_builders::write::{
    ThingUpdate, delete_by_iid, delete_by_iid_returning, entity_insert, update_by_iid,
};
use crate::plan_builders::{LITERAL_CONTROL_CHARACTER, ValueTransport};
use crate::projected_batch::{
    ProjectedBatch, ProjectedBatchInvocationControl, ProjectedBatchOperation, ProjectedBatchRow,
};
use crate::projected_batch_executor::{ProjectedBatchExecutor, ProjectedBatchResult};
use crate::projected_model::{
    ProjectedAttributeValue, ProjectedCreate, ProjectedReference, ProjectedRolePlayer,
    ProjectedThing,
};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::query_plan_exec::PlanAnswer;
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::{
    AnswerCancellation, AnswerConsumer, AnswerControl, AnswerItem, AnswerLimits,
    BoundedAnswerLimits, BoundedAnswerReader, BoundedAnswerStats, QueryResult,
};
use crate::session::database::DatabaseExecutionIdentity;
use crate::session::{Database, TransactionContext, TxType};
use crate::value::AttributeValue;
use crate::{ClassifiedCommitError, OrmError};
use type_bridge_contract::query_plan::v3::DocumentRole;

/// Binding-neutral exact single-model CRUD executor.
///
/// Every provider descriptor and label is derived from the installed runtime
/// projection. Database-bound writes own one transaction through mutation,
/// complete rehydration, and one classified commit. Methods ending in
/// `_in_transaction` borrow the caller's context and never commit, roll back,
/// or close it.
pub struct ProjectedCrudExecutor<'projection> {
    installed: &'projection InstalledRuntimeProjection,
}

/// Compatibility stage for an existing generated facade consuming the common
/// projected CRUD seam.
///
/// New bindings should expose only [`SdkExecutionDiagnostic`]. This hidden
/// stage exists so the released Rust facade can retain its established error
/// phase while adopting the shared executor.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectedCrudCompatibilityStage {
    /// Generated input or projection preflight failed.
    Input,
    /// Provider evidence or complete projected hydration failed.
    Hydration,
    /// Transaction lifecycle or commit failed.
    Transaction,
}

/// Private compatibility cause retained only for an existing generated facade.
///
/// Its debug representation is always redacted. C and new binding APIs consume
/// only the stable diagnostic and never receive this value.
#[doc(hidden)]
pub enum ProjectedCrudCompatibilityCause {
    /// Original ORM failure from the released Rust execution path.
    Orm(OrmError),
    /// Original classified commit failure from the released Rust path.
    Commit(ClassifiedCommitError),
}

impl fmt::Debug for ProjectedCrudCompatibilityCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Orm(_) => formatter.write_str("Orm([REDACTED])"),
            Self::Commit(_) => formatter.write_str("Commit([REDACTED])"),
        }
    }
}

/// Redacted projected diagnostic plus a Rust-facade-only compatibility cause.
#[doc(hidden)]
pub struct ProjectedCrudCompatibilityFailure {
    diagnostic: Box<SdkExecutionDiagnostic>,
    stage: ProjectedCrudCompatibilityStage,
    cause: Option<Box<ProjectedCrudCompatibilityCause>>,
}

impl ProjectedCrudCompatibilityFailure {
    fn new(
        diagnostic: SdkExecutionDiagnostic,
        stage: ProjectedCrudCompatibilityStage,
        cause: Option<ProjectedCrudCompatibilityCause>,
    ) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
            stage,
            cause: cause.map(Box::new),
        }
    }

    /// Return the stable redacted binding-neutral diagnostic.
    #[must_use]
    pub fn diagnostic(&self) -> &SdkExecutionDiagnostic {
        &self.diagnostic
    }

    /// Return the compatibility phase selected before provider text is hidden.
    #[must_use]
    pub const fn stage(&self) -> ProjectedCrudCompatibilityStage {
        self.stage
    }

    /// Consume the failure and return its existing-facade-only cause.
    #[must_use]
    pub fn into_cause(self) -> Option<ProjectedCrudCompatibilityCause> {
        self.cause.map(|cause| *cause)
    }

    /// Consume the failure without discarding either compatibility component.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        SdkExecutionDiagnostic,
        ProjectedCrudCompatibilityStage,
        Option<ProjectedCrudCompatibilityCause>,
    ) {
        (*self.diagnostic, self.stage, self.cause.map(|cause| *cause))
    }

    /// Consume the failure and discard its private cause.
    #[must_use]
    pub fn into_diagnostic(self) -> SdkExecutionDiagnostic {
        *self.diagnostic
    }
}

impl fmt::Debug for ProjectedCrudCompatibilityFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectedCrudCompatibilityFailure")
            .field("diagnostic", &self.diagnostic)
            .field("stage", &self.stage)
            .field("cause", &self.cause.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Default)]
struct CompatibilityCapture {
    cause: Mutex<Option<ProjectedCrudCompatibilityCause>>,
}

impl CompatibilityCapture {
    fn record_orm(&self, error: OrmError) {
        let mut cause = self
            .cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cause.is_none() {
            *cause = Some(ProjectedCrudCompatibilityCause::Orm(error));
        }
    }

    fn take(&self) -> Option<ProjectedCrudCompatibilityCause> {
        self.cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[derive(Clone, Copy, Default)]
struct ExecutionMode<'capture> {
    capture: Option<&'capture CompatibilityCapture>,
}

impl<'capture> ExecutionMode<'capture> {
    const fn compatibility(capture: &'capture CompatibilityCapture) -> Self {
        Self {
            capture: Some(capture),
        }
    }

    fn orm_at_type(
        self,
        error: OrmError,
        operation: SdkProviderOperation,
        type_id: &TypeId,
    ) -> SdkExecutionDiagnostic {
        let diagnostic = append_path(
            lower_orm_error(&error, operation),
            [SdkDiagnosticPathSegment::Type(type_id.clone())],
        );
        if let Some(capture) = self.capture {
            capture.record_orm(error);
        }
        diagnostic
    }
}

impl<'projection> ProjectedCrudExecutor<'projection> {
    /// Create an executor over one verified installed runtime projection.
    #[must_use]
    pub const fn new(installed: &'projection InstalledRuntimeProjection) -> Self {
        Self { installed }
    }

    /// Validate one projected relation create against a database identity
    /// without opening a transaction or performing provider I/O.
    #[doc(hidden)]
    pub fn preflight_relation_create_for_database_with_compatibility(
        &self,
        database: &Database,
        input: &ProjectedCreate,
    ) -> Result<(), ProjectedCrudCompatibilityFailure> {
        let prepared = self
            .prepare_relation_create(input)
            .map_err(compatibility_without_cause)?;
        require_role_player_database_identity(&prepared, &database.execution_identity())
            .map_err(compatibility_without_cause)
    }

    /// Insert and completely rehydrate one exact entity in an owned write transaction.
    pub async fn insert_entity(
        &self,
        database: &Database,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .insert_entity_controlled(database, input, limits, AnswerCancellation::default())
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .insert_entity_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Insert and completely rehydrate one exact entity in a borrowed write transaction.
    pub async fn insert_entity_in_transaction(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .insert_entity_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.insert_entity_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Put and completely rehydrate one exact entity in an owned write transaction.
    pub async fn put_entity(
        &self,
        database: &Database,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .put_entity_controlled(database, input, limits, AnswerCancellation::default())
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .put_entity_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Put and completely rehydrate one exact entity in a borrowed write transaction.
    pub async fn put_entity_in_transaction(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .put_entity_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.put_entity_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Read one exact entity by canonical IID in an owned read transaction.
    pub async fn get_entity_by_iid(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .get_entity_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Read,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .get_entity_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Read one exact entity by canonical IID in a borrowed read-capable transaction.
    pub async fn get_entity_by_iid_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .get_entity_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.get_entity_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Replace and completely rehydrate one exact entity in an owned write transaction.
    pub async fn update_entity(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .update_entity_controlled(
                    database,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        validate_iid(&prepared.type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .update_entity_prepared(&transaction, iid, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Replace and completely rehydrate one exact entity in a borrowed write transaction.
    pub async fn update_entity_in_transaction(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .update_entity_in_transaction_controlled(
                    transaction,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_create(input)?;
        validate_iid(&prepared.type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.update_entity_prepared(transaction, iid, &prepared, ExecutionMode::default())
            .await
    }

    /// Blind-delete one exact entity in an owned write transaction.
    ///
    /// An absent IID is a successful no-op, matching TypeDB exact-delete
    /// semantics and the released binding behavior.
    pub async fn delete_entity_by_iid(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .delete_entity_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .delete_entity_prepared(&transaction, &prepared, ExecutionMode::default(), false)
            .await
            .map(|_| ());
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Blind-delete one exact entity in a borrowed write transaction.
    ///
    /// An absent IID is a successful no-op.
    pub async fn delete_entity_by_iid_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_entity_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_entity_prepared(transaction, &prepared, ExecutionMode::default(), false)
            .await
            .map(|_| ())
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .delete_entity_by_iid_returning_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .delete_entity_prepared(&transaction, &prepared, ExecutionMode::default(), true)
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_entity_by_iid_returning_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_entity_prepared(transaction, &prepared, ExecutionMode::default(), true)
            .await
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_returning_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Entity)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_returning(database, &batch, control)
            .await
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_returning_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact entity and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_entity_by_iid_returning_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Entity)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_returning_in_transaction(transaction, &batch, control)
            .await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .delete_relation_by_iid_returning_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .delete_relation_prepared(&transaction, &prepared, ExecutionMode::default(), true)
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_relation_by_iid_returning_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_relation_prepared(transaction, &prepared, ExecutionMode::default(), true)
            .await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_returning_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_returning(database, &batch, control)
            .await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_returning_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact relation and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_relation_by_iid_returning_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_returning_in_transaction(transaction, &batch, control)
            .await
    }

    async fn execute_controlled_single_delete_returning(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        ProjectedBatchExecutor::new(self.installed)
            .returning_deleted()
            .execute_mapped(database, batch, control, None, move |result| {
                single_batch_deleted_thing(result, &model)
            })
            .await
    }

    async fn execute_controlled_single_delete_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        ProjectedBatchExecutor::new(self.installed)
            .returning_deleted()
            .execute_in_transaction_mapped(transaction, batch, control, None, move |result| {
                single_batch_deleted_thing(result, &model)
            })
            .await
    }

    /// Count exact instances of one projected entity in an owned read transaction.
    pub async fn count_entities(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .count_entities_controlled(database, type_id, limits, AnswerCancellation::default())
                .await;
        }
        let prepared = self.prepare_entity_type(type_id)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Read,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .count_entities_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Count exact instances of one projected entity in a borrowed read-capable transaction.
    pub async fn count_entities_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .count_entities_in_transaction_controlled(
                    transaction,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_entity_type(type_id)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.count_entities_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Insert and completely rehydrate one exact relation in an owned write transaction.
    pub async fn insert_relation(
        &self,
        database: &Database,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .insert_relation_controlled(database, input, limits, AnswerCancellation::default())
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        let database_identity = database.execution_identity();
        require_role_player_database_identity(&prepared, &database_identity)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .insert_relation_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Insert and completely rehydrate one exact relation in a borrowed write transaction.
    pub async fn insert_relation_in_transaction(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .insert_relation_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.insert_relation_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Put and completely rehydrate one exact relation in an owned write transaction.
    pub async fn put_relation(
        &self,
        database: &Database,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .put_relation_controlled(database, input, limits, AnswerCancellation::default())
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        let database_identity = database.execution_identity();
        require_role_player_database_identity(&prepared, &database_identity)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .put_relation_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Put and completely rehydrate one exact relation in a borrowed write transaction.
    pub async fn put_relation_in_transaction(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .put_relation_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.put_relation_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Read one exact relation by canonical IID in an owned read transaction.
    pub async fn get_relation_by_iid(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .get_relation_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Read,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .get_relation_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Read one exact relation by canonical IID in a borrowed read-capable transaction.
    pub async fn get_relation_by_iid_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .get_relation_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.get_relation_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Replace and completely rehydrate one exact relation in an owned write transaction.
    pub async fn update_relation(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .update_relation_controlled(
                    database,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        validate_iid(&prepared.type_id, iid)?;
        let database_identity = database.execution_identity();
        require_role_player_database_identity(&prepared, &database_identity)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .update_relation_prepared(&transaction, iid, &prepared, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Replace and completely rehydrate one exact relation in a borrowed write transaction.
    pub async fn update_relation_in_transaction(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .update_relation_in_transaction_controlled(
                    transaction,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_create(input)?;
        validate_iid(&prepared.type_id, iid)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.update_relation_prepared(transaction, iid, &prepared, ExecutionMode::default())
            .await
    }

    /// Blind-delete one exact relation in an owned write transaction.
    ///
    /// An absent IID is a successful no-op, matching TypeDB exact-delete
    /// semantics and the released binding behavior.
    pub async fn delete_relation_by_iid(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .delete_relation_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Write,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .delete_relation_prepared(&transaction, &prepared, ExecutionMode::default(), false)
            .await
            .map(|_| ());
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Blind-delete one exact relation in a borrowed write transaction.
    ///
    /// An absent IID is a successful no-op.
    pub async fn delete_relation_by_iid_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_relation_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_relation_prepared(transaction, &prepared, ExecutionMode::default(), false)
            .await
            .map(|_| ())
    }

    /// Count exact instances of one projected relation in an owned read transaction.
    pub async fn count_relations(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        if let Some(limits) = database.answer_limits() {
            return self
                .count_relations_controlled(
                    database,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_type(type_id)?;
        let transaction = self
            .open_transaction(
                database,
                TxType::Read,
                &prepared.type_id,
                ExecutionMode::default(),
            )
            .await?;
        let result = self
            .count_relations_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Count exact instances of one projected relation in a borrowed read-capable transaction.
    pub async fn count_relations_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .count_relations_in_transaction_controlled(
                    transaction,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await;
        }
        let prepared = self.prepare_relation_type(type_id)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.count_relations_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Execute one policy-aware entity insert in an owned write transaction.
    #[doc(hidden)]
    pub async fn insert_entity_controlled(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.insert_entity_controlled_with_control(
            database,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware entity insert with a pre-captured control.
    #[doc(hidden)]
    pub async fn insert_entity_controlled_with_control(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware entity insert in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn insert_entity_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.insert_entity_in_transaction_controlled_with_control(
            transaction,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity insert with a pre-captured control.
    #[doc(hidden)]
    pub async fn insert_entity_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware entity put in an owned write transaction.
    #[doc(hidden)]
    pub async fn put_entity_controlled(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.put_entity_controlled_with_control(
            database,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware entity put with a pre-captured control.
    #[doc(hidden)]
    pub async fn put_entity_controlled_with_control(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware entity put in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn put_entity_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.put_entity_in_transaction_controlled_with_control(
            transaction,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity put with a pre-captured control.
    #[doc(hidden)]
    pub async fn put_entity_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware entity replacement in an owned write transaction.
    #[doc(hidden)]
    pub async fn update_entity_controlled(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.update_entity_controlled_with_control(
            database,
            iid,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware entity replacement with a pre-captured control.
    #[doc(hidden)]
    pub async fn update_entity_controlled_with_control(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware entity replacement in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn update_entity_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.update_entity_in_transaction_controlled_with_control(
            transaction,
            iid,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity replacement with a pre-captured control.
    #[doc(hidden)]
    pub async fn update_entity_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware entity delete in an owned write transaction.
    #[doc(hidden)]
    pub async fn delete_entity_by_iid_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware entity delete with a pre-captured control.
    #[doc(hidden)]
    pub async fn delete_entity_by_iid_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Entity)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete(database, &batch, control)
            .await
    }

    /// Execute one policy-aware entity delete in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn delete_entity_by_iid_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity delete with a pre-captured control.
    #[doc(hidden)]
    pub async fn delete_entity_by_iid_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Entity)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware relation insert in an owned write transaction.
    #[doc(hidden)]
    pub async fn insert_relation_controlled(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.insert_relation_controlled_with_control(
            database,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware relation insert with a pre-captured control.
    #[doc(hidden)]
    pub async fn insert_relation_controlled_with_control(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware relation insert in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn insert_relation_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.insert_relation_in_transaction_controlled_with_control(
            transaction,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation insert with a pre-captured control.
    #[doc(hidden)]
    pub async fn insert_relation_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware relation put in an owned write transaction.
    #[doc(hidden)]
    pub async fn put_relation_controlled(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.put_relation_controlled_with_control(
            database,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware relation put with a pre-captured control.
    #[doc(hidden)]
    pub async fn put_relation_controlled_with_control(
        &self,
        database: &Database,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware relation put in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn put_relation_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.put_relation_in_transaction_controlled_with_control(
            transaction,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation put with a pre-captured control.
    #[doc(hidden)]
    pub async fn put_relation_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware relation replacement in an owned write transaction.
    #[doc(hidden)]
    pub async fn update_relation_controlled(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.update_relation_controlled_with_control(
            database,
            iid,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware relation replacement with a pre-captured control.
    #[doc(hidden)]
    pub async fn update_relation_controlled_with_control(
        &self,
        database: &Database,
        iid: &str,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        self.execute_controlled_single_thing(database, &batch, control)
            .await
    }

    /// Execute one policy-aware relation replacement in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn update_relation_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.update_relation_in_transaction_controlled_with_control(
            transaction,
            iid,
            input,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation replacement with a pre-captured control.
    #[doc(hidden)]
    pub async fn update_relation_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        self.execute_controlled_single_thing_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware relation delete in an owned write transaction.
    #[doc(hidden)]
    pub async fn delete_relation_by_iid_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware relation delete with a pre-captured control.
    #[doc(hidden)]
    pub async fn delete_relation_by_iid_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete(database, &batch, control)
            .await
    }

    /// Execute one policy-aware relation delete in a borrowed write transaction.
    #[doc(hidden)]
    pub async fn delete_relation_by_iid_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation delete with a pre-captured control.
    #[doc(hidden)]
    pub async fn delete_relation_by_iid_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        validate_iid(type_id, iid)?;
        control.check()?;
        let batch = self.single_controlled_batch(
            type_id,
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            },
            &control,
        )?;
        self.execute_controlled_single_delete_in_transaction(transaction, &batch, control)
            .await
    }

    /// Execute one policy-aware entity IID read in an owned read transaction.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.get_entity_by_iid_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware entity IID read with a pre-captured invocation control.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        control.check(&prepared.type_id)?;
        let transaction = self
            .open_controlled_read_transaction(database, &prepared.type_id, &control)
            .await?;
        let result = self
            .get_entity_prepared_controlled(&transaction, &prepared, &control)
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Execute one policy-aware entity IID read in a borrowed read-capable transaction.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.get_entity_by_iid_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity IID read with a pre-captured control.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.get_entity_prepared_controlled(transaction, &prepared, &control)
            .await
    }

    /// Execute one policy-aware exact entity count in an owned read transaction.
    #[doc(hidden)]
    pub async fn count_entities_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_entities_controlled_with_control(
            database,
            type_id,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware exact entity count with a pre-captured control.
    #[doc(hidden)]
    pub async fn count_entities_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        control: ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let prepared = self.prepare_entity_type(type_id)?;
        control.check(&prepared.type_id)?;
        let transaction = self
            .open_controlled_read_transaction(database, &prepared.type_id, &control)
            .await?;
        let result = self
            .count_entities_prepared_controlled(&transaction, &prepared, &control)
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Execute one policy-aware exact entity count in a borrowed read-capable transaction.
    #[doc(hidden)]
    pub async fn count_entities_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_entities_in_transaction_controlled_with_control(
            transaction,
            type_id,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware entity count with a pre-captured control.
    #[doc(hidden)]
    pub async fn count_entities_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        control: ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let prepared = self.prepare_entity_type(type_id)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.count_entities_prepared_controlled(transaction, &prepared, &control)
            .await
    }

    /// Execute one policy-aware relation IID read in an owned read transaction.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.get_relation_by_iid_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware relation IID read with a pre-captured control.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        control.check(&prepared.type_id)?;
        let transaction = self
            .open_controlled_read_transaction(database, &prepared.type_id, &control)
            .await?;
        let result = self
            .get_relation_prepared_controlled(&transaction, &prepared, &control)
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Execute one policy-aware relation IID read in a borrowed read-capable transaction.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        self.get_relation_by_iid_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation IID read with a pre-captured control.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.get_relation_prepared_controlled(transaction, &prepared, &control)
            .await
    }

    /// Execute one policy-aware exact relation count in an owned read transaction.
    #[doc(hidden)]
    pub async fn count_relations_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_relations_controlled_with_control(
            database,
            type_id,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one policy-aware exact relation count with a pre-captured control.
    #[doc(hidden)]
    pub async fn count_relations_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        control: ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let prepared = self.prepare_relation_type(type_id)?;
        control.check(&prepared.type_id)?;
        let transaction = self
            .open_controlled_read_transaction(database, &prepared.type_id, &control)
            .await?;
        let result = self
            .count_relations_prepared_controlled(&transaction, &prepared, &control)
            .await;
        finish_owned_read(transaction, &prepared.type_id, result).await
    }

    /// Execute one policy-aware exact relation count in a borrowed read-capable transaction.
    #[doc(hidden)]
    pub async fn count_relations_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_relations_in_transaction_controlled_with_control(
            transaction,
            type_id,
            ProjectedCrudInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Execute one borrowed policy-aware relation count with a pre-captured control.
    #[doc(hidden)]
    pub async fn count_relations_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        control: ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let prepared = self.prepare_relation_type(type_id)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.count_relations_prepared_controlled(transaction, &prepared, &control)
            .await
    }

    /// Execute a borrowed entity insert while retaining the released Rust
    /// facade's private compatibility cause.
    #[doc(hidden)]
    pub async fn insert_entity_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .insert_entity_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_create(input)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .insert_entity_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed entity put with a private compatibility cause.
    #[doc(hidden)]
    pub async fn put_entity_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .put_entity_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_create(input)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .put_entity_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed entity read with a private compatibility cause.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .get_entity_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .get_entity_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed entity update with a private compatibility cause.
    #[doc(hidden)]
    pub async fn update_entity_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .update_entity_in_transaction_controlled(
                    transaction,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_create(input)
            .map_err(compatibility_without_cause)?;
        validate_iid(&prepared.type_id, iid).map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .update_entity_prepared(
                transaction,
                iid,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed entity delete with a private compatibility cause.
    #[doc(hidden)]
    pub async fn delete_entity_by_iid_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_entity_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .delete_entity_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
                false,
            )
            .await
            .map(|_| ());
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed exact entity count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_entities_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .count_entities_in_transaction_controlled(
                    transaction,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_type(type_id)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .count_entities_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed relation insert with a private compatibility cause.
    #[doc(hidden)]
    pub async fn insert_relation_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .insert_relation_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_create(input)
            .map_err(compatibility_without_cause)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .insert_relation_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed relation put with a private compatibility cause.
    #[doc(hidden)]
    pub async fn put_relation_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .put_relation_in_transaction_controlled(
                    transaction,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_create(input)
            .map_err(compatibility_without_cause)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .put_relation_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed relation read with a private compatibility cause.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .get_relation_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .get_relation_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed relation update with a private compatibility cause.
    #[doc(hidden)]
    pub async fn update_relation_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        input: &ProjectedCreate,
    ) -> Result<ProjectedThing, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .update_relation_in_transaction_controlled(
                    transaction,
                    iid,
                    input,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_create(input)
            .map_err(compatibility_without_cause)?;
        validate_iid(&prepared.type_id, iid).map_err(compatibility_without_cause)?;
        require_role_player_database_identity(&prepared, transaction.execution_identity())
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .update_relation_prepared(
                transaction,
                iid,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed relation delete with a private compatibility cause.
    #[doc(hidden)]
    pub async fn delete_relation_by_iid_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .delete_relation_by_iid_in_transaction_controlled(
                    transaction,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .delete_relation_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
                false,
            )
            .await
            .map(|_| ());
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed exact relation count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_relations_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = transaction.answer_limits() {
            return self
                .count_relations_in_transaction_controlled(
                    transaction,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_type(type_id)
            .map_err(compatibility_without_cause)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let result = self
            .count_relations_prepared(
                transaction,
                &prepared,
                ExecutionMode::compatibility(&capture),
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a database-owned entity read with a private compatibility cause.
    #[doc(hidden)]
    pub async fn get_entity_by_iid_with_compatibility(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = database.answer_limits() {
            return self
                .get_entity_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let mode = ExecutionMode::compatibility(&capture);
        let transaction = self
            .open_transaction(database, TxType::Read, &prepared.type_id, mode)
            .await
            .map_err(|diagnostic| compatibility_failure(diagnostic, &capture))?;
        let result = self
            .get_entity_prepared(&transaction, &prepared, mode)
            .await;
        finish_compatibility_read(transaction, &prepared.type_id, result, &capture, mode).await
    }

    /// Execute a database-owned entity count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_entities_with_compatibility(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = database.answer_limits() {
            return self
                .count_entities_controlled(database, type_id, limits, AnswerCancellation::default())
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_entity_type(type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let mode = ExecutionMode::compatibility(&capture);
        let transaction = self
            .open_transaction(database, TxType::Read, &prepared.type_id, mode)
            .await
            .map_err(|diagnostic| compatibility_failure(diagnostic, &capture))?;
        let result = self
            .count_entities_prepared(&transaction, &prepared, mode)
            .await;
        finish_compatibility_read(transaction, &prepared.type_id, result, &capture, mode).await
    }

    /// Execute a database-owned relation read with a private compatibility cause.
    #[doc(hidden)]
    pub async fn get_relation_by_iid_with_compatibility(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = database.answer_limits() {
            return self
                .get_relation_by_iid_controlled(
                    database,
                    type_id,
                    iid,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_identity(type_id, iid)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let mode = ExecutionMode::compatibility(&capture);
        let transaction = self
            .open_transaction(database, TxType::Read, &prepared.type_id, mode)
            .await
            .map_err(|diagnostic| compatibility_failure(diagnostic, &capture))?;
        let result = self
            .get_relation_prepared(&transaction, &prepared, mode)
            .await;
        finish_compatibility_read(transaction, &prepared.type_id, result, &capture, mode).await
    }

    /// Execute a database-owned relation count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_relations_with_compatibility(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
        if let Some(limits) = database.answer_limits() {
            return self
                .count_relations_controlled(
                    database,
                    type_id,
                    limits,
                    AnswerCancellation::default(),
                )
                .await
                .map_err(compatibility_without_cause);
        }
        let prepared = self
            .prepare_relation_type(type_id)
            .map_err(compatibility_without_cause)?;
        let capture = CompatibilityCapture::default();
        let mode = ExecutionMode::compatibility(&capture);
        let transaction = self
            .open_transaction(database, TxType::Read, &prepared.type_id, mode)
            .await
            .map_err(|diagnostic| compatibility_failure(diagnostic, &capture))?;
        let result = self
            .count_relations_prepared(&transaction, &prepared, mode)
            .await;
        finish_compatibility_read(transaction, &prepared.type_id, result, &capture, mode).await
    }

    fn single_controlled_batch(
        &self,
        model: &TypeId,
        operation: ProjectedBatchOperation,
        row: ProjectedBatchRow,
        control: &ProjectedBatchInvocationControl,
    ) -> Result<ProjectedBatch, SdkExecutionDiagnostic> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(1)
            .map_err(|_| ProjectedBatch::binding_allocation_failure())?;
        rows.push(row);
        ProjectedBatch::try_new_for_invocation(
            self.installed,
            model.clone(),
            operation,
            rows,
            control,
        )
    }

    async fn execute_controlled_single_thing(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        let mapper_model = model.clone();
        ProjectedBatchExecutor::new(self.installed)
            .execute_mapped(database, batch, control, None, move |result| {
                single_batch_thing(result, &mapper_model).map(Some)
            })
            .await?
            .ok_or_else(|| single_batch_result_invalid(&model))
    }

    async fn execute_controlled_single_thing_in_transaction(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        let mapper_model = model.clone();
        ProjectedBatchExecutor::new(self.installed)
            .execute_in_transaction_mapped(transaction, batch, control, None, move |result| {
                single_batch_thing(result, &mapper_model).map(Some)
            })
            .await?
            .ok_or_else(|| single_batch_result_invalid(&model))
    }

    async fn execute_controlled_single_delete(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        ProjectedBatchExecutor::new(self.installed)
            .execute_mapped(database, batch, control, (), move |result| {
                single_batch_delete(result, &model)
            })
            .await
    }

    async fn execute_controlled_single_delete_in_transaction(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        ProjectedBatchExecutor::new(self.installed)
            .execute_in_transaction_mapped(transaction, batch, control, (), move |result| {
                single_batch_delete(result, &model)
            })
            .await
    }

    async fn open_controlled_read_transaction(
        &self,
        database: &Database,
        type_id: &TypeId,
        control: &ProjectedCrudInvocationControl,
    ) -> Result<TransactionContext, SdkExecutionDiagnostic> {
        await_crud_controlled(
            database.transaction_context(TxType::Read),
            control.deadline,
            &control.cancellation,
        )
        .await
        .map_err(|error| {
            controlled_crud_orm(error, SdkProviderOperation::OpenReadTransaction, type_id)
        })
    }

    async fn get_entity_prepared_controlled(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityIdentity,
        control: &ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        control.check(&prepared.type_id)?;
        let query = query_builder::build_dynamic_entity_fetch_by_iid_exact(
            &prepared.descriptor,
            &prepared.iid,
            "$e",
        )
        .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))?;
        let mut ledger = ControlledCrudLedger::try_new(
            control.limits,
            controlled_read_input_bytes(&prepared.type_id, Some(&prepared.iid)),
        )?;
        let answer = self
            .execute_controlled_read_query(
                transaction,
                &prepared.type_id,
                &query,
                ControlledAnswerKind::Documents,
                control,
                &mut ledger,
            )
            .await?;
        let QueryResult::Documents(documents) = answer else {
            unreachable!("controlled answer collector preserves its expected kind")
        };
        let row = match documents.as_slice() {
            [] => None,
            [document] => Some(
                hydrate_dynamic_entity(&prepared.descriptor, document).map_err(|error| {
                    orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
                })?,
            ),
            _ => {
                return Err(integrity_at_type(
                    "entity_iid_cardinality_invalid",
                    "The provider returned multiple exact entities for one IID",
                    &prepared.type_id,
                ));
            }
        };
        let thing = row
            .map(|row| {
                self.hydrate_entity(
                    row,
                    &prepared.type_id,
                    &prepared.iid,
                    transaction.execution_identity(),
                    ExecutionMode::default(),
                )
            })
            .transpose()?;
        if let Some(thing) = thing.as_ref() {
            ledger.charge_projected(thing)?;
        }
        control.check(&prepared.type_id)?;
        Ok(thing)
    }

    async fn count_entities_prepared_controlled(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityType,
        control: &ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        control.check(&prepared.type_id)?;
        let query = query_builder::build_dynamic_entity_count_exact(&prepared.descriptor, "$e")
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))?;
        self.execute_controlled_count(
            transaction,
            &prepared.type_id,
            &query,
            control,
            controlled_read_input_bytes(&prepared.type_id, None),
        )
        .await
    }

    async fn get_relation_prepared_controlled(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationIdentity,
        control: &ProjectedCrudInvocationControl,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        control.check(&prepared.type_id)?;
        let query = query_builder::build_dynamic_relation_fetch_by_iid_exact(
            &prepared.descriptor,
            &prepared.iid,
            "$r",
        )
        .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))?;
        let mut ledger = ControlledCrudLedger::try_new(
            control.limits,
            controlled_read_input_bytes(&prepared.type_id, Some(&prepared.iid)),
        )?;
        let answer = self
            .execute_controlled_read_query(
                transaction,
                &prepared.type_id,
                &query,
                ControlledAnswerKind::Documents,
                control,
                &mut ledger,
            )
            .await?;
        let QueryResult::Documents(documents) = answer else {
            unreachable!("controlled answer collector preserves its expected kind")
        };
        let rows =
            coalesce_dynamic_relation_by_iid(&prepared.descriptor, &documents, &prepared.iid)
                .map_err(|error| {
                    orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
                })?;
        let thing = match rows.as_slice() {
            [] => None,
            [row] => Some(self.hydrate_relation(
                row.clone(),
                &prepared.type_id,
                &prepared.iid,
                transaction.execution_identity(),
                ExecutionMode::default(),
            )?),
            _ => return Err(relation_ambiguity(&prepared.type_id)),
        };
        if let Some(thing) = thing.as_ref() {
            ledger.charge_projected(thing)?;
        }
        control.check(&prepared.type_id)?;
        Ok(thing)
    }

    async fn count_relations_prepared_controlled(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationType,
        control: &ProjectedCrudInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        control.check(&prepared.type_id)?;
        let query = query_builder::build_dynamic_relation_count_exact(&prepared.descriptor, "$r")
            .map_err(|error| {
            orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
        })?;
        self.execute_controlled_count(
            transaction,
            &prepared.type_id,
            &query,
            control,
            controlled_read_input_bytes(&prepared.type_id, None),
        )
        .await
    }

    async fn execute_controlled_count(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        query: &str,
        control: &ProjectedCrudInvocationControl,
        input_bytes: u64,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let mut ledger = ControlledCrudLedger::try_new(control.limits, input_bytes)?;
        let answer = self
            .execute_controlled_read_query(
                transaction,
                type_id,
                query,
                ControlledAnswerKind::Rows,
                control,
                &mut ledger,
            )
            .await?;
        if !matches!(&answer, QueryResult::Rows(rows) if rows.len() == 1) {
            return Err(integrity_at_type(
                "provider_count_cardinality_invalid",
                "The provider count query did not return exactly one row",
                type_id,
            ));
        }
        let count = extract_count(&answer)
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        ledger.charge_scalar_bytes(std::mem::size_of::<u64>() as u64)?;
        control.check(type_id)?;
        Ok(count)
    }

    async fn execute_controlled_read_query(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        query: &str,
        expected: ControlledAnswerKind,
        control: &ProjectedCrudInvocationControl,
        ledger: &mut ControlledCrudLedger,
    ) -> Result<QueryResult, SdkExecutionDiagnostic> {
        control.check(type_id)?;
        ledger.charge_statement(u64::try_from(query.len()).unwrap_or(u64::MAX))?;
        let limits = AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: ledger.remaining_items(),
                max_bytes: ledger.remaining_bytes(),
                deadline: Some(control.deadline.instant()),
                cancellation: control.cancellation.clone(),
            },
            max_collection_members: control.limits.collection_members,
        };
        let mut collector = ControlledAnswerCollector::new(expected, type_id.label().as_str());
        let mut observed = LocallyBoundedCrudConsumer::new(&mut collector, limits.answer.clone());
        let future = transaction.query_v2_bounded(query, limits, &mut observed);
        let mut provider = await_crud_controlled(future, control.deadline, &control.cancellation)
            .await
            .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Read, type_id))?;
        let local = observed.stats();
        provider.processed_items = provider.processed_items.max(local.processed_items);
        provider.response_bytes = provider.response_bytes.max(local.response_bytes);
        provider.stopped_early |= local.stopped_early;
        drop(observed);
        ledger.record_reply(provider)?;
        control.check(type_id)?;
        Ok(collector.finish())
    }

    async fn open_transaction(
        &self,
        database: &Database,
        tx_type: TxType,
        type_id: &TypeId,
        mode: ExecutionMode<'_>,
    ) -> Result<TransactionContext, SdkExecutionDiagnostic> {
        let operation = match tx_type {
            TxType::Read => SdkProviderOperation::OpenReadTransaction,
            TxType::Write => SdkProviderOperation::OpenWriteTransaction,
            TxType::Schema => SdkProviderOperation::Schema,
        };
        database
            .transaction_context(tx_type)
            .await
            .map_err(|error| mode.orm_at_type(error, operation, type_id))
    }

    fn prepare_entity_create(
        &self,
        input: &ProjectedCreate,
    ) -> Result<PreparedEntityCreate, SdkExecutionDiagnostic> {
        input.validate_for(self.installed)?;
        let prepared = self.prepare_entity_type(input.type_id())?;
        Ok(PreparedEntityCreate {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
            attributes: self.lower_create_fields(input)?,
            keyed_put: self.lower_keyed_put(input)?,
            plan_write: self.lower_plan_write(input)?,
        })
    }

    fn prepare_relation_create(
        &self,
        input: &ProjectedCreate,
    ) -> Result<PreparedRelationCreate, SdkExecutionDiagnostic> {
        input.validate_for(self.installed)?;
        let prepared = self.prepare_relation_type(input.type_id())?;
        Ok(PreparedRelationCreate {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
            attributes: self.lower_create_fields(input)?,
            plan_fields: self.lower_create_plan_fields(input)?,
            plan_write: self.lower_plan_write(input)?,
            role_players: self.lower_create_roles(input)?,
        })
    }

    fn prepare_entity_type(
        &self,
        type_id: &TypeId,
    ) -> Result<PreparedEntityType, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Entity)?;
        let descriptor = self
            .installed
            .entity_descriptor(type_id)
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        Ok(PreparedEntityType {
            type_id: type_id.clone(),
            descriptor: Arc::new(descriptor.clone()),
        })
    }

    fn prepare_relation_type(
        &self,
        type_id: &TypeId,
    ) -> Result<PreparedRelationType, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        let descriptor = self
            .installed
            .relation_descriptor(type_id)
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        Ok(PreparedRelationType {
            type_id: type_id.clone(),
            descriptor: Arc::new(descriptor.clone()),
        })
    }

    fn prepare_entity_identity(
        &self,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<PreparedEntityIdentity, SdkExecutionDiagnostic> {
        let prepared = self.prepare_entity_type(type_id)?;
        validate_iid(type_id, iid)?;
        Ok(PreparedEntityIdentity {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
            iid: iid.to_owned(),
        })
    }

    fn prepare_relation_identity(
        &self,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<PreparedRelationIdentity, SdkExecutionDiagnostic> {
        let prepared = self.prepare_relation_type(type_id)?;
        validate_iid(type_id, iid)?;
        Ok(PreparedRelationIdentity {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
            iid: iid.to_owned(),
        })
    }

    fn require_exact_model(
        &self,
        type_id: &TypeId,
        expected_kind: TypeKind,
    ) -> Result<&ModelProjection, SdkExecutionDiagnostic> {
        if type_id.kind() != expected_kind {
            return Err(invalid_input_at_type(
                "wrong_model_kind",
                "The exact CRUD operation received a different projected model kind",
                type_id,
            ));
        }
        let model = self
            .installed
            .projection()
            .models()
            .get(type_id)
            .ok_or_else(|| {
                invalid_input_at_type(
                    "model_not_projected",
                    "The model type is absent from the installed runtime projection",
                    type_id,
                )
            })?;
        if model.declaration().is_abstract() || !model.declaration().is_constructible() {
            return Err(invalid_input_at_type(
                "model_not_constructible",
                "Exact CRUD requires a concrete constructible projected model",
                type_id,
            ));
        }
        Ok(model)
    }

    fn lower_create_fields(
        &self,
        input: &ProjectedCreate,
    ) -> Result<DynamicAttributeMap, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let mut attributes = Vec::new();
        for (field_id, values) in input.fields() {
            let token = model.query_tokens().fields().get(field_id).ok_or_else(|| {
                integrity_at_field(
                    "projected_field_token_missing",
                    "The create facet refers to an absent projected ownership token",
                    input.type_id(),
                    field_id,
                )
            })?;
            for value in values {
                value.validate_for(self.installed)?;
                attributes.push((
                    token.id().attribute().label().as_str().to_owned(),
                    value.to_attribute_value(),
                ));
            }
        }
        Ok(attributes)
    }

    /// The create's field values keyed by their declaring ownership, which
    /// is the identity a write plan validates.
    fn lower_create_plan_fields(
        &self,
        input: &ProjectedCreate,
    ) -> Result<Vec<(OwnsFactId, CanonicalValue)>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let mut fields = Vec::new();
        for (field_id, values) in input.fields() {
            let token = model.query_tokens().fields().get(field_id).ok_or_else(|| {
                integrity_at_field(
                    "projected_field_token_missing",
                    "The create facet refers to an absent projected ownership token",
                    input.type_id(),
                    field_id,
                )
            })?;
            fields.extend(
                values
                    .iter()
                    .map(|value| (token.declaring_id().clone(), value.value().clone())),
            );
        }
        Ok(fields)
    }

    /// A create as an insert or update plan input: the given non-key values
    /// and every non-key ownership and role the update clears first (the
    /// helpers' complete replacement). `None` keeps the released path for
    /// models with ordered fields or roles.
    fn lower_plan_write(
        &self,
        input: &ProjectedCreate,
    ) -> Result<Option<PreparedPlanWrite>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let tokens = model.query_tokens();
        if tokens
            .fields()
            .values()
            .any(|token| token.multiplicity().collection_mode() != CollectionMode::Unordered)
            || tokens
                .roles()
                .values()
                .any(|token| token.multiplicity().collection_mode() != CollectionMode::Unordered)
        {
            return Ok(None);
        }
        let mut created = Vec::new();
        let mut fields = Vec::new();
        for (field_id, values) in input.fields() {
            let token = tokens.fields().get(field_id).ok_or_else(|| {
                integrity_at_field(
                    "projected_field_token_missing",
                    "The create facet refers to an absent projected ownership token",
                    input.type_id(),
                    field_id,
                )
            })?;
            let values = values
                .iter()
                .map(|value| (token.declaring_id().clone(), value.value().clone()));
            if token.is_key() {
                created.extend(values);
            } else {
                created.extend(values.clone());
                fields.extend(values);
            }
        }
        let mut cleared_fields: Vec<OwnsFactId> = tokens
            .fields()
            .values()
            .filter(|token| !token.is_key())
            .map(|token| token.declaring_id().clone())
            .collect();
        cleared_fields.sort();
        cleared_fields.dedup();
        Ok(Some(PreparedPlanWrite {
            created,
            fields,
            cleared_fields,
            cleared_roles: tokens.roles().keys().cloned().collect(),
        }))
    }

    /// Split a create into the keyed put plan's keys, single-valued fields,
    /// ownerships to clear and multi-valued values to attach.
    ///
    /// `None` keeps the released path: a create without a key value is an
    /// insert (no read either), and ordered fields need list syntax the plan
    /// vocabulary does not have.
    fn lower_keyed_put(
        &self,
        input: &ProjectedCreate,
    ) -> Result<Option<PreparedKeyedPut>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let tokens = model.query_tokens().fields();
        let mut put = PreparedKeyedPut {
            keys: Vec::new(),
            fields: Vec::new(),
            cleared: Vec::new(),
            attached: Vec::new(),
        };
        for (field_id, values) in input.fields() {
            let token = tokens.get(field_id).ok_or_else(|| {
                integrity_at_field(
                    "projected_field_token_missing",
                    "The create facet refers to an absent projected ownership token",
                    input.type_id(),
                    field_id,
                )
            })?;
            let multiplicity = token.multiplicity();
            if values.is_empty() && !token.is_key() {
                if multiplicity.collection_mode() != CollectionMode::Unordered {
                    return Ok(None);
                }
                put.cleared.push(token.declaring_id().clone());
                continue;
            }
            let target = if token.is_key() {
                &mut put.keys
            } else if multiplicity.collection_mode() != CollectionMode::Unordered {
                return Ok(None);
            } else if multiplicity.container() == ProjectedContainer::Scalar && values.len() <= 1 {
                &mut put.fields
            } else {
                // A multi-valued field is emptied, then its values attached.
                put.cleared.push(token.declaring_id().clone());
                &mut put.attached
            };
            target.extend(
                values
                    .iter()
                    .map(|value| (token.declaring_id().clone(), value.value().clone())),
            );
        }
        if put.keys.is_empty() {
            return Ok(None);
        }
        for (field_id, token) in tokens {
            if token.is_key() || input.fields().contains_key(field_id) {
                continue;
            }
            if token.multiplicity().collection_mode() != CollectionMode::Unordered {
                return Ok(None);
            }
            put.cleared.push(token.declaring_id().clone());
        }
        Ok(Some(put))
    }

    fn lower_create_roles(
        &self,
        input: &ProjectedCreate,
    ) -> Result<Vec<PreparedRolePlayerInput>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let descriptor = self
            .installed
            .relation_descriptor(input.type_id())
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, input.type_id()))?;
        let mut output = Vec::new();
        for (role_id, references) in input.roles() {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_token_missing",
                    "The create facet refers to an absent projected role token",
                    input.type_id(),
                    role_id,
                )
            })?;
            let descriptor_role = unique_descriptor_role(descriptor, token).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_descriptor_mismatch",
                    "The projected role does not have one exact provider descriptor",
                    input.type_id(),
                    role_id,
                )
            })?;
            let create_role = model.create().roles().get(role_id).ok_or_else(|| {
                integrity_at_role(
                    "projected_create_role_missing",
                    "The create facet refers to an absent projected role",
                    input.type_id(),
                    role_id,
                )
            })?;
            if !descriptor_role_matches_token(descriptor_role, token) {
                return Err(integrity_at_role(
                    "projected_role_descriptor_mismatch",
                    "The projected role-player domain does not match its provider descriptor",
                    input.type_id(),
                    role_id,
                ));
            }
            for reference in references {
                reference.validate_for(self.installed)?;
                let candidate_types = self.concrete_role_player_types(
                    reference.type_id(),
                    create_role.players(),
                    token,
                );
                if candidate_types.is_empty() {
                    return Err(invalid_input_at_role(
                        "role_player_not_accepted",
                        "The referenced thing domain is outside the projected role domain",
                        input.type_id(),
                        role_id,
                    ));
                }
                let plan_key = if reference.iid().is_some() {
                    None
                } else {
                    reference.keys().iter().next().map(|(field_id, value)| {
                        (field_id.attribute().clone(), value.value().clone())
                    })
                };
                let key = if reference.iid().is_some() {
                    None
                } else {
                    let mut keys = reference.keys().iter();
                    let Some((field_id, value)) = keys.next() else {
                        return Err(invalid_input_at_role(
                            "ambiguous_reference_keys",
                            "A key backed relation reference requires exactly one projected key",
                            input.type_id(),
                            role_id,
                        ));
                    };
                    if keys.next().is_some() {
                        return Err(invalid_input_at_role(
                            "ambiguous_reference_keys",
                            "A key backed relation reference requires exactly one projected key",
                            input.type_id(),
                            role_id,
                        ));
                    }
                    Some((
                        field_id.attribute().label().as_str().to_owned(),
                        value.to_attribute_value(),
                    ))
                };
                output.push(PreparedRolePlayerInput {
                    role_id: role_id.clone(),
                    role_name: descriptor_role.role_name.clone(),
                    candidate_types,
                    iid: reference.iid().map(str::to_owned),
                    key,
                    plan_key,
                    database_identity: reference.database_identity().cloned(),
                });
            }
        }
        Ok(output)
    }

    fn concrete_role_player_types(
        &self,
        referenced_type: &TypeId,
        projected_players: &BTreeSet<ProjectedModelUse>,
        token: &RoleTokenProjection,
    ) -> Vec<TypeId> {
        self.installed
            .projection()
            .models()
            .values()
            .filter(|candidate| {
                matches!(candidate.id().kind(), TypeKind::Entity | TypeKind::Relation)
                    && !candidate.declaration().is_abstract()
                    && candidate.declaration().is_constructible()
                    && is_same_or_subtype(self.installed, candidate.id(), referenced_type)
                    && projected_players.iter().any(|allowed| {
                        is_same_or_subtype(self.installed, candidate.id(), allowed.id())
                    })
                    && token
                        .accepted_players()
                        .iter()
                        .any(|allowed| is_same_or_subtype(self.installed, candidate.id(), allowed))
            })
            .map(|candidate| candidate.id().clone())
            .collect()
    }

    fn model_integrity(
        &self,
        type_id: &TypeId,
    ) -> Result<&ModelProjection, SdkExecutionDiagnostic> {
        self.installed
            .projection()
            .models()
            .get(type_id)
            .ok_or_else(|| {
                integrity_at_type(
                    "runtime_projection_mismatch",
                    "The installed runtime projection omits the selected model",
                    type_id,
                )
            })
    }

    async fn insert_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let (Some(write), Ok(authority)) =
            (&prepared.plan_write, self.installed.plan_authority())
        {
            let type_id = &prepared.type_id;
            let transport = plan_transport(transaction, type_id, mode).await?;
            if let Some(plan) = plan_or_released(
                entity_insert(&authority, transport, type_id, &write.created),
                type_id,
            )? {
                let documents =
                    execute_write_rows(transaction, plan.local(), type_id, mode).await?;
                let hydrated =
                    self.hydrate_entity_documents(transaction, prepared, &documents, mode);
                return after_write(transaction, hydrated).await;
            }
        }
        let manager = DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        let iid = manager
            .insert(&prepared.attributes)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        let row = manager
            .get_by_iid_exact(&iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?
            .ok_or_else(|| mutation_rehydration_missing(&prepared.type_id))?;
        self.hydrate_entity(
            row,
            &prepared.type_id,
            &iid,
            transaction.execution_identity(),
            mode,
        )
    }

    async fn put_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let manager = DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        // Without a plan authority the released path runs unchanged.
        let authority = prepared
            .keyed_put
            .as_ref()
            .and_then(|put| Some((put, self.installed.plan_authority().ok()?)));
        if let Some((put, authority)) = authority
            && let Some(thing) = self
                .put_entity_by_plan(transaction, prepared, put, &authority, mode)
                .await?
        {
            return Ok(thing);
        }
        let iid = manager
            .put_exact(&prepared.attributes)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        self.rehydrate_entity(&manager, transaction, &prepared.type_id, &iid, mode)
            .await
    }

    async fn rehydrate_entity(
        &self,
        manager: &DynamicEntityManager<'_>,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let row = manager
            .get_by_iid_exact(iid)
            .await
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?
            .ok_or_else(|| mutation_rehydration_missing(type_id))?;
        self.hydrate_entity(row, type_id, iid, transaction.execution_identity(), mode)
    }

    async fn get_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let manager = DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        manager
            .get_by_iid_exact(&prepared.iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?
            .map(|row| {
                self.hydrate_entity(
                    row,
                    &prepared.type_id,
                    &prepared.iid,
                    transaction.execution_identity(),
                    mode,
                )
            })
            .transpose()
    }

    async fn update_entity_prepared(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        prepared: &PreparedEntityCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if let (Some(write), Ok(authority)) =
            (&prepared.plan_write, self.installed.plan_authority())
        {
            let type_id = &prepared.type_id;
            let transport = plan_transport(transaction, type_id, mode).await?;
            let built = update_by_iid(
                &authority,
                transport,
                ThingUpdate {
                    type_id,
                    iid,
                    cleared_fields: &write.cleared_fields,
                    fields: &write.fields,
                    cleared_roles: &[],
                    players: &[],
                    document_roles: Vec::new(),
                },
            );
            if let Some(plan) = plan_or_released(built, type_id)? {
                let documents =
                    execute_write_rows(transaction, plan.local(), type_id, mode).await?;
                // No document: the IID matched nothing, so nothing was written.
                if documents.is_empty() {
                    return Err(mutation_rehydration_missing(type_id));
                }
                let hydrated =
                    self.hydrate_entity_documents(transaction, prepared, &documents, mode);
                return after_write(transaction, hydrated).await;
            }
        }
        let manager = DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        manager
            .update_exact(iid, &prepared.attributes)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        let row = manager
            .get_by_iid_exact(iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?
            .ok_or_else(|| mutation_rehydration_missing(&prepared.type_id))?;
        self.hydrate_entity(
            row,
            &prepared.type_id,
            iid,
            transaction.execution_identity(),
            mode,
        )
    }

    async fn delete_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityIdentity,
        mode: ExecutionMode<'_>,
        returning: bool,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let authority = self.installed.plan_authority();
        if returning || authority.is_ok() {
            let authority =
                authority.map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
            let roles = if returning { Some(Vec::new()) } else { None };
            let documents = execute_delete_plan(
                transaction,
                &authority,
                &prepared.type_id,
                &prepared.iid,
                roles,
                mode,
            )
            .await?;
            if documents.is_empty() {
                return Ok(None);
            }
            let hydrate = || {
                let mut thing = {
                    if documents.len() != 1 {
                        return Err(identity_cardinality(&prepared.type_id));
                    }
                    let row = hydrate_dynamic_entity(&prepared.descriptor, &documents[0]).map_err(
                        |error| {
                            mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
                        },
                    )?;
                    self.hydrate_entity(
                        row,
                        &prepared.type_id,
                        &prepared.iid,
                        transaction.execution_identity(),
                        mode,
                    )?
                };
                thing.mark_deleted()?;
                Ok(Some(thing))
            };
            return after_write(transaction, hydrate()).await;
        }
        DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        )
        .delete_by_iid_exact(&prepared.iid)
        .await
        .map(|()| None)
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id))
    }

    async fn count_entities_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityType,
        mode: ExecutionMode<'_>,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        )
        .count_exact()
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))
    }

    async fn insert_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let manager = DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        // A relation without players has nothing to resolve, and without a
        // plan authority the released path runs unchanged.
        let authority = if prepared.role_players.is_empty() {
            None
        } else {
            self.installed.plan_authority().ok()
        };
        if let Some(authority) = authority
            && let Some(thing) = self
                .insert_relation_by_plan(transaction, prepared, &authority, mode)
                .await?
        {
            return Ok(thing);
        }
        let iid = if let Some(role_players) = direct_entity_role_player_inputs(prepared) {
            // Preserve the released one-query path for exact concrete entity
            // references. Abstract domains and relation-as-player references
            // require the projected kind-aware resolver below.
            let query = query_builder::build_projected_relation_insert_with_iid(
                &prepared.descriptor,
                &prepared.attributes,
                &role_players,
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            let answer = transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            extract_one_provider_iid(answer, &prepared.type_id, None)?
        } else {
            let resolved = self
                .resolve_relation_players_exact(transaction, prepared, mode)
                .await?;
            let query = query_builder::build_projected_relation_insert_resolved_with_iid(
                &prepared.descriptor,
                &prepared.attributes,
                &resolved_role_player_tuples(&resolved),
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            let answer = transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            extract_one_provider_iid(answer, &prepared.type_id, None)?
        };
        self.rehydrate_relation(&manager, transaction, &prepared.type_id, &iid, mode)
            .await
    }

    async fn rehydrate_relation(
        &self,
        manager: &DynamicRelationManager<'_>,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let rows = manager
            .get_by_iid_exact(iid)
            .await
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let row = require_mutation_relation_row(rows, type_id)?;
        self.hydrate_relation(row, type_id, iid, transaction.execution_identity(), mode)
    }

    /// Insert one relation as one write plan whose match resolves every
    /// role-player reference and whose fetch answers the hydration document,
    /// so nothing is read before or after the write.
    ///
    /// Zero documents mean a reference matched nothing and nothing was
    /// written; that keeps the released missing-reference diagnostic. Every
    /// failure after that is raised after writes, so it poisons a borrowed
    /// transaction: several relations (a polymorphic reference matched
    /// several things), two references of one role resolving to one player
    /// (which the released resolver refused before writing), or hydration.
    async fn insert_relation_by_plan(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        authority: &PlanAuthority,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        let resolved_path = direct_entity_role_player_inputs(prepared).is_none();
        if resolved_path {
            reject_duplicate_references(prepared)?;
        }
        let transport = plan_transport(transaction, type_id, mode).await?;
        let players = plan_references(prepared)?;
        let document_roles = self.document_roles(prepared)?;
        let plan = relation_insert(
            authority,
            transport,
            type_id,
            &prepared.plan_fields,
            &players,
            document_roles,
        );
        let Some(plan) = plan_or_released(plan, type_id)? else {
            return Ok(None);
        };
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        if documents.is_empty() {
            return Err(identity_cardinality(type_id));
        }
        let hydrate = || {
            let iid = single_document_iid(&documents, type_id)?;
            if resolved_path {
                reject_resolved_duplicates(prepared, &documents)?;
            }
            let rows = coalesce_dynamic_relation_by_iid(&prepared.descriptor, &documents, &iid)
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
            let row = require_mutation_relation_row(rows, type_id)?;
            self.hydrate_relation(row, type_id, &iid, transaction.execution_identity(), mode)
        };
        after_write(transaction, hydrate()).await.map(Some)
    }

    /// Every descriptor role in descriptor order (the document key order of
    /// the released read-back), all listed optional: the fetch then yields a
    /// document for every inserted relation, so zero documents strictly
    /// mean nothing was inserted. Role cardinality is checked by hydration.
    fn document_roles(
        &self,
        prepared: &PreparedRelationCreate,
    ) -> Result<Vec<DocumentRole>, SdkExecutionDiagnostic> {
        self.document_roles_for(&prepared.type_id, &prepared.descriptor)
    }

    fn document_roles_for(
        &self,
        type_id: &TypeId,
        descriptor: &RelationDescriptor,
    ) -> Result<Vec<DocumentRole>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(type_id)?;
        descriptor
            .roles
            .iter()
            .map(|descriptor_role| {
                // A relation's role labels are unique (inherited roles keep
                // their declaring relation but not a second label), so the
                // label finds the one projected role.
                let role = model
                    .query_tokens()
                    .roles()
                    .keys()
                    .find(|role| role.label().as_str() == descriptor_role.role_name)
                    .ok_or_else(|| write_plan_invalid(type_id))?;
                Ok(DocumentRole::new(role.clone(), false))
            })
            .collect()
    }

    /// Put one keyed entity as one write plan that answers its hydration
    /// document: no key lookup precedes it and no read-back follows it.
    async fn put_entity_by_plan(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityCreate,
        put: &PreparedKeyedPut,
        authority: &PlanAuthority,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        let transport = plan_transport(transaction, type_id, mode).await?;
        let plan = keyed_entity_put(
            authority,
            transport,
            &KeyedPut {
                type_id,
                keys: &put.keys,
                fields: &put.fields,
                cleared: &put.cleared,
                attached: &put.attached,
            },
        );
        let Some(plan) = plan_or_released(plan, type_id)? else {
            return Ok(None);
        };
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrate = || {
            let iid = single_document_iid(&documents, type_id)?;
            let row = hydrate_dynamic_entity(&prepared.descriptor, &documents[0])
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
            self.hydrate_entity(row, type_id, &iid, transaction.execution_identity(), mode)
        };
        after_write(transaction, hydrate()).await.map(Some)
    }

    fn hydrate_entity_documents(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityCreate,
        documents: &[serde_json::Value],
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        // An insert always answers its row; none means it was not returned.
        if documents.is_empty() {
            return Err(mutation_rehydration_missing(type_id));
        }
        let iid = single_document_iid(documents, type_id)?;
        let row = hydrate_dynamic_entity(&prepared.descriptor, &documents[0])
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        self.hydrate_entity(row, type_id, &iid, transaction.execution_identity(), mode)
    }

    /// Update one relation as one plan: match it and every reference,
    /// clear its non-key fields and every role, attach the given values and
    /// players, and fetch the result. Nothing is read before or after.
    async fn update_relation_by_plan(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        prepared: &PreparedRelationCreate,
        write: &PreparedPlanWrite,
        authority: &PlanAuthority,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        let resolved_path = direct_entity_role_player_inputs(prepared).is_none();
        if resolved_path {
            reject_duplicate_references(prepared)?;
        }
        let transport = plan_transport(transaction, type_id, mode).await?;
        let players = plan_references(prepared)?;
        let plan = update_by_iid(
            authority,
            transport,
            ThingUpdate {
                type_id,
                iid,
                cleared_fields: &write.cleared_fields,
                fields: &write.fields,
                cleared_roles: &write.cleared_roles,
                players: &players,
                document_roles: self.document_roles(prepared)?,
            },
        );
        let Some(plan) = plan_or_released(plan, type_id)? else {
            return Ok(None);
        };
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        // No document: the IID or a reference matched nothing, so nothing
        // was written. Only on this failure path, one read tells which, to
        // keep the released codes.
        if documents.is_empty() {
            let manager = DynamicRelationManager::with_canonical_transaction(
                transaction.clone(),
                Arc::clone(&prepared.descriptor),
            );
            let present = manager
                .get_by_iid_exact(iid)
                .await
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
            return Err(if present.is_empty() {
                mutation_rehydration_missing(type_id)
            } else if resolved_path {
                identity_cardinality(type_id)
            } else {
                mode.orm_at_type(
                    crate::OrmError::Hydration {
                        type_name: prepared.descriptor.type_name.clone(),
                        message: "A relation player reference matched no thing".into(),
                    },
                    SdkProviderOperation::Write,
                    type_id,
                )
            });
        }
        let hydrate = || {
            let iid = single_document_iid(&documents, type_id)?;
            if resolved_path {
                reject_resolved_duplicates(prepared, &documents)?;
            }
            let rows = coalesce_dynamic_relation_by_iid(&prepared.descriptor, &documents, &iid)
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
            let row = require_mutation_relation_row(rows, type_id)?;
            self.hydrate_relation(row, type_id, &iid, transaction.execution_identity(), mode)
        };
        after_write(transaction, hydrate()).await.map(Some)
    }

    async fn resolve_relation_players_exact(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<Vec<ResolvedRolePlayerInput>, SdkExecutionDiagnostic> {
        let mut resolved = Vec::with_capacity(prepared.role_players.len());
        for player in &prepared.role_players {
            let mut matches = Vec::new();
            for candidate_type in &player.candidate_types {
                let query = query_builder::build_projected_role_player_lookup(
                    candidate_type.label().as_str(),
                    candidate_type.kind(),
                    player.iid.as_deref(),
                    player
                        .key
                        .as_ref()
                        .map(|(name, value)| (name.as_str(), value)),
                )
                .map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?;
                let answer = transaction.query_canonical(&query).await.map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?;
                if let Some(iid) =
                    extract_optional_provider_iid(answer, &prepared.type_id, player.iid.as_deref())?
                {
                    matches.push((candidate_type.clone(), iid));
                }
            }
            let [(player_type, iid)] = matches.as_slice() else {
                return Err(integrity_at_type(
                    "provider_identity_cardinality_invalid",
                    "The provider identity query did not resolve exactly one thing",
                    &prepared.type_id,
                ));
            };
            if resolved.iter().any(|existing: &ResolvedRolePlayerInput| {
                existing.role_name == player.role_name && existing.iid == *iid
            }) {
                return Err(integrity_at_type(
                    "duplicate_resolved_role_player",
                    "Exact relation references resolved to a duplicate role player",
                    &prepared.type_id,
                ));
            }
            resolved.push(ResolvedRolePlayerInput {
                player_type: player_type.clone(),
                iid: iid.clone(),
                role_name: player.role_name.clone(),
            });
        }
        Ok(resolved)
    }

    async fn put_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let manager = DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        let iid = if let Some(role_players) = direct_entity_role_player_inputs(prepared) {
            manager
                .put_exact(&prepared.attributes, &role_players)
                .await
                .map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?
        } else {
            self.put_projected_relation_state(transaction, prepared, mode)
                .await?
        };
        let rows = manager.get_by_iid_exact(&iid).await.map_err(|error| {
            mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
        })?;
        let row = require_mutation_relation_row(rows, &prepared.type_id)?;
        self.hydrate_relation(
            row,
            &prepared.type_id,
            &iid,
            transaction.execution_identity(),
            mode,
        )
    }

    async fn get_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let manager = DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        let rows = manager
            .get_by_iid_exact(&prepared.iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?;
        match rows.as_slice() {
            [] => Ok(None),
            [row] => self
                .hydrate_relation(
                    row.clone(),
                    &prepared.type_id,
                    &prepared.iid,
                    transaction.execution_identity(),
                    mode,
                )
                .map(Some),
            _ => Err(relation_ambiguity(&prepared.type_id)),
        }
    }

    async fn update_relation_prepared(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        // Without players the released path runs: it refuses a role below
        // its minimum, where clearing every role would delete the relation.
        if !prepared.role_players.is_empty()
            && let (Some(write), Ok(authority)) =
                (&prepared.plan_write, self.installed.plan_authority())
            && let Some(thing) = self
                .update_relation_by_plan(transaction, iid, prepared, write, &authority, mode)
                .await?
        {
            return Ok(thing);
        }
        let manager = DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        if let Some(role_players) = direct_entity_role_player_inputs(prepared) {
            manager
                .update_exact(iid, &prepared.attributes, &role_players)
                .await
                .map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?;
        } else {
            let resolved = self
                .resolve_relation_players_exact(transaction, prepared, mode)
                .await?;
            self.update_projected_relation_state(transaction, iid, prepared, &resolved, mode)
                .await?;
        }
        let rows = manager.get_by_iid_exact(iid).await.map_err(|error| {
            mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
        })?;
        let row = require_mutation_relation_row(rows, &prepared.type_id)?;
        self.hydrate_relation(
            row,
            &prepared.type_id,
            iid,
            transaction.execution_identity(),
            mode,
        )
    }

    async fn put_projected_relation_state(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<String, SdkExecutionDiagnostic> {
        let existing_iid = if let Some(query) =
            query_builder::build_dynamic_relation_exact_key_lookup(
                &prepared.descriptor,
                &prepared.attributes,
                "$r",
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })? {
            let answer = transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            extract_optional_provider_iid(answer, &prepared.type_id, None)?
        } else {
            None
        };
        let resolved = self
            .resolve_relation_players_exact(transaction, prepared, mode)
            .await?;
        if let Some(iid) = existing_iid {
            self.update_projected_relation_state(transaction, &iid, prepared, &resolved, mode)
                .await?;
            Ok(iid)
        } else {
            let query = query_builder::build_projected_relation_insert_resolved_with_iid(
                &prepared.descriptor,
                &prepared.attributes,
                &resolved_role_player_tuples(&resolved),
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            let answer = transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            extract_one_provider_iid(answer, &prepared.type_id, None)
        }
    }

    async fn update_projected_relation_state(
        &self,
        transaction: &TransactionContext,
        iid: &str,
        prepared: &PreparedRelationCreate,
        resolved: &[ResolvedRolePlayerInput],
        mode: ExecutionMode<'_>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if !prepared
            .descriptor
            .owned_attributes
            .iter()
            .all(|attribute| attribute.is_key())
        {
            let query = query_builder::build_dynamic_relation_update_exact(
                &prepared.descriptor,
                iid,
                &prepared.attributes,
                "$r",
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        }
        for role in &prepared.descriptor.roles {
            let query = query_builder::build_dynamic_relation_clear_role(
                &prepared.descriptor,
                iid,
                &role.role_name,
                "$r",
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        }
        if !resolved.is_empty() {
            let query = query_builder::build_projected_relation_attach(
                &prepared.descriptor,
                iid,
                &resolved_role_player_tuples(resolved),
            )
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
            transaction.query_canonical(&query).await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })?;
        }
        Ok(())
    }

    async fn delete_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationIdentity,
        mode: ExecutionMode<'_>,
        returning: bool,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let authority = self.installed.plan_authority();
        if returning || authority.is_ok() {
            let authority =
                authority.map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
            let roles = if returning {
                Some(self.document_roles_for(&prepared.type_id, &prepared.descriptor)?)
            } else {
                None
            };
            let documents = execute_delete_plan(
                transaction,
                &authority,
                &prepared.type_id,
                &prepared.iid,
                roles,
                mode,
            )
            .await?;
            if documents.is_empty() {
                return Ok(None);
            }
            let hydrate = || {
                let mut thing = {
                    let row = coalesce_dynamic_relation_by_iid(
                        &prepared.descriptor,
                        &documents,
                        &prepared.iid,
                    )
                    .map_err(|error| {
                        mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
                    })?;
                    let row = require_mutation_relation_row(row, &prepared.type_id)?;
                    self.hydrate_relation(
                        row,
                        &prepared.type_id,
                        &prepared.iid,
                        transaction.execution_identity(),
                        mode,
                    )?
                };
                thing.mark_deleted()?;
                Ok(Some(thing))
            };
            return after_write(transaction, hydrate()).await;
        }
        DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        )
        .delete_by_iid_exact(&prepared.iid)
        .await
        .map(|()| None)
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id))
    }

    async fn count_relations_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationType,
        mode: ExecutionMode<'_>,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        )
        .count_exact()
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))
    }

    fn hydrate_entity(
        &self,
        row: DynamicEntityRow,
        type_id: &TypeId,
        requested_iid: &str,
        database_identity: &DatabaseExecutionIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let iid = exact_row_identity(row.iid, row.type_name, type_id, requested_iid)?;
        let fields = self.hydrate_fields(type_id, &row.attributes, mode)?;
        ProjectedThing::try_new_for_database(
            self.installed,
            type_id.clone(),
            iid,
            fields,
            Vec::new(),
            database_identity.clone(),
        )
        .map_err(provider_integrity)
    }

    fn hydrate_relation(
        &self,
        row: DynamicRelationRow,
        type_id: &TypeId,
        requested_iid: &str,
        database_identity: &DatabaseExecutionIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        let iid = exact_row_identity(row.iid, row.type_name, type_id, requested_iid)?;
        let fields = self.hydrate_fields(type_id, &row.attributes, mode)?;
        let roles = self.hydrate_roles(type_id, &row.role_players, database_identity, mode)?;
        ProjectedThing::try_new_for_database(
            self.installed,
            type_id.clone(),
            iid,
            fields,
            roles,
            database_identity.clone(),
        )
        .map_err(provider_integrity)
    }

    fn hydrate_fields(
        &self,
        type_id: &TypeId,
        attributes: &DynamicAttributeMap,
        _mode: ExecutionMode<'_>,
    ) -> Result<Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(type_id)?;
        let mut by_provider_label = BTreeMap::<&str, Vec<&OwnsFactId>>::new();
        for field in model.complete_read().fields() {
            by_provider_label
                .entry(field.token().attribute().label().as_str())
                .or_default()
                .push(field.token());
        }
        let mut evidence = BTreeMap::<OwnsFactId, Vec<&AttributeValue>>::new();
        for (label, value) in attributes {
            let candidates = by_provider_label
                .get(label.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [field_id] = candidates else {
                return Err(integrity_at_type(
                    if candidates.is_empty() {
                        "unexpected_provider_attribute"
                    } else {
                        "ambiguous_provider_attribute"
                    },
                    if candidates.is_empty() {
                        "The provider row contains an unprojected ownership"
                    } else {
                        "The provider ownership maps to more than one projected field"
                    },
                    type_id,
                ));
            };
            evidence.entry((*field_id).clone()).or_default().push(value);
        }

        let mut fields = Vec::new();
        for read_field in model.complete_read().fields() {
            let field_id = read_field.token().clone();
            let attribute_type = attribute_type(&field_id);
            let mut projected = Vec::new();
            for value in evidence.remove(&field_id).unwrap_or_default() {
                let canonical = attribute_to_canonical(value).map_err(|()| {
                    integrity_at_field(
                        "provider_scalar_invalid",
                        "The provider ownership value is outside its canonical scalar domain",
                        type_id,
                        &field_id,
                    )
                })?;
                projected.push(
                    ProjectedAttributeValue::try_new(
                        self.installed,
                        attribute_type.clone(),
                        canonical,
                    )
                    .map_err(provider_integrity)?,
                );
            }
            fields.push((field_id, projected));
        }
        Ok(fields)
    }

    fn hydrate_roles(
        &self,
        type_id: &TypeId,
        players: &[DynamicRolePlayer],
        database_identity: &DatabaseExecutionIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<Vec<(RoleId, Vec<ProjectedRolePlayer>)>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(type_id)?;
        let descriptor = self
            .installed
            .relation_descriptor(type_id)
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let mut by_label = BTreeMap::<&str, Vec<&RoleId>>::new();
        for role_id in model.complete_read().roles().keys() {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_token_missing",
                    "The complete read facet refers to an absent projected role token",
                    type_id,
                    role_id,
                )
            })?;
            by_label
                .entry(token.role().label().as_str())
                .or_default()
                .push(role_id);
        }
        let mut evidence = BTreeMap::<RoleId, Vec<&DynamicRolePlayer>>::new();
        for player in players {
            let candidates = by_label
                .get(player.role_name.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [role_id] = candidates else {
                return Err(integrity_at_type(
                    if candidates.is_empty() {
                        "unexpected_provider_role"
                    } else {
                        "ambiguous_provider_role"
                    },
                    if candidates.is_empty() {
                        "The provider row contains an unprojected active role"
                    } else {
                        "The provider role maps to more than one projected role"
                    },
                    type_id,
                ));
            };
            evidence.entry((*role_id).clone()).or_default().push(player);
        }

        for (role_id, role_players) in &evidence {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_token_missing",
                    "The complete read facet refers to an absent projected role token",
                    type_id,
                    role_id,
                )
            })?;
            if token
                .annotations()
                .keys()
                .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct)
                && let Some((first_index, duplicate_index)) =
                    first_provider_player_duplicate(role_players)
            {
                return Err(ordered_distinct_duplicate_at_role(
                    type_id,
                    role_id,
                    first_index,
                    duplicate_index,
                ));
            }
        }

        let mut roles = Vec::new();
        for (role_id, read_role) in model.complete_read().roles() {
            let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_token_missing",
                    "The complete read facet refers to an absent projected role token",
                    type_id,
                    role_id,
                )
            })?;
            let descriptor_role = unique_descriptor_role(descriptor, token).ok_or_else(|| {
                integrity_at_role(
                    "projected_role_descriptor_mismatch",
                    "The projected role does not have one exact provider descriptor",
                    type_id,
                    role_id,
                )
            })?;
            if !descriptor_role_matches_token(descriptor_role, token) {
                return Err(integrity_at_role(
                    "projected_role_descriptor_mismatch",
                    "The projected role-player domain does not match its provider descriptor",
                    type_id,
                    role_id,
                ));
            }
            let mut hydrated = Vec::new();
            for (index, player) in evidence
                .remove(role_id)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                hydrated.push(self.hydrate_role_player(
                    type_id,
                    role_id,
                    index,
                    read_role,
                    token,
                    player,
                    database_identity,
                    mode,
                )?);
            }
            roles.push((role_id.clone(), hydrated));
        }
        Ok(roles)
    }

    #[allow(clippy::too_many_arguments)]
    fn hydrate_role_player(
        &self,
        relation_type: &TypeId,
        role_id: &RoleId,
        index: usize,
        read_role: &ReadRoleProjection,
        token: &RoleTokenProjection,
        player: &DynamicRolePlayer,
        database_identity: &DatabaseExecutionIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<ProjectedRolePlayer, SdkExecutionDiagnostic> {
        let iid = player.player_iid.as_deref().ok_or_else(|| {
            integrity_at_role(
                "hydrated_player_iid_missing",
                "A provider role player omitted its canonical IID",
                relation_type,
                role_id,
            )
        })?;
        if !is_canonical_thing_iid(iid) {
            return Err(integrity_at_role(
                "hydrated_player_iid_invalid",
                "A provider role player returned a noncanonical IID",
                relation_type,
                role_id,
            ));
        }
        let type_name = player.player_type_name.as_deref().ok_or_else(|| {
            integrity_at_role(
                "hydrated_player_type_missing",
                "A provider role player omitted its exact concrete type",
                relation_type,
                role_id,
            )
        })?;
        let candidates =
            self.installed
                .projection()
                .models()
                .values()
                .filter(|candidate| {
                    candidate.id().label().as_str() == type_name
                        && matches!(candidate.id().kind(), TypeKind::Entity | TypeKind::Relation)
                        && !candidate.declaration().is_abstract()
                        && candidate.declaration().is_constructible()
                        && read_role.players().iter().any(|allowed| {
                            is_same_or_subtype(self.installed, candidate.id(), allowed.id())
                        })
                        && token.accepted_players().iter().any(|allowed| {
                            is_same_or_subtype(self.installed, candidate.id(), allowed)
                        })
                })
                .map(ModelProjection::id)
                .collect::<Vec<_>>();
        let [player_type] = candidates.as_slice() else {
            return Err(integrity_at_role(
                if candidates.is_empty() {
                    "hydrated_role_player_not_accepted"
                } else {
                    "projected_player_type_ambiguous"
                },
                if candidates.is_empty() {
                    "The provider role player type is outside the projected role domain"
                } else {
                    "The provider role player label does not select one projected type"
                },
                relation_type,
                role_id,
            ));
        };
        self.require_exact_model(player_type, player_type.kind())
            .map_err(provider_integrity)?;
        let form = ProjectedRolePlayer::form_for_read_role(self.installed, read_role, player_type)
            .map_err(provider_integrity)?;
        let attributes = self
            .installed
            .role_player_attributes(player_type, &player.attributes)
            .map_err(|error| {
                let diagnostic = mode.orm_at_type(error, SdkProviderOperation::Read, relation_type);
                append_path(
                    diagnostic,
                    [
                        SdkDiagnosticPathSegment::Role(role_id.clone()),
                        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
                        SdkDiagnosticPathSegment::Type((*player_type).clone()),
                    ],
                )
            })?;
        let player_model = self.model_integrity(player_type)?;
        let fields = match form {
            ProjectedModelForm::Complete => self.hydrate_fields(player_type, &attributes, mode)?,
            ProjectedModelForm::Reference => Vec::new(),
        };
        let keys = match form {
            ProjectedModelForm::Complete => fields
                .iter()
                .flat_map(|(field, values)| {
                    values
                        .iter()
                        .cloned()
                        .map(move |value| (field.clone(), value))
                })
                .filter(|(field, _)| player_model.reference_read().key_fields().contains(field))
                .collect(),
            ProjectedModelForm::Reference => {
                self.hydrate_reference_keys(player_type, player_model, &attributes)?
            }
        };
        let reference = ProjectedReference::try_new_for_database(
            self.installed,
            (*player_type).clone(),
            Some(iid.to_owned()),
            keys,
            database_identity.clone(),
        )
        .map_err(provider_integrity)?;
        match form {
            ProjectedModelForm::Complete => ProjectedRolePlayer::try_new_complete_for_hydration(
                self.installed,
                read_role,
                reference,
                fields,
            ),
            ProjectedModelForm::Reference => ProjectedRolePlayer::try_new_reference_for_hydration(
                self.installed,
                read_role,
                reference,
            ),
        }
        .map_err(provider_integrity)
    }

    fn hydrate_reference_keys(
        &self,
        player_type: &TypeId,
        player_model: &ModelProjection,
        attributes: &DynamicAttributeMap,
    ) -> Result<Vec<(OwnsFactId, ProjectedAttributeValue)>, SdkExecutionDiagnostic> {
        let mut keys = Vec::new();
        for field_id in player_model.reference_read().key_fields() {
            let values = attributes
                .iter()
                .filter(|(label, _)| label == field_id.attribute().label().as_str())
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            match values.as_slice() {
                [] => {}
                [value] => {
                    let canonical = attribute_to_canonical(value).map_err(|()| {
                        integrity_at_field(
                            "provider_scalar_invalid",
                            "The provider reference key is outside its canonical scalar domain",
                            player_type,
                            field_id,
                        )
                    })?;
                    keys.push((
                        field_id.clone(),
                        ProjectedAttributeValue::try_new(
                            self.installed,
                            attribute_type(field_id),
                            canonical,
                        )
                        .map_err(provider_integrity)?,
                    ));
                }
                _ => {
                    return Err(integrity_at_field(
                        "duplicate_reference_key",
                        "The provider role player repeats one projected reference key",
                        player_type,
                        field_id,
                    ));
                }
            }
        }
        Ok(keys)
    }
}

/// One invocation policy captured before single-CRUD validation or allocation.
#[doc(hidden)]
pub struct ProjectedCrudInvocationControl {
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl ProjectedCrudInvocationControl {
    /// Capture effective limits, one absolute deadline, and cancellation owner.
    #[must_use]
    pub fn capture(limits: QueryExecutionResourceLimits, cancellation: AnswerCancellation) -> Self {
        let limits = limits.effective();
        Self {
            limits,
            deadline: QueryExecutionDeadline::for_limits(limits),
            cancellation,
        }
    }

    /// Recheck the captured cancellation owner and deadline at one type path.
    pub fn checkpoint(&self, type_id: &TypeId) -> Result<(), SdkExecutionDiagnostic> {
        let diagnostic = if self.cancellation.is_cancelled() {
            Some(SdkExecutionDiagnostic::data_operation_cancelled())
        } else if self.deadline.is_expired() {
            Some(SdkExecutionDiagnostic::data_operation_deadline_exceeded())
        } else {
            None
        };
        diagnostic.map_or(Ok(()), |diagnostic| {
            Err(append_path(
                diagnostic,
                [SdkDiagnosticPathSegment::Type(type_id.clone())],
            ))
        })
    }

    fn check(&self, type_id: &TypeId) -> Result<(), SdkExecutionDiagnostic> {
        self.checkpoint(type_id)
    }
}

enum CrudControlledAwaitError<E> {
    Interrupted(SdkExecutionDiagnostic),
    Inner(E),
}

async fn await_crud_controlled<F, T, E>(
    future: F,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> std::result::Result<T, CrudControlledAwaitError<E>>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(CrudControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_cancelled())),
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.instant())) => Err(CrudControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_deadline_exceeded())),
        result = &mut future => result.map_err(CrudControlledAwaitError::Inner),
    }
}

fn controlled_crud_orm(
    error: CrudControlledAwaitError<OrmError>,
    operation: SdkProviderOperation,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    match error {
        CrudControlledAwaitError::Interrupted(diagnostic) => append_path(
            diagnostic,
            [SdkDiagnosticPathSegment::Type(type_id.clone())],
        ),
        CrudControlledAwaitError::Inner(error) => orm_at_type(error, operation, type_id),
    }
}

#[derive(Clone, Copy)]
enum ControlledAnswerKind {
    Documents,
    Rows,
}

struct ControlledAnswerCollector<'type_name> {
    expected: ControlledAnswerKind,
    type_name: &'type_name str,
    values: Vec<serde_json::Value>,
}

impl<'type_name> ControlledAnswerCollector<'type_name> {
    const fn new(expected: ControlledAnswerKind, type_name: &'type_name str) -> Self {
        Self {
            expected,
            type_name,
            values: Vec::new(),
        }
    }

    fn finish(self) -> QueryResult {
        match self.expected {
            ControlledAnswerKind::Documents => QueryResult::Documents(self.values),
            ControlledAnswerKind::Rows => QueryResult::Rows(self.values),
        }
    }
}

impl AnswerConsumer for ControlledAnswerCollector<'_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        let value = match (self.expected, item) {
            (ControlledAnswerKind::Documents, AnswerItem::Document(value))
            | (ControlledAnswerKind::Rows, AnswerItem::Row(value)) => value,
            _ => {
                return Err(OrmError::Hydration {
                    type_name: self.type_name.to_owned(),
                    message: "Provider returned the wrong answer kind for controlled CRUD".into(),
                });
            }
        };
        self.values.try_reserve(1).map_err(|_| {
            OrmError::from(
                MatchError::new(
                    MatchErrorCategory::ResourceLimit,
                    "projected_crud_answer_allocation_exhausted",
                    "controlled CRUD could not reserve bounded provider-answer storage",
                )
                .at(MatchErrorPathSegment::ProviderEvidence),
            )
        })?;
        self.values.push(value);
        Ok(AnswerControl::Continue)
    }
}

struct LocallyBoundedCrudConsumer<'consumer> {
    inner: &'consumer mut dyn AnswerConsumer,
    reader: BoundedAnswerReader,
}

impl<'consumer> LocallyBoundedCrudConsumer<'consumer> {
    fn new(inner: &'consumer mut dyn AnswerConsumer, limits: BoundedAnswerLimits) -> Self {
        Self {
            inner,
            reader: BoundedAnswerReader::new(limits),
        }
    }

    const fn stats(&self) -> BoundedAnswerStats {
        self.reader.stats()
    }
}

impl AnswerConsumer for LocallyBoundedCrudConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        self.reader.accept(item, self.inner)
    }
}

struct ControlledCrudLedger {
    limits: QueryExecutionResourceLimits,
    bytes: u64,
    items: u64,
    statements: u32,
}

impl ControlledCrudLedger {
    fn try_new(
        limits: QueryExecutionResourceLimits,
        input_bytes: u64,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        if input_bytes > limits.bytes {
            return Err(crud_resource_limit(
                "projected_crud_input_byte_limit",
                "Controlled CRUD input exceeds the captured byte ceiling",
                "bytes",
                input_bytes,
                limits.bytes,
            ));
        }
        Ok(Self {
            limits,
            bytes: input_bytes,
            items: 0,
            statements: 0,
        })
    }

    fn charge_statement(&mut self, request_bytes: u64) -> Result<(), SdkExecutionDiagnostic> {
        let statements = self.statements.saturating_add(1);
        if statements > self.limits.statements {
            return Err(crud_resource_limit(
                "projected_crud_statement_limit",
                "Controlled CRUD exceeds the provider statement ceiling",
                "statements",
                u64::from(statements),
                u64::from(self.limits.statements),
            ));
        }
        self.charge_bytes(
            request_bytes,
            "projected_crud_transport_byte_limit",
            "Controlled CRUD request transport exceeds the captured byte ceiling",
        )?;
        self.statements = statements;
        Ok(())
    }

    fn record_reply(&mut self, stats: BoundedAnswerStats) -> Result<(), SdkExecutionDiagnostic> {
        let items = self.items.saturating_add(stats.processed_items);
        if items > self.limits.items {
            return Err(crud_resource_limit(
                "projected_crud_reply_item_limit",
                "Controlled CRUD provider answers exceed the captured item ceiling",
                "items",
                items,
                self.limits.items,
            ));
        }
        self.items = items;
        self.charge_bytes(
            stats.response_bytes,
            "projected_crud_reply_byte_limit",
            "Controlled CRUD provider answers exceed the captured byte ceiling",
        )
    }

    const fn remaining_bytes(&self) -> u64 {
        self.limits.bytes.saturating_sub(self.bytes)
    }

    const fn remaining_items(&self) -> u64 {
        self.limits.items.saturating_sub(self.items)
    }

    fn charge_projected(&mut self, thing: &ProjectedThing) -> Result<(), SdkExecutionDiagnostic> {
        let bytes = u64::try_from(thing.resource_measure().bytes()).unwrap_or(u64::MAX);
        let mut graph_nodes = 1_u64;
        let mut attribute_values = 0_u64;
        let mut collection_members = 0_u64;
        let mut role_players = 0_u64;
        for values in thing.fields().values() {
            let count = u64::try_from(values.len()).unwrap_or(u64::MAX);
            attribute_values = attribute_values.saturating_add(count);
            collection_members = collection_members.saturating_add(count);
        }
        for players in thing.roles().values() {
            let count = u64::try_from(players.len()).unwrap_or(u64::MAX);
            graph_nodes = graph_nodes.saturating_add(count);
            role_players = role_players.saturating_add(count);
            collection_members = collection_members.saturating_add(count);
            for player in players {
                for values in player.fields().values() {
                    let count = u64::try_from(values.len()).unwrap_or(u64::MAX);
                    attribute_values = attribute_values.saturating_add(count);
                    collection_members = collection_members.saturating_add(count);
                }
                if player.fields().is_empty() {
                    let count = u64::try_from(player.keys().len()).unwrap_or(u64::MAX);
                    attribute_values = attribute_values.saturating_add(count);
                    collection_members = collection_members.saturating_add(count);
                }
            }
        }
        for (dimension, actual, maximum) in [
            ("graph_nodes", graph_nodes, self.limits.graph_nodes),
            (
                "attribute_values",
                attribute_values,
                self.limits.attribute_values,
            ),
            (
                "collection_members",
                collection_members,
                self.limits.collection_members,
            ),
            ("role_players", role_players, self.limits.role_players),
        ] {
            if actual > maximum {
                return Err(crud_resource_limit(
                    "projected_crud_output_resource_limit",
                    "Controlled CRUD output exceeds a captured resource ceiling",
                    dimension,
                    actual,
                    maximum,
                ));
            }
        }
        self.charge_bytes(
            bytes,
            "projected_crud_output_byte_limit",
            "Controlled CRUD projected output exceeds the captured byte ceiling",
        )
    }

    fn charge_scalar_bytes(&mut self, bytes: u64) -> Result<(), SdkExecutionDiagnostic> {
        self.charge_bytes(
            bytes,
            "projected_crud_output_byte_limit",
            "Controlled CRUD scalar output exceeds the captured byte ceiling",
        )
    }

    fn charge_bytes(
        &mut self,
        amount: u64,
        code: &'static str,
        message: &'static str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let actual = self.bytes.saturating_add(amount);
        if actual > self.limits.bytes {
            return Err(crud_resource_limit(
                code,
                message,
                "bytes",
                actual,
                self.limits.bytes,
            ));
        }
        self.bytes = actual;
        Ok(())
    }
}

fn single_batch_thing(
    result: ProjectedBatchResult,
    type_id: &TypeId,
) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
    let ProjectedBatchResult::Things(mut things) = result else {
        return Err(single_batch_result_invalid(type_id));
    };
    if things.len() != 1 {
        return Err(single_batch_result_invalid(type_id));
    }
    things
        .pop()
        .ok_or_else(|| single_batch_result_invalid(type_id))
}

fn single_batch_deleted_thing(
    result: ProjectedBatchResult,
    type_id: &TypeId,
) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
    match result {
        ProjectedBatchResult::Things(mut things) if things.len() <= 1 => Ok(things.pop()),
        _ => Err(single_batch_result_invalid(type_id)),
    }
}

fn single_batch_delete(
    result: ProjectedBatchResult,
    type_id: &TypeId,
) -> Result<(), SdkExecutionDiagnostic> {
    match result {
        ProjectedBatchResult::Deleted => Ok(()),
        ProjectedBatchResult::Things(_) => Err(single_batch_result_invalid(type_id)),
    }
}

fn single_batch_result_invalid(type_id: &TypeId) -> SdkExecutionDiagnostic {
    integrity_at_type(
        "projected_crud_batch_result_invalid",
        "The common batch executor returned an invalid single-CRUD result shape",
        type_id,
    )
}

fn controlled_read_input_bytes(type_id: &TypeId, iid: Option<&str>) -> u64 {
    u64::try_from(
        1_usize
            .saturating_add(type_id.label().as_str().len())
            .saturating_add(iid.map_or(0, str::len)),
    )
    .unwrap_or(u64::MAX)
}

fn crud_resource_limit(
    code: &'static str,
    message: &'static str,
    dimension: &'static str,
    actual: u64,
    maximum: u64,
) -> SdkExecutionDiagnostic {
    let diagnostic = append_path(
        SdkExecutionDiagnostic::resource_limit(sdk_code(code), sdk_message(message)),
        [
            SdkDiagnosticPathSegment::Argument(sdk_name("limits")),
            SdkDiagnosticPathSegment::Argument(sdk_name(dimension)),
        ],
    );
    if dimension == "bytes" {
        diagnostic
            .try_with_detail(
                sdk_name("actual_bytes"),
                SdkDiagnosticDetailValue::ByteCount(actual),
            )
            .and_then(|diagnostic| {
                diagnostic.try_with_detail(
                    sdk_name("maximum_bytes"),
                    SdkDiagnosticDetailValue::ByteCount(maximum),
                )
            })
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    } else {
        diagnostic
            .try_with_detail(sdk_name("actual"), SdkDiagnosticDetailValue::Count(actual))
            .and_then(|diagnostic| {
                diagnostic.try_with_detail(
                    sdk_name("maximum"),
                    SdkDiagnosticDetailValue::Count(maximum),
                )
            })
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    }
}

struct PreparedEntityType {
    type_id: TypeId,
    descriptor: Arc<EntityDescriptor>,
}

struct PreparedEntityIdentity {
    type_id: TypeId,
    descriptor: Arc<EntityDescriptor>,
    iid: String,
}

struct PreparedEntityCreate {
    type_id: TypeId,
    descriptor: Arc<EntityDescriptor>,
    attributes: DynamicAttributeMap,
    /// The create as a keyed put plan input, when one can express it.
    keyed_put: Option<PreparedKeyedPut>,
    /// The create as an insert or update plan input.
    plan_write: Option<PreparedPlanWrite>,
}

/// Declaring ownerships and values of one keyed put.
struct PreparedKeyedPut {
    keys: Vec<(OwnsFactId, CanonicalValue)>,
    fields: Vec<(OwnsFactId, CanonicalValue)>,
    cleared: Vec<OwnsFactId>,
    attached: Vec<(OwnsFactId, CanonicalValue)>,
}

/// A create in plan form: the given values and every non-key ownership an
/// update clears first. Absent when an ordered field or role needs list
/// syntax the plan vocabulary does not have.
struct PreparedPlanWrite {
    /// Every given value, keys included, for an insert.
    created: Vec<(OwnsFactId, CanonicalValue)>,
    /// The given non-key values, for an update.
    fields: Vec<(OwnsFactId, CanonicalValue)>,
    cleared_fields: Vec<OwnsFactId>,
    cleared_roles: Vec<RoleId>,
}

struct PreparedRelationType {
    type_id: TypeId,
    descriptor: Arc<RelationDescriptor>,
}

struct PreparedRelationIdentity {
    type_id: TypeId,
    descriptor: Arc<RelationDescriptor>,
    iid: String,
}

struct PreparedRelationCreate {
    type_id: TypeId,
    descriptor: Arc<RelationDescriptor>,
    attributes: DynamicAttributeMap,
    /// The same field values in plan form.
    plan_fields: Vec<(OwnsFactId, CanonicalValue)>,
    /// The create as an update plan input.
    plan_write: Option<PreparedPlanWrite>,
    role_players: Vec<PreparedRolePlayerInput>,
}

struct PreparedRolePlayerInput {
    role_id: RoleId,
    role_name: String,
    candidate_types: Vec<TypeId>,
    iid: Option<String>,
    key: Option<(String, AttributeValue)>,
    /// The key in plan form; present exactly when `key` is.
    plan_key: Option<(AttributeId, CanonicalValue)>,
    database_identity: Option<DatabaseExecutionIdentity>,
}

struct ResolvedRolePlayerInput {
    player_type: TypeId,
    iid: String,
    role_name: String,
}

async fn finish_owned_read<T>(
    transaction: TransactionContext,
    type_id: &TypeId,
    result: Result<T, SdkExecutionDiagnostic>,
) -> Result<T, SdkExecutionDiagnostic> {
    match result {
        Ok(value) => {
            transaction
                .close()
                .await
                .map_err(|error| orm_at_type(error, SdkProviderOperation::Close, type_id))?;
            Ok(value)
        }
        Err(primary) => {
            let _ = transaction.close().await;
            Err(primary)
        }
    }
}

fn compatibility_without_cause(
    diagnostic: SdkExecutionDiagnostic,
) -> ProjectedCrudCompatibilityFailure {
    let stage = compatibility_stage(&diagnostic, None);
    ProjectedCrudCompatibilityFailure::new(diagnostic, stage, None)
}

fn compatibility_failure(
    diagnostic: SdkExecutionDiagnostic,
    capture: &CompatibilityCapture,
) -> ProjectedCrudCompatibilityFailure {
    let cause = capture.take();
    let stage = compatibility_stage(&diagnostic, cause.as_ref());
    ProjectedCrudCompatibilityFailure::new(diagnostic, stage, cause)
}

fn compatibility_result<T>(
    result: Result<T, SdkExecutionDiagnostic>,
    capture: &CompatibilityCapture,
) -> Result<T, ProjectedCrudCompatibilityFailure> {
    result.map_err(|diagnostic| compatibility_failure(diagnostic, capture))
}

async fn finish_compatibility_read<T>(
    transaction: TransactionContext,
    type_id: &TypeId,
    result: Result<T, SdkExecutionDiagnostic>,
    capture: &CompatibilityCapture,
    mode: ExecutionMode<'_>,
) -> Result<T, ProjectedCrudCompatibilityFailure> {
    match result {
        Ok(value) => {
            transaction
                .close()
                .await
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Close, type_id))
                .map_err(|diagnostic| compatibility_failure(diagnostic, capture))?;
            Ok(value)
        }
        Err(primary) => {
            let _ = transaction.close().await;
            Err(compatibility_failure(primary, capture))
        }
    }
}

fn compatibility_stage(
    diagnostic: &SdkExecutionDiagnostic,
    cause: Option<&ProjectedCrudCompatibilityCause>,
) -> ProjectedCrudCompatibilityStage {
    match cause {
        Some(ProjectedCrudCompatibilityCause::Commit(_)) => {
            return ProjectedCrudCompatibilityStage::Transaction;
        }
        Some(ProjectedCrudCompatibilityCause::Orm(OrmError::Transaction(_))) => {
            return ProjectedCrudCompatibilityStage::Transaction;
        }
        Some(ProjectedCrudCompatibilityCause::Orm(OrmError::Hydration { .. })) => {
            return ProjectedCrudCompatibilityStage::Hydration;
        }
        Some(ProjectedCrudCompatibilityCause::Orm(_)) | None => {}
    }

    if diagnostic.category() == SdkDiagnosticCategory::Transaction {
        return ProjectedCrudCompatibilityStage::Transaction;
    }
    if diagnostic.code().as_str() == "runtime_projection_mismatch"
        && diagnostic
            .path()
            .iter()
            .any(|segment| matches!(segment, SdkDiagnosticPathSegment::Role(_)))
    {
        return ProjectedCrudCompatibilityStage::Hydration;
    }
    if matches!(
        diagnostic.code().as_str(),
        "mutation_rehydration_missing"
            | "relation_hydration_ambiguous"
            | "hydrated_iid_missing"
            | "hydrated_iid_mismatch"
            | "hydrated_type_mismatch"
            | "unexpected_provider_attribute"
            | "ambiguous_provider_attribute"
            | "provider_scalar_invalid"
            | "unexpected_provider_role"
            | "ambiguous_provider_role"
            | "hydrated_player_iid_missing"
            | "hydrated_player_iid_invalid"
            | "hydrated_player_type_missing"
            | "hydrated_role_player_not_accepted"
            | "hydrated_role_player_form_ambiguous"
            | "hydrated_role_player_form_mismatch"
            | "complete_relation_role_player_unsupported"
            | "hydrated_reference_key_mismatch"
            | "projected_player_type_ambiguous"
            | "duplicate_reference_key"
            | "provider_identity_answer_invalid"
            | "provider_identity_cardinality_invalid"
            | "provider_identity_mismatch"
            | "ordered_distinct_duplicate"
    ) {
        ProjectedCrudCompatibilityStage::Hydration
    } else {
        ProjectedCrudCompatibilityStage::Input
    }
}

fn require_role_player_database_identity(
    prepared: &PreparedRelationCreate,
    expected: &DatabaseExecutionIdentity,
) -> Result<(), SdkExecutionDiagnostic> {
    for player in &prepared.role_players {
        if player
            .database_identity
            .as_ref()
            .is_some_and(|identity| identity != expected)
        {
            return Err(invalid_input_at_role(
                "reference_database_mismatch",
                "The projected reference belongs to a different database execution identity",
                &prepared.type_id,
                &player.role_id,
            ));
        }
    }
    Ok(())
}

async fn finish_owned_write<T>(
    transaction: TransactionContext,
    type_id: &TypeId,
    result: Result<T, SdkExecutionDiagnostic>,
) -> Result<T, SdkExecutionDiagnostic> {
    match result {
        Ok(value) => {
            transaction
                .commit_classified()
                .await
                .map_err(|error| lower_commit_error(&error))
                .map_err(|diagnostic| {
                    append_path(
                        diagnostic,
                        [SdkDiagnosticPathSegment::Type(type_id.clone())],
                    )
                })?;
            Ok(value)
        }
        Err(primary) => {
            let _ = transaction.rollback().await;
            Err(primary)
        }
    }
}

fn require_transaction(
    transaction: &TransactionContext,
    required: TxType,
    type_id: &TypeId,
) -> Result<(), SdkExecutionDiagnostic> {
    let allowed = match required {
        TxType::Read => matches!(transaction.tx_type(), TxType::Read | TxType::Write),
        TxType::Write => transaction.tx_type() == TxType::Write,
        TxType::Schema => transaction.tx_type() == TxType::Schema,
    };
    if allowed {
        Ok(())
    } else {
        Err(invalid_input_at_type(
            "transaction_type_mismatch",
            "The borrowed transaction cannot execute the requested exact CRUD operation",
            type_id,
        ))
    }
}

fn validate_iid(type_id: &TypeId, iid: &str) -> Result<(), SdkExecutionDiagnostic> {
    if is_canonical_thing_iid(iid) {
        Ok(())
    } else {
        Err(invalid_input_at_type(
            "noncanonical_iid",
            "Exact CRUD requires a canonical TypeDB thing IID",
            type_id,
        ))
    }
}

fn exact_row_identity(
    iid: Option<String>,
    concrete_type: Option<String>,
    type_id: &TypeId,
    requested_iid: &str,
) -> Result<String, SdkExecutionDiagnostic> {
    let iid = iid.ok_or_else(|| {
        integrity_at_type(
            "hydrated_iid_missing",
            "The provider row omitted its canonical IID",
            type_id,
        )
    })?;
    if iid != requested_iid || !is_canonical_thing_iid(&iid) {
        return Err(integrity_at_type(
            "hydrated_iid_mismatch",
            "The provider row returned a different or noncanonical IID",
            type_id,
        ));
    }
    if concrete_type.as_deref() != Some(type_id.label().as_str()) {
        return Err(integrity_at_type(
            "hydrated_type_mismatch",
            "The provider row returned a different exact concrete type",
            type_id,
        ));
    }
    Ok(iid)
}

fn extract_one_provider_iid(
    answer: QueryResult,
    type_id: &TypeId,
    expected: Option<&str>,
) -> Result<String, SdkExecutionDiagnostic> {
    let QueryResult::Documents(documents) = answer else {
        return Err(integrity_at_type(
            "provider_identity_answer_invalid",
            "The provider identity query returned an unexpected answer kind",
            type_id,
        ));
    };
    let [document] = documents.as_slice() else {
        return Err(integrity_at_type(
            "provider_identity_cardinality_invalid",
            "The provider identity query did not resolve exactly one thing",
            type_id,
        ));
    };
    let iid = document
        .as_object()
        .and_then(|object| object.get("iid"))
        .map(|value| value.get("value").unwrap_or(value))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            integrity_at_type(
                "provider_identity_answer_invalid",
                "The provider identity query omitted its canonical IID",
                type_id,
            )
        })?
        .to_owned();
    if !is_canonical_thing_iid(&iid) || expected.is_some_and(|expected| expected != iid) {
        return Err(integrity_at_type(
            "provider_identity_mismatch",
            "The provider identity query returned a different or noncanonical IID",
            type_id,
        ));
    }
    Ok(iid)
}

fn extract_optional_provider_iid(
    answer: QueryResult,
    type_id: &TypeId,
    expected: Option<&str>,
) -> Result<Option<String>, SdkExecutionDiagnostic> {
    let QueryResult::Documents(documents) = answer else {
        return Err(integrity_at_type(
            "provider_identity_answer_invalid",
            "The provider identity query returned an unexpected answer kind",
            type_id,
        ));
    };
    match documents.as_slice() {
        [] => Ok(None),
        [_] => {
            extract_one_provider_iid(QueryResult::Documents(documents), type_id, expected).map(Some)
        }
        _ => Err(integrity_at_type(
            "provider_identity_cardinality_invalid",
            "The provider identity query did not resolve at most one exact thing",
            type_id,
        )),
    }
}

/// Every role-player reference of a create in plan form.
fn plan_references(
    prepared: &PreparedRelationCreate,
) -> Result<Vec<(RoleId, Reference<'_>)>, SdkExecutionDiagnostic> {
    prepared
        .role_players
        .iter()
        .map(|player| {
            let identity = match (&player.iid, &player.plan_key) {
                (Some(iid), _) => ReferenceIdentity::Iid(iid),
                (None, Some((attribute, value))) => ReferenceIdentity::Key {
                    attribute,
                    value: value.clone(),
                },
                (None, None) => return Err(write_plan_invalid(&prepared.type_id)),
            };
            Ok((
                player.role_id.clone(),
                Reference {
                    candidates: &player.candidate_types,
                    identity,
                },
            ))
        })
        .collect()
}

/// Delete one thing by IID as one plan; an absent thing is not an error.
///
/// The answer is not inspected: like the released blind delete, any
/// successful provider answer (rows or a bare acknowledgement) is success.
async fn execute_delete_plan(
    transaction: &TransactionContext,
    authority: &PlanAuthority,
    type_id: &TypeId,
    iid: &str,
    returning_roles: Option<Vec<DocumentRole>>,
    mode: ExecutionMode<'_>,
) -> Result<Vec<serde_json::Value>, SdkExecutionDiagnostic> {
    if let Some(roles) = returning_roles {
        let plan = delete_by_iid_returning(authority, type_id, iid, roles)
            .map_err(|diagnostic| plan_build_failure(&diagnostic, type_id))?;
        return execute_write_rows(transaction, plan.local(), type_id, mode).await;
    }
    let plan = delete_by_iid(authority, type_id, iid)
        .map_err(|diagnostic| plan_build_failure(&diagnostic, type_id))?;
    let query = transaction
        .prepare_plan(plan.local())
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
    let deleted = transaction
        .dispatch_plan(query)
        .await
        .map(|_| Vec::new())
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id));
    after_write(transaction, deleted).await
}

/// Values travel as `given` rows when the transaction supports them.
async fn plan_transport(
    transaction: &TransactionContext,
    type_id: &TypeId,
    mode: ExecutionMode<'_>,
) -> Result<ValueTransport, SdkExecutionDiagnostic> {
    let given = transaction
        .supports_given_stage()
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
    Ok(if given {
        ValueTransport::Given
    } else {
        ValueTransport::Literal
    })
}

/// Execute a write plan and return its rows. Lowering failures reach no
/// provider and leave the transaction as it was; a failure after dispatch
/// may follow writes, so it poisons a borrowed transaction.
async fn execute_write_rows(
    transaction: &TransactionContext,
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    type_id: &TypeId,
    mode: ExecutionMode<'_>,
) -> Result<Vec<serde_json::Value>, SdkExecutionDiagnostic> {
    let query = transaction
        .prepare_plan(plan)
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
    let rows = transaction
        .dispatch_plan(query)
        .await
        .and_then(|answer| TransactionContext::resolve_plan(plan, answer))
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))
        .and_then(|answer| match answer {
            PlanAnswer::Documents(rows) => Ok(rows),
            _ => Err(integrity_at_type(
                "provider_identity_answer_invalid",
                "The provider identity query returned an unexpected answer kind",
                type_id,
            )),
        });
    after_write(transaction, rows).await
}

/// Latch a borrowed transaction rollback-only when a failure follows a
/// dispatched write. An owned transaction is rolled back by its caller.
async fn after_write<T>(
    transaction: &TransactionContext,
    result: Result<T, SdkExecutionDiagnostic>,
) -> Result<T, SdkExecutionDiagnostic> {
    if let Err(diagnostic) = &result {
        transaction.latch_rollback_only(diagnostic).await;
    }
    result
}

fn identity_cardinality(type_id: &TypeId) -> SdkExecutionDiagnostic {
    integrity_at_type(
        "provider_identity_cardinality_invalid",
        "The provider identity query did not resolve exactly one thing",
        type_id,
    )
}

/// The one thing every document describes (`_iid`).
fn single_document_iid(
    documents: &[serde_json::Value],
    type_id: &TypeId,
) -> Result<String, SdkExecutionDiagnostic> {
    let mut iids = BTreeSet::new();
    for document in documents {
        iids.insert(document_iid(document, "_iid", type_id)?.ok_or_else(|| {
            integrity_at_type(
                "provider_identity_answer_invalid",
                "The provider identity query omitted its canonical IID",
                type_id,
            )
        })?);
    }
    match iids.len() {
        1 => Ok(iids.pop_first().expect("one IID")),
        _ => Err(identity_cardinality(type_id)),
    }
}

/// Two references of one role that resolved to the same player: the role
/// lists fewer distinct players than it has references.
fn reject_resolved_duplicates(
    prepared: &PreparedRelationCreate,
    documents: &[serde_json::Value],
) -> Result<(), SdkExecutionDiagnostic> {
    let type_id = &prepared.type_id;
    for (index, role) in prepared.descriptor.roles.iter().enumerate() {
        let references = prepared
            .role_players
            .iter()
            .filter(|player| player.role_name == role.role_name)
            .count();
        if references < 2 {
            continue;
        }
        let key = format!("_role_{index}_iid");
        let mut players = BTreeSet::new();
        for document in documents {
            if let Some(iid) = document_iid(document, &key, type_id)? {
                players.insert(iid);
            }
        }
        if players.len() < references {
            return Err(integrity_at_type(
                "duplicate_resolved_role_player",
                "Exact relation references resolved to a duplicate role player",
                type_id,
            ));
        }
    }
    Ok(())
}

/// Read one canonical IID key of a fetched document; `null` is absent.
fn document_iid(
    document: &serde_json::Value,
    key: &str,
    type_id: &TypeId,
) -> Result<Option<String>, SdkExecutionDiagnostic> {
    let invalid = || {
        integrity_at_type(
            "provider_identity_answer_invalid",
            "The provider identity query omitted its canonical IID",
            type_id,
        )
    };
    match document.as_object().ok_or_else(invalid)?.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let iid = value
                .get("value")
                .unwrap_or(value)
                .as_str()
                .ok_or_else(invalid)?;
            if !is_canonical_thing_iid(iid) {
                return Err(integrity_at_type(
                    "provider_identity_mismatch",
                    "The provider identity query returned a different or noncanonical IID",
                    type_id,
                ));
            }
            Ok(Some(iid.to_owned()))
        }
    }
}

/// Two references to one thing in one role were refused after resolution;
/// resolution now happens inside the write, so equal references are refused
/// before it.
fn reject_duplicate_references(
    prepared: &PreparedRelationCreate,
) -> Result<(), SdkExecutionDiagnostic> {
    for (index, player) in prepared.role_players.iter().enumerate() {
        let duplicate = prepared.role_players[..index].iter().any(|earlier| {
            earlier.role_name == player.role_name
                && earlier.iid == player.iid
                && earlier.plan_key == player.plan_key
                && earlier
                    .candidate_types
                    .iter()
                    .any(|candidate| player.candidate_types.contains(candidate))
        });
        if duplicate {
            return Err(integrity_at_type(
                "duplicate_resolved_role_player",
                "Exact relation references resolved to a duplicate role player",
                &prepared.type_id,
            ));
        }
    }
    Ok(())
}

/// A built plan, or `None` when the released path must run instead: on a
/// connection without the `given` stage, text with a control character
/// cannot become a plan literal, while the released path escapes it.
fn plan_or_released<T>(
    built: std::result::Result<T, Diagnostic>,
    type_id: &TypeId,
) -> Result<Option<T>, SdkExecutionDiagnostic> {
    match built {
        Ok(plan) => Ok(Some(plan)),
        Err(diagnostic) if diagnostic.code().as_str() == LITERAL_CONTROL_CHARACTER => Ok(None),
        Err(_) => Err(write_plan_invalid(type_id)),
    }
}

/// A plan that cannot be built is a bridge defect.
fn plan_build_failure(_diagnostic: &Diagnostic, type_id: &TypeId) -> SdkExecutionDiagnostic {
    write_plan_invalid(type_id)
}

fn write_plan_invalid(type_id: &TypeId) -> SdkExecutionDiagnostic {
    integrity_at_type(
        "write_plan_invalid",
        "The projected write cannot be built as a validated plan",
        type_id,
    )
}

fn direct_entity_role_player_inputs(
    prepared: &PreparedRelationCreate,
) -> Option<Vec<DynamicRolePlayerInput>> {
    prepared
        .role_players
        .iter()
        .map(|player| {
            let [candidate] = player.candidate_types.as_slice() else {
                return None;
            };
            if candidate.kind() != TypeKind::Entity {
                return None;
            }
            Some(DynamicRolePlayerInput {
                role_name: player.role_name.clone(),
                player_type_name: candidate.label().as_str().to_owned(),
                iid: player.iid.clone(),
                key: player.key.clone(),
            })
        })
        .collect()
}

fn resolved_role_player_tuples(
    resolved: &[ResolvedRolePlayerInput],
) -> Vec<(TypeId, String, String)> {
    resolved
        .iter()
        .map(|player| {
            (
                player.player_type.clone(),
                player.iid.clone(),
                player.role_name.clone(),
            )
        })
        .collect()
}

fn require_mutation_relation_row(
    rows: Vec<DynamicRelationRow>,
    type_id: &TypeId,
) -> Result<DynamicRelationRow, SdkExecutionDiagnostic> {
    match rows.as_slice() {
        [row] => Ok(row.clone()),
        [] => Err(mutation_rehydration_missing(type_id)),
        _ => Err(relation_ambiguity(type_id)),
    }
}

fn unique_descriptor_role<'a>(
    descriptor: &'a RelationDescriptor,
    token: &RoleTokenProjection,
) -> Option<&'a RoleDescriptor> {
    let mut matches = descriptor
        .roles
        .iter()
        .filter(|role| role.role_name == token.role().label().as_str());
    let role = matches.next()?;
    matches.next().is_none().then_some(role)
}

fn descriptor_role_matches_token(
    descriptor_role: &RoleDescriptor,
    token: &RoleTokenProjection,
) -> bool {
    descriptor_role.player_type_names.len() == token.accepted_players().len()
        && token.accepted_players().iter().all(|player| {
            descriptor_role
                .player_type_names
                .iter()
                .filter(|label| label.as_str() == player.label().as_str())
                .count()
                == 1
        })
}

fn is_same_or_subtype(
    installed: &InstalledRuntimeProjection,
    candidate: &TypeId,
    ancestor: &TypeId,
) -> bool {
    let mut current = Some(candidate);
    let mut visited = std::collections::BTreeSet::new();
    while let Some(type_id) = current {
        if type_id == ancestor {
            return true;
        }
        if !visited.insert(type_id) {
            return false;
        }
        current = installed
            .projection()
            .models()
            .get(type_id)
            .and_then(|model| model.declaration().parent());
    }
    false
}

fn attribute_to_canonical(value: &AttributeValue) -> Result<CanonicalValue, ()> {
    crate::runtime_projection::canonical_attribute_value(value).map_err(|_| ())
}

fn attribute_type(field_id: &OwnsFactId) -> TypeId {
    TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str())
        .expect("an ownership identity contains a validated attribute label")
}

fn mutation_rehydration_missing(type_id: &TypeId) -> SdkExecutionDiagnostic {
    integrity_at_type(
        "mutation_rehydration_missing",
        "The exact mutation did not produce one rehydratable provider row",
        type_id,
    )
}

fn relation_ambiguity(type_id: &TypeId) -> SdkExecutionDiagnostic {
    integrity_at_type(
        "relation_hydration_ambiguous",
        "The exact relation IID resolved to more than one logical relation",
        type_id,
    )
}

fn first_provider_player_duplicate(players: &[&DynamicRolePlayer]) -> Option<(usize, usize)> {
    let mut seen = BTreeMap::<&str, usize>::new();
    for (index, player) in players.iter().enumerate() {
        let Some(iid) = player.player_iid.as_deref() else {
            continue;
        };
        if let Some(first_index) = seen.insert(iid, index) {
            return Some((first_index, index));
        }
    }
    None
}

fn ordered_distinct_duplicate_at_role(
    type_id: &TypeId,
    role_id: &RoleId,
    first_index: usize,
    duplicate_index: usize,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::integrity(
            sdk_code("ordered_distinct_duplicate"),
            sdk_message(
                "The ordered-distinct projected collection contains a duplicate canonical member",
            ),
        ),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
            SdkDiagnosticPathSegment::Index(u64::try_from(duplicate_index).unwrap_or(u64::MAX)),
        ],
    )
    .try_with_detail(
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("first_index")
            .expect("static ordered-distinct detail name is canonical"),
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticDetailValue::Count(
            u64::try_from(first_index).unwrap_or(u64::MAX),
        ),
    )
    .expect("the static ordered-distinct detail is unique")
    .try_with_detail(
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("duplicate_index")
            .expect("static ordered-distinct detail name is canonical"),
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticDetailValue::Count(
            u64::try_from(duplicate_index).unwrap_or(u64::MAX),
        ),
    )
    .expect("the static ordered-distinct details fit the contract")
}

fn orm_at_type(
    error: crate::OrmError,
    operation: SdkProviderOperation,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    append_path(
        lower_orm_error(&error, operation),
        [SdkDiagnosticPathSegment::Type(type_id.clone())],
    )
}

fn provider_integrity(diagnostic: SdkExecutionDiagnostic) -> SdkExecutionDiagnostic {
    if diagnostic.category() != SdkDiagnosticCategory::InvalidInput {
        return diagnostic;
    }
    let mut converted =
        SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message());
    for segment in diagnostic.path() {
        converted = converted
            .try_at(segment.clone())
            .expect("an existing SDK diagnostic path remains bounded");
    }
    for (name, value) in diagnostic.details() {
        converted = converted
            .try_with_detail(name.clone(), value.clone())
            .expect("existing SDK diagnostic details remain bounded and unique");
    }
    converted
}

fn invalid_input_at_type(
    code: &'static str,
    message: &'static str,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        [SdkDiagnosticPathSegment::Type(type_id.clone())],
    )
}

fn invalid_input_at_role(
    code: &'static str,
    message: &'static str,
    type_id: &TypeId,
    role_id: &RoleId,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
        ],
    )
}

fn integrity_at_type(
    code: &'static str,
    message: &'static str,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        [SdkDiagnosticPathSegment::Type(type_id.clone())],
    )
}

fn integrity_at_field(
    code: &'static str,
    message: &'static str,
    type_id: &TypeId,
    field_id: &OwnsFactId,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Field(field_id.clone()),
        ],
    )
}

fn integrity_at_role(
    code: &'static str,
    message: &'static str,
    type_id: &TypeId,
    role_id: &RoleId,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Role(role_id.clone()),
        ],
    )
}

fn append_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = diagnostic
            .try_at(segment)
            .expect("exact CRUD diagnostic paths fit the SDK contract");
    }
    diagnostic
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static exact CRUD diagnostic code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static exact CRUD diagnostic message is valid")
}

fn sdk_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static exact CRUD diagnostic name is canonical")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::future::pending;

    use serde_json::json;
    use tokio::sync::Notify;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::projection::{
        BindingTarget, CSymbolPrefix, ProjectionConfig, ProjectionHandler,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::temporal::TimeZoneDesignator;
    use type_bridge_contract::value::CanonicalString;
    use type_bridge_core_lib::version::Version;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use crate::session::TransactionContextState;
    use crate::session::backend::{BoxFuture, DriverBackend, GivenRowsSpec, TransactionOps};

    enum ControlledProviderResponse {
        Items(Vec<AnswerItem>),
        Error(&'static str),
        Pending(Arc<Notify>),
    }

    #[derive(Default)]
    struct ControlledProviderState {
        responses: VecDeque<ControlledProviderResponse>,
        opens: Vec<TxType>,
        bounded_reads: usize,
        bounded_given: usize,
        unbounded_queries: usize,
        commits: usize,
        rollbacks: usize,
        closes: usize,
    }

    struct ControlledProviderBackend {
        state: Arc<Mutex<ControlledProviderState>>,
    }

    impl DriverBackend for ControlledProviderBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, crate::Result<Box<dyn TransactionOps>>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = ControlledProviderTransaction {
                state: Arc::clone(&self.state),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<Version> {
            Some(Version::new(3, 12, 1))
        }

        fn supports_given_rows(&self) -> bool {
            true
        }
    }

    struct ControlledProviderTransaction {
        state: Arc<Mutex<ControlledProviderState>>,
    }

    impl ControlledProviderTransaction {
        fn response(&self) -> ControlledProviderResponse {
            self.state
                .lock()
                .unwrap()
                .responses
                .pop_front()
                .expect("controlled CRUD issued an unexpected provider query")
        }
    }

    impl TransactionOps for ControlledProviderTransaction {
        fn supports_given_rows(&self) -> bool {
            true
        }

        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, crate::Result<QueryResult>> {
            self.state.lock().unwrap().unbounded_queries += 1;
            Box::pin(async { panic!("controlled CRUD used an unbounded provider query") })
        }

        fn query_v2_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            limits: AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, crate::Result<BoundedAnswerStats>> {
            self.state.lock().unwrap().bounded_reads += 1;
            stream_controlled_response(self.response(), limits.answer, consumer)
        }

        fn query_v2_with_rows_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            _rows: GivenRowsSpec,
            limits: AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, crate::Result<BoundedAnswerStats>> {
            self.state.lock().unwrap().bounded_given += 1;
            stream_controlled_response(self.response(), limits.answer, consumer)
        }

        fn commit(&mut self) -> BoxFuture<'_, crate::Result<()>> {
            self.state.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn commit_classified(
            &mut self,
        ) -> BoxFuture<'_, std::result::Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, crate::Result<()>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, crate::Result<()>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    fn stream_controlled_response<'consumer>(
        response: ControlledProviderResponse,
        limits: BoundedAnswerLimits,
        consumer: &'consumer mut dyn AnswerConsumer,
    ) -> BoxFuture<'consumer, crate::Result<BoundedAnswerStats>> {
        Box::pin(async move {
            match response {
                ControlledProviderResponse::Items(items) => {
                    let mut reader = BoundedAnswerReader::new(limits);
                    reader.check_before_read()?;
                    for item in items {
                        if reader.accept(item, consumer)? == AnswerControl::Stop {
                            break;
                        }
                    }
                    Ok(reader.stats())
                }
                ControlledProviderResponse::Error(message) => {
                    Err(OrmError::QueryExecution(message.to_owned()))
                }
                ControlledProviderResponse::Pending(entered) => {
                    entered.notify_one();
                    pending::<crate::Result<BoundedAnswerStats>>().await
                }
            }
        })
    }

    fn controlled_installed() -> InstalledRuntimeProjection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("controlled-crud.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 4 } }
plays:
  person:
    membership: [member]
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::C,
                &ProjectionConfig::c(CSymbolPrefix::new("controlled_crud").unwrap()),
                &[ProjectionHandler::c_v3()],
                &[],
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn controlled_type(kind: TypeKind) -> TypeId {
        TypeId::new(
            kind,
            if kind == TypeKind::Entity {
                "person"
            } else {
                "membership"
            },
        )
        .unwrap()
    }

    fn controlled_create(
        installed: &InstalledRuntimeProjection,
        kind: TypeKind,
    ) -> ProjectedCreate {
        let type_id = controlled_type(kind);
        let field =
            OwnsFactId::new(type_id.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let value = ProjectedAttributeValue::try_new(
            installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new("key").unwrap()),
        )
        .unwrap();
        let roles = if kind == TypeKind::Relation {
            vec![(
                RoleId::new("membership", "member").unwrap(),
                vec![
                    ProjectedReference::try_new(
                        installed,
                        controlled_type(TypeKind::Entity),
                        Some("0x10".to_owned()),
                        vec![],
                    )
                    .unwrap(),
                ],
            )]
        } else {
            vec![]
        };
        ProjectedCreate::try_new(installed, type_id, vec![(field, vec![value])], roles).unwrap()
    }

    fn controlled_mutation_answers(kind: TypeKind) -> Vec<ControlledProviderResponse> {
        let (iid, label) = if kind == TypeKind::Entity {
            ("0x20", "person")
        } else {
            ("0x30", "membership")
        };
        let mut answers = Vec::new();
        if kind == TypeKind::Relation {
            answers.push(ControlledProviderResponse::Items(vec![
                AnswerItem::Document(json!({
                    "kind": 1,
                    "ordinal": 0,
                    "reference_ordinal": 0,
                    "iid": "0x10",
                    "type": "person"
                })),
            ]));
        }
        answers.extend([
            ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                "ordinal": 0,
                "iid": iid
            }))]),
            ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                "ordinal": 0,
                "_iid": iid,
                "_type": label,
                "attributes": {"identifier": "key"},
                "role_players": if kind == TypeKind::Relation {
                    json!([{
                        "role": "membership:member",
                        "iid": "0x10",
                        "type_name": "person",
                        "attributes": {"identifier": [{"value": "player-key"}]}
                    }])
                } else {
                    json!([])
                }
            }))]),
        ]);
        answers
    }

    fn controlled_database(
        responses: Vec<ControlledProviderResponse>,
    ) -> (Database, Arc<Mutex<ControlledProviderState>>) {
        let state = Arc::new(Mutex::new(ControlledProviderState {
            responses: responses.into(),
            ..ControlledProviderState::default()
        }));
        let database = Database::with_backend(
            Box::new(ControlledProviderBackend {
                state: Arc::clone(&state),
            }),
            "controlled-crud",
        );
        (database, state)
    }

    fn datetime_tz(value: &str) -> type_bridge_contract::temporal::CanonicalDateTimeTz {
        let CanonicalValue::DateTimeTz(value) =
            attribute_to_canonical(&AttributeValue::DateTimeTZ(value.to_owned()))
                .expect("provider datetime-tz evidence should hydrate")
        else {
            panic!("datetime-tz evidence changed scalar domain");
        };
        value
    }

    #[test]
    fn projected_crud_hydrates_fixed_named_and_overlap_datetime_tz_evidence() {
        let fixed = datetime_tz("+10000-01-02T03:04:05.500000000-05:30");
        assert_eq!(fixed.zone(), &TimeZoneDesignator::OffsetSeconds(-19_800));

        let named = datetime_tz("2024-07-01T12:00:00.000000000 Europe/Amsterdam");
        assert_eq!(
            named.zone(),
            &TimeZoneDesignator::Named("Europe/Amsterdam".to_owned())
        );
        assert_eq!(named.effective_offset_seconds(), 7_200);

        let earlier = datetime_tz("2024-10-27T01:30:00+01:00[Europe/London]");
        let later = datetime_tz("2024-10-27T01:30:00Z[Europe/London]");
        assert_eq!(earlier.effective_offset_seconds(), 3_600);
        assert_eq!(later.effective_offset_seconds(), 0);
        assert!(earlier.semantic_utc_nanoseconds() < later.semantic_utc_nanoseconds());
    }

    #[test]
    fn controlled_crud_preflight_and_reply_limits_are_exact() {
        let limits = QueryExecutionResourceLimits::tightened(30_000, 1, 64, 1, 1, 1, 0, 0);
        let mut ledger = ControlledCrudLedger::try_new(limits, 4).unwrap();
        let diagnostic = ledger.charge_statement(1).unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "projected_crud_statement_limit");

        let limits = QueryExecutionResourceLimits::tightened(30_000, 1, 64, 1, 1, 1, 0, 1);
        let mut ledger = ControlledCrudLedger::try_new(limits, 4).unwrap();
        ledger.charge_statement(4).unwrap();
        let diagnostic = ledger
            .record_reply(BoundedAnswerStats {
                processed_items: 2,
                response_bytes: 1,
                stopped_early: false,
            })
            .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_crud_reply_item_limit"
        );
    }

    #[test]
    fn controlled_crud_local_reader_rejects_before_collector_growth() {
        let mut collector = ControlledAnswerCollector::new(ControlledAnswerKind::Documents, "x");
        let mut bounded = LocallyBoundedCrudConsumer::new(
            &mut collector,
            BoundedAnswerLimits {
                max_items: 0,
                max_bytes: 64,
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
        );
        let error = bounded
            .accept(AnswerItem::Document(serde_json::json!({"iid": "0x1"})))
            .unwrap_err();
        assert!(matches!(error, OrmError::Match(_)));
        drop(bounded);
        let QueryResult::Documents(values) = collector.finish() else {
            panic!("collector answer kind changed")
        };
        assert!(values.is_empty());
    }

    #[test]
    fn controlled_crud_collector_uses_amortized_fallible_growth() {
        let mut collector = ControlledAnswerCollector::new(ControlledAnswerKind::Documents, "x");
        let mut previous_capacity = collector.values.capacity();
        let mut capacity_changes = 0_u32;
        for _ in 0..4_096 {
            collector
                .accept(AnswerItem::Document(serde_json::Value::Null))
                .unwrap();
            if collector.values.capacity() != previous_capacity {
                previous_capacity = collector.values.capacity();
                capacity_changes += 1;
            }
        }
        assert!(
            capacity_changes <= 32,
            "observed {capacity_changes} growths"
        );
        assert_eq!(collector.values.len(), 4_096);
    }

    #[tokio::test]
    async fn controlled_mutations_route_through_batch_atomicity_for_both_model_kinds() {
        let installed = controlled_installed();
        let executor = ProjectedCrudExecutor::new(&installed);

        let (database, state) = controlled_database(controlled_mutation_answers(TypeKind::Entity));
        let thing = executor
            .insert_entity_controlled(
                &database,
                &controlled_create(&installed, TypeKind::Entity),
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(thing.iid(), "0x20");
        {
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.bounded_given, 2);
            assert_eq!(state.unbounded_queries, 0);
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
        }

        let (database, state) =
            controlled_database(controlled_mutation_answers(TypeKind::Relation));
        let transaction = database.transaction_context(TxType::Write).await.unwrap();
        let thing = executor
            .insert_relation_in_transaction_controlled(
                &transaction,
                &controlled_create(&installed, TypeKind::Relation),
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(thing.iid(), "0x30");
        assert_eq!(
            transaction.lifecycle_state().await,
            TransactionContextState::Active
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.bounded_given, 3);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        }
        transaction.rollback().await.unwrap();

        let (database, state) = controlled_database(vec![ControlledProviderResponse::Error(
            "private owned mutation failure",
        )]);
        let diagnostic = executor
            .insert_entity_controlled(
                &database,
                &controlled_create(&installed, TypeKind::Entity),
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "provider_operation_failed");
        assert!(!format!("{diagnostic:?}").contains("private owned mutation failure"));
        {
            let state = state.lock().unwrap();
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 1);
        }

        let (database, state) = controlled_database(vec![
            ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x10",
                "type": "person"
            }))]),
            ControlledProviderResponse::Error("private borrowed mutation failure"),
        ]);
        let transaction = database.transaction_context(TxType::Write).await.unwrap();
        let diagnostic = executor
            .insert_relation_in_transaction_controlled(
                &transaction,
                &controlled_create(&installed, TypeKind::Relation),
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            transaction.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert_eq!(transaction.rollback_only_cause().await, Some(diagnostic));
        {
            let state = state.lock().unwrap();
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        }
        transaction.rollback().await.unwrap();
        assert_eq!(state.lock().unwrap().rollbacks, 1);
    }

    #[tokio::test]
    async fn controlled_get_and_count_stream_bounded_and_keep_borrowed_reads_reusable() {
        let installed = controlled_installed();
        let executor = ProjectedCrudExecutor::new(&installed);
        let person = controlled_type(TypeKind::Entity);
        let membership = controlled_type(TypeKind::Relation);

        let (database, state) = controlled_database(vec![
            ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                "_iid": "0x20",
                "_type": "person",
                "attributes": {"identifier": [{"value": "key"}]}
            }))]),
            ControlledProviderResponse::Items(vec![AnswerItem::Row(json!({"$count": 1}))]),
        ]);
        let thing = executor
            .get_entity_by_iid_controlled(
                &database,
                &person,
                "0x20",
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(thing.iid(), "0x20");
        assert_eq!(
            executor
                .count_entities_controlled(
                    &database,
                    &person,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                )
                .await
                .unwrap(),
            1
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Read, TxType::Read]);
            assert_eq!(state.bounded_reads, 2);
            assert_eq!(state.unbounded_queries, 0);
            assert_eq!(state.closes, 2);
        }

        let entered = Arc::new(Notify::new());
        let (database, state) = controlled_database(vec![
            ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                "_iid": "0x30",
                "_type": "membership",
                "attributes": {"identifier": [{"value": "key"}]}
            }))]),
            ControlledProviderResponse::Items(vec![AnswerItem::Row(json!({"$count": 2}))]),
            ControlledProviderResponse::Pending(Arc::clone(&entered)),
            ControlledProviderResponse::Items(vec![AnswerItem::Row(json!({"$count": 1}))]),
        ]);
        let transaction = database.transaction_context(TxType::Read).await.unwrap();
        let relation = executor
            .get_relation_by_iid_in_transaction_controlled(
                &transaction,
                &membership,
                "0x30",
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(relation.iid(), "0x30");
        assert_eq!(
            executor
                .count_relations_in_transaction_controlled(
                    &transaction,
                    &membership,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                )
                .await
                .unwrap(),
            2
        );

        let cancellation = AnswerCancellation::default();
        let cancellation_trigger = cancellation.clone();
        let operation = executor.get_entity_by_iid_in_transaction_controlled(
            &transaction,
            &person,
            "0x20",
            QueryExecutionResourceLimits::default(),
            cancellation,
        );
        let trigger = async {
            entered.notified().await;
            cancellation_trigger.cancel();
        };
        let (result, ()) = tokio::join!(operation, trigger);
        assert_eq!(result.unwrap_err().code().as_str(), "provider_cancelled");
        assert_eq!(
            transaction.lifecycle_state().await,
            TransactionContextState::Active
        );
        assert_eq!(
            executor
                .count_entities_in_transaction_controlled(
                    &transaction,
                    &person,
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                )
                .await
                .unwrap(),
            1
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.bounded_reads, 4);
            assert_eq!(state.unbounded_queries, 0);
            assert_eq!(state.closes, 0);
        }
        transaction.close().await.unwrap();
        assert_eq!(state.lock().unwrap().closes, 1);
    }

    #[test]
    fn controlled_crud_cancellation_and_batch_result_shape_are_redacted() {
        let type_id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let control = ProjectedCrudInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            cancellation,
        );
        let diagnostic = control.check(&type_id).unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "provider_cancelled");
        assert_eq!(
            single_batch_delete(ProjectedBatchResult::Things(Vec::new()), &type_id)
                .unwrap_err()
                .code()
                .as_str(),
            "projected_crud_batch_result_invalid"
        );
        assert!(single_batch_delete(ProjectedBatchResult::Deleted, &type_id).is_ok());
    }
}
