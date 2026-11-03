#![deny(missing_docs)]

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::{
    ProjectedBatchOperation, ProjectedBatchRow, ProjectedCrudExecutor, ProjectedManagerComparison,
};

use crate::__codegen::{
    CompleteModel, EncodedCreate, FieldToken, HydrationCapability, IntoEncodedCreate, Model,
    QueryValued, RelationModel, SubtypeRootModel,
};
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, ModelKind};
use crate::projected_batch::{
    checked_batch_ordinal, create_operation, create_rows, delete_rows, encode_batch_create,
    execute_owned_delete, execute_owned_things, prepare_batch, project_encoded_batch_create,
    reserved_binding_vec, validate_binding_row_count,
};
use crate::projected_codec::{materialize_projected, project_create};
use type_bridge_orm::{ProjectedManagerFilterExecutor, QueryTarget};

use crate::projected_filter::ProjectedRelationFilter;
use crate::projected_filter::manager_read_control;
use crate::relation_codec::resolve_relation_authority;
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

/// Schema-bound, model-branded manager for exact relation operations.
/// Exact reads and writes exclude subtypes; methods return client input/schema-validation,
/// database/transaction/close, or hydration/model-validation errors as applicable.
pub struct RelationManager<'db, S: Schema, M: RelationModel<Schema = S>> {
    db: &'db Database<S>,
    hooks: HookRunner,
    marker: PhantomData<M>,
}

impl<'db, S: Schema, M: RelationModel<Schema = S>> Clone for RelationManager<'db, S, M> {
    fn clone(&self) -> Self {
        Self {
            db: self.db,
            hooks: self.hooks.clone(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, M: RelationModel<Schema = S>> RelationManager<'_, S, M> {
    pub(crate) fn new(db: &Database<S>) -> RelationManager<'_, S, M> {
        RelationManager {
            db,
            hooks: HookRunner::default(),
            marker: PhantomData,
        }
    }
}

impl<'db, S, M> RelationManager<'db, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
    /// Start one empty immutable canonical filter for this exact relation model.
    pub fn filter(&self) -> Result<ProjectedRelationFilter<'db, S, M>> {
        ProjectedRelationFilter::for_database(self.db)
    }

