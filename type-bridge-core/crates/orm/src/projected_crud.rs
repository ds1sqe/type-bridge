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
};
use crate::_manager::hydration::{coalesce_dynamic_relations, hydrate_dynamic_entity};
use crate::_manager::{DynamicEntityManager, DynamicRelationManager};
use crate::_registry::DescriptorRegistry;
use crate::WriteResult;
use crate::execution_diagnostic::{lower_commit_error, lower_orm_error};
use crate::match_request::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use crate::plan_builders::ValueTransport;
use crate::plan_builders::authority::PlanAuthority;
use crate::plan_builders::insert::relation_insert;
use crate::plan_builders::manager::{ManagerPlans, ManagerRead, ReadTerminal};
use crate::plan_builders::put::{KeyedPut, RelationPut, keyed_entity_put, keyed_relation_put};
use crate::plan_builders::reference::{Reference, ReferenceIdentity};
use crate::plan_builders::write::{
    ThingUpdate, delete_by_iid, delete_by_iid_returning, entity_insert, update_by_iid,
};
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

#[path = "projected_crud_controlled_update.rs"]
mod controlled_update;
#[doc(hidden)]
pub use controlled_update::WriteTarget as ProjectedWriteTarget;

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
pub(crate) struct ExecutionMode<'capture> {
    capture: Option<&'capture CompatibilityCapture>,
}

impl<'capture> ExecutionMode<'capture> {
    const fn compatibility(capture: &'capture CompatibilityCapture) -> Self {
        Self {
            capture: Some(capture),
        }
    }

