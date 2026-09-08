#![deny(missing_docs)]

use std::marker::PhantomData;
use std::sync::Arc;

use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_orm::_manager::DynamicRelationManager;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::{DynamicAttributeMap, DynamicRelationRow, DynamicRolePlayerInput};

use crate::__codegen::{
    CompleteModel, EncodedCreate, HydrationCapability, IntoEncodedCreate, RelationModel,
    SubtypeRootModel,
};
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, ModelKind};
use crate::relation_codec::{
    hydrate_relation, lower_relation_create, resolve_discovered_relation,
    resolve_relation_authority,
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

fn missing_post_write_row() -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "missing_post_write_row",
        vec!["iid".into()],
        "written relation was not returned",
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

/// Exact-fetch, coalesce, hydrate, and materialize one freshly written relation
/// through the shared open context without any transaction-terminal operation.
pub(crate) async fn rehydrate_written_relation<M>(
    manager: &DynamicRelationManager<'_>,
    iid: &str,
    id: &type_bridge_contract::id::TypeId,
    installed: &type_bridge_orm::InstalledRuntimeProjection,
) -> Result<M>
where
    M: crate::__codegen::CompleteModel,
{
    let rows = manager
        .get_by_iid_exact(iid)
        .await
        .map_err(Error::from_orm)?;
    let row = one_coalesced_row(rows)?.ok_or_else(missing_post_write_row)?;
    let hydrated = hydrate_relation(row, id, installed)?;
    M::materialize(&hydrated, &HydrationCapability::new()).map_err(|error| {
        crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
    })
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

impl<S, M> RelationManager<'_, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
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
            return self.write(input, false).await;
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
        let output = self.write(input, false).await?;
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
            return self.write(input, true).await;
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
        let output = self.write(input, true).await?;
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
        let mut states = Vec::with_capacity(inputs.len());
        for input in &encoded {
            states.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Relation,
                        CrudOperation::Insert,
                        None,
                        Some(input),
                    )
                    .await?,
            );
        }
        let outputs = self.write_many(inputs, false).await?;
        for ((input, output), state) in encoded.iter().zip(&outputs).zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Relation,
                    CrudOperation::Insert,
                    Some(output.iid()),
                    Some(input),
                    state,
                )
                .await;
        }
        Ok(outputs)
    }
    /// Applies the per-item [`Self::put`] key-or-insert rule, including complete replacement
    /// of non-key ownership and active role players for existing exact rows, returning
    /// complete freshly hydrated models in input order, or one error for the whole call with
    /// no partial vector.
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
        let mut states = Vec::with_capacity(inputs.len());
        for input in &encoded {
            states.push(
                self.hooks
                    .run_pre(
                        M::TYPE_ID_JSON,
                        ModelKind::Relation,
                        CrudOperation::Put,
                        None,
                        Some(input),
                    )
                    .await?,
            );
        }
        let outputs = self.write_many(inputs, true).await?;
        for ((input, output), state) in encoded.iter().zip(&outputs).zip(states) {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    ModelKind::Relation,
                    CrudOperation::Put,
                    Some(output.iid()),
                    Some(input),
                    state,
                )
                .await;
        }
        Ok(outputs)
    }
    /// Completely replaces non-key ownership and the complete effective active-role player
    /// set on the exact relation at canonical `iid`, preserves that IID and its keys, and
    /// returns its complete freshly hydrated model.
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
                ModelKind::Relation,
                CrudOperation::Update,
                Some(iid),
                Some(&encoded),
            )
            .await?;
        let output = self.update_write(iid, input).await?;
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

    async fn update_write(&self, iid: &str, input: M::Create) -> Result<M> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let prepared = lower_relation_create(input, &id, installed)?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicRelationManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        if let Err(error) = manager
            .update_exact(iid, &prepared.attributes, &prepared.role_players)
            .await
        {
            let _ = tx.rollback().await;
            return Err(Error::from_orm(error));
        }
        let rows = match manager.get_by_iid_exact(iid).await {
            Ok(rows) => rows,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let row = match one_coalesced_row(rows) {
            Ok(Some(row)) => row,
            Ok(None) => {
                let _ = tx.rollback().await;
                return Err(missing_post_write_row());
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        let hydrated = match hydrate_relation(row, &id, installed) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        let value = match M::materialize(&hydrated, &HydrationCapability::new()) {
            Ok(value) => value,
            Err(error) => {
                let mapped = crate::entity_codec::map_validation_error(
                    error,
                    ModelValidationPhase::Hydration,
                );
                let _ = tx.rollback().await;
                return Err(mapped);
            }
        };
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(value)
    }
    /// Deletes only the exact relation at canonical `iid`; subtype instances are not
    /// targeted.
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
                ModelKind::Relation,
                CrudOperation::Delete,
                Some(iid),
                None,
            )
            .await?;
        self.delete_write(iid).await?;
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

    async fn delete_write(&self, iid: &str) -> Result<()> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_relation_authority(
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
        let manager = DynamicRelationManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        if let Err(error) = manager.delete_by_iid_exact(iid).await {
            let _ = tx.rollback().await;
            return Err(Error::from_orm(error));
        }
        tx.commit().await.map_err(Error::from_orm)
    }

    /// Atomically replaces each exact relation identified by its canonical IID and returns
    /// complete freshly hydrated models in input order. Every input and pre-hook completes
    /// before database work begins; any write or hydration failure rolls back the whole batch.
    pub async fn update_many(&self, inputs: Vec<(String, M::Create)>) -> Result<Vec<M>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_relation_authority(
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
            let lowered = lower_relation_create(input, &id, installed)?;
            prepared.push((iid, encoded, lowered));
        }

        let mut states = Vec::new();
        if self.hooks.has_hooks() {
            states.reserve(prepared.len());
            for (iid, encoded, _) in &prepared {
                states.push(
                    self.hooks
                        .run_pre(
                            M::TYPE_ID_JSON,
                            ModelKind::Relation,
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
            DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let mut outputs = Vec::with_capacity(prepared.len());
        for (iid, _, lowered) in &prepared {
            if let Err(error) = manager
                .update_exact(iid, &lowered.attributes, &lowered.role_players)
                .await
            {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
            match rehydrate_written_relation::<M>(&manager, iid, &id, installed).await {
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
                        ModelKind::Relation,
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

    /// Atomically deletes every exact relation at the supplied canonical IIDs. All IIDs and
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
        let (_id, descriptor) = resolve_relation_authority(
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
                            ModelKind::Relation,
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
            DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
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
                        ModelKind::Relation,
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
        let (id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let prepared = lower_relation_create(input, &id, installed)?;
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager = DynamicRelationManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(descriptor.clone()),
        );
        let iid = if put {
            manager
                .put_exact(&prepared.attributes, &prepared.role_players)
                .await
        } else {
            manager
                .insert(&prepared.attributes, &prepared.role_players)
                .await
        };
        let iid = match iid {
            Ok(iid) => iid,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let value = match self
            .hydrate_in_transaction(&manager, &iid, &id, installed)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(value)
    }

    async fn write_many(&self, inputs: Vec<M::Create>, put: bool) -> Result<Vec<M>> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let mut lowered: Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)> =
            Vec::with_capacity(inputs.len());
        for input in inputs {
            let prepared = lower_relation_create(input, &id, installed)?;
            lowered.push((prepared.attributes, prepared.role_players));
        }
        let tx = self
            .db
            .inner_orm()
            .transaction_context(TxType::Write)
            .await
            .map_err(Error::from_orm)?;
        let manager =
            DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let iids = match if put {
            manager.put_many_exact(&lowered).await
        } else {
            manager.insert_many(&lowered).await
        } {
            Ok(iids) if iids.len() == lowered.len() => iids,
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
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(Error::from_orm(error));
            }
        };
        let mut out = Vec::with_capacity(iids.len());
        for iid in iids {
            match self
                .hydrate_in_transaction(&manager, &iid, &id, installed)
                .await
            {
                Ok(value) => out.push(value),
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            }
        }
        tx.commit().await.map_err(Error::from_orm)?;
        Ok(out)
    }

    async fn hydrate_in_transaction(
        &self,
        manager: &DynamicRelationManager<'_>,
        iid: &str,
        id: &type_bridge_contract::id::TypeId,
        installed: &type_bridge_orm::InstalledRuntimeProjection,
    ) -> Result<M> {
        rehydrate_written_relation(manager, iid, id, installed).await
    }

    /// Counts only exact relations, excluding subtypes.
    pub async fn count(&self) -> Result<u64> {
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (_id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        DynamicRelationManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor.clone()))
            .count_exact()
            .await
            .map_err(Error::from_orm)
    }

    /// Reads one exact coalesced relation by canonical IID; invalid IIDs are rejected before
    /// I/O and a valid but absent relation returns `None`.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<M>> {
        if !is_canonical_thing_iid(iid) {
            return Err(invalid_iid());
        }
        let installed = self.db.installed_schema().ok_or_else(schema_not_bound)?;
        let (id, descriptor) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            true,
        )?;
        let rows = DynamicRelationManager::new_canonical(
            self.db.inner_orm(),
            Arc::new(descriptor.clone()),
        )
        .get_by_iid_exact(iid)
        .await
        .map_err(Error::from_orm)?;
        match one_coalesced_row(rows)? {
            None => Ok(None),
            Some(row) => {
                let hydrated = hydrate_relation(row, &id, installed)?;
                let value =
                    M::materialize(&hydrated, &HydrationCapability::new()).map_err(|error| {
                        crate::entity_codec::map_validation_error(
                            error,
                            ModelValidationPhase::Hydration,
                        )
                    })?;
                Ok(Some(value))
            }
        }
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
        let rows = DynamicRelationManager::new_canonical(
            self.db.inner_orm(),
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
    fn missing_concrete_row() -> Error {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "missing_concrete_row",
            vec!["iid".into()],
            "discovered relation row is missing",
            None,
        )
    }

    async fn rehydrate_discovered(
        tx: &type_bridge_orm::session::context::TransactionContext,
        identity: &type_bridge_orm::DynamicRelationIdentity,
        installed: &type_bridge_orm::InstalledRuntimeProjection,
    ) -> Result<M::Subtypes> {
        let (child_id, child_descriptor) =
            resolve_discovered_relation(&identity.type_name, installed)?;
        let child = DynamicRelationManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(child_descriptor),
        );
        let rows = child
            .get_by_iid_exact(&identity.iid)
            .await
            .map_err(Error::from_orm)?;
        let row = one_coalesced_row(rows)?.ok_or_else(Self::missing_concrete_row)?;
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
        let manager =
            DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let identity = match manager.discover_by_iid(iid).await {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.close().await;
                return Err(Error::from_orm(error));
            }
        };
        let out = match identity {
            None => None,
            Some(identity) => match Self::rehydrate_discovered(&tx, &identity, installed).await {
                Ok(value) => Some(value),
                Err(error) => {
                    let _ = tx.close().await;
                    return Err(error);
                }
            },
        };
        tx.close().await.map_err(Error::from_orm)?;
        Ok(out)
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
        let manager =
            DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(descriptor));
        let identities = match manager.discover_all().await {
            Ok(value) => value,
            Err(error) => {
                let _ = tx.close().await;
                return Err(Error::from_orm(error));
            }
        };
        let mut out = Vec::with_capacity(identities.len());
        for identity in identities {
            match Self::rehydrate_discovered(&tx, &identity, installed).await {
                Ok(value) => out.push(value),
                Err(error) => {
                    let _ = tx.close().await;
                    return Err(error);
                }
            }
        }
        tx.close().await.map_err(Error::from_orm)?;
        Ok(out)
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
        DynamicRelationManager::new_canonical(self.db.inner_orm(), Arc::new(descriptor))
            .count()
            .await
            .map_err(Error::from_orm)
    }
}
