#![deny(missing_docs)]

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_orm::_manager::DynamicEntityManager;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::{
    ProjectedBatchOperation, ProjectedBatchRow, ProjectedCrudExecutor, ProjectedManagerComparison,
};

use crate::__codegen::{
    CompleteModel, EncodedCreate, EntityModel, FieldToken, HydrationCapability, IntoEncodedCreate,
    Model, QueryValued, SubtypeRootModel,
};
use crate::entity_codec::{
    hydrate_entity, map_validation_error, resolve_discovered_entity, resolve_entity_authority,
};
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, ModelKind};
use crate::projected_batch::{
    checked_batch_ordinal, create_operation, create_rows, delete_rows, encode_batch_create,
    execute_owned_delete, execute_owned_things, prepare_batch, project_encoded_batch_create,
    reserved_binding_vec, validate_binding_row_count,
};
use crate::projected_codec::{materialize_projected, project_create};
use crate::projected_filter::ProjectedEntityFilter;
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

impl<S, M> EntitySubtypeManager<'_, S, M>
where
    S: Schema,
    M: SubtypeRootModel<Schema = S> + EntityModel<Schema = S>,
{
    /// Reads one canonical IID across the root and its generated concrete descendants.
    /// Invalid IIDs are rejected before I/O; a valid but absent IID returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M::Subtypes>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Read)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        );
        let manager = if self.db.inner_orm().supports_given_stage() {
            manager
        } else {
            manager.literal_values()
        };
        let result = async {
            let Some(row) = manager.get_by_iid(iid).await.map_err(Error::from_orm)? else {
                return Ok(None);
            };
            let (child_id, _) =
                resolve_discovered_entity(row.type_name.as_deref().unwrap_or_default(), installed)?;
            let hydrated = hydrate_entity(row, &child_id, installed)?;
            M::__tb_dispatch_subtype(&hydrated, &HydrationCapability::new())
                .map(Some)
                .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
        }
        .await;
        match result {
            Ok(value) => {
                tx.close().await.map_err(Error::from_orm)?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.close().await;
                Err(error)
            }
        }
    }
    /// Reads all root/descendant models in application result order, materialized as the
    /// generated leaf or family result. Validation, database, hydration, and close errors
    /// are returned without exposing implementation details.
    pub async fn all(&self) -> Result<Vec<M::Subtypes>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Read)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        );
        let manager = if self.db.inner_orm().supports_given_stage() {
            manager
        } else {
            manager.literal_values()
        };
        let result = async {
            let rows = manager.all().await.map_err(Error::from_orm)?;
            rows.into_iter()
                .map(|row| {
                    let (child_id, _) = resolve_discovered_entity(
                        row.type_name.as_deref().unwrap_or_default(),
                        installed,
                    )?;
                    let hydrated = hydrate_entity(row, &child_id, installed)?;
                    M::__tb_dispatch_subtype(&hydrated, &HydrationCapability::new()).map_err(
                        |error| map_validation_error(error, ModelValidationPhase::Hydration),
                    )
                })
                .collect()
        }
        .await;
        match result {
            Ok(value) => {
                tx.close().await.map_err(Error::from_orm)?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.close().await;
                Err(error)
            }
        }
    }
    /// Counts the root and all concrete descendants using the inclusive subtype scope.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        DynamicEntityManager::from_registry(
            self.db.inner_orm(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        )
        .count()
        .await
        .map_err(Error::from_orm)
    }
}

/// Schema-bound, model-branded manager for exact entity operations.
/// Exact reads and writes exclude subtypes; methods return client input/schema-validation,
/// database/transaction/close, or hydration/model-validation errors as applicable.
pub struct EntityManager<'db, S: Schema, M: EntityModel<Schema = S>> {
    db: &'db Database<S>,
    hooks: HookRunner,
    marker: PhantomData<M>,
}

/// Read-only, schema/model-branded manager for an inclusive generated subtype association.
/// Results are the generated associated leaf or closed family type; no writes are exposed.
/// Reads and counts can return input/schema-validation, database/transaction/close, or
/// hydration/model-validation errors.
pub struct EntitySubtypeManager<
    'db,
    S: Schema,
    M: SubtypeRootModel<Schema = S> + EntityModel<Schema = S>,
> {
    db: &'db Database<S>,
    marker: PhantomData<M>,
}

impl<'db, S: Schema, M: SubtypeRootModel<Schema = S> + EntityModel<Schema = S>>
    EntitySubtypeManager<'db, S, M>
{
    pub(crate) fn new(db: &'db Database<S>) -> Self {
        Self {
            db,
            marker: PhantomData,
        }
    }
}

