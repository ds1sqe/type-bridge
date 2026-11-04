//! Generic CRUD manager for TypeDB entities.
//!
//! [`EntityManager`] provides typed insert, fetch, delete, and count
//! operations backed by the session layer.

use std::marker::PhantomData;
use std::sync::Arc;

use super::dynamic::DynamicEntityManager;
use super::hydration::materialize_entity_row;
use crate::_dynamic::DynamicAttributeMap;
use crate::_entity::TypeBridgeEntity;
use crate::_registry::DescriptorRegistry;
use crate::WriteResult;
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, TypeKind};
use crate::plan_builders::bulk_write::WriteKind;
use crate::query::EntityQuery;
use crate::session::Database;
use crate::session::backend::TxType;

/// Generic CRUD manager for a specific entity type.
///
/// Wraps a [`Database`] reference and provides typed operations for
/// inserting, fetching, deleting, and counting entities.
///
/// # Example
///
/// This example is ignored because it requires a live TypeDB service and
/// generated `Person` model values from a consumer package.
///
/// ```ignore
/// let manager = EntityManager::<Person>::from_registry(&db, registry)?;
/// manager.insert(&mut alice).await?;
/// let people = manager.all().await?;
/// ```
pub struct EntityManager<'db, T: TypeBridgeEntity> {
    db: &'db Database,
    hooks: HookRunner,
    dynamic: DynamicEntityManager<'db>,
    _marker: PhantomData<T>,
}

