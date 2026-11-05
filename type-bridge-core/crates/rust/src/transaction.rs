#![deny(missing_docs)]
//! Client-owned write transactions and their borrowing exact managers.

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::{TypeId, is_canonical_thing_iid};
use type_bridge_orm::_descriptor::{EntityDescriptor, RelationDescriptor};
use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::session::context::TransactionContext;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, ProjectedBatchOperation, ProjectedCrudExecutor,
    QueryExecutionResourceLimits,
};

use crate::__codegen::{CompleteModel, EntityModel, HydrationCapability, RelationModel};
use crate::entity_codec::{hydrate_entity, map_validation_error, resolve_entity_authority};
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, ModelKind};
use crate::projected_batch::{
    create_operation, create_rows, delete_rows, execute_borrowed_delete, execute_borrowed_things,
    prepare_batch, update_rows, validate_binding_row_count,
};
use crate::projected_codec::{materialize_projected, project_create};
use crate::relation_codec::{hydrate_relation, resolve_relation_authority};
use crate::schema::Schema;
use crate::{Database, Result, WriteResult};

#[cfg(test)]
mod tests;

fn invalid_iid() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "invalid_iid",
        vec!["iid".into()],
        "IID is not canonical",
        None,
    )
}

fn schema_not_bound() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "schema_not_bound",
        vec![],
        "database is not schema-bound",
        None,
    )
}

/// One client-owned reusable read transaction over a schema-bound database.
///
/// Query sessions borrow this wrapper and execute every terminal on its one
/// retained read context. Closing consumes the wrapper without commit
/// semantics; dropping an open wrapper likewise cannot commit.
pub struct ReadTransaction<'db, S: Schema> {
    tx: TransactionContext,
    db: &'db Database<S>,
    installed: Arc<InstalledRuntimeProjection>,
    registry: Arc<DescriptorRegistry>,
}

impl<S: Schema> std::fmt::Debug for ReadTransaction<'_, S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadTransaction")
            .field("database", &self.db.database_name())
            .finish_non_exhaustive()
    }
}

impl<'db, S: Schema> ReadTransaction<'db, S> {
    pub(crate) async fn open(db: &'db Database<S>) -> Result<Self> {
        let installed = Arc::clone(db.installed_schema().ok_or_else(schema_not_bound)?);
        let registry = Arc::clone(db.match_registry().ok_or_else(schema_not_bound)?);
        let tx = db
            .inner_orm()
            .transaction_context(TxType::Read)
            .await
            .map_err(Error::from_orm)?;
        Ok(Self {
            tx,
            db,
            installed,
            registry,
        })
    }

    /// Create a read-only exact entity manager borrowing this transaction.
    /// Every derived filter terminal reuses the retained read context.
    pub fn entities<M>(&self) -> crate::projected_filter::ReadEntityManager<'_, S, M>
    where
        M: crate::__codegen::EntityModel<Schema = S> + crate::__codegen::CompleteModel,
    {
        crate::projected_filter::ReadEntityManager::new(&self.installed, &self.tx)
    }

    /// Create a read-only exact relation manager borrowing this transaction.
    /// Every derived filter terminal reuses the retained read context.
    pub fn relations<M>(&self) -> crate::projected_filter::ReadRelationManager<'_, S, M>
    where
        M: crate::__codegen::RelationModel<Schema = S> + crate::__codegen::CompleteModel,
    {
        crate::projected_filter::ReadRelationManager::new(&self.installed, &self.tx)
    }

