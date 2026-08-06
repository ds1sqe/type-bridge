#![deny(missing_docs)]

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_orm::_manager::DynamicEntityManager;
use type_bridge_orm::session::backend::TxType;

use crate::__codegen::{
    CompleteModel, EncodedCreate, EntityModel, HydrationCapability, IntoEncodedCreate,
    SubtypeRootModel,
};
use crate::entity_codec::{
    hydrate_entity, lower_entity_create, map_validation_error, resolve_discovered_entity,
    resolve_entity_authority,
};
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, ModelKind};
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

/// Exact-fetch, hydrate, and materialize one freshly written entity through the
/// shared open context without any transaction-terminal operation.
pub(crate) async fn rehydrate_written_entity<M>(
    manager: &DynamicEntityManager<'_>,
    iid: &str,
    id: &type_bridge_contract::id::TypeId,
    installed: &type_bridge_orm::InstalledRuntimeProjection,
) -> Result<M>
where
    M: crate::__codegen::CompleteModel,
{
    let row = manager
        .get_by_iid_exact(iid)
        .await
        .map_err(Error::from_orm)?
        .ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Hydration,
                "missing_post_write_row",
                vec!["iid".into()],
                "written entity was not returned",
                None,
            )
        })?;
    let hydrated = hydrate_entity(row, id, installed)?;
    M::materialize(&hydrated, &HydrationCapability::new())
        .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
}

impl<S, M> EntitySubtypeManager<'_, S, M>
where
    S: Schema,
    M: SubtypeRootModel<Schema = S> + EntityModel<Schema = S>,
{
    /// Reads one canonical IID across the root and its generated concrete descendants.
    /// Invalid IIDs are rejected before I/O; a valid but absent IID returns `None`.
    pub async fn get_by_iid(&self, _iid: &str) -> Result<Option<M::Subtypes>> {
        if !is_canonical_thing_iid(_iid) {
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
        let manager =
            DynamicEntityManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let identity = match manager.discover_by_iid(_iid).await {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.close().await;
                return Err(Error::from_orm(e));
            }
        };
        let out = match identity {
            None => None,
            Some(identity) => {
                let (child_id, child_descriptor) =
                    match resolve_discovered_entity(&identity.type_name, installed) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = tx.close().await;
                            return Err(e);
                        }
                    };
                let child = DynamicEntityManager::with_canonical_transaction(
                    tx.clone(),
                    Arc::new(child_descriptor),
                );
                let row = match child.get_by_iid_exact(&identity.iid).await {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        let _ = tx.close().await;
                        return Err(Error::model_validation(
                            ModelValidationPhase::Hydration,
                            "missing_concrete_row",
                            vec!["iid".into()],
                            "discovered entity row is missing",
                            None,
                        ));
                    }
                    Err(e) => {
                        let _ = tx.close().await;
                        return Err(Error::from_orm(e));
                    }
                };
                let h = match hydrate_entity(row, &child_id, installed) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.close().await;
                        return Err(e);
                    }
                };
                Some(
                    match M::__tb_dispatch_subtype(&h, &HydrationCapability::new()) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = tx.close().await;
                            return Err(map_validation_error(e, ModelValidationPhase::Hydration));
                        }
                    },
                )
            }
        };
        tx.close().await.map_err(Error::from_orm)?;
        Ok(out)
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
        let manager =
            DynamicEntityManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let identities = match manager.discover_all().await {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.close().await;
                return Err(Error::from_orm(e));
            }
        };
        let mut out = Vec::with_capacity(identities.len());
        for identity in identities {
            let type_json = identity.type_name;
            let (child_id, child_descriptor) =
                match resolve_discovered_entity(&type_json, installed) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.close().await;
                        return Err(e);
                    }
                };
            let child = DynamicEntityManager::with_canonical_transaction(
                tx.clone(),
                Arc::new(child_descriptor),
            );
            let row = match child.get_by_iid_exact(&identity.iid).await {
                Ok(Some(v)) => v,
                Ok(None) => {
                    let _ = tx.close().await;
                    return Err(Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "missing_concrete_row",
                        vec!["iid".into()],
                        "discovered entity row is missing",
                        None,
                    ));
                }
                Err(e) => {
                    let _ = tx.close().await;
                    return Err(Error::from_orm(e));
                }
            };
            let h = match hydrate_entity(row, &child_id, installed) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.close().await;
                    return Err(e);
                }
            };
            out.push(
                match M::__tb_dispatch_subtype(&h, &HydrationCapability::new()) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.close().await;
                        return Err(map_validation_error(e, ModelValidationPhase::Hydration));
                    }
                },
            );
        }
        tx.close().await.map_err(Error::from_orm)?;
        Ok(out)
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
        DynamicEntityManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor))
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

