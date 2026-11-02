#![deny(missing_docs)]

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_orm::_manager::DynamicRelationManager;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::{
    DynamicRelationRow, ProjectedBatchOperation, ProjectedBatchRow, ProjectedCrudExecutor,
    ProjectedManagerComparison,
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
use crate::projected_filter::ProjectedRelationFilter;
use crate::relation_codec::{
    hydrate_relation, resolve_discovered_relation, resolve_relation_authority,
};
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

fn ambiguous_provider_row() -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "ambiguous_provider_row",
        vec!["iid".into()],
        "provider returned multiple coalesced rows for one exact IID",
        None,
    )
}

pub(crate) fn one_coalesced_row(
    mut rows: Vec<DynamicRelationRow>,
) -> Result<Option<DynamicRelationRow>> {
    match rows.len() {
        0 => Ok(None),
        1 => Ok(Some(rows.remove(0))),
        _ => Err(ambiguous_provider_row()),
    }
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
    pub async fn insert(&self, input: M::Create) -> Result<M> {
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
                Some(output.iid()),
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
    pub async fn put(&self, input: M::Create) -> Result<M> {
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
                Some(output.iid()),
                Some(&encoded),
                state,
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
    /// Applies the per-item [`Self::put`] key-or-insert rule, including complete replacement
    /// of non-key ownership and active role players for existing exact rows, returning
    /// complete freshly hydrated models in input order, or one error for the whole call with
    /// no partial vector.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        validate_binding_row_count(inputs.len())?;
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, true).await;
        }
        self.write_many_with_hooks(inputs, CrudOperation::Put).await
    }
    /// Completely replaces non-key ownership and the complete effective active-role player
    /// set on the exact relation at canonical `iid`, preserves that IID and its keys, and
    /// returns its complete freshly hydrated model.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<M> {
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
                Some(output.iid()),
                Some(&encoded),
                state,
            )
            .await;
        Ok(output)
    }

    /// Delete an exact model and return its pre-delete snapshot, or `None` when absent.
    pub async fn delete_returning(&self, iid: &str) -> Result<Option<M>> {
        Ok(self.delete_many_returning(&[iid.to_owned()]).await?.pop())
    }

    /// Atomically delete exact models and return existing snapshots in input order.
    /// Pre-hooks run before provider work; post-hooks run after commit.
    pub async fn delete_many_returning(&self, iids: &[String]) -> Result<Vec<M>> {
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
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
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
        for (((_, encoded), output), state) in hook_inputs.iter().zip(&outputs).zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Relation,
                    CrudOperation::Update,
                    Some(output.iid()),
                    Some(encoded),
                    state,
                )
                .await;
        }
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
    ) -> Result<M> {
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
        let executor = ProjectedCrudExecutor::new(installed);
        executor
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
        let projected = match operation {
            CrudOperation::Insert => {
                executor
                    .insert_relation_in_transaction_with_compatibility(&tx, &create)
                    .await
            }
            CrudOperation::Put => {
                executor
                    .put_relation_in_transaction_with_compatibility(&tx, &create)
                    .await
            }
            CrudOperation::Update => {
                executor
                    .update_relation_in_transaction_with_compatibility(
                        &tx,
                        iid.expect("projected relation update requires a checked IID"),
                        &create,
                    )
                    .await
            }
            CrudOperation::Delete => unreachable!("delete has no generated create payload"),
        };
        let projected = match projected {
            Ok(value) => value,
            Err(error) => {
                let mapped =
                    Error::from_projected_crud(error, ModelKind::Relation, Some(operation));
                let _ = tx.rollback().await;
                return Err(mapped);
            }
        };
        let value = match materialize_projected(projected, installed) {
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
    ) -> Result<Vec<M>> {
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
        for ((input, output), state) in encoded.iter().zip(&outputs).zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Relation,
                    crud_operation,
                    Some(output.iid()),
                    Some(input),
                    state,
                )
                .await;
        }
        Ok(outputs)
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
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
        let (id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = DynamicRelationManager::from_registry(
            self.db.inner_orm(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor.clone()),
        )
        .all_exact()
        .await
        .map_err(Error::from_orm)?;
        rows.into_iter()
            .map(|row| {
                let hydrated = hydrate_relation(row, &id, installed)?;
                M::materialize(&hydrated, &HydrationCapability::new()).map_err(|error| {
                    crate::entity_codec::map_validation_error(
                        error,
                        ModelValidationPhase::Hydration,
                    )
                })
            })
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
    fn materialize_subtype(
        row: DynamicRelationRow,
        installed: &type_bridge_orm::InstalledRuntimeProjection,
    ) -> Result<M::Subtypes> {
        let (child_id, _) =
            resolve_discovered_relation(row.type_name.as_deref().unwrap_or_default(), installed)?;
        let hydrated = hydrate_relation(row, &child_id, installed)?;
        M::__tb_dispatch_subtype(&hydrated, &HydrationCapability::new()).map_err(|error| {
            crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
        })
    }

    /// Reads one canonical IID across the root and its generated concrete descendants.
    /// Invalid IIDs are rejected before I/O; a valid but absent IID returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M::Subtypes>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_relation_authority(
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
        let manager = DynamicRelationManager::with_registry_transaction(
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
            one_coalesced_row(manager.get_by_iid(iid).await.map_err(Error::from_orm)?)?
                .map(|row| Self::materialize_subtype(row, installed))
                .transpose()
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

    /// Reads all root/descendant relations in application result order, materialized as the
    /// generated leaf or family result. Validation, database, hydration, and close errors
    /// are returned without exposing implementation details.
    pub async fn all(&self) -> Result<Vec<M::Subtypes>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_relation_authority(
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
        let manager = DynamicRelationManager::with_registry_transaction(
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
            manager
                .all()
                .await
                .map_err(Error::from_orm)?
                .into_iter()
                .map(|row| Self::materialize_subtype(row, installed))
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
        let (_id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        DynamicRelationManager::from_registry(
            self.db.inner_orm(),
            installed.manager_registry().map_err(Error::from_orm)?,
            Arc::new(descriptor),
        )
        .count()
        .await
        .map_err(Error::from_orm)
    }
}