    /// Start one owner-branded query session borrowing this read context.
    #[must_use]
    pub fn query(&self) -> crate::query::QuerySession<'_, S> {
        self.query_with_resources(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    /// Start one borrowed query session with one common tighten-only resource
    /// policy and caller-owned cooperative cancellation signal.
    #[must_use]
    pub fn query_with_resources(
        &self,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> crate::query::QuerySession<'_, S> {
        crate::query::QuerySession::borrowed(
            Arc::clone(&self.installed),
            Arc::clone(&self.registry),
            &self.tx,
            self.db.operation_limits(resources),
            cancellation,
        )
    }

    /// Close this read transaction without committing.
    pub async fn close(self) -> Result<()> {
        self.tx.close().await.map_err(Error::from_orm)
    }
}

/// One client-owned open write transaction over a schema-bound database.
///
/// The wrapper is not cloneable and owns the sole retained engine context.
/// Manager handles borrow the wrapper, so [`Self::commit`] and
/// [`Self::rollback`] — which consume it — cannot run while a manager is
/// retained, and a second terminal operation is unrepresentable. Operations
/// never auto-commit: an operation error leaves terminal control with the
/// caller, and dropping an open wrapper releases the context without commit.
/// Single writes retain the mutation lease through generated model materialization.
/// A failure before mutation dispatch leaves the transaction active, while a
/// failure after dispatch makes it
/// rollback-only and [`Self::commit`] returns `transaction_rollback_only`.
pub struct WriteTransaction<'db, S: Schema> {
    tx: TransactionContext,
    db: &'db Database<S>,
}

impl<S: Schema> std::fmt::Debug for WriteTransaction<'_, S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteTransaction")
            .field("database", &self.db.database_name())
            .finish_non_exhaustive()
    }
}

impl<'db, S: Schema> WriteTransaction<'db, S> {
    pub(crate) async fn open(db: &'db Database<S>) -> Result<WriteTransaction<'db, S>> {
        db.installed_schema().ok_or_else(schema_not_bound)?;
        let tx = db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        Ok(WriteTransaction { tx, db })
    }

    /// Create an exact entity manager borrowing this open transaction.
    pub fn entities<M>(&self) -> TransactionEntityManager<'_, S, M>
    where
        M: EntityModel<Schema = S>,
    {
        TransactionEntityManager {
            transaction: self,
            marker: PhantomData,
        }
    }

    /// Create an exact relation manager borrowing this open transaction.
    pub fn relations<M>(&self) -> TransactionRelationManager<'_, S, M>
    where
        M: RelationModel<Schema = S>,
    {
        TransactionRelationManager {
            transaction: self,
            marker: PhantomData,
        }
    }

    /// Commit every operation performed in this transaction, consuming it.
    pub async fn commit(self) -> Result<()> {
        self.tx
            .commit_sdk()
            .await
            .map_err(|error| Error::from_projected_batch(error, ModelValidationPhase::Input))
    }

    /// Roll back every operation performed in this transaction, consuming it.
    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await.map_err(Error::from_orm)
    }

    /// Roll back writes while retaining this transaction for more work.
    pub async fn rollback_keep(&mut self) -> Result<()> {
        self.tx.rollback().await.map_err(Error::from_orm)
    }

    fn installed(&self) -> Result<&InstalledRuntimeProjection> {
        self.db
            .installed_schema()
            .map(Arc::as_ref)
            .ok_or_else(schema_not_bound)
    }
}

/// Schema-bound, model-branded exact entity manager borrowing one open
/// client write transaction. Operations reuse the shared open context and
/// never commit, roll back, or close it; errors are returned with the
/// transaction left open for the caller to decide.
pub struct TransactionEntityManager<'t, S: Schema, M: EntityModel<Schema = S>> {
    transaction: &'t WriteTransaction<'t, S>,
    marker: PhantomData<M>,
}