impl<'db, S, M> EntityManager<'db, S, M>
where
    S: Schema,
    M: SubtypeRootModel<Schema = S> + EntityModel<Schema = S>,
{
    /// Switches only the read scope and result shape to the generated inclusive subtype
    /// association; it does not add write operations.
    pub fn subtypes(&self) -> EntitySubtypeManager<'db, S, M> {
        EntitySubtypeManager::new(self.db)
    }
}

impl<'db, S: Schema, M: EntityModel<Schema = S>> Clone for EntityManager<'db, S, M> {
    fn clone(&self) -> Self {
        Self {
            db: self.db,
            hooks: self.hooks.clone(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, M: EntityModel<Schema = S>> EntityManager<'_, S, M> {
    pub(crate) fn new(db: &Database<S>) -> EntityManager<'_, S, M> {
        EntityManager {
            db,
            hooks: HookRunner::default(),
            marker: PhantomData,
        }
    }
}

impl<'db, S, M> EntityManager<'db, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
    /// Start one empty immutable canonical filter for this exact entity model.
    pub fn filter(&self) -> Result<ProjectedEntityFilter<'db, S, M>> {
        ProjectedEntityFilter::for_database(self.db)
    }

    /// Start a canonical filter with one exact generated field comparison.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<ProjectedEntityFilter<'db, S, M>>
    where
        V: Model<Schema = S> + QueryValued,
    {
        self.filter()?.where_(field, operator, value)
    }

    /// Register one generated-model lifecycle hook on this manager.
    /// Pre-hooks run in registration order and post-hooks in reverse order.
    pub fn add_hook(&mut self, hook: Arc<dyn LifecycleHook>) -> &mut Self {
        self.hooks.add(hook);
        self
    }

    fn encode_hook_input(input: &M::Create) -> Result<EncodedCreate> {
        input
            .clone()
            .into_encoded_create()
            .map_err(|error| map_validation_error(error, ModelValidationPhase::Input))
    }