impl<S, M> EntityManager<'_, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
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
    pub async fn insert(&self, input: M::Create) -> Result<M> {
        if !self.hooks.has_hooks() {
            return self.write(input, false).await;
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
        let output = self.write(input, false).await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Insert,
                Some(output.iid()),
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
    pub async fn put(&self, input: M::Create) -> Result<M> {
        if !self.hooks.has_hooks() {
            return self.write(input, true).await;
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
        let output = self.write(input, true).await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Put,
                Some(output.iid()),
                Some(&encoded),
                metadata,
            )
            .await;
        Ok(output)
    }
    /// Inserts each item and returns complete freshly hydrated models in input order, or one
    /// error for the whole call with no partial result vector.
    pub async fn insert_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, false).await;
        }
        let encoded = inputs
            .iter()
            .map(Self::encode_hook_input)
            .collect::<Result<Vec<_>>>()?;
        let mut metadata = Vec::with_capacity(inputs.len());
        for input in &encoded {
            metadata.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Entity,
                        CrudOperation::Insert,
                        None,
                        Some(input),
                    )
                    .await?,
            );
        }
        let outputs = self.write_many(inputs.clone(), false).await?;
        for ((input, output), metadata) in encoded.iter().zip(&outputs).zip(metadata) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    CrudOperation::Insert,
                    Some(output.iid()),
                    Some(input),
                    metadata,
                )
                .await;
        }
        Ok(outputs)
    }
    /// Applies the per-item [`Self::put`] key-or-insert rule, including complete replacement of
    /// non-key ownership for existing exact rows, returning complete freshly hydrated models
    /// in input order, or one error for the whole call with no partial vector.
    pub async fn put_many(&self, inputs: Vec<M::Create>) -> Result<Vec<M>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if !self.hooks.has_hooks() {
            return self.write_many(inputs, true).await;
        }
        let encoded = inputs
            .iter()
            .map(Self::encode_hook_input)
            .collect::<Result<Vec<_>>>()?;
        let mut metadata = Vec::with_capacity(inputs.len());
        for input in &encoded {
            metadata.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Entity,
                        CrudOperation::Put,
                        None,
                        Some(input),
                    )
                    .await?,
            );
        }
        let outputs = self.write_many(inputs.clone(), true).await?;
        for ((input, output), metadata) in encoded.iter().zip(&outputs).zip(metadata) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Entity,
                    CrudOperation::Put,
                    Some(output.iid()),
                    Some(input),
                    metadata,
                )
                .await;
        }
        Ok(outputs)
    }
    /// Completely replaces non-key ownership on the exact model at canonical `iid`, preserves
    /// that IID, and returns its complete freshly hydrated model. Omitted optional values and
    /// empty multivalue collections remove prior ownership.
    pub async fn update(&self, iid: &str, input: M::Create) -> Result<M> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        if !self.hooks.has_hooks() {
            return self.update_write(iid, input).await;
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
        let output = self.update_write(iid, input).await?;
        self.hooks
            .run_post(
                M::TYPE_ID_JSON,
                ModelKind::Entity,
                CrudOperation::Update,
                Some(output.iid()),
                Some(&encoded),
                state,
            )
            .await;
        Ok(output)
    }

    async fn update_write(&self, iid: &str, input: M::Create) -> Result<M> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let attrs = lower_entity_create(input, &id, installed)?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicEntityManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        if let Err(error) = manager.update_exact(iid, &attrs).await {
            let _ = tx.rollback().await;
            return Err(Error::from_orm(error));
        }
        let row = match manager.get_by_iid_exact(iid).await {
            Ok(Some(row)) => row,
            Ok(None) => {
                let _ = tx.rollback().await;
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "missing_post_write_row",
                    vec!["iid".into()],
                    "updated entity was not returned",
                    None,
                ));
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let hydrated = match hydrate_entity(row, &id, installed) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        let value = match M::materialize(&hydrated, &HydrationCapability::new()) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(map_validation_error(error, ModelValidationPhase::Hydration));
            }
        };
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(value)
    }
    /// Deletes only the exact model at canonical `iid`; subtype instances are not targeted.
    pub async fn delete(&self, iid: &str) -> Result<()> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        if !self.hooks.has_hooks() {
            return self.delete_write(iid).await;
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
        self.delete_write(iid).await?;
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

    async fn delete_write(&self, iid: &str) -> Result<()> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
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
        let manager = DynamicEntityManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        if let Err(error) = manager.delete_by_iid_exact(iid).await {
            let _ = tx.rollback().await;
            return Err(Error::from_orm(error));
        }
        tx.commit().await.map_err(Error::from_orm)
    }

    /// Atomically replaces each exact entity identified by its canonical IID and returns
    /// complete freshly hydrated models in input order. Every input and pre-hook completes
    /// before database work begins; any write or hydration failure rolls back the whole batch.
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let mut prepared = Vec::with_capacity(inputs.len());
        for (iid, input) in inputs {
            if !is_canonical_thing_iid(&iid) {
                return Err(invalid_iid());
            }
            let encoded = Self::encode_hook_input(&input)?;
            let attributes = lower_entity_create(input, &id, installed)?;
            prepared.push((iid, encoded, attributes));
        }

        let mut states = Vec::new();
        if self.hooks.has_hooks() {
            states.reserve(prepared.len());
            for (iid, encoded, _) in &prepared {
                states.push(
                    self.hooks
                        .run_pre(
                            M::TYPE_ID_JSON,
                            ModelKind::Entity,
                            CrudOperation::Update,
                            Some(iid),
                            Some(encoded),
                        )
                        .await?,
                );
            }
        }

        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager =
            DynamicEntityManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let mut outputs = Vec::with_capacity(prepared.len());
        for (iid, _, attributes) in &prepared {
            if let Err(error) = manager.update_exact(iid, attributes).await {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
            match rehydrate_written_entity::<M>(&manager, iid, &id, installed).await {
                Ok(output) => outputs.push(output),
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            }
        }
        tx.commit().await.map_err(Error::from_orm)?;

        if self.hooks.has_hooks() {
            for (((_, encoded, _), output), state) in prepared.iter().zip(&outputs).zip(states) {
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
        }
        Ok(outputs)
    }

    /// Atomically deletes every exact entity at the supplied canonical IIDs. All IIDs and
    /// pre-hooks are accepted before database work begins; any write failure rolls back the
    /// whole batch, and post-hook failures do not change the committed result.
    pub async fn delete_many(&self, iids: &[String]) -> Result<()> {
        if iids.is_empty() {
            return Ok(());
        }
        if iids.iter().any(|iid| !is_canonical_thing_iid(iid)) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;

        let mut states = Vec::new();
        if self.hooks.has_hooks() {
            states.reserve(iids.len());
            for iid in iids {
                states.push(
                    self.hooks
                        .run_pre(
                            M::TYPE_ID_JSON,
                            ModelKind::Entity,
                            CrudOperation::Delete,
                            Some(iid),
                            None,
                        )
                        .await?,
                );
            }
        }

        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager =
            DynamicEntityManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        for iid in iids {
            if let Err(error) = manager.delete_by_iid_exact(iid).await {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        }
        tx.commit().await.map_err(Error::from_orm)?;

        if self.hooks.has_hooks() {
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
        }
        Ok(())
    }

    async fn write(&self, input: M::Create, put: bool) -> Result<M> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let attrs = lower_entity_create(input, &id, installed)?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicEntityManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        let iid = if put {
            manager.put_exact(&attrs).await
        } else {
            manager.insert(&attrs).await
        };
        let iid = match iid {
            Ok(iid) => iid,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let row = match manager.get_by_iid_exact(&iid).await {
            Ok(Some(row)) => row,
            Ok(None) => {
                let _ = tx.rollback().await;
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "missing_post_write_row",
                    vec!["iid".into()],
                    "written entity was not returned",
                    None,
                ));
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let hydrated = match hydrate_entity(row, &id, installed) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        let value = match M::materialize(&hydrated, &HydrationCapability::new()) {
            Ok(value) => value,
            Err(error) => {
                let mapped = map_validation_error(error, ModelValidationPhase::Hydration);
                let _ = tx.rollback().await;
                return Err(mapped);
            }
        };
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(value)
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let mut lowered = Vec::with_capacity(inputs.len());
        for input in inputs {
            lowered.push(lower_entity_create(input, &id, installed)?);
        }
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager =
            DynamicEntityManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let iids = match if put {
            manager.put_many_exact(&lowered).await
        } else {
            manager.insert_many(&lowered).await
        } {
            Ok(v) if v.len() == lowered.len() => v,
            Ok(_) => {
                let _ = tx.rollback().await;
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "iid_count_mismatch",
                    vec!["iid".into()],
                    "provider returned an unexpected IID count",
                    None,
                ));
            }
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(e));
            }
        };
        let mut out = Vec::with_capacity(iids.len());
        for iid in iids {
            let row = match manager.get_by_iid_exact(&iid).await {
                Ok(Some(r)) => r,
                Ok(None) => {
                    let _ = tx.rollback().await;
                    return Err(Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "missing_post_write_row",
                        vec!["iid".into()],
                        "written entity was not returned",
                        None,
                    ));
                }
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Err(Error::from_orm(e));
                }
            };
            let h = match hydrate_entity(row, &id, installed) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Err(e);
                }
            };
            let value = match M::materialize(&h, &HydrationCapability::new()) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Err(map_validation_error(e, ModelValidationPhase::Hydration));
                }
            };
            out.push(value);
        }
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(out)
    }
    /// Counts only exact models, excluding subtypes.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        DynamicEntityManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor.clone()))
            .count_exact()
            .await
            .map_err(Error::from_orm)
    }

    /// Reads one exact model by canonical IID; invalid IIDs are rejected before I/O and a
    /// valid but absent model returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let row =
            DynamicEntityManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor.clone()))
                .get_by_iid_exact(iid)
                .await
                .map_err(Error::from_orm)?;
        match row {
            None => Ok(None),
            Some(r) => {
                let h = hydrate_entity(r, &id, installed)?;
                let value = M::materialize(&h, &HydrationCapability::new())
                    .map_err(|e| map_validation_error(e, ModelValidationPhase::Hydration))?;
                Ok(Some(value))
            }
        }
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
        let rows =
            DynamicEntityManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor.clone()))
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
