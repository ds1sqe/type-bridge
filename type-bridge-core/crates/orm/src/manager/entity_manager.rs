//! Generic CRUD manager for TypeDB entities.
//!
//! [`EntityManager`] provides typed insert, fetch, delete, and count
//! operations backed by the session layer.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::_entity::TypeBridgeEntity;
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, TypeKind};
use crate::query::EntityQuery;
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

use super::hydration::{extract_count, hydrate_entity};
use super::query_builder;

/// Generic CRUD manager for a specific entity type.
///
/// Wraps a [`Database`] reference and provides typed operations for
/// inserting, fetching, deleting, and counting entities.
///
/// # Example
///
/// ```ignore
/// let manager = EntityManager::<Person>::new(&db);
/// manager.insert(&mut alice).await?;
/// let people = manager.all().await?;
/// ```
pub struct EntityManager<'db, T: TypeBridgeEntity> {
    db: &'db Database,
    hooks: HookRunner,
    _marker: PhantomData<T>,
}

impl<'db, T: TypeBridgeEntity> EntityManager<'db, T> {
    /// Create a new manager for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self {
            db,
            hooks: HookRunner::new(),
            _marker: PhantomData,
        }
    }

    /// Register a lifecycle hook.
    ///
    /// Hooks run in registration order for pre-hooks and reverse order
    /// for post-hooks. Returns `&mut Self` for chaining.
    pub fn add_hook(&mut self, hook: Arc<dyn LifecycleHook>) -> &mut Self {
        self.hooks.add_hook(hook);
        self
    }

    /// Insert an entity and return the assigned IID.
    ///
    /// The entity's IID is also set in-place via [`TypeBridgeEntity::set_iid`].
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn insert(&self, entity: &mut T) -> Result<String> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Insert,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let typeql = query_builder::build_insert_with_iid::<T>(entity, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "INSERT");

        let result = self.db.execute_raw(&typeql, TxType::Write).await?;
        let iid = match result {
            QueryResult::Documents(docs) => {
                let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                    type_name: T::TYPE_NAME.into(),
                    message: "Insert returned no documents".into(),
                })?;

                let iid = doc
                    .get("iid")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                    .ok_or_else(|| OrmError::Hydration {
                        type_name: T::TYPE_NAME.into(),
                        message: "No IID in insert response".into(),
                    })?
                    .to_string();

                entity.set_iid(iid.clone());
                Ok(iid)
            }
            QueryResult::Ok => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Documents from insert+fetch, got Ok".into(),
            }),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Documents from insert+fetch, got Rows".into(),
            }),
        }?;

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Insert,
                entity.to_attribute_values(),
                Some(iid.clone()),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(iid)
    }

    /// Fetch entities matching the given filters.
    ///
    /// Returns an empty vec if no entities match.
    #[tracing::instrument(skip(self, filters), fields(entity_type = T::TYPE_NAME))]
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<T>> {
        let typeql = query_builder::build_fetch::<T>(filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "FETCH");

        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => {
                docs.iter().map(|doc| hydrate_entity::<T>(doc)).collect()
            }
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
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

        let typeql = query_builder::build_delete::<T>(entity, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "DELETE");
        self.db.execute_raw(&typeql, TxType::Write).await?;

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
    pub async fn update(&self, entity: &T) -> Result<()> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Update,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let typeql = query_builder::build_update::<T>(entity, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "UPDATE");
        self.db.execute_raw(&typeql, TxType::Write).await?;

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Update,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(())
    }

    /// Idempotent insert-or-update (put) for an entity.
    ///
    /// If a matching entity exists, updates it. Otherwise inserts a new one.
    /// Returns the IID of the entity (existing or newly created).
    #[tracing::instrument(skip(self, entity), fields(entity_type = T::TYPE_NAME))]
    pub async fn put(&self, entity: &mut T) -> Result<String> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Put,
                entity.to_attribute_values(),
                entity.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let typeql = query_builder::build_put::<T>(entity, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "PUT");

        let result = self.db.execute_raw(&typeql, TxType::Write).await?;
        let iid = match result {
            QueryResult::Documents(docs) => {
                let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                    type_name: T::TYPE_NAME.into(),
                    message: "Put returned no documents".into(),
                })?;

                let iid = doc
                    .get("iid")
                    .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                    .ok_or_else(|| OrmError::Hydration {
                        type_name: T::TYPE_NAME.into(),
                        message: "No IID in put response".into(),
                    })?
                    .to_string();

                entity.set_iid(iid.clone());
                Ok(iid)
            }
            QueryResult::Ok => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Documents from put+fetch, got Ok".into(),
            }),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Documents from put+fetch, got Rows".into(),
            }),
        }?;

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                T::TYPE_NAME,
                TypeKind::Entity,
                CrudOperation::Put,
                entity.to_attribute_values(),
                Some(iid.clone()),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(iid)
    }

    /// Create a chainable query builder for this entity type.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let adults = manager.query()
    ///     .filter(Expr::gte("age", AttributeValue::Long(18)))
    ///     .order_by("name", SortDir::Asc)
    ///     .limit(10)
    ///     .execute().await?;
    /// ```
    pub fn query(&self) -> EntityQuery<'db, T> {
        EntityQuery::new(self.db)
    }

    /// Count entities matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(entity_type = T::TYPE_NAME))]
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let typeql = query_builder::build_count::<T>(filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "COUNT");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
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
        let mut iids = Vec::with_capacity(entities.len());

        for entity in entities.iter_mut() {
            let typeql = query_builder::build_insert_with_iid::<T>(entity, "$e")?;
            tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "INSERT BATCH");

            let result = tx.query(&typeql).await?;
            match result {
                QueryResult::Documents(docs) => {
                    let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                        type_name: T::TYPE_NAME.into(),
                        message: "Insert returned no documents".into(),
                    })?;
                    let iid = doc
                        .get("iid")
                        .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                        .ok_or_else(|| OrmError::Hydration {
                            type_name: T::TYPE_NAME.into(),
                            message: "No IID in insert response".into(),
                        })?
                        .to_string();
                    entity.set_iid(iid.clone());
                    iids.push(iid);
                }
                _ => {
                    return Err(OrmError::Hydration {
                        type_name: T::TYPE_NAME.into(),
                        message: "Expected Documents from insert+fetch".into(),
                    });
                }
            }
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
        for entity in entities {
            let typeql = query_builder::build_delete::<T>(entity, "$e")?;
            tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "DELETE BATCH");
            tx.query(&typeql).await?;
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
        for entity in entities {
            let typeql = query_builder::build_update::<T>(entity, "$e")?;
            tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "UPDATE BATCH");
            tx.query(&typeql).await?;
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