impl<'t, S: Schema, M: EntityModel<Schema = S>> Copy for TransactionEntityManager<'t, S, M> {}
impl<'t, S: Schema, M: EntityModel<Schema = S>> Clone for TransactionEntityManager<'t, S, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'t, S, M> TransactionEntityManager<'t, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
    /// Start an immutable exact-model filter in this open write transaction.
    pub fn filter(&self) -> Result<crate::ProjectedEntityFilter<'t, S, M>> {
        crate::ProjectedEntityFilter::for_target(
            crate::manager_target::ManagerTarget {
                installed: self.transaction.installed()?,
                target: type_bridge_orm::QueryTarget::Transaction(&self.transaction.tx),
            },
            crate::hooks::HookRunner::default(),
        )
    }

    fn exact(
        &self,
    ) -> Result<(
        TypeId,
        &InstalledRuntimeProjection,
        DynamicEntityManager<'static>,
    )> {
        let installed = self.transaction.installed()?;
        let (id, descriptor): (TypeId, EntityDescriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let manager = DynamicEntityManager::with_registry_transaction(
            self.transaction.tx.clone(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        );
        let manager = if self.transaction.db.inner_orm().supports_given_stage() {
            manager
        } else {
            manager.literal_values()
        };
        Ok((id, installed, manager))
    }

    /// Inserts one exact entity in the open transaction and returns its
    /// complete freshly hydrated model without committing.
    pub async fn insert(&self, input: M::Create) -> Result<WriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Insert,
            None,
            ModelKind::Entity,
        )
        .await
    }

    /// Applies the exact key-or-insert put rule in the open transaction,
    /// including complete replacement of non-key ownership for an existing
    /// exact row, without committing.
    pub async fn put(&self, input: M::Create) -> Result<WriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Put,
            None,
            ModelKind::Entity,
        )
        .await
    }

    /// Inserts each item in input order in the open transaction, returning
    /// complete freshly hydrated models or one error, without committing.
    /// A batch failure after mutation dispatch makes the
    /// transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        self.write_many(inputs, false).await
    }

    /// Applies the per-item put rule in input order in the open transaction,
    /// returning complete freshly hydrated models or one error, without
    /// committing. A batch failure after mutation dispatch makes
    /// the transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        self.write_many(inputs, true).await
    }

    async fn write_many(
        &self,
        inputs: Vec<M::Create>,
        put: bool,
    ) -> Result<crate::BatchWriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = create_rows(installed, &id, inputs)?;
        let operation = create_operation(installed, &id, put);
        let batch = prepare_batch(installed, id, operation, rows)?;
        execute_borrowed_things(&self.transaction.tx, installed, &batch).await
    }

    /// Completely replaces non-key ownership on the exact model at canonical
    /// `iid` in the open transaction, preserving that IID, without committing.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<WriteResult<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Update,
            Some(iid),
            ModelKind::Entity,
        )
        .await
    }

    /// Delete the exact model and return its pre-delete snapshot without committing.
    /// An absent IID returns an empty write result.
    pub async fn delete_returning(&self, iid: &str) -> Result<WriteResult<M>> {
        self.delete_many_returning(&[iid.to_owned()])
            .await
            .map(|result| WriteResult::new(result.into_items()))
    }

    /// Delete exact models and return existing snapshots in input order without committing.
    pub async fn delete_many_returning(
        &self,
        iids: &[String],
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(iids.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let batch = prepare_batch(
            installed,
            id,
            ProjectedBatchOperation::Delete,
            delete_rows(iids)?,
        )?;
        crate::projected_batch::execute_borrowed_deleted_things(
            &self.transaction.tx,
            installed,
            &batch,
        )
        .await
    }

    /// Deletes only the exact model at canonical `iid` in the open
    /// transaction, without committing.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .delete_entity_by_iid_in_transaction_with_compatibility(&self.transaction.tx, &id, iid)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Delete))
            })
    }

    /// Replaces each exact entity identified by its canonical IID in input
    /// order without committing. Batch failures after mutation
    /// dispatch make the transaction rollback-only.
    pub async fn update_many(
        &self,
        inputs: Vec<(String, M::Create)>,
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = update_rows(installed, &id, inputs)?;
        let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;
        execute_borrowed_things(&self.transaction.tx, installed, &batch).await
    }

    /// Deletes every exact entity at the supplied canonical IIDs without
    /// committing. Batch failures after mutation dispatch make
    /// the transaction rollback-only.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        validate_binding_row_count(iids.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let batch = prepare_batch(
            installed,
            id,
            ProjectedBatchOperation::Delete,
            delete_rows(iids)?,
        )?;
        execute_borrowed_delete(&self.transaction.tx, installed, &batch).await
    }

    /// Reads one exact model by canonical IID through the open transaction,
    /// observing this transaction's uncommitted writes.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .get_entity_by_iid_in_transaction_with_compatibility(&self.transaction.tx, &id, iid)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Entity, None))?
            .map(|projected| materialize_projected(projected, installed))
            .transpose()
    }

    /// Reads all exact models through the open transaction, observing this
    /// transaction's uncommitted writes.
    pub async fn all(&self) -> Result<Vec<M>> {
        let (id, installed, manager) = self.exact()?;
        let rows = manager.all_exact().await.map_err(Error::from_orm)?;
        rows.into_iter()
            .map(|row| {
                let hydrated = hydrate_entity(row, &id, installed)?;
                M::materialize(&hydrated, &HydrationCapability::new())
                    .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
            })
            .collect()
    }

    /// Counts only exact models through the open transaction.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .count_entities_in_transaction_with_compatibility(&self.transaction.tx, &id)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Entity, None))
    }
}