    /// Inserts one exact entity and returns its complete freshly hydrated model. Errors may
    /// be input/schema validation, database/transaction/close, or model hydration errors.
    pub async fn insert(&self, input: M::Create) -> Result<WriteResult<M>> {
        if !self.hooks.has_hooks() {
            return self
                .projected_write(input, CrudOperation::Insert, None)
                .await;
        }
        let encoded = Self::encode_hook_input(&input)?;
        let metadata = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Insert,
                None,
                Some(&encoded),
            )
            .await?;
        let output = self
            .projected_write(input, CrudOperation::Insert, None)
            .await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Insert,
                output.item().map(|item| item.iid()),
                Some(&encoded),
                metadata,
            )
            .await;
        Ok(output)
    }
    /// Uses a projected exact-model key when one is available; otherwise inserts. For an
    /// existing exact row, the supplied create value completely replaces non-key ownership:
    /// omitted optional values and empty multivalue collections remove prior ownership. It
    /// never reuses a subtype instance, and returns a complete freshly hydrated model.
    pub async fn put(&self, input: M::Create) -> Result<WriteResult<M>> {
        if !self.hooks.has_hooks() {
            return self.projected_write(input, CrudOperation::Put, None).await;
        }
        let encoded = Self::encode_hook_input(&input)?;
        let metadata = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Put,
                None,
                Some(&encoded),
            )
            .await?;
        let output = self
            .projected_write(input, CrudOperation::Put, None)
            .await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Put,
                output.item().map(|item| item.iid()),
                Some(&encoded),
                metadata,
            )
            .await;
        Ok(output)
    }
    /// Inserts each item and returns complete freshly hydrated models in input order, or one
    /// error for the whole call with no partial result vector.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        validate_binding_row_count(inputs.len())?;
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, false).await;
        }
        self.write_many_with_hooks(inputs, CrudOperation::Insert)
            .await
    }
    /// Applies the per-item [`Self::put`] key-or-insert rule, including complete replacement of
    /// non-key ownership for existing exact rows, returning complete freshly hydrated models
    /// in input order, or one error for the whole call with no partial vector.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        validate_binding_row_count(inputs.len())?;
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, true).await;
        }
        self.write_many_with_hooks(inputs, CrudOperation::Put).await
    }
    /// Completely replaces non-key ownership on the exact model at canonical `iid`, preserves
    /// that IID, and returns its complete freshly hydrated model. Omitted optional values and
    /// empty multivalue collections remove prior ownership.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<WriteResult<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        if !self.hooks.has_hooks() {
            return self
                .projected_write(input, CrudOperation::Update, Some(iid))
                .await;
        }
        let encoded = Self::encode_hook_input(&input)?;
        let state = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Update,
                Some(iid),
                Some(&encoded),
            )
            .await?;
        let output = self
            .projected_write(input, CrudOperation::Update, Some(iid))
            .await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Update,
                output.item().map(|item| item.iid()),
                Some(&encoded),
                state,
            )
            .await;
        Ok(output)
    }

    /// Delete an exact model and return its snapshots in a write result.
    pub async fn delete_returning(&self, iid: &str) -> Result<WriteResult<M>> {
        self.delete_many_returning(&[iid.to_owned()])
            .await
            .map(WriteResult::new)
    }

    /// Atomically delete exact models and return existing snapshots in input order.
    /// Pre-hooks run before provider work; post-hooks run after commit.
    pub async fn delete_many_returning(&self, iids: &[String]) -> Result<Vec<M>> {
        validate_binding_row_count(iids.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_entity_authority(
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
        let mut states = if self.hooks.has_hooks() {
            reserved_binding_vec(iids.len())?
        } else {
            Vec::new()
        };
        if self.hooks.has_hooks() {
            for (ordinal, iid) in iids.iter().enumerate() {
                states.push(
                    self.hooks
                        .run_pre(
                            M::TYPE_ID_JSON,
                            ModelKind::Entity,
                            CrudOperation::Delete,
                            Some(iid),
                            None,
                        )
                        .await
                        .map_err(|error| {
                            error.with_projected_batch_row(checked_batch_ordinal(ordinal))
                        })?,
                );
            }
        }
        let snapshots = crate::projected_batch::execute_owned_deleted_things(
            self.db.inner_orm(),
            installed,
            &batch,
        )
        .await?;
        for (iid, state) in iids.iter().zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    CrudOperation::Delete,
                    Some(iid),
                    None,
                    state,
                )
                .await;
        }
        Ok(snapshots)
    }

    /// Deletes only the exact model at canonical `iid`; subtype instances are not targeted.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        if !self.hooks.has_hooks() {
            return self.projected_delete(iid).await;
        }
        let state = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Delete,
                Some(iid),
                None,
            )
            .await?;
        self.projected_delete(iid).await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Delete,
                Some(iid),
                None,
                state,
            )
            .await;
        Ok(())
    }

    /// Atomically replaces each exact entity identified by its canonical IID and returns
    /// complete freshly hydrated models in input order. Every input and pre-hook completes
    /// before database work begins; any write or hydration failure rolls back the whole batch.
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
        validate_binding_row_count(inputs.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;

        if !self.hooks.has_hooks() {
            let rows = crate::projected_batch::update_rows(installed, &id, inputs)?;
            let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;
            return execute_owned_things(self.db.inner_orm(), installed, &batch).await;
        }
        let mut hook_inputs = reserved_binding_vec(inputs.len())?;
        let mut rows = reserved_binding_vec(inputs.len())?;
        for (ordinal, (iid, input)) in inputs.into_iter().enumerate() {
            let encoded = encode_batch_create(input, ordinal)?;
            rows.push(ProjectedBatchRow::Update {
                iid: iid.clone(),
                replacement: project_encoded_batch_create(&encoded, &id, installed, ordinal)?,
            });
            hook_inputs.push((iid, encoded));
        }
        let batch = prepare_batch(installed, id, ProjectedBatchOperation::Update, rows)?;

        let mut states = reserved_binding_vec(hook_inputs.len())?;
        for (ordinal, (iid, encoded)) in hook_inputs.iter().enumerate() {
            states.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Entity,
                        CrudOperation::Update,
                        Some(iid),
                        Some(encoded),
                    )
                    .await
                    .map_err(|error| {
                        error.with_projected_batch_row(checked_batch_ordinal(ordinal))
                    })?,
            );
        }

        let outputs = execute_owned_things::<M>(self.db.inner_orm(), installed, &batch).await?;
        for (((_, encoded), output), state) in hook_inputs.iter().zip(&outputs).zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    CrudOperation::Update,
                    Some(output.iid()),
                    Some(encoded),
                    state,
                )
                .await;
        }
        Ok(outputs)
    }

    /// Atomically deletes every exact entity at the supplied canonical IIDs. All IIDs and
    /// pre-hooks are accepted before database work begins; any write failure rolls back the
    /// whole batch, and post-hook failures do not change the committed result.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        validate_binding_row_count(iids.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
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
        let mut states = if self.hooks.has_hooks() {
            reserved_binding_vec(iids.len())?
        } else {
            Vec::new()
        };
        if self.hooks.has_hooks() {
            for (ordinal, iid) in iids.iter().enumerate() {
                states.push(
                    self.hooks
                        .run_pre(
                            M::TYPE_ID_JSON,
                            ModelKind::Entity,
                            CrudOperation::Delete,
                            Some(iid),
                            None,
                        )
                        .await
                        .map_err(|error| {
                            error.with_projected_batch_row(checked_batch_ordinal(ordinal))
                        })?,
                );
            }
        }
        execute_owned_delete(self.db.inner_orm(), installed, &batch).await?;
        for (iid, state) in iids.iter().zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    CrudOperation::Delete,
                    Some(iid),
                    None,
                    state,
                )
                .await;
        }
        Ok(())
    }

    async fn projected_write(
        &self,
        input: M::Create,
        operation: CrudOperation,
        iid: Option<&str>,
    ) -> Result<WriteResult<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        create
            .validate_native_write()
            .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let value = match crate::projected_batch::execute_transaction_single(
            &tx,
            installed,
            create,
            operation,
            iid,
            ModelKind::Entity,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        tx.commit_classified()
            .await
            .map_err(|error| Error::from_orm(error.into_orm_error()))?;
        Ok(value)
    }

    async fn projected_delete(&self, iid: &str) -> Result<()> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let result = ProjectedCrudExecutor::new(installed)
            .delete_entity_by_iid_in_transaction_with_compatibility(&tx, &id, iid)
            .await;
        if let Err(error) = result {
            let mapped =
                Error::from_projected_crud(error, ModelKind::Entity, Some(CrudOperation::Delete));
            let _ = tx.rollback().await;
            return Err(mapped);
        }
        tx.commit_classified()
            .await
            .map_err(|error| Error::from_orm(error.into_orm_error()))
    }

    async fn write_many_with_hooks(
        &self,
        inputs: Vec<M::Create>,
        crud_operation: CrudOperation,
    ) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let batch_operation =
            create_operation(installed, &id, matches!(crud_operation, CrudOperation::Put));
        let mut encoded = reserved_binding_vec(inputs.len())?;
        let mut rows = reserved_binding_vec(inputs.len())?;
        for (ordinal, input) in inputs.into_iter().enumerate() {
            let input = encode_batch_create(input, ordinal)?;
            rows.push(ProjectedBatchRow::Create(project_encoded_batch_create(
                &input, &id, installed, ordinal,
            )?));
            encoded.push(input);
        }
        let batch = prepare_batch(installed, id, batch_operation, rows)?;

        let mut metadata = reserved_binding_vec(encoded.len())?;
        for (ordinal, input) in encoded.iter().enumerate() {
            metadata.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Entity,
                        crud_operation,
                        None,
                        Some(input),
                    )
                    .await
                    .map_err(|error| {
                        error.with_projected_batch_row(checked_batch_ordinal(ordinal))
                    })?,
            );
        }

        let outputs = execute_owned_things::<M>(self.db.inner_orm(), installed, &batch).await?;
        for ((input, output), metadata) in encoded.iter().zip(&outputs).zip(metadata) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    crud_operation,
                    Some(output.iid()),
                    Some(input),
                    metadata,
                )
                .await;
        }
        Ok(outputs)
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = create_rows(installed, &id, inputs)?;
        let operation = create_operation(installed, &id, put);
        let batch = prepare_batch(installed, id, operation, rows)?;
        execute_owned_things(self.db.inner_orm(), installed, &batch).await
    }
    /// Counts only exact models, excluding subtypes.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .count_entities_with_compatibility(self.db.inner_orm(), &id)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Entity, None))
    }

    /// Reads one exact model by canonical IID; invalid IIDs are rejected before I/O and a
    /// valid but absent model returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .get_entity_by_iid_with_compatibility(self.db.inner_orm(), &id, iid)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Entity, None))?
            .map(|projected| materialize_projected(projected, installed))
            .transpose()
    }

    /// Reads all exact models in application result order, excluding subtypes; each result is
    /// a complete hydrated model.
    pub async fn all(&self) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = DynamicEntityManager::from_registry(
            self.db.inner_orm(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor.clone()),
        )
        .all_exact()
        .await
        .map_err(Error::from_orm)?;
        rows.into_iter()
            .map(|r| {
                let h = hydrate_entity(r, &id, installed)?;
                M::materialize(&h, &HydrationCapability::new())
                    .map_err(|e| map_validation_error(e, ModelValidationPhase::Hydration))
            })
            .collect()
    }
}
