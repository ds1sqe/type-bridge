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
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, TypeKind};
use crate::query::RelationQuery;
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

use super::hydration::{extract_count, hydrate_relation};
use super::query_builder;

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
/// let manager = RelationManager::<Employment>::new(&db);
/// manager.insert(&mut employment).await?;
/// let rels = manager.all().await?;
/// ```
pub struct RelationManager<'db, R: TypeBridgeRelation> {
    db: &'db Database,
    hooks: HookRunner,
    dynamic: Option<DynamicRelationManager<'db>>,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationManager<'db, R> {
    /// Create a new manager for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self {
            db,
            hooks: HookRunner::new(),
            dynamic: None,
            _marker: PhantomData,
        }
    }

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
            dynamic: Some(DynamicRelationManager::from_registry(
                db, registry, descriptor,
            )),
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

    /// Insert a relation and return the assigned IID.
    ///
    /// The relation's IID is also set in-place via [`TypeBridgeRelation::set_iid`].
    #[tracing::instrument(skip(self, relation), fields(relation_type = R::TYPE_NAME))]
    pub async fn insert(&self, relation: &mut R) -> Result<String> {
        if self.hooks.has_hooks() {
            let mut ctx = HookRunner::build_context(
                R::TYPE_NAME,
                TypeKind::Relation,
                CrudOperation::Insert,
                relation.to_attribute_values(),
                relation.iid().map(String::from),
            );
            self.hooks.run_pre_hooks(&mut ctx).await?;
        }

        let iid = if let Some(manager) = &self.dynamic {
            let iid = manager
                .insert(
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
            relation.set_iid(iid.clone());
            iid
        } else {
            let typeql = query_builder::build_relation_insert_with_iid::<R>(relation, "$r")?;
            tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "INSERT RELATION");

            // raw remainder: no registry authority; removed in 06
            let result = self.db.execute_raw(&typeql, TxType::Write).await?;
            match result {
                QueryResult::Documents(docs) => {
                    let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                        type_name: R::TYPE_NAME.into(),
                        message: "Insert returned no documents".into(),
                    })?;

                    let iid = doc
                        .get("iid")
                        .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                        .ok_or_else(|| OrmError::Hydration {
                            type_name: R::TYPE_NAME.into(),
                            message: "No IID in insert response".into(),
                        })?
                        .to_string();

                    relation.set_iid(iid.clone());
                    Ok(iid)
                }
                QueryResult::Ok => Err(OrmError::Hydration {
                    type_name: R::TYPE_NAME.into(),
                    message: "Expected Documents from insert+fetch, got Ok".into(),
                }),
                QueryResult::Rows(_) => Err(OrmError::Hydration {
                    type_name: R::TYPE_NAME.into(),
                    message: "Expected Documents from insert+fetch, got Rows".into(),
                }),
            }?
        };

        if self.hooks.has_hooks() {
            let ctx = HookRunner::build_context(
                R::TYPE_NAME,
                TypeKind::Relation,
                CrudOperation::Insert,
                relation.to_attribute_values(),
                Some(iid.clone()),
            );
            self.hooks.run_post_hooks(&ctx).await;
        }

        Ok(iid)
    }

    /// Fetch relations matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<R>> {
        if let Some(manager) = &self.dynamic {
            return manager
                .get(filters)
                .await?
                .into_iter()
                .map(materialize_relation_row::<R>)
                .collect();
        }

        let typeql = query_builder::build_relation_fetch::<R>(filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "FETCH RELATION");

        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => {
                docs.iter().map(|doc| hydrate_relation::<R>(doc)).collect()
            }
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: R::TYPE_NAME.into(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
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

        if let Some(manager) = &self.dynamic {
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
        } else {
            let typeql = query_builder::build_relation_delete::<R>(relation, "$r")?;
            tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "DELETE RELATION");
            // raw remainder: no registry authority; removed in 06
            self.db.execute_raw(&typeql, TxType::Write).await?;
        }

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

    /// Delete one instance and return its complete pre-delete snapshot, if present.
    ///
    /// Requires [`Self::from_registry`]. Hydration receives the exact `_type`;
    /// relation snapshots also include `role_players` in `from_document`.
    pub async fn delete_returning(&self, relation: &R) -> Result<Option<R>> {
        Ok(self
            .delete_many_returning(std::slice::from_ref(relation))
            .await?
            .pop())
    }

    /// Capture every requested instance before deleting any, preserving input order.
    ///
    /// Missing instances are omitted. Duplicate IIDs are refused. Snapshot
    /// decoding finishes before commit; post-hooks run only after commit succeeds.
    /// Requires [`Self::from_registry`].
    pub async fn delete_many_returning(&self, relations: &[R]) -> Result<Vec<R>> {
        let manager = self.dynamic.as_ref().ok_or_else(|| {
            crate::plan_builders::manager::invalid(
                "manager_plan_authority_unavailable",
                "Deletion snapshots require a registry-backed manager",
            )
        })?;
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
        let query = RelationQuery::new(self.db);
        match &self.dynamic {
            Some(manager) => query.with_manager(manager.clone()),
            None => query,
        }
    }

    /// Count relations matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        if let Some(manager) = &self.dynamic {
            return manager.count_with_filters(filters).await;
        }

        let typeql = query_builder::build_relation_count::<R>(filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "COUNT RELATION");
        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
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
        let dynamic = self
            .dynamic
            .as_ref()
            .map(|manager| manager.in_transaction(tx.clone()));
        let mut iids = Vec::with_capacity(relations.len());

        if let Some(manager) = &dynamic {
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
            iids = match manager.insert_many(&items).await {
                Ok(iids) => iids,
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            };
            for (relation, iid) in relations.iter_mut().zip(&iids) {
                relation.set_iid(iid.clone());
            }
        } else {
            for relation in relations.iter_mut() {
                let typeql = query_builder::build_relation_insert_with_iid::<R>(relation, "$r")?;
                tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "INSERT RELATION BATCH");

                // raw remainder: no registry authority; removed in 06
                let result = tx.query(&typeql).await?;
                match result {
                    QueryResult::Documents(docs) => {
                        let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                            type_name: R::TYPE_NAME.into(),
                            message: "Insert returned no documents".into(),
                        })?;
                        let iid = doc
                            .get("iid")
                            .and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str()))
                            .ok_or_else(|| OrmError::Hydration {
                                type_name: R::TYPE_NAME.into(),
                                message: "No IID in insert response".into(),
                            })?
                            .to_string();
                        relation.set_iid(iid.clone());
                        iids.push(iid);
                    }
                    _ => {
                        return Err(OrmError::Hydration {
                            type_name: R::TYPE_NAME.into(),
                            message: "Expected Documents from insert+fetch".into(),
                        });
                    }
                }
            }
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
        let dynamic = self
            .dynamic
            .as_ref()
            .map(|manager| manager.in_transaction(tx.clone()));
        if let Some(manager) = &dynamic {
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
        } else {
            for relation in relations {
                let typeql = query_builder::build_relation_delete::<R>(relation, "$r")?;
                tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "DELETE RELATION BATCH");
                // raw remainder: no registry authority; removed in 06
                tx.query(&typeql).await?;
            }
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