    /// Start a canonical filter with one exact generated field comparison.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<ProjectedRelationFilter<'db, S, M>>
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
        input.clone().into_encoded_create().map_err(|error| {
            crate::entity_codec::map_validation_error(error, ModelValidationPhase::Input)
        })
    }

    /// Inserts one exact relation with its complete active role players and returns the
    /// complete freshly hydrated model. Errors may be input/schema validation,
    /// database/transaction/close, or model hydration errors.
    pub async fn insert(&self, input: M::Create) -> Result<WriteResult<M>> {
        if !self.hooks.has_hooks() {
            return self
                .projected_write(input, CrudOperation::Insert, None)
                .await;
        }
        let encoded = Self::encode_hook_input(&input)?;
        let state = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Relation,
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
                ModelKind::Relation,
                CrudOperation::Insert,
                output.item().map(|item| item.iid()),
                Some(&encoded),
                state,
            )
            .await;
        Ok(output)
    }
    /// Uses the projected exact-model key when one usable key is carried; otherwise inserts.
    /// For an existing exact row, the supplied create value completely replaces non-key
    /// ownership and the complete effective active-role player set while preserving the
    /// exact IID and keys. It returns a complete freshly hydrated model.
    pub async fn put(&self, input: M::Create) -> Result<WriteResult<M>> {
        if !self.hooks.has_hooks() {
            return self.projected_write(input, CrudOperation::Put, None).await;
        }
        let encoded = Self::encode_hook_input(&input)?;
        let state = self
            .hooks
            .run_pre(
                M::TYPE_ID_JSON,
                ModelKind::Relation,
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
                ModelKind::Relation,
                CrudOperation::Put,
                output.item().map(|item| item.iid()),
                Some(&encoded),
                state,
            )
            .await;
        Ok(output)
    }
    /// Inserts each item and returns complete freshly hydrated models in input order, or one
    /// error for the whole call with no partial result vector.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, false).await;
        }
        self.write_many_with_hooks(inputs, CrudOperation::Insert)
            .await
    }
    /// Applies the per-item [`Self::put`] key-or-insert rule, including complete replacement
    /// of non-key ownership and active role players for existing exact rows, returning
    /// complete freshly hydrated models in input order, or one error for the whole call with
    /// no partial vector.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, true).await;
        }
        self.write_many_with_hooks(inputs, CrudOperation::Put).await
    }
    /// Completely replaces non-key ownership and the complete effective active-role player
    /// set on the exact relation at canonical `iid`, preserves that IID and its keys, and
    /// returns its complete freshly hydrated model.
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
                ModelKind::Relation,
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
                ModelKind::Relation,
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
            .map(|result| WriteResult::new(result.into_items()))
    }

    /// Atomically delete exact models and return existing snapshots in input order.
    /// Pre-hooks run before provider work; post-hooks run after commit.
    pub async fn delete_many_returning(
        &self,
        iids: &[String],
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(iids.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_relation_authority(
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
                            ModelKind::Relation,
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
                    ModelKind::Relation,
                    CrudOperation::Delete,
                    Some(iid),
                    None,
                    state,
                )
                .await;
        }
        Ok(snapshots)
    }

    /// Deletes only the exact relation at canonical `iid`; subtype instances are not
    /// targeted.
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
                ModelKind::Relation,
                CrudOperation::Delete,
                Some(iid),
                None,
            )
            .await?;
        self.projected_delete(iid).await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Relation,
                CrudOperation::Delete,
                Some(iid),
                None,
                state,
            )
            .await;
        Ok(())
    }

    /// Atomically replaces each exact relation identified by its canonical IID and returns
    /// complete freshly hydrated models in input order. Every input and pre-hook completes
    /// before database work begins; any write or hydration failure rolls back the whole batch.
    pub async fn update_many(
        &self,
        inputs: Vec<(String, M::Create)>,
    ) -> Result<crate::BatchWriteResult<M>> {
        validate_binding_row_count(inputs.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_relation_authority(
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
                        ModelKind::Relation,
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
        self.hooks
            .run_post_batch(
                M::TYPE_ID_JSON,
                ModelKind::Relation,
                CrudOperation::Update,
                |index| &hook_inputs[index].1,
                states,
                &outputs,
            )
            .await;
        Ok(outputs)
    }

    /// Atomically deletes every exact relation at the supplied canonical IIDs. All IIDs and
    /// pre-hooks are accepted before database work begins; any write failure rolls back the
    /// whole batch, and post-hook failures do not change the committed result.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        validate_binding_row_count(iids.len())?;
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
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
                            ModelKind::Relation,
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
                    ModelKind::Relation,
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
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let create = project_create(input, &id, installed)?;
        create
            .validate_native_write()
            .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))?;
        ProjectedCrudExecutor::new(installed)
            .preflight_relation_create_for_database_with_compatibility(self.db.inner_orm(), &create)
            .map_err(|error| {
                Error::from_projected_crud(error, ModelKind::Relation, Some(operation))
            })?;
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
            ModelKind::Relation,
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
        let (id, _descriptor) = resolve_relation_authority(
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
            .delete_relation_by_iid_in_transaction_with_compatibility(&tx, &id, iid)
            .await;
        if let Err(error) = result {
            let mapped =
                Error::from_projected_crud(error, ModelKind::Relation, Some(CrudOperation::Delete));
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
    ) -> Result<crate::BatchWriteResult<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_relation_authority(
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

        let mut states = reserved_binding_vec(encoded.len())?;
        for (ordinal, input) in encoded.iter().enumerate() {
            states.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Relation,
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
        self.hooks
            .run_post_batch(
                M::TYPE_ID_JSON,
                ModelKind::Relation,
                crud_operation,
                |index| &encoded[index],
                states,
                &outputs,
            )
            .await;
        Ok(outputs)
    }

    async fn write_many(
        &self,
        inputs: Vec<M::Create>,
        put: bool,
    ) -> Result<crate::BatchWriteResult<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_relation_authority(
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

    /// Counts only exact relations, excluding subtypes.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .count_relations_with_compatibility(self.db.inner_orm(), &id)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Relation, None))
    }

    /// Reads one exact coalesced relation by canonical IID; invalid IIDs are rejected before
    /// I/O and a valid but absent relation returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedCrudExecutor::new(installed)
            .get_relation_by_iid_with_compatibility(self.db.inner_orm(), &id, iid)
            .await
            .map_err(|error| Error::from_projected_crud(error, ModelKind::Relation, None))?
            .map(|projected| materialize_projected(projected, installed))
            .transpose()
    }

    /// Reads all exact coalesced relations in application result order, excluding subtypes;
    /// each result is a complete hydrated model.
    pub async fn all(&self) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        ProjectedManagerFilterExecutor::new(installed)
            .compatibility_all_on_target(
                QueryTarget::Database(self.db.inner_orm()),
                &id,
                &[],
                manager_read_control(self.db.inner_orm()),
            )
            .await
            .map_err(crate::remote::target_error)?
            .into_iter()
            .map(|thing| materialize_projected(thing, installed))
            .collect()
    }
}