    fn controlled_crud_orm(
        self,
        error: CrudControlledAwaitError<OrmError>,
        operation: SdkProviderOperation,
        type_id: &TypeId,
    ) -> SdkExecutionDiagnostic {
        match error {
            CrudControlledAwaitError::Interrupted(diagnostic) => append_path(
                diagnostic,
                [SdkDiagnosticPathSegment::Type(type_id.clone())],
            ),
            CrudControlledAwaitError::Inner(error) => self.orm_at_type(error, operation, type_id),
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

    /// Run one exact IID read on the connection target. An omitted invocation
    /// control preserves the direct default path; proxy reads retain the
    /// connection's bounded transport policy.
    #[doc(hidden)]
    pub async fn get_by_iid_on_target(
        &self,
        target: crate::QueryTarget<'_>,
        type_id: &TypeId,
        iid: &str,
        control: impl Into<Option<ProjectedCrudInvocationControl>>,
    ) -> Result<Option<ProjectedThing>, crate::QueryTargetError> {
        let control = control.into();
        if control.is_none() {
            match (type_id.kind(), target) {
                (TypeKind::Entity, crate::QueryTarget::Database(database)) => {
                    return self
                        .get_entity_by_iid(database, type_id, iid)
                        .await
                        .map_err(Into::into);
                }
                (TypeKind::Relation, crate::QueryTarget::Database(database)) => {
                    return self
                        .get_relation_by_iid(database, type_id, iid)
                        .await
                        .map_err(Into::into);
                }
                (TypeKind::Entity, crate::QueryTarget::Transaction(transaction)) => {
                    return self
                        .get_entity_by_iid_in_transaction(transaction, type_id, iid)
                        .await
                        .map_err(Into::into);
                }
                (TypeKind::Relation, crate::QueryTarget::Transaction(transaction)) => {
                    return self
                        .get_relation_by_iid_in_transaction(transaction, type_id, iid)
                        .await
                        .map_err(Into::into);
                }
                _ => {}
            }
        }
        let control = control.unwrap_or_else(|| {
            ProjectedCrudInvocationControl::capture(
                target.answer_limits().unwrap_or_default(),
                AnswerCancellation::default(),
            )
        });
        let control = control.constrained_by(target.answer_limits());
        match (type_id.kind(), target) {
            (TypeKind::Entity, crate::QueryTarget::Database(database)) => self
                .get_entity_by_iid_controlled_with_control(database, type_id, iid, control)
                .await
                .map_err(Into::into),
            (TypeKind::Relation, crate::QueryTarget::Database(database)) => self
                .get_relation_by_iid_controlled_with_control(database, type_id, iid, control)
                .await
                .map_err(Into::into),
            (TypeKind::Entity, crate::QueryTarget::Transaction(transaction)) => self
                .get_entity_by_iid_in_transaction_controlled_with_control(
                    transaction,
                    type_id,
                    iid,
                    control,
                )
                .await
                .map_err(Into::into),
            (TypeKind::Relation, crate::QueryTarget::Transaction(transaction)) => self
                .get_relation_by_iid_in_transaction_controlled_with_control(
                    transaction,
                    type_id,
                    iid,
                    control,
                )
                .await
                .map_err(Into::into),
            (TypeKind::Entity | TypeKind::Relation, crate::QueryTarget::Proxy(proxy)) => {
                match type_id.kind() {
                    TypeKind::Entity => {
                        self.prepare_entity_identity(type_id, iid)?;
                    }
                    _ => {
                        self.prepare_relation_identity(type_id, iid)?;
                    }
                }
                control.check(type_id)?;
                self.require_remote_read_authority(type_id)?;
                let mut ledger = ControlledCrudLedger::try_new(
                    control.limits,
                    controlled_read_input_bytes(type_id, Some(iid)),
                )?;
                let limits = crate::QueryTarget::Proxy(proxy)
                    .manager_query_limits(control.deadline, &control.cancellation)
                    .await?;
                let (registry, program) = self.manager_read_program(
                    type_id,
                    Some(iid),
                    ReadTerminal::Fetch,
                    limits.as_ref(),
                )?;
                let ManagerRead::Fetch(stack) = program else {
                    unreachable!("exact model fetch")
                };
                let answers = proxy
                    .execute_read_plans(
                        stack.plans(),
                        control.limits,
                        &control.cancellation,
                        control.deadline,
                    )
                    .await?;
                let identity = proxy.execution_identity();
                let thing = if type_id.kind() == TypeKind::Entity {
                    if stack.plans().len() == 1
                        && matches!(answers.first(), Some(PlanAnswer::Documents(documents)) if documents.len() > 1)
                    {
                        return Err(integrity_at_type(
                            "entity_iid_cardinality_invalid",
                            "The provider returned multiple exact entities for one IID",
                            type_id,
                        )
                        .into());
                    }
                    let mut rows = self.resolve_exact_read(
                        &registry,
                        &stack,
                        &answers,
                        type_id,
                        &control,
                        crate::_manager::dynamic::plan_entity_row,
                    )?;
                    match rows.len() {
                        0 => None,
                        1 => Some(self.hydrate_entity(
                            rows.pop().expect("one exact entity"),
                            type_id,
                            iid,
                            identity,
                            ExecutionMode::default(),
                        )?),
                        _ => {
                            return Err(integrity_at_type(
                                "entity_iid_cardinality_invalid",
                                "The provider returned multiple exact entities for one IID",
                                type_id,
                            )
                            .into());
                        }
                    }
                } else {
                    let mut rows = self.resolve_exact_read(
                        &registry,
                        &stack,
                        &answers,
                        type_id,
                        &control,
                        crate::_manager::dynamic::plan_relation_row,
                    )?;
                    match rows.len() {
                        0 => None,
                        1 => Some(self.hydrate_relation(
                            rows.pop().expect("one exact relation"),
                            type_id,
                            iid,
                            identity,
                            ExecutionMode::default(),
                        )?),
                        _ => return Err(relation_ambiguity(type_id).into()),
                    }
                };
                if let Some(thing) = &thing {
                    ledger.charge_projected(thing)?;
                }
                control.check(type_id)?;
                Ok(thing)
            }
            _ => Err(invalid_input_at_type(
                "wrong_model_kind",
                "Exact CRUD requires an entity or relation",
                type_id,
            )
            .into()),
        }
    }

    /// Run one policy-aware exact model count on the connection target.
    #[doc(hidden)]
    pub async fn count_on_target(
        &self,
        target: crate::QueryTarget<'_>,
        type_id: &TypeId,
        control: ProjectedCrudInvocationControl,
    ) -> Result<u64, crate::QueryTargetError> {
        let control = control.constrained_by(target.answer_limits());
        match (type_id.kind(), target) {
            (TypeKind::Entity, crate::QueryTarget::Database(database)) => self
                .count_entities_controlled_with_control(database, type_id, control)
                .await
                .map_err(Into::into),
            (TypeKind::Relation, crate::QueryTarget::Database(database)) => self
                .count_relations_controlled_with_control(database, type_id, control)
                .await
                .map_err(Into::into),
            (TypeKind::Entity, crate::QueryTarget::Transaction(transaction)) => self
                .count_entities_in_transaction_controlled_with_control(
                    transaction,
                    type_id,
                    control,
                )
                .await
                .map_err(Into::into),
            (TypeKind::Relation, crate::QueryTarget::Transaction(transaction)) => self
                .count_relations_in_transaction_controlled_with_control(
                    transaction,
                    type_id,
                    control,
                )
                .await
                .map_err(Into::into),
            (TypeKind::Entity | TypeKind::Relation, crate::QueryTarget::Proxy(proxy)) => {
                self.require_exact_model(type_id, type_id.kind())?;
                control.check(type_id)?;
                self.require_remote_read_authority(type_id)?;
                let mut ledger = ControlledCrudLedger::try_new(
                    control.limits,
                    controlled_read_input_bytes(type_id, None),
                )?;
                let limits = crate::QueryTarget::Proxy(proxy)
                    .manager_query_limits(control.deadline, &control.cancellation)
                    .await?;
                let (_, program) =
                    self.manager_read_program(type_id, None, ReadTerminal::Count, limits.as_ref())?;
                let answers = proxy
                    .execute_read_plans(
                        program.plans(),
                        control.limits,
                        &control.cancellation,
                        control.deadline,
                    )
                    .await?;
                let [answer] = answers.as_slice() else {
                    return Err(integrity_at_type(
                        "provider_count_cardinality_invalid",
                        "The provider count query did not return exactly one answer",
                        type_id,
                    )
                    .into());
                };
                let count = self.resolve_exact_count(&program.plans()[0], answer, type_id)?;
                ledger.charge_scalar_bytes(std::mem::size_of::<u64>() as u64)?;
                control.check(type_id)?;
                Ok(count)
            }
            _ => Err(invalid_input_at_type(
                "wrong_model_kind",
                "Exact CRUD requires an entity or relation",
                type_id,
            )
            .into()),
        }
    }

    fn require_remote_read_authority(
        &self,
        type_id: &TypeId,
    ) -> Result<(), SdkExecutionDiagnostic> {
        if self
            .installed
            .plan_authority()
            .map_err(|error| {
                orm_at_type(
                    crate::plan_builders::manager::diagnostic(error),
                    SdkProviderOperation::Read,
                    type_id,
                )
            })?
            .verified()
        {
            return Ok(());
        }
        Err(invalid_input_at_type(
            "write_plan_authority_unverified",
            "A remotely staged manager plan requires verified schema authority",
            type_id,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_returning_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_entity_by_iid_returning_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact entity and return every pre-delete snapshot in a write result.
    pub async fn delete_entity_by_iid_returning_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning_controlled(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_returning_controlled_with_control(
            database,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning_controlled_with_control(
        &self,
        database: &Database,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning_in_transaction_controlled(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.delete_relation_by_iid_returning_in_transaction_controlled_with_control(
            transaction,
            type_id,
            iid,
            ProjectedBatchInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Delete an exact relation and return every pre-delete snapshot in a write result.
    pub async fn delete_relation_by_iid_returning_in_transaction_controlled_with_control(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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

    // Box only controlled execution so default single-CRUD futures stay small.
    async fn execute_controlled_single_delete_returning(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        Box::pin(
            ProjectedBatchExecutor::new(self.installed)
                .returning_deleted()
                .execute_mapped(
                    database,
                    batch,
                    control,
                    WriteResult::new(Vec::new()),
                    move |result| single_batch_write_result(result, &model),
                ),
        )
        .await
    }

    async fn execute_controlled_single_delete_returning_in_transaction(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let model = batch.model().clone();
        Box::pin(
            ProjectedBatchExecutor::new(self.installed)
                .returning_deleted()
                .execute_in_transaction_mapped(
                    transaction,
                    batch,
                    control,
                    WriteResult::new(Vec::new()),
                    move |result| single_batch_write_result(result, &model),
                ),
        )
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
            .put_relation_prepared(&transaction, &prepared, input, ExecutionMode::default())
            .await;
        finish_owned_write(transaction, &prepared.type_id, result).await
    }

    /// Put and completely rehydrate one exact relation in a borrowed write transaction.
    pub async fn put_relation_in_transaction(
        &self,
        transaction: &TransactionContext,
        input: &ProjectedCreate,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
        self.put_relation_prepared(transaction, &prepared, input, ExecutionMode::default())
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Entity)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
        self.execute_controlled_delete(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
        )
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
        self.execute_controlled_delete(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
        )
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(input.clone()),
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
            Ok,
        ))
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.require_exact_model(input.type_id(), TypeKind::Relation)?;
        validate_iid(input.type_id(), iid)?;
        control.check()?;
        let batch = self.single_controlled_write_batch(
            input.type_id(),
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement: input.clone(),
            },
            &control,
        )?;
        Box::pin(self.execute_controlled_single_write(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
            Ok,
        ))
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
        self.execute_controlled_delete(
            controlled_update::WriteTarget::Database(database),
            &batch,
            control,
        )
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
        self.execute_controlled_delete(
            controlled_update::WriteTarget::Transaction(transaction),
            &batch,
            control,
        )
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
                input,
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
    ) -> Result<WriteResult<ProjectedThing>, ProjectedCrudCompatibilityFailure> {
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
        let mut ledger = ControlledCrudLedger::try_new(
            control.limits,
            controlled_read_input_bytes(&prepared.type_id, Some(&prepared.iid)),
        )?;
        let mut rows = self
            .execute_controlled_exact_read(
                transaction,
                &prepared.type_id,
                &prepared.iid,
                control,
                &mut ledger,
                crate::_manager::dynamic::plan_entity_row,
            )
            .await?;
        let thing = match rows.len() {
            0 => None,
            1 => Some(self.hydrate_entity(
                rows.pop().expect("one exact entity"),
                &prepared.type_id,
                &prepared.iid,
                transaction.execution_identity(),
                ExecutionMode::default(),
            )?),
            _ => {
                return Err(integrity_at_type(
                    "entity_iid_cardinality_invalid",
                    "The provider returned multiple exact entities for one IID",
                    &prepared.type_id,
                ));
            }
        };
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
        self.execute_controlled_count(
            transaction,
            &prepared.type_id,
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
        let mut ledger = ControlledCrudLedger::try_new(
            control.limits,
            controlled_read_input_bytes(&prepared.type_id, Some(&prepared.iid)),
        )?;
        let mut rows = self
            .execute_controlled_exact_read(
                transaction,
                &prepared.type_id,
                &prepared.iid,
                control,
                &mut ledger,
                crate::_manager::dynamic::plan_relation_row,
            )
            .await?;
        let thing = match rows.len() {
            0 => None,
            1 => Some(self.hydrate_relation(
                rows.pop().expect("one exact relation"),
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
        self.execute_controlled_count(
            transaction,
            &prepared.type_id,
            control,
            controlled_read_input_bytes(&prepared.type_id, None),
        )
        .await
    }

    async fn execute_controlled_exact_read<T>(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        iid: &str,
        control: &ProjectedCrudInvocationControl,
        ledger: &mut ControlledCrudLedger,
        map: impl Fn(
            &crate::_registry::DescriptorRegistry,
            &crate::match_request::HydratedThing,
        ) -> crate::Result<T>,
    ) -> Result<Vec<T>, SdkExecutionDiagnostic> {
        let (registry, program) = self.manager_read_program(
            type_id,
            Some(iid),
            ReadTerminal::Fetch,
            transaction.shared_query_limits(),
        )?;
        let ManagerRead::Fetch(stack) = program else {
            unreachable!("exact model fetch")
        };
        let mut answers = Vec::with_capacity(stack.plans().len());
        for plan in stack.plans() {
            control.check(type_id)?;
            let query = await_crud_controlled(
                transaction.prepare_plan(plan),
                control.deadline,
                &control.cancellation,
            )
            .await
            .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Read, type_id))?;
            let answer = self
                .execute_controlled_read_query(
                    transaction,
                    type_id,
                    &query,
                    ControlledAnswerKind::Documents,
                    control,
                    ledger,
                )
                .await?;
            if type_id.kind() == TypeKind::Entity
                && stack.plans().len() == 1
                && matches!(&answer, QueryResult::Documents(documents) if documents.len() > 1)
            {
                return Err(integrity_at_type(
                    "entity_iid_cardinality_invalid",
                    "The provider returned multiple exact entities for one IID",
                    type_id,
                ));
            }
            answers.push(
                TransactionContext::resolve_plan(plan, answer)
                    .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?,
            );
        }
        self.resolve_exact_read(&registry, &stack, &answers, type_id, control, map)
    }

    fn resolve_exact_read<T>(
        &self,
        registry: &crate::_registry::DescriptorRegistry,
        stack: &crate::match_request::PlanFetchStack,
        answers: &[PlanAnswer],
        type_id: &TypeId,
        control: &ProjectedCrudInvocationControl,
        map: impl Fn(
            &crate::_registry::DescriptorRegistry,
            &crate::match_request::HydratedThing,
        ) -> crate::Result<T>,
    ) -> Result<Vec<T>, SdkExecutionDiagnostic> {
        let proof = stack
            .resolve(registry, answers, stack.execution_limits(control.limits))
            .map_err(|error| orm_at_type(error.into(), SdkProviderOperation::Read, type_id))?;
        let crate::match_request::MatchResult::Rows { rows } = proof
            .result()
            .for_request(proof.request())
            .map_err(|error| orm_at_type(error.into(), SdkProviderOperation::Read, type_id))?
        else {
            unreachable!("exact model rows")
        };
        rows.iter()
            .map(|row| match row.slots() {
                [crate::match_request::SlotValue::One(thing)] => map(registry, thing)
                    .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id)),
                _ => Err(integrity_at_type(
                    "manager_result_shape",
                    "An exact read requires one model slot",
                    type_id,
                )),
            })
            .collect()
    }

    async fn execute_controlled_count(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        control: &ProjectedCrudInvocationControl,
        input_bytes: u64,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let (_, program) = self.manager_read_program(
            type_id,
            None,
            ReadTerminal::Count,
            transaction.shared_query_limits(),
        )?;
        let plan = &program.plans()[0];
        let query = await_crud_controlled(
            transaction.prepare_plan(plan),
            control.deadline,
            &control.cancellation,
        )
        .await
        .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Read, type_id))?;
        let mut ledger = ControlledCrudLedger::try_new(control.limits, input_bytes)?;
        let answer = self
            .execute_controlled_read_query(
                transaction,
                type_id,
                &query,
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
        let answer = TransactionContext::resolve_plan(plan, answer)
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let count = self.resolve_exact_count(plan, &answer, type_id)?;
        ledger.charge_scalar_bytes(std::mem::size_of::<u64>() as u64)?;
        control.check(type_id)?;
        Ok(count)
    }

    fn manager_read_program(
        &self,
        type_id: &TypeId,
        iid: Option<&str>,
        terminal: ReadTerminal,
        limits: Option<&Arc<type_bridge_contract::limits::QueryLimits>>,
    ) -> Result<(Arc<DescriptorRegistry>, ManagerRead), SdkExecutionDiagnostic> {
        let registry = self
            .installed
            .manager_registry()
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let plans = ManagerPlans::new(Arc::clone(&registry)).with_query_limits(limits);
        let expressions = iid.map(|iid| crate::_dynamic::DynamicExpr::Iid { iid: iid.into() });
        let program = plans
            .read(
                type_id.label().as_str(),
                true,
                expressions.as_slice(),
                &[],
                None,
                None,
                &[],
                terminal,
            )
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        Ok((registry, program))
    }

    fn resolve_exact_count(
        &self,
        plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
        answer: &PlanAnswer,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows { rows } =
            answer.to_staged_v3(plan).map_err(|error| {
                orm_at_type(
                    crate::plan_builders::manager::diagnostic(error),
                    SdkProviderOperation::Read,
                    type_id,
                )
            })?
        else {
            return Err(integrity_at_type(
                "provider_count_cardinality_invalid",
                "The provider count query did not return rows",
                type_id,
            ));
        };
        let count = match rows.as_slice() {
            [row] => match row.as_slice() {
                [
                    type_bridge_contract::query_remote::RemoteValue::Value {
                        value: CanonicalValue::Long(count),
                    },
                ] => u64::try_from(*count).map_err(|_| {
                    integrity_at_type(
                        "provider_count_cardinality_invalid",
                        "The provider count is outside the count domain",
                        type_id,
                    )
                })?,
                _ => {
                    return Err(integrity_at_type(
                        "provider_count_cardinality_invalid",
                        "The provider count query did not return one integer",
                        type_id,
                    ));
                }
            },
            _ => {
                return Err(integrity_at_type(
                    "provider_count_cardinality_invalid",
                    "The provider count query did not return one row",
                    type_id,
                ));
            }
        };
        Ok(count)
    }

    async fn execute_controlled_read_query(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
        query: &crate::session::staged::StagedQuery,
        expected: ControlledAnswerKind,
        control: &ProjectedCrudInvocationControl,
        ledger: &mut ControlledCrudLedger,
    ) -> Result<QueryResult, SdkExecutionDiagnostic> {
        control.check(type_id)?;
        let source = query.typeql.as_str();
        let given_bytes = query
            .rows
            .as_ref()
            .map(crate::projected_batch_executor::given_rows_bytes)
            .transpose()?
            .unwrap_or(0);
        ledger.charge_statement(
            u64::try_from(source.len())
                .unwrap_or(u64::MAX)
                .saturating_add(given_bytes),
        )?;
        let limits = AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: ledger.remaining_items(),
                max_bytes: ledger.remaining_bytes(),
                deadline: control.deadline.instant(),
                cancellation: control.cancellation.clone(),
            },
            max_collection_members: control.limits.collection_members,
            ..Default::default()
        };
        let mut collector = ControlledAnswerCollector::new(expected, type_id.label().as_str());
        let mut observed = LocallyBoundedCrudConsumer::new(&mut collector, limits.answer.clone());
        let future = transaction.dispatch_read_plan_bounded(query, limits, &mut observed);
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

    async fn entity_manager(
        &self,
        transaction: &TransactionContext,
        descriptor: Arc<EntityDescriptor>,
        type_id: &TypeId,
        mode: ExecutionMode<'_>,
    ) -> Result<DynamicEntityManager<'static>, SdkExecutionDiagnostic> {
        let registry = self
            .installed
            .manager_registry()
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let manager = DynamicEntityManager::with_registry_transaction(
            transaction.clone(),
            registry,
            descriptor,
        );
        Ok(
            if transaction
                .supports_given_stage()
                .await
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?
            {
                manager
            } else {
                manager.literal_values()
            },
        )
    }

    async fn relation_manager(
        &self,
        transaction: &TransactionContext,
        descriptor: Arc<RelationDescriptor>,
        type_id: &TypeId,
        mode: ExecutionMode<'_>,
    ) -> Result<DynamicRelationManager<'static>, SdkExecutionDiagnostic> {
        let registry = self
            .installed
            .manager_registry()
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        let manager = DynamicRelationManager::with_registry_transaction(
            transaction.clone(),
            registry,
            descriptor,
        );
        Ok(
            if transaction
                .supports_given_stage()
                .await
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?
            {
                manager
            } else {
                manager.literal_values()
            },
        )
    }

    fn prepare_entity_create(
        &self,
        input: &ProjectedCreate,
    ) -> Result<PreparedEntityCreate, SdkExecutionDiagnostic> {
        input.validate_for(self.installed)?;
        input.validate_native_write()?;
        let prepared = self.prepare_entity_type(input.type_id())?;
        Ok(PreparedEntityCreate {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
            keyed_put: self.lower_keyed_put(input)?,
            plan_write: self.lower_plan_write(input)?,
        })
    }

    fn prepare_relation_create(
        &self,
        input: &ProjectedCreate,
    ) -> Result<PreparedRelationCreate, SdkExecutionDiagnostic> {
        input.validate_for(self.installed)?;
        input.validate_native_write()?;
        let prepared = self.prepare_relation_type(input.type_id())?;
        Ok(PreparedRelationCreate {
            type_id: prepared.type_id,
            descriptor: prepared.descriptor,
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
            .manager_registry()
            .and_then(|registry| registry.entity(type_id.label().as_str()))
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        Ok(PreparedEntityType {
            type_id: type_id.clone(),
            descriptor,
        })
    }

    fn prepare_relation_type(
        &self,
        type_id: &TypeId,
    ) -> Result<PreparedRelationType, SdkExecutionDiagnostic> {
        self.require_exact_model(type_id, TypeKind::Relation)?;
        let descriptor = self
            .installed
            .manager_registry()
            .and_then(|registry| registry.relation(type_id.label().as_str()))
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        Ok(PreparedRelationType {
            type_id: type_id.clone(),
            descriptor,
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

    /// A create as an insert or update plan input: the given non-key values
    /// and every non-key ownership and role the update clears first (the
    /// helpers' complete replacement).
    fn lower_plan_write(
        &self,
        input: &ProjectedCreate,
    ) -> Result<PreparedPlanWrite, SdkExecutionDiagnostic> {
        let model = self.model_integrity(input.type_id())?;
        let tokens = model.query_tokens();
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
            .filter(|token| {
                !token.is_key() && token.multiplicity().collection_mode().is_unordered()
            })
            .map(|token| token.declaring_id().clone())
            .collect();
        cleared_fields.sort();
        cleared_fields.dedup();
        Ok(PreparedPlanWrite {
            created,
            fields,
            cleared_fields,
            cleared_roles: tokens
                .roles()
                .iter()
                .filter(|(_, token)| token.multiplicity().collection_mode().is_unordered())
                .map(|(role, _)| role.clone())
                .collect(),
        })
    }

    /// Split a create into the keyed put plan's keys, single-valued fields,
    /// ownerships to clear and multi-valued values to attach.
    ///
    /// A create without a key value uses the native insert plan without a read;
    /// empty ordered fields are omitted.
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
                    continue;
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
                continue;
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
                    Some((field_id.attribute().clone(), value.value().clone()))
                };
                output.push(PreparedRolePlayerInput {
                    role_id: role_id.clone(),
                    candidate_types,
                    iid: reference.iid().map(str::to_owned),
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let write = &prepared.plan_write;
        let type_id = &prepared.type_id;
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, type_id))?;
        let transport = plan_transport(transaction, type_id, mode).await?;
        let plan = entity_insert(&authority, transport, type_id, &write.created)
            .map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self.hydrate_entity_documents(
            transaction.execution_identity(),
            &prepared.type_id,
            &prepared.descriptor,
            &documents,
            mode,
        );
        after_write(transaction, hydrated).await
    }

    async fn put_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let Some(put) = &prepared.keyed_put else {
            return self
                .insert_entity_prepared(transaction, prepared, mode)
                .await;
        };
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
        self.put_entity_by_plan(transaction, prepared, put, &authority, mode)
            .await
    }

    async fn get_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let manager = self
            .entity_manager(
                transaction,
                Arc::clone(&prepared.descriptor),
                &prepared.type_id,
                mode,
            )
            .await?;
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let write = &prepared.plan_write;
        let type_id = &prepared.type_id;
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, type_id))?;
        let transport = plan_transport(transaction, type_id, mode).await?;
        let plan = update_by_iid(
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
        )
        .map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self
            .hydrate_entity_documents(
                transaction.execution_identity(),
                &prepared.type_id,
                &prepared.descriptor,
                &documents,
                mode,
            )
            .and_then(|result| {
                for thing in result.items() {
                    require_update_iid(thing.iid(), iid, type_id)?;
                }
                Ok(result)
            });
        after_write(transaction, hydrated).await
    }

    async fn delete_entity_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityIdentity,
        mode: ExecutionMode<'_>,
        returning: bool,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        if returning
            && prepared
                .descriptor
                .owned_attributes
                .iter()
                .all(|attribute| !attribute.is_ordered)
        {
            let registry = self.installed.manager_registry().map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?;
            let manager = DynamicEntityManager::with_registry_transaction(
                transaction.clone(),
                registry,
                Arc::clone(&prepared.descriptor),
            );
            let manager = if !transaction.supports_given_stage().await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })? {
                manager.literal_values()
            } else {
                manager
            };
            let rows = manager
                .delete_models_returning(std::slice::from_ref(&prepared.iid), true)
                .await
                .map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?;
            let hydrate = || {
                rows.into_iter()
                    .map(|row| {
                        let mut thing = self.hydrate_entity(
                            row,
                            &prepared.type_id,
                            &prepared.iid,
                            transaction.execution_identity(),
                            mode,
                        )?;
                        thing.mark_deleted()?;
                        Ok(thing)
                    })
                    .collect::<Result<Vec<_>, SdkExecutionDiagnostic>>()
                    .map(WriteResult::new)
            };
            return after_write(transaction, hydrate()).await;
        }
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
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
            return Ok(WriteResult::new(Vec::new()));
        }
        let hydrate = || {
            let mut items = self
                .hydrate_entity_documents(
                    transaction.execution_identity(),
                    &prepared.type_id,
                    &prepared.descriptor,
                    &documents,
                    mode,
                )?
                .into_items();
            for thing in &mut items {
                thing.mark_deleted()?;
            }
            Ok(WriteResult::new(items))
        };
        after_write(transaction, hydrate()).await
    }

    async fn count_entities_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedEntityType,
        mode: ExecutionMode<'_>,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.entity_manager(
            transaction,
            Arc::clone(&prepared.descriptor),
            &prepared.type_id,
            mode,
        )
        .await?
        .count_exact()
        .await
        .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id))
    }

    async fn insert_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
        self.insert_relation_by_plan(transaction, prepared, &authority, mode)
            .await
    }

    /// Insert one relation as one write plan whose match resolves every
    /// role-player reference and whose fetch answers the hydration document,
    /// so nothing is read before or after the write.
    ///
    /// An empty answer returns an empty write result.
    /// Every answered root is hydrated in answer order, including fan-out;
    /// hydration failures still poison a borrowed write transaction.
    async fn insert_relation_by_plan(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        authority: &PlanAuthority,
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        let transport = plan_transport(transaction, type_id, mode).await?;
        let players = plan_references(prepared)?;
        let document_roles = self.document_roles(prepared)?;
        let plan = relation_insert(
            authority,
            transport,
            type_id,
            &prepared.plan_write.created,
            &players,
            document_roles,
        );
        let plan = plan.map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self.hydrate_relation_documents(
            transaction.execution_identity(),
            &prepared.type_id,
            &prepared.descriptor,
            &documents,
            mode,
        );
        after_write(transaction, hydrated).await
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
        let transport = plan_transport(transaction, type_id, mode).await?;
        let plan = keyed_entity_put(
            authority,
            transport,
            &KeyedPut {
                include_subtypes: false,
                return_document: true,
                type_id,
                keys: &put.keys,
                fields: &put.fields,
                cleared: &put.cleared,
                attached: &put.attached,
            },
        );
        let plan = plan.map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self.hydrate_entity_documents(
            transaction.execution_identity(),
            &prepared.type_id,
            &prepared.descriptor,
            &documents,
            mode,
        );
        after_write(transaction, hydrated).await
    }

    pub(crate) fn hydrate_entity_documents(
        &self,
        identity: &DatabaseExecutionIdentity,
        type_id: &TypeId,
        descriptor: &EntityDescriptor,
        documents: &[serde_json::Value],
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for document in documents {
            let iid = document_iid(document, "_iid", type_id)?.ok_or_else(|| {
                integrity_at_type(
                    "provider_identity_answer_invalid",
                    "The provider identity query omitted its canonical IID",
                    type_id,
                )
            })?;
            if seen.insert(iid.clone()) {
                let row = hydrate_dynamic_entity(descriptor, document).map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Read, type_id)
                })?;
                items.push(self.hydrate_entity(row, type_id, &iid, identity, mode)?);
            }
        }
        Ok(WriteResult::new(items))
    }

    pub(crate) fn hydrate_relation_documents(
        &self,
        identity: &DatabaseExecutionIdentity,
        type_id: &TypeId,
        descriptor: &RelationDescriptor,
        documents: &[serde_json::Value],
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let rows = coalesce_dynamic_relations(descriptor, documents)
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Read, type_id))?;
        rows.into_iter()
            .map(|row| {
                let iid = row
                    .iid
                    .clone()
                    .ok_or_else(|| mutation_rehydration_missing(type_id))?;
                self.hydrate_relation(row, type_id, &iid, identity, mode)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(WriteResult::new)
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let type_id = &prepared.type_id;
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
        let plan = plan.map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self
            .hydrate_relation_documents(
                transaction.execution_identity(),
                &prepared.type_id,
                &prepared.descriptor,
                &documents,
                mode,
            )
            .and_then(|result| {
                for thing in result.items() {
                    require_update_iid(thing.iid(), iid, type_id)?;
                }
                Ok(result)
            });
        after_write(transaction, hydrated).await
    }

    async fn put_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationCreate,
        input: &ProjectedCreate,
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
        let Some(put) = self.lower_keyed_put(input)? else {
            return self
                .insert_relation_by_plan(transaction, prepared, &authority, mode)
                .await;
        };
        let type_id = &prepared.type_id;
        let players = plan_references(prepared)?;
        let plan = keyed_relation_put(
            &authority,
            plan_transport(transaction, type_id, mode).await?,
            &KeyedPut {
                include_subtypes: false,
                return_document: true,
                type_id,
                keys: &put.keys,
                fields: &put.fields,
                cleared: &put.cleared,
                attached: &put.attached,
            },
            RelationPut {
                players: &players,
                cleared_roles: &prepared.plan_write.cleared_roles,
                document_roles: self.document_roles(prepared)?,
            },
        )
        .map_err(|error| plan_build_failure(&error, type_id))?;
        let documents = execute_write_rows(transaction, plan.local(), type_id, mode).await?;
        let hydrated = self.hydrate_relation_documents(
            transaction.execution_identity(),
            &prepared.type_id,
            &prepared.descriptor,
            &documents,
            mode,
        );
        after_write(transaction, hydrated).await
    }

    async fn get_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationIdentity,
        mode: ExecutionMode<'_>,
    ) -> Result<Option<ProjectedThing>, SdkExecutionDiagnostic> {
        let manager = self
            .relation_manager(
                transaction,
                Arc::clone(&prepared.descriptor),
                &prepared.type_id,
                mode,
            )
            .await?;
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
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
        self.update_relation_by_plan(
            transaction,
            iid,
            prepared,
            &prepared.plan_write,
            &authority,
            mode,
        )
        .await
    }

    async fn delete_relation_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationIdentity,
        mode: ExecutionMode<'_>,
        returning: bool,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        if returning
            && prepared.descriptor.roles.iter().all(|role| !role.ordered)
            && prepared
                .descriptor
                .owned_attributes
                .iter()
                .all(|attribute| !attribute.is_ordered)
        {
            let registry = self.installed.manager_registry().map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })?;
            let manager = DynamicRelationManager::with_registry_transaction(
                transaction.clone(),
                registry,
                Arc::clone(&prepared.descriptor),
            );
            let manager = if !transaction.supports_given_stage().await.map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Read, &prepared.type_id)
            })? {
                manager.literal_values()
            } else {
                manager
            };
            let rows = manager
                .delete_models_returning(std::slice::from_ref(&prepared.iid), true)
                .await
                .map_err(|error| {
                    mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
                })?;
            let hydrate = || {
                rows.into_iter()
                    .map(|row| {
                        let mut thing = self.hydrate_relation(
                            row,
                            &prepared.type_id,
                            &prepared.iid,
                            transaction.execution_identity(),
                            mode,
                        )?;
                        thing.mark_deleted()?;
                        Ok(thing)
                    })
                    .collect::<Result<Vec<_>, SdkExecutionDiagnostic>>()
                    .map(WriteResult::new)
            };
            return after_write(transaction, hydrate()).await;
        }
        let authority = self
            .installed
            .plan_authority_with_query_limits(transaction.shared_query_limits())
            .map_err(|error| plan_build_failure(&error, &prepared.type_id))?;
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
            return Ok(WriteResult::new(Vec::new()));
        }
        let hydrate = || {
            let mut items = self
                .hydrate_relation_documents(
                    transaction.execution_identity(),
                    &prepared.type_id,
                    &prepared.descriptor,
                    &documents,
                    mode,
                )?
                .into_items();
            for thing in &mut items {
                thing.mark_deleted()?;
            }
            Ok(WriteResult::new(items))
        };
        after_write(transaction, hydrate()).await
    }

    async fn count_relations_prepared(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedRelationType,
        mode: ExecutionMode<'_>,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.relation_manager(
            transaction,
            Arc::clone(&prepared.descriptor),
            &prepared.type_id,
            mode,
        )
        .await?
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
            if token.multiplicity().collection_mode() == CollectionMode::OrderedList {
                return Err(integrity_at_role(
                    "query_plan_hydration_ordered_role",
                    "populated ordered roles require provider fetch ordinals",
                    type_id,
                    role_id,
                ));
            }
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

    fn constrained_by(mut self, ceiling: Option<QueryExecutionResourceLimits>) -> Self {
        if let Some(ceiling) = ceiling {
            self.limits = self.limits.constrained_by(ceiling);
            self.deadline = self
                .deadline
                .earliest(QueryExecutionDeadline::for_limits(ceiling));
        }
        self
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
        _ = deadline.elapsed() => Err(CrudControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_deadline_exceeded())),
        result = &mut future => result.map_err(CrudControlledAwaitError::Inner),
    }
}