impl<'db, T: TypeBridgeEntity> EntityManager<'db, T> {
    /// Create a manager whose operations use native plans and installed registry authority.
    pub fn from_registry(db: &'db Database, registry: Arc<DescriptorRegistry>) -> Result<Self> {
        if registry.installed_schema_authority().is_none() {
            return Err(crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "A registry-backed manager requires installed schema authority",
            ));
        }
        let descriptor = registry.entity(T::TYPE_NAME)?;
        Ok(Self {
            db,
            hooks: HookRunner::new(),
            dynamic: DynamicEntityManager::from_registry(db, registry, descriptor),
            _marker: PhantomData,
        })
    }

    /// Register a lifecycle hook.
    ///
    /// Hooks run in registration order for pre-hooks and reverse order
    /// for post-hooks. Returns `&mut Self` for chaining.
    pub fn add_hook(&mut self, hook: Arc<dyn LifecycleHook>) -> &mut Self {
        self.hooks.add_hook(hook);
        self
    }

    /// Insert an entity and return every hydrated root.
    ///
    /// The input IID is attached only when exactly one root is returned.
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn insert(&self, entity: &mut T) -> Result<WriteResult<T>> {
        if self.hooks.has_hooks() {
            let mut context = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Insert,
                entity.to_attribute_values(),
                entity.iid().map(str::to_owned),
            );
            self.hooks.run_pre_hooks(&mut context).await?;
        }
        let manager = &self.dynamic;
        let attributes = entity
            .to_attribute_values()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect::<DynamicAttributeMap>();
        let result = manager
            .write_mapped(WriteKind::Insert, None, &attributes, |rows| {
                rows.into_items()
                    .into_iter()
                    .map(materialize_entity_row::<T>)
                    .collect::<Result<Vec<_>>>()
                    .map(WriteResult::new)
            })
            .await?;
        if let Some(item) = result.item()
            && let Some(iid) = item.iid()
        {
            entity.set_iid(iid.to_owned());
        }
        if self.hooks.has_hooks() {
            for item in result.items() {
                let context = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Insert,
                    item.to_attribute_values(),
                    item.iid().map(str::to_owned),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(result)
    }

    /// Fetch entities matching the given filters.
    ///
    /// Returns an empty vec if no entities match.
    #[tracing::instrument(skip(self, filters), fields(entity_type = T::TYPE_NAME))]
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<T>> {
        let manager = &self.dynamic;
        return manager
            .get(filters)
            .await?
            .into_iter()
            .map(materialize_entity_row::<T>)
            .collect();
    }

    /// Fetch exactly one entity matching the filters.
    ///
    /// Returns [`OrmError::NotFound`] if no match, or a hydration error
    /// if more than one entity matches.
    #[tracing::instrument(skip(self, filters), fields(entity_type = T::TYPE_NAME))]
    pub async fn get_one(&self, filters: &[Filter]) -> Result<T> {
        let results = self.get(filters).await?;
        match results.len() {
            0 => Err(OrmError::NotFound(format!(
                "No {} matching filters",
                T::TYPE_NAME
            ))),
            1 => Ok(results.into_iter().next().unwrap()),
            n => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: format!("Expected 1 result, got {n}"),
            }),
        }
    }

    /// Fetch all entities of this type.
    #[tracing::instrument(skip(self), fields(entity_type = T::TYPE_NAME))]
    pub async fn all(&self) -> Result<Vec<T>> {
        self.get(&[]).await
    }

    /// Delete a specific entity instance.
    ///
    /// Identification uses the entity's IID (if available) or its @key
    /// attributes for matching.
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn delete(&self, entity: &T) -> Result<()> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Delete,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let manager = &self.dynamic;
        manager
            .delete_identity(
                entity.iid(),
                &entity
                    .to_attribute_values()
                    .into_iter()
                    .map(|(name, value)| (name.into(), value))
                    .collect::<DynamicAttributeMap>(),
            )
            .await?;

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Delete,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(())
    }

    /// Delete one instance and return every complete pre-delete snapshot.
    ///
    /// Requires [`Self::from_registry`]. Hydration receives the exact `_type`;
    /// relation snapshots also include `role_players` in `from_document`.
    pub async fn delete_returning(&self, entity: &T) -> Result<WriteResult<T>> {
        self.delete_many_returning(std::slice::from_ref(entity))
            .await
            .map(WriteResult::new)
    }

    /// Capture every requested instance before deleting any, preserving input order.
    ///
    /// Missing instances are omitted. Duplicate IIDs are refused. Snapshot
    /// decoding finishes before commit; post-hooks run only after commit succeeds.
    /// Requires [`Self::from_registry`].
    pub async fn delete_many_returning(&self, entities: &[T]) -> Result<Vec<T>> {
        let manager = &self.dynamic;
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        if self.hooks.has_hooks() {
            for entity in entities {
                let mut context = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Delete,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut context).await?;
            }
        }
        let tx = self.db.transaction_context(TxType::Write).await?;
        let manager = manager.in_transaction(tx.clone());
        let result = async {
            let materialize = |rows: Vec<crate::_dynamic::DynamicEntityRow>| {
                rows.into_iter()
                    .map(materialize_entity_row::<T>)
                    .collect::<Result<Vec<_>>>()
            };
            if let Some(iids) = entities
                .iter()
                .map(|entity| entity.iid().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
            {
                manager.delete_snapshots(&iids, materialize).await
            } else {
                let items = entities
                    .iter()
                    .map(|entity| {
                        (
                            entity.iid().map(str::to_owned),
                            if entity.iid().is_some() {
                                vec![]
                            } else {
                                entity
                                    .to_attribute_values()
                                    .into_iter()
                                    .map(|(name, value)| (name.into(), value))
                                    .collect::<DynamicAttributeMap>()
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                manager.delete_identity_snapshots(&items, materialize).await
            }
        }
        .await;
        let snapshots = match result {
            Ok(snapshots) => {
                super::returning::commit(&tx).await?;
                snapshots
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        if self.hooks.has_hooks() {
            for entity in entities {
                let context = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Delete,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(snapshots)
    }

    /// Count all entities of this type.
    #[tracing::instrument(skip(self), fields(entity_type = T::TYPE_NAME))]
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Update an entity's non-key attributes in the database.
    ///
    /// Identifies the entity by IID or @key attributes, then updates
    /// all other attribute values. Only non-key attributes are modified.
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn update(&self, entity: &T) -> Result<WriteResult<T>> {
        if self.hooks.has_hooks() {
            let mut context = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Update,
                entity.to_attribute_values(),
                entity.iid().map(str::to_owned),
            );
            self.hooks.run_pre_hooks(&mut context).await?;
        }
        let manager = &self.dynamic;
        let attributes = entity
            .to_attribute_values()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect::<DynamicAttributeMap>();
        let result = manager
            .write_mapped(
                WriteKind::PartialUpdate,
                entity.iid(),
                &attributes,
                |rows| {
                    rows.into_items()
                        .into_iter()
                        .map(materialize_entity_row::<T>)
                        .collect::<Result<Vec<_>>>()
                        .map(WriteResult::new)
                },
            )
            .await?;
        if self.hooks.has_hooks() {
            for item in result.items() {
                let context = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Update,
                    item.to_attribute_values(),
                    item.iid().map(str::to_owned),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(result)
    }

    /// Idempotent insert-or-update (put) for an entity.
    ///
    /// If a matching entity exists, updates it. Otherwise inserts a new one.
    /// Returns every complete hydrated root in a write result.
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn put(&self, entity: &mut T) -> Result<WriteResult<T>> {
        if self.hooks.has_hooks() {
            let mut context = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Put,
                entity.to_attribute_values(),
                entity.iid().map(str::to_owned),
            );
            self.hooks.run_pre_hooks(&mut context).await?;
        }
        let manager = &self.dynamic;
        let attributes = entity
            .to_attribute_values()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect::<DynamicAttributeMap>();
        let result = manager
            .write_mapped(WriteKind::PartialPut, None, &attributes, |rows| {
                rows.into_items()
                    .into_iter()
                    .map(materialize_entity_row::<T>)
                    .collect::<Result<Vec<_>>>()
                    .map(WriteResult::new)
            })
            .await?;
        if let Some(item) = result.item()
            && let Some(iid) = item.iid()
        {
            entity.set_iid(iid.to_owned());
        }
        if self.hooks.has_hooks() {
            for item in result.items() {
                let context = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Put,
                    item.to_attribute_values(),
                    item.iid().map(str::to_owned),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(result)
    }

    /// Create a chainable query builder for this entity type.
    ///
    /// # Example
    ///
    /// This example is ignored because it requires a live TypeDB service and
    /// a generated entity type bound by the surrounding manager.
    ///
    /// ```ignore
    /// let adults = manager.query()
    ///     .filter(Expr::gte("age", AttributeValue::Long(18)))
    ///     .order_by("name", SortDir::Asc)
    ///     .limit(10)
    ///     .execute().await?;
    /// ```
    pub fn query(&self) -> EntityQuery<'db, T> {
        EntityQuery::from_manager(self.dynamic.clone())
    }

    /// Count entities matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(entity_type = T::TYPE_NAME))]
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let manager = &self.dynamic;
        return manager.count_with_filters(filters).await;
    }

    /// Insert multiple entities in a single transaction.
    ///
    /// Each entity's IID is set in-place. Returns a vector of assigned IIDs.
    /// Pre-hooks run for ALL entities before the transaction starts; if any
    /// rejects, the entire batch aborts. Post-hooks run after commit.
    #[tracing::instrument(skip(self, entities), fields(entity_type = T::TYPE_NAME, count = entities.len()))]
    pub async fn insert_many(&self, entities: &mut [T]) -> Result<Vec<String>> {
        // Pre-hooks for all entities before the transaction.
        if self.hooks.has_hooks() {
            for entity in entities.iter() {
                let mut ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Insert,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut ctx).await?;
            }
        }

        let tx = self.db.transaction_context(TxType::Write).await?;
        let dynamic = self.dynamic.in_transaction(tx.clone());

        let manager = &dynamic;
        let items = entities
            .iter()
            .map(|entity| {
                entity
                    .to_attribute_values()
                    .into_iter()
                    .map(|(name, value)| (name.into(), value))
                    .collect::<DynamicAttributeMap>()
            })
            .collect::<Vec<_>>();
        let iids = match manager.insert_many(&items).await {
            Ok(iids) => iids,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        for (entity, iid) in entities.iter_mut().zip(&iids) {
            entity.set_iid(iid.clone());
        }

        tx.commit().await?;

        // Post-hooks after successful commit.
        if self.hooks.has_hooks() {
            for entity in entities.iter() {
                let ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Insert,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&ctx).await;
            }
        }

        Ok(iids)
    }

    /// Delete multiple entities in a single transaction.
    ///
    /// Pre-hooks run for ALL entities before the transaction. Post-hooks
    /// run after commit.
    #[tracing::instrument(skip(self, entities), fields(entity_type = T::TYPE_NAME, count = entities.len()))]
    pub async fn delete_many(&self, entities: &[T]) -> Result<()> {
        if self.hooks.has_hooks() {
            for entity in entities {
                let mut ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Delete,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut ctx).await?;
            }
        }

        let tx = self.db.transaction_context(TxType::Write).await?;
        let dynamic = self.dynamic.in_transaction(tx.clone());
        let manager = &dynamic;
        let items = entities
            .iter()
            .map(|entity| {
                (
                    entity.iid().map(str::to_owned),
                    entity
                        .to_attribute_values()
                        .into_iter()
                        .map(|(name, value)| (name.into(), value))
                        .collect::<DynamicAttributeMap>(),
                )
            })
            .collect::<Vec<_>>();
        if let Err(error) = manager
            .mutate_many_identities(&items, crate::plan_builders::bulk_write::WriteKind::Delete)
            .await
        {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await?;

        if self.hooks.has_hooks() {
            for entity in entities {
                let ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Delete,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&ctx).await;
            }
        }

        Ok(())
    }

    /// Update multiple entities in a single transaction.
    ///
    /// Pre-hooks run for ALL entities before the transaction. Post-hooks
    /// run after commit.
    #[tracing::instrument(skip(self, entities), fields(entity_type = T::TYPE_NAME, count = entities.len()))]
    pub async fn update_many(&self, entities: &[T]) -> Result<()> {
        if self.hooks.has_hooks() {
            for entity in entities {
                let mut ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Update,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut ctx).await?;
            }
        }

        let tx = self.db.transaction_context(TxType::Write).await?;
        let dynamic = self.dynamic.in_transaction(tx.clone());
        let manager = &dynamic;
        let items = entities
            .iter()
            .map(|entity| {
                (
                    entity.iid().map(str::to_owned),
                    entity
                        .to_attribute_values()
                        .into_iter()
                        .map(|(name, value)| (name.into(), value))
                        .collect::<DynamicAttributeMap>(),
                )
            })
            .collect::<Vec<_>>();
        if let Err(error) = manager
            .mutate_many_identities(
                &items,
                crate::plan_builders::bulk_write::WriteKind::PartialUpdate,
            )
            .await
        {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await?;

        if self.hooks.has_hooks() {
            for entity in entities {
                let ctx = HookRunner::build_context(
                    T::TYPE_NAME,
                    TypeKind::Entity,
                    CrudOperation::Update,
                    entity.to_attribute_values(),
                    entity.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&ctx).await;
            }
        }

        Ok(())
    }
}
