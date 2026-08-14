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
    AnswerCancellation, DynamicAttributeMap, DynamicRolePlayerInput, InstalledRuntimeProjection,
    ProjectedBatchOperation, ProjectedCrudExecutor, QueryExecutionResourceLimits,
};

use crate::__codegen::{CompleteModel, EntityModel, HydrationCapability, RelationModel};
use crate::entity_codec::{
    hydrate_entity, lower_entity_create, map_validation_error, resolve_entity_authority,
};
use crate::entity_manager::rehydrate_written_entity;
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, ModelKind};
use crate::projected_batch::{
    create_rows, delete_rows, execute_borrowed_delete, execute_borrowed_things, prepare_batch,
    update_rows, uses_successor_batch_runtime, validate_binding_row_count,
};
use crate::projected_codec::{materialize_projected, project_create};
use crate::relation_codec::{hydrate_relation, lower_relation_create, resolve_relation_authority};
use crate::relation_manager::rehydrate_written_relation;
use crate::schema::Schema;
use crate::{Database, Result};

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
            &self.installed,
            Arc::clone(&self.registry),
            &self.tx,
            resources,
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
/// Under the successor runtime, a batch failure before mutation dispatch
/// leaves the transaction active, while a failure after dispatch makes it
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
        if uses_successor_batch_runtime(self.installed()?) {
            self.tx
                .commit_sdk()
                .await
                .map_err(|error| Error::from_projected_batch(error, ModelValidationPhase::Input))
        } else {
            self.tx.commit().await.map_err(Error::from_orm)
        }
    }

    /// Roll back every operation performed in this transaction, consuming it.
    pub async fn rollback(self) -> Result<()> {
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

impl<S, M> TransactionEntityManager<'_, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
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
        let manager = DynamicEntityManager::with_canonical_transaction(
            self.transaction.tx.clone(),
            Arc::new(descriptor),
        );
        Ok((id, installed, manager))
    }

    /// Inserts one exact entity in the open transaction and returns its
    /// complete freshly hydrated model without committing.
    pub async fn insert(&self, input: M::Create) -> Result<M> {
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .insert_entity_in_transaction_with_compatibility(&self.transaction.tx, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Insert))
            })?;
        materialize_projected(projected, installed)
    }

    /// Applies the exact key-or-insert put rule in the open transaction,
    /// including complete replacement of non-key ownership for an existing
    /// exact row, without committing.
    pub async fn put(&self, input: M::Create) -> Result<M> {
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .put_entity_in_transaction_with_compatibility(&self.transaction.tx, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Put))
            })?;
        materialize_projected(projected, installed)
    }

    /// Inserts each item in input order in the open transaction, returning
    /// complete freshly hydrated models or one error, without committing.
    /// A successor-runtime failure after mutation dispatch makes the
    /// transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if uses_successor_batch_runtime(self.transaction.installed()?) {
            validate_binding_row_count(inputs.len())?;
        }
        self.write_many(inputs, false).await
    }

    /// Applies the per-item put rule in input order in the open transaction,
    /// returning complete freshly hydrated models or one error, without
    /// committing. A successor-runtime failure after mutation dispatch makes
    /// the transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if uses_successor_batch_runtime(self.transaction.installed()?) {
            validate_binding_row_count(inputs.len())?;
        }
        self.write_many(inputs, true).await
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if inputs.is_empty() && !successor {
            return Ok(Vec::new());
        }
        let (id, installed, manager) = self.exact()?;
        if successor {
            let rows = create_rows(installed, &id, inputs)?;
            let operation = if put {
                ProjectedBatchOperation::Put
            } else {
                ProjectedBatchOperation::Insert
            };
            let batch = prepare_batch(installed, id, operation, rows)?;
            return execute_borrowed_things(&self.transaction.tx, installed, &batch).await;
        }
        let mut lowered = Vec::with_capacity(inputs.len());
        for input in inputs {
            lowered.push(lower_entity_create(input, &id, installed)?);
        }
        let iids = if put {
            manager.put_many_exact(&lowered).await
        } else {
            manager.insert_many(&lowered).await
        }
        .map_err(Error::from_orm)?;
        if iids.len() != lowered.len() {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "iid_count_mismatch",
                vec!["iid".into()],
                "provider returned an unexpected IID count",
                None,
            ));
        }
        let mut out = Vec::with_capacity(iids.len());
        for iid in iids {
            out.push(rehydrate_written_entity(&manager, &iid, &id, installed).await?);
        }
        Ok(out)
    }

    /// Completely replaces non-key ownership on the exact model at canonical
    /// `iid` in the open transaction, preserving that IID, without committing.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<M> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .update_entity_in_transaction_with_compatibility(&self.transaction.tx, iid, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Update))
            })?;
        materialize_projected(projected, installed)
    }

    /// Deletes only the exact model at canonical `iid` in the open
    /// transaction, without committing.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
        ProjectedCrudExecutor::new(installed)
            .delete_entity_by_iid_in_transaction_with_compatibility(&self.transaction.tx, &id, iid)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Delete))
            })
    }

    /// Replaces each exact entity identified by its canonical IID in input
    /// order without committing. Successor-runtime failures after mutation
    /// dispatch make the transaction rollback-only.
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if successor {
            validate_binding_row_count(inputs.len())?;
        }
        if inputs.is_empty() && !successor {
            return Ok(Vec::new());
        }
        if successor {
            let (id, installed, _manager) = self.exact()?;
            let rows = update_rows(installed, &id, inputs)?;
            let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;
            return execute_borrowed_things(&self.transaction.tx, installed, &batch).await;
        }
        if inputs.iter().any(|(iid, _)| !is_canonical_thing_iid(iid)) {
            return Err(invalid_iid());
        }
        let (id, installed, manager) = self.exact()?;

        let mut prepared = Vec::with_capacity(inputs.len());
        for (iid, input) in inputs {
            prepared.push((iid, lower_entity_create(input, &id, installed)?));
        }
        let mut output = Vec::with_capacity(prepared.len());
        for (iid, attributes) in prepared {
            manager
                .update_exact(&iid, &attributes)
                .await
                .map_err(Error::from_orm)?;
            output.push(rehydrate_written_entity(&manager, &iid, &id, installed).await?);
        }
        Ok(output)
    }

    /// Deletes every exact entity at the supplied canonical IIDs without
    /// committing. Successor-runtime failures after mutation dispatch make
    /// the transaction rollback-only.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if successor {
            validate_binding_row_count(iids.len())?;
        }
        if iids.is_empty() && !successor {
            return Ok(());
        }
        if successor {
            let (id, installed, _manager) = self.exact()?;
            let batch = prepare_batch(
                installed,
                id,
                ProjectedBatchOperation::Delete,
                delete_rows(iids)?,
            )?;
            return execute_borrowed_delete(&self.transaction.tx, installed, &batch).await;
        }
        if iids.iter().any(|iid| !is_canonical_thing_iid(iid)) {
            return Err(invalid_iid());
        }
        let (_id, _installed, manager) = self.exact()?;
        for iid in iids {
            manager
                .delete_by_iid_exact(iid)
                .await
                .map_err(Error::from_orm)?;
        }
        Ok(())
    }

    /// Reads one exact model by canonical IID through the open transaction,
    /// observing this transaction's uncommitted writes.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
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
        let (id, installed, _manager) = self.exact()?;
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