/// Read-only, schema/model-branded manager for an inclusive generated subtype association.
/// Results are the generated associated leaf or closed family type; no writes are exposed.
/// Reads and counts can return input/schema-validation, database/transaction/close, or
/// hydration/model-validation errors.
pub struct RelationSubtypeManager<
    'db,
    S: Schema,
    M: SubtypeRootModel<Schema = S> + RelationModel<Schema = S>,
> {
    db: &'db Database<S>,
    marker: PhantomData<M>,
}

impl<'db, S: Schema, M: SubtypeRootModel<Schema = S> + RelationModel<Schema = S>>
    RelationSubtypeManager<'db, S, M>
{
    pub(crate) fn new(db: &'db Database<S>) -> Self {
        Self {
            db,
            marker: PhantomData,
        }
    }
}

impl<'db, S, M> RelationManager<'db, S, M>
where
    S: Schema,
    M: SubtypeRootModel<Schema = S> + RelationModel<Schema = S>,
{
    /// Switches only the read scope and result shape to the generated inclusive subtype
    /// association; it does not add write operations.
    pub fn subtypes(&self) -> RelationSubtypeManager<'db, S, M> {
        RelationSubtypeManager::new(self.db)
    }
}

impl<S, M> RelationSubtypeManager<'_, S, M>
where
    S: Schema,
    M: SubtypeRootModel<Schema = S> + RelationModel<Schema = S>,
{
    /// Reads one canonical IID across the root and its generated concrete descendants.
    /// Invalid IIDs are rejected before I/O; a valid but absent IID returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M::Subtypes>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        ProjectedManagerFilterExecutor::new(installed)
            .get_by_iid_including_subtypes_on_target(
                QueryTarget::Database(self.db.inner_orm()),
                &id,
                iid,
                manager_read_control(self.db.inner_orm()),
            )
            .await
            .map_err(crate::remote::target_error)?
            .map(|thing| {
                let hydrated =
                    crate::projected_codec::projected_to_hydrated_row(&thing, installed)?;
                M::__tb_dispatch_subtype(&hydrated, &HydrationCapability::new()).map_err(|error| {
                    crate::entity_codec::map_validation_error(
                        error,
                        ModelValidationPhase::Hydration,
                    )
                })
            })
            .transpose()
    }

    /// Reads all root/descendant relations in application result order, materialized as the
    /// generated leaf or family result. Validation, database, hydration, and close errors
    /// are returned without exposing implementation details.
    pub async fn all(&self) -> Result<Vec<M::Subtypes>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        ProjectedManagerFilterExecutor::new(installed)
            .all_including_subtypes_on_target(
                QueryTarget::Database(self.db.inner_orm()),
                &id,
                manager_read_control(self.db.inner_orm()),
            )
            .await
            .map_err(crate::remote::target_error)?
            .into_iter()
            .map(|thing| {
                let hydrated =
                    crate::projected_codec::projected_to_hydrated_row(&thing, installed)?;
                M::__tb_dispatch_subtype(&hydrated, &HydrationCapability::new()).map_err(|error| {
                    crate::entity_codec::map_validation_error(
                        error,
                        ModelValidationPhase::Hydration,
                    )
                })
            })
            .collect()
    }

    /// Counts the root and all concrete descendants using the inclusive subtype scope.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        ProjectedManagerFilterExecutor::new(installed)
            .count_including_subtypes_on_target(
                QueryTarget::Database(self.db.inner_orm()),
                &id,
                manager_read_control(self.db.inner_orm()),
            )
            .await
            .map_err(crate::remote::target_error)
    }
}