/// Schema-bound, model-branded exact relation manager borrowing one open
/// client write transaction. Operations reuse the shared open context and
/// never commit, roll back, or close it; errors are returned with the
/// transaction left open for the caller to decide.
pub struct TransactionRelationManager<'t, S: Schema, M: RelationModel<Schema = S>> {
    transaction: &'t WriteTransaction<'t, S>,
    marker: PhantomData<M>,
}

impl<'t, S: Schema, M: RelationModel<Schema = S>> Copy for TransactionRelationManager<'t, S, M> {}
impl<'t, S: Schema, M: RelationModel<Schema = S>> Clone for TransactionRelationManager<'t, S, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'t, S, M> TransactionRelationManager<'t, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
    /// Start an immutable exact-model filter in this open write transaction.
    pub fn filter(&self) -> Result<crate::ProjectedRelationFilter<'t, S, M>> {
        crate::ProjectedRelationFilter::for_target(
            crate::manager_target::ManagerTarget {
                installed: self.transaction.installed()?,
                target: type_bridge_orm::QueryTarget::Transaction(&self.transaction.tx),
            },
            crate::hooks::HookRunner::default(),
        )
    }

    fn exact(
        &self,
    ) -> Result<(
        TypeId,
        &InstalledRuntimeProjection,
        DynamicRelationManager<'static>,
    )> {
        let installed = self.transaction.installed()?;
        let (id, descriptor): (TypeId, RelationDescriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let manager = DynamicRelationManager::with_registry_transaction(
            self.transaction.tx.clone(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        );
        let manager = if self.transaction.db.inner_orm().supports_given_stage() {
            manager
        } else {
            manager.literal_values()
        };
        Ok((id, installed, manager))
    }

    /// Inserts one exact relation with its complete active role players in the
    /// open transaction and returns its complete freshly hydrated model
    /// without committing.
    pub async fn insert(&self, input: M::Create) -> Result<WriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Insert,
            None,
            ModelKind::Relation,
        )
        .await
    }

    /// Applies the exact key-or-insert put rule in the open transaction,
    /// including complete replacement of non-key ownership and active role
    /// players for an existing exact row, without committing.
    pub async fn put(&self, input: M::Create) -> Result<WriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Put,
            None,
            ModelKind::Relation,
        )
        .await
    }

    /// Inserts each item in input order in the open transaction, returning
    /// complete freshly hydrated models or one error, without committing.
    /// A batch failure after mutation dispatch makes the
    /// transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        self.write_many(inputs, false).await
    }

    /// Applies the per-item put rule in input order in the open transaction,
    /// returning complete freshly hydrated models or one error, without
    /// committing. A batch failure after mutation dispatch makes
    /// the transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        self.write_many(inputs, true).await
    }

    async fn write_many(
        &self,
        inputs: Vec<M::Create>,
        put: bool,
    ) -> Result<crate::BatchWriteResult<M>> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = create_rows(installed, &id, inputs)?;
        let operation = create_operation(installed, &id, put);
        let batch = prepare_batch(installed, id, operation, rows)?;
        execute_borrowed_things(&self.transaction.tx, installed, &batch).await
    }

    /// Completely replaces non-key ownership and the complete effective
    /// active-role player set on the exact relation at canonical `iid` in the
    /// open transaction, preserving that IID, without committing.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<WriteResult<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        crate::projected_batch::execute_transaction_single(
            &self.transaction.tx,
            installed,
            create,
            CrudOperation::Update,
            Some(iid),
            ModelKind::Relation,
        )
        .await
    }

    /// Delete the exact model and return its pre-delete snapshot without committing.
    /// An absent IID returns an empty write result.
    pub async fn delete_returning(&self, iid: &str) -> Result<WriteResult<M>> {
        self.delete_many_returning(&[iid.to_owned()])
            .await
            .map(|result| WriteResult::new(result.into_items()))
    }

    /// Delete exact models and return existing snapshots in input order without committing.
    pub async fn delete_many_returning(
        &self,
        iids: &[String],
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(iids.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let batch = prepare_batch(
            installed,
            id,
            ProjectedBatchOperation::Delete,
            delete_rows(iids)?,
        )?;
        crate::projected_batch::execute_borrowed_deleted_things(
            &self.transaction.tx,
            installed,
            &batch,
        )
        .await
    }

    /// Deletes only the exact relation at canonical `iid` in the open
    /// transaction, without committing.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .delete_relation_by_iid_in_transaction_with_compatibility(
                &self.transaction.tx,
                &id,
                iid,
            )
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Relation, Some(CrudOperation::Delete))
            })
    }

    /// Replaces each exact relation identified by its canonical IID in input
    /// order without committing. Batch failures after mutation
    /// dispatch make the transaction rollback-only.
    pub async fn update_many(
        &self,
        inputs: Vec<(String, M::Create)>,
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = update_rows(installed, &id, inputs)?;
        let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;
        execute_borrowed_things(&self.transaction.tx, installed, &batch).await
    }

    /// Deletes every exact relation at the supplied canonical IIDs without
    /// committing. Batch failures after mutation dispatch make
    /// the transaction rollback-only.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        validate_binding_row_count(iids.len())?;
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let batch = prepare_batch(
            installed,
            id,
            ProjectedBatchOperation::Delete,
            delete_rows(iids)?,
        )?;
        execute_borrowed_delete(&self.transaction.tx, installed, &batch).await
    }

    /// Reads one exact coalesced relation by canonical IID through the open
    /// transaction, observing this transaction's uncommitted writes.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .get_relation_by_iid_in_transaction_with_compatibility(&self.transaction.tx, &id, iid)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Relation, None))?
            .map(|projected| materialize_projected(projected, installed))
            .transpose()
    }

    /// Reads all exact coalesced relations through the open transaction,
    /// observing this transaction's uncommitted writes.
    pub async fn all(&self) -> Result<Vec<M>> {
        let (id, installed, manager) = self.exact()?;
        let rows = manager.all_exact().await.map_err(Error::from_orm)?;
        rows.into_iter()
            .map(|row| {
                let hydrated = hydrate_relation(row, &id, installed)?;
                M::materialize(&hydrated, &HydrationCapability::new())
                    .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
            })
            .collect()
    }

    /// Counts only exact relations through the open transaction.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.transaction.installed()?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .count_relations_in_transaction_with_compatibility(&self.transaction.tx, &id)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Relation, None))
    }
}
