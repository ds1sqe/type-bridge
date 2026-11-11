//! Generic CRUD manager for TypeDB relations.
//!
//! [`RelationManager`] provides typed insert, fetch, delete, and count
//! operations for relation types, parallel to [`EntityManager`](super::EntityManager).

use std::marker::PhantomData;
use std::sync::Arc;

use super::dynamic::DynamicRelationManager;
use super::hydration::materialize_relation_row;
use crate::_dynamic::DynamicAttributeMap;
use crate::_registry::DescriptorRegistry;
use crate::_relation::TypeBridgeRelation;
use crate::WriteResult;
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, TypeKind};
use crate::plan_builders::bulk_write::WriteKind;
use crate::query::RelationQuery;
use crate::session::Database;
use crate::session::backend::TxType;

/// Generic CRUD manager for a specific relation type.
///
/// Wraps a [`Database`] reference and provides typed operations for
/// inserting, fetching, deleting, and counting relations.
///
/// # Example
///
/// This example is ignored because it requires a live TypeDB service and a
/// generated `Employment` relation from a consumer package.
///
/// ```ignore
/// let manager = RelationManager::<Employment>::from_registry(&db, registry)?;
/// manager.insert(&mut employment).await?;
/// let rels = manager.all().await?;
/// ```
pub struct RelationManager<'db, R: TypeBridgeRelation> {
    db: &'db Database,
    hooks: HookRunner,
    dynamic: DynamicRelationManager<'db>,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationManager<'db, R> {
    /// Create a manager whose operations use native plans and installed registry authority.
    pub fn from_registry(db: &'db Database, registry: Arc<DescriptorRegistry>) -> Result<Self> {
        if registry.installed_schema_authority().is_none() {
            return Err(crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "A registry-backed manager requires installed schema authority",
            ));
        }
        let descriptor = registry.relation(R::TYPE_NAME)?;
        Ok(Self {
            db,
            hooks: HookRunner::new(),
            dynamic: DynamicRelationManager::from_registry(db, registry, descriptor),
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

    /// Insert a relation and return every hydrated root.
    ///
    /// The input IID is attached only when exactly one root is returned.
    #[tracing::instrument(skip(self, relation), fields(relation_type = R::TYPE_NAME))]
    pub async fn insert(&self, relation: &mut R) -> Result<WriteResult<R>> {
        if self.hooks.has_hooks() {
            let mut context = HookRunner::build_context(
                R::TYPE_NAME,
                TypeKind::Relation,
                CrudOperation::Insert,
                relation.to_attribute_values(),
                relation.iid().map(str::to_owned),
            );
            self.hooks.run_pre_hooks(&mut context).await?;
        }
        let manager = &self.dynamic;
        let attributes = relation
            .to_attribute_values()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect::<DynamicAttributeMap>();
        let players = relation
            .to_role_player_refs()
            .into_iter()
            .map(|player| crate::_dynamic::DynamicRolePlayerInput {
                role_name: player.role.into(),
                player_type_name: player.entity_type_name.into(),
                iid: player.iid,
                key: player.key.map(|(name, value)| (name.into(), value)),
            })
            .collect::<Vec<_>>();
        let result = manager
            .write_mapped(WriteKind::Insert, None, &attributes, &players, |rows| {
                rows.into_items()
                    .into_iter()
                    .map(materialize_relation_row::<R>)
                    .collect::<Result<Vec<_>>>()
                    .map(WriteResult::new)
            })
            .await?;
        if let Some(item) = result.item()
            && let Some(iid) = item.iid()
        {
            relation.set_iid(iid.to_owned());
        }
        if self.hooks.has_hooks() {
            for item in result.items() {
                let context = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Insert,
                    item.to_attribute_values(),
                    item.iid().map(str::to_owned),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(result)
    }

    /// Fetch relations matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<R>> {
        let manager = &self.dynamic;
        return manager
            .get(filters)
            .await?
            .into_iter()
            .map(materialize_relation_row::<R>)
            .collect();
    }

    /// Fetch exactly one relation matching the filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn get_one(&self, filters: &[Filter]) -> Result<R> {
        let results = self.get(filters).await?;
        match results.len() {
            0 => Err(OrmError::NotFound(format!(
                "No {} matching filters",
                R::TYPE_NAME
            ))),
            1 => Ok(results.into_iter().next().unwrap()),
            n => Err(OrmError::Hydration {
                type_name: R::TYPE_NAME.into(),
                message: format!("Expected 1 result, got {n}"),
            }),
        }
    }

    /// Fetch all relations of this type.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn all(&self) -> Result<Vec<R>> {
        self.get(&[]).await
    }

    /// Delete a specific relation instance.
    #[tracing::instrument(skip(self, relation), fields(relation_type = R::TYPE_NAME))]
    pub async fn delete(&self, relation: &R) -> Result<()> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                R::TYPE_NAME,
                TypeKind::Relation,
                CrudOperation::Delete,
                relation.to_attribute_values(),
                relation.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let manager = &self.dynamic;
        manager
            .delete_identity(
                relation.iid(),
                &relation
                    .to_attribute_values()
                    .into_iter()
                    .map(|(name, value)| (name.into(), value))
                    .collect::<DynamicAttributeMap>(),
                &relation
                    .to_role_player_refs()
                    .into_iter()
                    .map(|player| crate::_dynamic::DynamicRolePlayerInput {
                        role_name: player.role.into(),
                        player_type_name: player.entity_type_name.into(),
                        iid: player.iid,
                        key: player.key.map(|(name, value)| (name.into(), value)),
                    })
                    .collect::<Vec<_>>(),
            )
            .await?;

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                R::TYPE_NAME,
                TypeKind::Relation,
                CrudOperation::Delete,
                relation.to_attribute_values(),
                relation.iid().map(String::from),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(())
    }

    /// Delete one instance and return every complete pre-delete snapshot.
    ///
    /// Requires [`Self::from_registry`]. Hydration receives the exact `_type`;
    /// relation snapshots also include `role_players` in `from_document`.
    pub async fn delete_returning(&self, relation: &R) -> Result<WriteResult<R>> {
        self.delete_many_returning(std::slice::from_ref(relation))
            .await
            .map(WriteResult::new)
    }

    /// Capture every requested instance before deleting any, preserving input order.
    ///
    /// Missing instances are omitted. Duplicate IIDs are refused. Snapshot
    /// decoding finishes before commit; post-hooks run only after commit succeeds.
    /// Requires [`Self::from_registry`].
    pub async fn delete_many_returning(&self, relations: &[R]) -> Result<Vec<R>> {
        let manager = &self.dynamic;
        if relations.is_empty() {
            return Ok(Vec::new());
        }
        if self.hooks.has_hooks() {
            for relation in relations {
                let mut context = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Delete,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut context).await?;
            }
        }
        let tx = self.db.transaction_context(TxType::Write).await?;
        let manager = manager.in_transaction(tx.clone());
        let result = async {
            let materialize = |rows: Vec<crate::_dynamic::DynamicRelationRow>| {
                rows.into_iter()
                    .map(materialize_relation_row::<R>)
                    .collect::<Result<Vec<_>>>()
            };
            if let Some(iids) = relations
                .iter()
                .map(|relation| relation.iid().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
            {
                manager.delete_snapshots(&iids, materialize).await
            } else {
                let items = relations
                    .iter()
                    .map(|relation| {
                        (
                            relation.iid().map(str::to_owned),
                            if relation.iid().is_some() {
                                vec![]
                            } else {
                                relation
                                    .to_attribute_values()
                                    .into_iter()
                                    .map(|(name, value)| (name.into(), value))
                                    .collect::<DynamicAttributeMap>()
                            },
                            if relation.iid().is_some() {
                                vec![]
                            } else {
                                relation
                                    .to_role_player_refs()
                                    .into_iter()
                                    .map(|player| crate::_dynamic::DynamicRolePlayerInput {
                                        role_name: player.role.into(),
                                        player_type_name: player.entity_type_name.into(),
                                        iid: player.iid,
                                        key: player.key.map(|(name, value)| (name.into(), value)),
                                    })
                                    .collect()
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
            for relation in relations {
                let context = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Delete,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&context).await;
            }
        }
        Ok(snapshots)
    }

    /// Count all relations of this type.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Create a chainable query builder for this relation type.
    ///
    /// # Example
    ///
    /// This example is ignored because it requires a live TypeDB service and
    /// a generated relation type bound by the surrounding manager.
    ///
    /// ```ignore
    /// let recent = manager.query()
    ///     .filter(Expr::gte("start-date", AttributeValue::Date("2024-01-01".into())))
    ///     .order_by("start-date", SortDir::Desc)
    ///     .limit(5)
    ///     .execute().await?;
    /// ```
    pub fn query(&self) -> RelationQuery<'db, R> {
        RelationQuery::from_manager(self.dynamic.clone())
    }

    /// Count relations matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let manager = &self.dynamic;
        return manager.count_with_filters(filters).await;
    }

    /// Insert multiple relations in a single transaction.
    ///
    /// Each relation's IID is set in-place. Returns a vector of assigned IIDs.
    /// Pre-hooks run for ALL relations before the transaction starts; if any
    /// rejects, the entire batch aborts. Post-hooks run after commit.
    #[tracing::instrument(skip(self, relations), fields(relation_type = R::TYPE_NAME, count = relations.len()))]
    pub async fn insert_many(&self, relations: &mut [R]) -> Result<Vec<String>> {
        if self.hooks.has_hooks() {
            for relation in relations.iter() {
                let mut ctx = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Insert,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut ctx).await?;
            }
        }

        let tx = self.db.transaction_context(TxType::Write).await?;
        let dynamic = self.dynamic.in_transaction(tx.clone());

        let manager = &dynamic;
        let items = relations
            .iter()
            .map(|relation| {
                (
                    relation
                        .to_attribute_values()
                        .into_iter()
                        .map(|(name, value)| (name.into(), value))
                        .collect::<DynamicAttributeMap>(),
                    relation
                        .to_role_player_refs()
                        .into_iter()
                        .map(|player| crate::_dynamic::DynamicRolePlayerInput {
                            role_name: player.role.into(),
                            player_type_name: player.entity_type_name.into(),
                            iid: player.iid,
                            key: player.key.map(|(name, value)| (name.into(), value)),
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let iids = match manager.insert_many(&items).await {
            Ok(iids) => iids,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        for (relation, iid) in relations.iter_mut().zip(&iids) {
            relation.set_iid(iid.clone());
        }

        tx.commit().await?;

        if self.hooks.has_hooks() {
            for relation in relations.iter() {
                let ctx = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Insert,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&ctx).await;
            }
        }

        Ok(iids)
    }

    /// Delete multiple relations in a single transaction.
    ///
    /// Pre-hooks run for ALL relations before the transaction. Post-hooks
    /// run after commit.
    #[tracing::instrument(skip(self, relations), fields(relation_type = R::TYPE_NAME, count = relations.len()))]
    pub async fn delete_many(&self, relations: &[R]) -> Result<()> {
        if self.hooks.has_hooks() {
            for relation in relations {
                let mut ctx = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Delete,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_pre_hooks(&mut ctx).await?;
            }
        }

        let tx = self.db.transaction_context(TxType::Write).await?;
        let dynamic = self.dynamic.in_transaction(tx.clone());
        let manager = &dynamic;
        let items = relations
            .iter()
            .map(|relation| {
                (
                    relation.iid().map(str::to_owned),
                    relation
                        .to_attribute_values()
                        .into_iter()
                        .map(|(name, value)| (name.into(), value))
                        .collect::<DynamicAttributeMap>(),
                    relation
                        .to_role_player_refs()
                        .into_iter()
                        .map(|player| crate::_dynamic::DynamicRolePlayerInput {
                            role_name: player.role.into(),
                            player_type_name: player.entity_type_name.into(),
                            iid: player.iid,
                            key: player.key.map(|(name, value)| (name.into(), value)),
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        if let Err(error) = manager.delete_many_identities(&items).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await?;

        if self.hooks.has_hooks() {
            for relation in relations {
                let ctx = HookRunner::build_context(
                    R::TYPE_NAME,
                    TypeKind::Relation,
                    CrudOperation::Delete,
                    relation.to_attribute_values(),
                    relation.iid().map(String::from),
                );
                self.hooks.run_post_hooks(&ctx).await;
            }
        }

        Ok(())
    }
}