fn controlled_crud_orm(
    error: CrudControlledAwaitError<OrmError>,
    operation: SdkProviderOperation,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    ExecutionMode::default().controlled_crud_orm(error, operation, type_id)
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
        if statements > self.limits.statement_limit() {
            return Err(crud_resource_limit(
                "projected_crud_statement_limit",
                "Controlled CRUD exceeds the provider statement ceiling",
                "statements",
                u64::from(statements),
                u64::from(self.limits.statement_limit()),
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

fn single_batch_write_result(
    result: ProjectedBatchResult,
    type_id: &TypeId,
) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
    match result {
        ProjectedBatchResult::Things(things) => Ok(WriteResult::new(things.into_items())),
        _ => Err(single_batch_result_invalid(type_id)),
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
    /// The create as a keyed put plan input, when one can express it.
    keyed_put: Option<PreparedKeyedPut>,
    /// The create as an insert or update plan input.
    plan_write: PreparedPlanWrite,
}

/// Declaring ownerships and values of one keyed put.
struct PreparedKeyedPut {
    keys: Vec<(OwnsFactId, CanonicalValue)>,
    fields: Vec<(OwnsFactId, CanonicalValue)>,
    cleared: Vec<OwnsFactId>,
    attached: Vec<(OwnsFactId, CanonicalValue)>,
}

/// A create in plan form: the given values and every non-key ownership an
/// update clears first. Empty ordered fields and roles are omitted.
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
    /// The create as an update plan input.
    plan_write: PreparedPlanWrite,
    role_players: Vec<PreparedRolePlayerInput>,
}

struct PreparedRolePlayerInput {
    role_id: RoleId,
    candidate_types: Vec<TypeId>,
    iid: Option<String>,
    /// Key identity for a reference without an IID.
    plan_key: Option<(AttributeId, CanonicalValue)>,
    database_identity: Option<DatabaseExecutionIdentity>,
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

pub(crate) fn validate_iid(type_id: &TypeId, iid: &str) -> Result<(), SdkExecutionDiagnostic> {
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

pub(crate) fn validate_returned_iid(
    iid: &str,
    requested_iid: &str,
    type_id: &TypeId,
) -> Result<(), SdkExecutionDiagnostic> {
    if iid != requested_iid || !is_canonical_thing_iid(iid) {
        return Err(integrity_at_type(
            "hydrated_iid_mismatch",
            "The provider row returned a different or noncanonical IID",
            type_id,
        ));
    }
    Ok(())
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
    validate_returned_iid(&iid, requested_iid, type_id)?;
    if concrete_type.as_deref() != Some(type_id.label().as_str()) {
        return Err(integrity_at_type(
            "hydrated_type_mismatch",
            "The provider row returned a different exact concrete type",
            type_id,
        ));
    }
    Ok(iid)
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

fn require_update_iid(
    returned: &str,
    expected: &str,
    type_id: &TypeId,
) -> Result<(), SdkExecutionDiagnostic> {
    if returned.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(crate::projected_batch_executor::integrity_at_row(
            "projected_batch_update_identity_changed",
            "A projected update returned a different exact target IID",
            type_id,
            0,
        ))
    }
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
        materialized_reads: usize,
        queries: Vec<(String, Option<GivenRowsSpec>)>,
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

        fn query_plan_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            limits: AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, crate::Result<BoundedAnswerStats>> {
            {
                let mut state = self.state.lock().unwrap();
                state.bounded_reads += 1;
                state.queries.push((_typeql.to_owned(), None));
            }
            stream_controlled_response(self.response(), limits.answer, consumer, _typeql)
        }

        fn query_plan_with_rows_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            _rows: GivenRowsSpec,
            limits: AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, crate::Result<BoundedAnswerStats>> {
            {
                let mut state = self.state.lock().unwrap();
                state.bounded_given += 1;
                state
                    .queries
                    .push((_typeql.to_owned(), Some(_rows.clone())));
            }
            stream_controlled_response(self.response(), limits.answer, consumer, _typeql)
        }

        fn query_materialized_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            rows: Option<GivenRowsSpec>,
            limits: AnswerLimits,
            _canonical: bool,
            _statement_display: Option<&'a str>,
        ) -> BoxFuture<'a, crate::Result<QueryResult>> {
            {
                let mut state = self.state.lock().unwrap();
                state.materialized_reads += 1;
                state.queries.push((typeql.to_owned(), rows));
            }
            let response = self.response();
            Box::pin(async move {
                let mut collector = ControlledAnswerCollector::new(
                    ControlledAnswerKind::Documents,
                    "native exact read",
                );
                stream_controlled_response(response, limits.answer, &mut collector, typeql).await?;
                Ok(collector.finish())
            })
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
        source: &'consumer str,
    ) -> BoxFuture<'consumer, crate::Result<BoundedAnswerStats>> {
        Box::pin(async move {
            match response {
                ControlledProviderResponse::Items(items) => {
                    let mut reader = BoundedAnswerReader::new(limits);
                    reader.check_before_read()?;
                    for item in items {
                        let item = match item {
                            AnswerItem::Document(document) => {
                                crate::native_batch_fixture::item(source, document)
                            }
                            item => item,
                        };
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
        controlled_installed_from(
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
        )
    }

    fn controlled_installed_from(schema: &str) -> InstalledRuntimeProjection {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("controlled-crud.yaml").unwrap(), schema)])
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
        let mut document = if kind == TypeKind::Entity {
            json!({"_iid": "0x20", "_type": "person", "attributes": {"identifier": "key"}})
        } else {
            json!({"_iid": "0x30", "_type": "membership", "attributes": {"identifier": "key"}})
        };
        if kind == TypeKind::Relation {
            document["_role_0_iid"] = json!("0x10");
            document["_role_0_type"] = json!("person");
            document["_role_0_attributes"] = json!({"identifier": "player-key"});
        }
        vec![ControlledProviderResponse::Items(vec![
            AnswerItem::Document(document),
        ])]
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
    async fn controlled_single_write_boundary_refuses_empty_or_multiple_inputs_before_io() {
        let installed = controlled_installed();
        let executor = ProjectedCrudExecutor::new(&installed);
        let model = controlled_type(TypeKind::Entity);
        let field =
            OwnsFactId::new(model.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        for count in [0, 2] {
            let rows = (0..count)
                .map(|index| {
                    let value = ProjectedAttributeValue::try_new(
                        &installed,
                        TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
                        CanonicalValue::String(
                            CanonicalString::new(format!("key-{index}")).unwrap(),
                        ),
                    )
                    .unwrap();
                    ProjectedBatchRow::Create(
                        ProjectedCreate::try_new(
                            &installed,
                            model.clone(),
                            vec![(field.clone(), vec![value])],
                            vec![],
                        )
                        .unwrap(),
                    )
                })
                .collect();
            let batch = ProjectedBatch::try_new(
                &installed,
                model.clone(),
                ProjectedBatchOperation::Insert,
                rows,
            )
            .unwrap();
            let (database, state) = controlled_database(vec![]);
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let diagnostic = executor
                .execute_controlled_single_write(
                    controlled_update::WriteTarget::Database(&database),
                    &batch,
                    control,
                    Ok,
                )
                .await
                .unwrap_err();
            assert_eq!(
                diagnostic.code().as_str(),
                "projected_crud_single_row_required"
            );
            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.queries.is_empty());
            assert_eq!(state.commits, 0);
        }
    }

    #[tokio::test]
    async fn controlled_single_write_mapper_refusal_precedes_commit_and_latches_borrowed() {
        let installed = controlled_installed();
        let executor = ProjectedCrudExecutor::new(&installed);
        for borrowed in [false, true] {
            let (database, state) =
                controlled_database(controlled_mutation_answers(TypeKind::Entity));
            let transaction = if borrowed {
                Some(database.transaction_context(TxType::Write).await.unwrap())
            } else {
                None
            };
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let batch = executor
                .single_controlled_write_batch(
                    &controlled_type(TypeKind::Entity),
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchRow::Create(controlled_create(&installed, TypeKind::Entity)),
                    &control,
                )
                .unwrap();
            let target = match &transaction {
                Some(transaction) => controlled_update::WriteTarget::Transaction(transaction),
                None => controlled_update::WriteTarget::Database(&database),
            };
            let diagnostic = executor
                .execute_controlled_single_write(target, &batch, control, |result| {
                    assert_eq!(result.count(), 1);
                    assert_eq!(result.item().unwrap().iid(), "0x20");
                    Err::<(), _>(integrity_at_type(
                        "mapped_result_refused",
                        "The binding cannot publish this result",
                        batch.model(),
                    ))
                })
                .await
                .unwrap_err();
            assert_eq!(diagnostic.code().as_str(), "mapped_result_refused");
            {
                let observed = state.lock().unwrap();
                assert_eq!(observed.opens, [TxType::Write]);
                assert_eq!(observed.queries.len(), 1);
                assert!(!observed.queries[0].0.contains("count("));
                assert_eq!(observed.commits, 0);
                assert_eq!(observed.rollbacks, usize::from(!borrowed));
            }
            if let Some(transaction) = transaction {
                assert_eq!(
                    transaction.lifecycle_state().await,
                    TransactionContextState::RollbackOnly
                );
                transaction.rollback().await.unwrap();
            }
            assert_eq!(state.lock().unwrap().rollbacks, 1);
        }
    }

    #[tokio::test]
    async fn controlled_single_inserts_preserve_atomicity_for_both_model_kinds() {
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
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        assert_eq!(thing.iid(), "0x20");
        {
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.bounded_given, 1);
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
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        assert_eq!(thing.iid(), "0x30");
        assert_eq!(
            transaction.lifecycle_state().await,
            TransactionContextState::Active
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.bounded_given, 1);
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

        let (database, state) = controlled_database(vec![ControlledProviderResponse::Error(
            "private borrowed mutation failure",
        )]);
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
    async fn controlled_exact_reads_match_ordinary_empty_ordered_reads_owned_and_borrowed() {
        let installed = controlled_installed_from(
            r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      tag: { ordered: true, card: { min: 0, max: 4 } }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 4 } }
      participant: { ordered: true, card: { min: 0, max: 4 } }
plays:
  person:
    membership: [member, participant]
"#,
        );
        let executor = ProjectedCrudExecutor::new(&installed);
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for borrowed in [false, true] {
                let type_id = controlled_type(kind);
                let mut sources = Vec::new();
                for controlled in [false, true] {
                    let mut document = json!({
                        "_iid":"0x20", "_type":type_id.label().as_str(),
                        "attributes":{"identifier":"key"}
                    });
                    if kind == TypeKind::Relation {
                        document["_role_0_iid"] = json!(null);
                        document["_role_0_type"] = json!(null);
                        document["_role_0_attributes"] = json!(null);
                        document["_role_1_iid"] = json!(null);
                        document["_role_1_type"] = json!(null);
                        document["_role_1_attributes"] = json!(null);
                    }
                    let (database, state) = controlled_database(vec![
                        ControlledProviderResponse::Items(vec![AnswerItem::Document(document)]),
                    ]);
                    let transaction = if borrowed {
                        Some(database.transaction_context(TxType::Read).await.unwrap())
                    } else {
                        None
                    };
                    let limits = QueryExecutionResourceLimits {
                        statements: Some(1),
                        ..Default::default()
                    };
                    let cancellation = AnswerCancellation::default();
                    let (observed_iid, ordered_empty) = if controlled {
                        let thing = match (kind, transaction.as_ref(), true) {
                            (TypeKind::Entity, None, true) => {
                                executor
                                    .get_entity_by_iid_controlled(
                                        &database,
                                        &type_id,
                                        "0x20",
                                        limits,
                                        cancellation,
                                    )
                                    .await
                            }
                            (TypeKind::Relation, None, true) => {
                                executor
                                    .get_relation_by_iid_controlled(
                                        &database,
                                        &type_id,
                                        "0x20",
                                        limits,
                                        cancellation,
                                    )
                                    .await
                            }
                            (TypeKind::Entity, Some(tx), true) => {
                                executor
                                    .get_entity_by_iid_in_transaction_controlled(
                                        tx,
                                        &type_id,
                                        "0x20",
                                        limits,
                                        cancellation,
                                    )
                                    .await
                            }
                            (TypeKind::Relation, Some(tx), true) => {
                                executor
                                    .get_relation_by_iid_in_transaction_controlled(
                                        tx,
                                        &type_id,
                                        "0x20",
                                        limits,
                                        cancellation,
                                    )
                                    .await
                            }
                            _ => unreachable!(),
                        }
                        .unwrap()
                        .unwrap();
                        let empty = if kind == TypeKind::Entity {
                            let tag =
                                OwnsFactId::new(type_id.clone(), AttributeId::new("tag").unwrap())
                                    .unwrap();
                            thing.fields()[&tag].is_empty()
                        } else {
                            thing.roles()[&RoleId::new("membership", "participant").unwrap()]
                                .is_empty()
                        };
                        (thing.iid().to_owned(), empty)
                    } else {
                        let registry = installed.manager_registry().unwrap();
                        if kind == TypeKind::Entity {
                            let prepared =
                                executor.prepare_entity_identity(&type_id, "0x20").unwrap();
                            let manager = if let Some(tx) = transaction.as_ref() {
                                DynamicEntityManager::with_registry_transaction(
                                    tx.clone(),
                                    registry,
                                    prepared.descriptor,
                                )
                            } else {
                                DynamicEntityManager::from_registry(
                                    &database,
                                    registry,
                                    prepared.descriptor,
                                )
                            };
                            let row = manager.get_by_iid_exact("0x20").await.unwrap().unwrap();
                            (
                                row.iid.unwrap(),
                                row.attributes.iter().all(|(name, _)| name != "tag"),
                            )
                        } else {
                            let prepared = executor
                                .prepare_relation_identity(&type_id, "0x20")
                                .unwrap();
                            let manager = if let Some(tx) = transaction.as_ref() {
                                DynamicRelationManager::with_registry_transaction(
                                    tx.clone(),
                                    registry,
                                    prepared.descriptor,
                                )
                            } else {
                                DynamicRelationManager::from_registry(
                                    &database,
                                    registry,
                                    prepared.descriptor,
                                )
                            };
                            let mut rows = manager.get_by_iid_exact("0x20").await.unwrap();
                            assert_eq!(rows.len(), 1);
                            let row = rows.pop().unwrap();
                            (
                                row.iid.unwrap(),
                                row.role_players
                                    .iter()
                                    .all(|player| player.role_name != "participant"),
                            )
                        }
                    };
                    assert_eq!(observed_iid, "0x20");
                    assert!(ordered_empty);
                    {
                        let state = state.lock().unwrap();
                        assert_eq!(state.queries.len(), 1);
                        assert_eq!(state.bounded_reads, usize::from(controlled));
                        assert_eq!(state.materialized_reads, usize::from(!controlled));
                        assert_eq!(state.bounded_given, 0);
                        assert_eq!(state.unbounded_queries, 0);
                        assert_eq!(state.opens, [TxType::Read]);
                        assert_eq!(state.closes, usize::from(!borrowed));
                        assert_eq!(state.commits, 0);
                        assert_eq!(state.rollbacks, 0);
                        sources.push(state.queries.clone());
                    }
                    if let Some(tx) = transaction {
                        assert_eq!(tx.lifecycle_state().await, TransactionContextState::Active);
                        tx.close().await.unwrap();
                        assert_eq!(state.lock().unwrap().closes, 1);
                    }
                }
                assert_eq!(sources[0], sources[1], "{kind:?}, borrowed={borrowed}");
            }
        }
        for borrowed in [false, true] {
            let populated = json!({
                "_iid":"0x20", "_type":"membership", "attributes":{"identifier":"key"},
                "_role_0_iid":null, "_role_0_type":null, "_role_0_attributes":null,
                "_role_1_iid":"0x10", "_role_1_type":"person",
                "_role_1_attributes":{"identifier":"player"}
            });
            let (database, state) = controlled_database(vec![
                ControlledProviderResponse::Items(vec![AnswerItem::Document(populated)]),
                ControlledProviderResponse::Items(vec![AnswerItem::Document(json!({
                    "_iid":"0x40", "_type":"person", "attributes":{"identifier":"reused"}
                }))]),
            ]);
            let transaction = if borrowed {
                Some(database.transaction_context(TxType::Read).await.unwrap())
            } else {
                None
            };
            let membership = controlled_type(TypeKind::Relation);
            let result = match transaction.as_ref() {
                Some(tx) => {
                    executor
                        .get_relation_by_iid_in_transaction_controlled(
                            tx,
                            &membership,
                            "0x20",
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                }
                None => {
                    executor
                        .get_relation_by_iid_controlled(
                            &database,
                            &membership,
                            "0x20",
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                }
            };
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_plan_hydration_ordered_role"
            );
            {
                let state = state.lock().unwrap();
                assert_eq!(state.queries.len(), 1);
                assert_eq!(state.closes, usize::from(!borrowed));
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 0);
            }
            if let Some(tx) = transaction {
                assert_eq!(tx.lifecycle_state().await, TransactionContextState::Active);
                let reused = executor
                    .get_entity_by_iid_in_transaction_controlled(
                        &tx,
                        &controlled_type(TypeKind::Entity),
                        "0x40",
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(reused.iid(), "0x40");
                assert_eq!(state.lock().unwrap().queries.len(), 2);
                tx.close().await.unwrap();
            }
            assert_eq!(state.lock().unwrap().closes, 1);
        }
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
            ControlledProviderResponse::Items(vec![AnswerItem::Row(
                json!({"count1": {"kind": "value", "value": CanonicalValue::Long(1)}}),
            )]),
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
            ControlledProviderResponse::Items(vec![AnswerItem::Row(
                json!({"count1": {"kind": "value", "value": CanonicalValue::Long(2)}}),
            )]),
            ControlledProviderResponse::Pending(Arc::clone(&entered)),
            ControlledProviderResponse::Items(vec![AnswerItem::Row(
                json!({"count1": {"kind": "value", "value": CanonicalValue::Long(1)}}),
            )]),
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

    #[tokio::test]
    async fn controlled_counts_apply_the_connection_query_limits_before_dispatch() {
        let installed = controlled_installed();
        let (database, state) = controlled_database(vec![]);
        let database = database
            .with_query_limits(
                type_bridge_contract::limits::QueryLimits::from_overrides([
                    ("bindings", 1),
                    ("selected_slots", 1),
                ])
                .unwrap(),
            )
            .unwrap();
        let error = ProjectedCrudExecutor::new(&installed)
            .count_on_target(
                crate::QueryTarget::Database(&database),
                &controlled_type(TypeKind::Entity),
                ProjectedCrudInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error
                .into_diagnostic(SdkProviderOperation::Read)
                .code()
                .as_str(),
            "query_plan_binding_limit"
        );
        let state = state.lock().unwrap();
        assert!(state.queries.is_empty());
        assert_eq!(state.bounded_reads, 0);
        assert_eq!(state.unbounded_queries, 0);
        assert_eq!(state.closes, 1);
    }

    #[tokio::test]
    async fn proxy_exact_reads_negotiate_uncached_limits_before_planning() {
        use crate::plan_authority::QueryAuthority;
        use crate::remote_signing::RemoteReplySigningKey;
        use crate::session::StagedTransport;
        use crate::{ProxyQueryConnection, QueryTarget, QueryTargetError};
        use type_bridge_contract::capability::CapabilityId;
        use type_bridge_contract::query_remote::RemoteValue;
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        use type_bridge_contract::query_remote_v3::{
            CAP_QUERY_REMOTE_ANSWER_DIMENSIONS, CAP_QUERY_REMOTE_STAGED_V3, StagedAnswerV3,
            StagedKindV3, StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
        };

        struct Transport {
            schema: type_bridge_contract::schema::DeclaredSchema,
            advertisement: RemoteCapabilities,
            signer: RemoteReplySigningKey,
            expected: Mutex<VecDeque<Vec<u8>>>,
            calls: std::sync::atomic::AtomicUsize,
            advertisements: std::sync::atomic::AtomicUsize,
        }
        impl StagedTransport for Transport {
            fn staged_capabilities(&self) -> BoxFuture<'_, Result<Vec<u8>, QueryTargetError>> {
                Box::pin(async move {
                    self.advertisements
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(self.advertisement.encode().unwrap())
                })
            }
            fn exchange_staged<'a>(
                &'a self,
                bytes: &'a [u8],
            ) -> BoxFuture<'a, Result<Vec<u8>, QueryTargetError>> {
                Box::pin(async move {
                    self.calls
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let request =
                        StagedRequestV3::decode(bytes, &self.schema, &self.advertisement).unwrap();
                    assert_eq!(request.kind(), StagedKindV3::Read);
                    let mut answers = Vec::new();
                    for entry in request.plans() {
                        let plan = entry.plan();
                        assert_eq!(
                            plan.canonical_bytes().unwrap(),
                            self.expected.lock().unwrap().pop_front().unwrap()
                        );
                        let answer = if matches!(
                            plan.pipeline().last(),
                            Some(type_bridge_contract::query_plan::v3::Stage::Fetch(_))
                        ) {
                            StagedAnswerV3::Documents { documents: vec![] }
                        } else {
                            StagedAnswerV3::Rows {
                                rows: if plan.pipeline().iter().any(|stage| {
                                    matches!(
                                        stage,
                                        type_bridge_contract::query_plan::v3::Stage::Reduce { .. }
                                    )
                                }) {
                                    vec![vec![RemoteValue::Value {
                                        value: CanonicalValue::Long(2),
                                    }]]
                                } else {
                                    vec![]
                                },
                            }
                        };
                        answers.push(answer);
                    }
                    Ok(
                        StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers)
                            .unwrap()
                            .encode_signed(&request, &self.advertisement, &self.signer)
                            .unwrap(),
                    )
                })
            }
        }
        let mut schema_source = String::from(
            "format: typebridge.schema/v2\nentities:\n  person: {}\nrelations:\n  membership:\n    relates:\n",
        );
        for index in 0..20 {
            schema_source.push_str(&format!("      member-{index}: {{ card: {{ min: 0 }} }}\n"));
        }
        schema_source.push_str("plays:\n  person:\n    membership: [");
        schema_source.push_str(
            &(0..20)
                .map(|index| format!("member-{index}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        schema_source.push_str("]\n");
        let installed = controlled_installed_from(&schema_source);
        let schema = installed.plan_authority().unwrap().declared().clone();
        let context = type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("crud-read-target-tests")
                .unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            type_bridge_schema::schema_authority_capability_vocabulary(),
        );
        let verified = type_bridge_schema::build_schema_authority(
            &schema,
            schema.required_capabilities(),
            &context,
        )
        .unwrap();
        let installed = installed.with_schema_authority(&verified);
        let executor = ProjectedCrudExecutor::new(&installed);
        let authority = Arc::new(
            QueryAuthority::from_declared_bytes(
                &type_bridge_contract::schema::encode_declared_schema(&schema).unwrap(),
                "crud-read-target-tests",
                "typedb-3.12.1/v1",
            )
            .unwrap(),
        );
        let signer = RemoteReplySigningKey::from_secret_bytes([73; 32]);
        let mut capabilities =
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        for capability in [
            CAP_QUERY_REMOTE_STAGED_V3,
            CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
        ] {
            capabilities.insert(CapabilityId::new(capability).unwrap());
        }
        let mut server = type_bridge_contract::limits::QueryLimits::V3;
        server.structural.bindings = 16;
        server.structural.selected_slots = 16;
        let server = Arc::new(server);
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("crud-read-target-tests", "crud-read-target-epoch").unwrap(),
            signer.public_key(),
        )
        .with_query_limits(*server)
        .unwrap();

        let mut client = type_bridge_contract::limits::QueryLimits::V3;
        client.structural.bindings = 1024;
        client.structural.selected_slots = 1024;
        client.structural.boolean_terms = 1024;
        let client = Arc::new(client);
        let model = controlled_type(TypeKind::Relation);
        let legacy = RemoteCapabilities::new(
            type_bridge_contract::capability::CapabilitySet::new(),
            advertisement.executor().clone(),
            signer.public_key(),
        );
        for count in [false, true] {
            let (_, expected) = executor
                .manager_read_program(
                    &model,
                    if count { None } else { Some("0xffff") },
                    if count {
                        ReadTerminal::Count
                    } else {
                        ReadTerminal::Fetch
                    },
                    Some(&server),
                )
                .unwrap();
            let transport = Arc::new(Transport {
                schema: schema.clone(),
                advertisement: advertisement.clone(),
                signer: RemoteReplySigningKey::from_secret_bytes([73; 32]),
                expected: Mutex::new(
                    expected
                        .plans()
                        .iter()
                        .map(|plan| plan.canonical_bytes().unwrap())
                        .collect(),
                ),
                calls: 0.into(),
                advertisements: 0.into(),
            });
            let connection = ProxyQueryConnection::with_exchange(
                legacy.encode().unwrap(),
                Arc::clone(&authority),
                QueryExecutionResourceLimits::default(),
            )
            .unwrap()
            .with_query_limits(*client)
            .unwrap()
            .with_staged_transport(transport.clone());
            assert!(connection.staged_advertisement().is_none());
            assert_eq!(connection.query_limits(), *client);
            let control =
                ProjectedCrudInvocationControl::capture(Default::default(), Default::default());
            if count {
                assert_eq!(
                    executor
                        .count_on_target(QueryTarget::Proxy(&connection), &model, control)
                        .await
                        .unwrap(),
                    2
                );
            } else {
                assert!(
                    executor
                        .get_by_iid_on_target(
                            QueryTarget::Proxy(&connection),
                            &model,
                            "0xffff",
                            control
                        )
                        .await
                        .unwrap()
                        .is_none()
                );
            }
            assert_eq!(connection.query_limits(), *server);
            assert_eq!(
                transport
                    .advertisements
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
            assert_eq!(
                transport.calls.load(std::sync::atomic::Ordering::Relaxed),
                1
            );
            assert!(transport.expected.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn proxy_exact_reads_keep_native_plan_bytes_and_application_errors() {
        use crate::plan_authority::QueryAuthority;
        use crate::remote_signing::RemoteReplySigningKey;
        use crate::session::StagedTransport;
        use crate::{ProxyQueryConnection, QueryTarget, QueryTargetError};
        use type_bridge_contract::capability::CapabilityId;
        use type_bridge_contract::query_remote::RemoteValue;
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        use type_bridge_contract::query_remote_v3::{
            CAP_QUERY_REMOTE_ANSWER_DIMENSIONS, CAP_QUERY_REMOTE_STAGED_V3, StagedAnswerV3,
            StagedKindV3, StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
        };

        struct Transport {
            schema: type_bridge_contract::schema::DeclaredSchema,
            advertisement: RemoteCapabilities,
            signer: RemoteReplySigningKey,
            expected: Mutex<VecDeque<Vec<u8>>>,
            calls: std::sync::atomic::AtomicUsize,
            fail: std::sync::atomic::AtomicBool,
        }
        impl StagedTransport for Transport {
            fn staged_capabilities(&self) -> BoxFuture<'_, Result<Vec<u8>, QueryTargetError>> {
                panic!("the staged advertisement is pinned")
            }
            fn exchange_staged<'a>(
                &'a self,
                bytes: &'a [u8],
            ) -> BoxFuture<'a, Result<Vec<u8>, QueryTargetError>> {
                Box::pin(async move {
                    self.calls
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(QueryTargetError::Callback(Box::new(std::io::Error::other(
                            "application cause",
                        ))));
                    }
                    let request =
                        StagedRequestV3::decode(bytes, &self.schema, &self.advertisement).unwrap();
                    assert_eq!(request.kind(), StagedKindV3::Read);
                    assert_eq!(request.plans().len(), 1);
                    let plan = request.plans()[0].plan();
                    assert_eq!(
                        plan.canonical_bytes().unwrap(),
                        self.expected.lock().unwrap().pop_front().unwrap()
                    );
                    let answer = if matches!(
                        plan.pipeline().last(),
                        Some(type_bridge_contract::query_plan::v3::Stage::Fetch(_))
                    ) {
                        StagedAnswerV3::Documents { documents: vec![] }
                    } else {
                        StagedAnswerV3::Rows {
                            rows: vec![vec![RemoteValue::Value {
                                value: CanonicalValue::Long(2),
                            }]],
                        }
                    };
                    Ok(
                        StagedReplyV3::new(&request, StagedOutcomeV3::Committed, vec![answer])
                            .unwrap()
                            .encode_signed(&request, &self.advertisement, &self.signer)
                            .unwrap(),
                    )
                })
            }
        }
        let installed = controlled_installed();
        let schema = installed.plan_authority().unwrap().declared().clone();
        let context = type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("crud-read-target-tests")
                .unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            type_bridge_schema::schema_authority_capability_vocabulary(),
        );
        let verified = type_bridge_schema::build_schema_authority(
            &schema,
            schema.required_capabilities(),
            &context,
        )
        .unwrap();
        let unverified = installed;
        let installed = controlled_installed().with_schema_authority(&verified);
        let executor = ProjectedCrudExecutor::new(&installed);
        let authority = Arc::new(
            QueryAuthority::from_declared_bytes(
                &type_bridge_contract::schema::encode_declared_schema(&schema).unwrap(),
                "crud-read-target-tests",
                "typedb-3.12.1/v1",
            )
            .unwrap(),
        );
        let signer = RemoteReplySigningKey::from_secret_bytes([73; 32]);
        let mut capabilities =
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        for capability in [
            CAP_QUERY_REMOTE_STAGED_V3,
            CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
        ] {
            capabilities.insert(CapabilityId::new(capability).unwrap());
        }
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("crud-read-target-tests", "crud-read-target-epoch").unwrap(),
            signer.public_key(),
        );
        let mut expected = VecDeque::new();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            let model = controlled_type(kind);
            for (iid, terminal) in [
                (None, ReadTerminal::Count),
                (Some("0xffff"), ReadTerminal::Fetch),
            ] {
                let (_, program) = executor
                    .manager_read_program(&model, iid, terminal, None)
                    .unwrap();
                assert_eq!(program.plans().len(), 1);
                expected.push_back(program.plans()[0].canonical_bytes().unwrap());
            }
        }
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            let (_, program) = executor
                .manager_read_program(
                    &controlled_type(kind),
                    Some("0xffff"),
                    ReadTerminal::Fetch,
                    None,
                )
                .unwrap();
            expected.push_back(program.plans()[0].canonical_bytes().unwrap());
        }
        let bytes = advertisement.encode().unwrap();
        let transport = Arc::new(Transport {
            schema,
            advertisement,
            signer,
            expected: Mutex::new(expected),
            calls: 0.into(),
            fail: false.into(),
        });
        let connection = ProxyQueryConnection::with_exchange(
            bytes,
            authority,
            QueryExecutionResourceLimits::default(),
        )
        .unwrap()
        .with_staged_transport(transport.clone());
        let control = || {
            ProjectedCrudInvocationControl::capture(
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
        };
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            let model = controlled_type(kind);
            assert_eq!(
                executor
                    .count_on_target(QueryTarget::Proxy(&connection), &model, control())
                    .await
                    .unwrap(),
                2
            );
            assert!(
                executor
                    .get_by_iid_on_target(
                        QueryTarget::Proxy(&connection),
                        &model,
                        "0xffff",
                        control()
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            assert!(
                executor
                    .get_by_iid_on_target(
                        QueryTarget::Proxy(&connection),
                        &controlled_type(kind),
                        "0xffff",
                        None
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert!(transport.expected.lock().unwrap().is_empty());
        assert_eq!(
            transport.calls.load(std::sync::atomic::Ordering::Relaxed),
            6
        );
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let error = executor
            .count_on_target(
                QueryTarget::Proxy(&connection),
                &controlled_type(TypeKind::Entity),
                ProjectedCrudInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    cancellation,
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error
                .into_diagnostic(SdkProviderOperation::Read)
                .code()
                .as_str(),
            "provider_cancelled"
        );
        assert_eq!(
            transport.calls.load(std::sync::atomic::Ordering::Relaxed),
            6
        );
        let error = ProjectedCrudExecutor::new(&unverified)
            .count_on_target(
                QueryTarget::Proxy(&connection),
                &controlled_type(TypeKind::Entity),
                control(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error
                .into_diagnostic(SdkProviderOperation::Read)
                .code()
                .as_str(),
            "write_plan_authority_unverified"
        );
        assert_eq!(
            transport.calls.load(std::sync::atomic::Ordering::Relaxed),
            6
        );
        transport
            .fail
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let error = executor
            .count_on_target(
                QueryTarget::Proxy(&connection),
                &controlled_type(TypeKind::Entity),
                control(),
            )
            .await
            .unwrap_err();
        assert_eq!(format!("{error:?}"), "Callback([REDACTED])");
        let QueryTargetError::Callback(source) = error else {
            panic!("application-owned cause")
        };
        assert_eq!(source.to_string(), "application cause");
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
            single_batch_write_result(ProjectedBatchResult::Deleted, &type_id)
                .unwrap_err()
                .code()
                .as_str(),
            "projected_crud_batch_result_invalid"
        );
        assert!(
            single_batch_write_result(
                ProjectedBatchResult::Things(crate::BatchWriteResult::default()),
                &type_id
            )
            .unwrap()
            .items()
            .is_empty()
        );
    }
}