impl<S, M> TransactionRelationManager<'_, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
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
        let manager = DynamicRelationManager::with_canonical_transaction(
            self.transaction.tx.clone(),
            Arc::new(descriptor),
        );
        Ok((id, installed, manager))
    }

    /// Inserts one exact relation with its complete active role players in the
    /// open transaction and returns its complete freshly hydrated model
    /// without committing.
    pub async fn insert(&self, input: M::Create) -> Result<M> {
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .insert_relation_in_transaction_with_compatibility(&self.transaction.tx, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Relation, Some(CrudOperation::Insert))
            })?;
        materialize_projected(projected, installed)
    }

    /// Applies the exact key-or-insert put rule in the open transaction,
    /// including complete replacement of non-key ownership and active role
    /// players for an existing exact row, without committing.
    pub async fn put(&self, input: M::Create) -> Result<M> {
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .put_relation_in_transaction_with_compatibility(&self.transaction.tx, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Relation, Some(CrudOperation::Put))
            })?;
        materialize_projected(projected, installed)
    }

    /// Inserts each item in input order in the open transaction, returning
    /// complete freshly hydrated models or one error, without committing.
    /// A successor-runtime failure after mutation dispatch makes the
    /// transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if uses_successor_batch_runtime(self.transaction.installed()?) {
            validate_binding_row_count(inputs.len())?;
        }
        self.write_many(inputs, false).await
    }

    /// Applies the per-item put rule in input order in the open transaction,
    /// returning complete freshly hydrated models or one error, without
    /// committing. A successor-runtime failure after mutation dispatch makes
    /// the transaction rollback-only; a pre-dispatch failure leaves it active.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if uses_successor_batch_runtime(self.transaction.installed()?) {
            validate_binding_row_count(inputs.len())?;
        }
        self.write_many(inputs, true).await
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if inputs.is_empty() && !successor {
            return Ok(Vec::new());
        }
        let (id, installed, manager) = self.exact()?;
        if successor {
            let rows = create_rows(installed, &id, inputs)?;
            let operation = if put {
                ProjectedBatchOperation::Put
            } else {
                ProjectedBatchOperation::Insert
            };
            let batch = prepare_batch(installed, id, operation, rows)?;
            return execute_borrowed_things(&self.transaction.tx, installed, &batch).await;
        }
        let mut lowered: Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)> =
            Vec::with_capacity(inputs.len());
        for input in inputs {
            let prepared = lower_relation_create(input, &id, installed)?;
            lowered.push((prepared.attributes, prepared.role_players));
        }
        let iids = if put {
            manager.put_many_exact(&lowered).await
        } else {
            manager.insert_many(&lowered).await
        }
        .map_err(Error::from_orm)?;
        if iids.len() != lowered.len() {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "iid_count_mismatch",
                vec!["iid".into()],
                "provider returned an unexpected IID count",
                None,
            ));
        }
        let mut out = Vec::with_capacity(iids.len());
        for iid in iids {
            out.push(rehydrate_written_relation(&manager, &iid, &id, installed).await?);
        }
        Ok(out)
    }

    /// Completely replaces non-key ownership and the complete effective
    /// active-role player set on the exact relation at canonical `iid` in the
    /// open transaction, preserving that IID, without committing.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<M> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
        let create = project_create(input, &id, installed)?;
        let projected = ProjectedCrudExecutor::new(installed)
            .update_relation_in_transaction_with_compatibility(&self.transaction.tx, iid, &create)
            .await
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Relation, Some(CrudOperation::Update))
            })?;
        materialize_projected(projected, installed)
    }

    /// Deletes only the exact relation at canonical `iid` in the open
    /// transaction, without committing.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
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
    /// order without committing. Successor-runtime failures after mutation
    /// dispatch make the transaction rollback-only.
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if successor {
            validate_binding_row_count(inputs.len())?;
        }
        if inputs.is_empty() && !successor {
            return Ok(Vec::new());
        }
        if successor {
            let (id, installed, _manager) = self.exact()?;
            let rows = update_rows(installed, &id, inputs)?;
            let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;
            return execute_borrowed_things(&self.transaction.tx, installed, &batch).await;
        }
        if inputs.iter().any(|(iid, _)| !is_canonical_thing_iid(iid)) {
            return Err(invalid_iid());
        }
        let (id, installed, manager) = self.exact()?;

        let mut prepared = Vec::with_capacity(inputs.len());
        for (iid, input) in inputs {
            prepared.push((iid, lower_relation_create(input, &id, installed)?));
        }
        let mut output = Vec::with_capacity(prepared.len());
        for (iid, replacement) in prepared {
            manager
                .update_exact(&iid, &replacement.attributes, &replacement.role_players)
                .await
                .map_err(Error::from_orm)?;
            output.push(rehydrate_written_relation(&manager, &iid, &id, installed).await?);
        }
        Ok(output)
    }

    /// Deletes every exact relation at the supplied canonical IIDs without
    /// committing. Successor-runtime failures after mutation dispatch make
    /// the transaction rollback-only.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        let successor = uses_successor_batch_runtime(self.transaction.installed()?);
        if successor {
            validate_binding_row_count(iids.len())?;
        }
        if iids.is_empty() && !successor {
            return Ok(());
        }
        if successor {
            let (id, installed, _manager) = self.exact()?;
            let batch = prepare_batch(
                installed,
                id,
                ProjectedBatchOperation::Delete,
                delete_rows(iids)?,
            )?;
            return execute_borrowed_delete(&self.transaction.tx, installed, &batch).await;
        }
        if iids.iter().any(|iid| !is_canonical_thing_iid(iid)) {
            return Err(invalid_iid());
        }
        let (_id, _installed, manager) = self.exact()?;
        for iid in iids {
            manager
                .delete_by_iid_exact(iid)
                .await
                .map_err(Error::from_orm)?;
        }
        Ok(())
    }

    /// Reads one exact coalesced relation by canonical IID through the open
    /// transaction, observing this transaction's uncommitted writes.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let (id, installed, _manager) = self.exact()?;
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
        let (id, installed, _manager) = self.exact()?;
        ProjectedCrudExecutor::new(installed)
            .count_relations_in_transaction_with_compatibility(&self.transaction.tx, &id)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Relation, None))
    }
}
