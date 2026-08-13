//! Exact single-model CRUD over a verified generated runtime projection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use type_bridge_contract::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{
    ModelProjection, ProjectedModelForm, ProjectedModelUse, ReadRoleProjection, RoleTokenProjection,
};
use type_bridge_contract::schema::{AnnotationKindId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticMessage, SdkDiagnosticPathSegment,
    SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_contract::value::CanonicalValue;

use crate::_descriptor::{EntityDescriptor, RelationDescriptor, RoleDescriptor};
use crate::_dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput,
};
use crate::_manager::{DynamicEntityManager, DynamicRelationManager, query_builder};
use crate::execution_diagnostic::{lower_commit_error, lower_orm_error};
use crate::projected_model::{
    ProjectedAttributeValue, ProjectedCreate, ProjectedReference, ProjectedRolePlayer,
    ProjectedThing,
};
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::QueryResult;
use crate::session::database::DatabaseExecutionIdentity;
use crate::session::{Database, TransactionContext, TxType};
use crate::value::AttributeValue;
use crate::{ClassifiedCommitError, OrmError};

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
            .delete_entity_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
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
        let prepared = self.prepare_entity_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_entity_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Count exact instances of one projected entity in an owned read transaction.
    pub async fn count_entities(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
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
            .delete_relation_prepared(&transaction, &prepared, ExecutionMode::default())
            .await;
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
        let prepared = self.prepare_relation_identity(type_id, iid)?;
        require_transaction(transaction, TxType::Write, &prepared.type_id)?;
        self.delete_relation_prepared(transaction, &prepared, ExecutionMode::default())
            .await
    }

    /// Count exact instances of one projected relation in an owned read transaction.
    pub async fn count_relations(
        &self,
        database: &Database,
        type_id: &TypeId,
    ) -> Result<u64, SdkExecutionDiagnostic> {
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
        let prepared = self.prepare_relation_type(type_id)?;
        require_transaction(transaction, TxType::Read, &prepared.type_id)?;
        self.count_relations_prepared(transaction, &prepared, ExecutionMode::default())
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
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed exact entity count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_entities_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
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
            )
            .await;
        compatibility_result(result, &capture)
    }

    /// Execute a borrowed exact relation count with a private compatibility cause.
    #[doc(hidden)]
    pub async fn count_relations_in_transaction_with_compatibility(
        &self,
        transaction: &TransactionContext,
        type_id: &TypeId,
    ) -> Result<u64, ProjectedCrudCompatibilityFailure> {
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
        let iid = manager
            .put_exact(&prepared.attributes)
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
    ) -> Result<(), SdkExecutionDiagnostic> {
        let manager = DynamicEntityManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        manager
            .delete_by_iid_exact(&prepared.iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })
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
    ) -> Result<(), SdkExecutionDiagnostic> {
        let manager = DynamicRelationManager::with_canonical_transaction(
            transaction.clone(),
            Arc::clone(&prepared.descriptor),
        );
        manager
            .delete_by_iid_exact(&prepared.iid)
            .await
            .map_err(|error| {
                mode.orm_at_type(error, SdkProviderOperation::Write, &prepared.type_id)
            })
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
    role_players: Vec<PreparedRolePlayerInput>,
}

struct PreparedRolePlayerInput {
    role_id: RoleId,
    role_name: String,
    candidate_types: Vec<TypeId>,
    iid: Option<String>,
    key: Option<(String, AttributeValue)>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::temporal::TimeZoneDesignator;

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
}
