//! Generic CRUD manager for TypeDB relations.
//!
//! [`RelationManager`] provides typed insert, fetch, delete, and count
//! operations for relation types, parallel to [`EntityManager`](super::EntityManager).

use std::marker::PhantomData;
use std::sync::Arc;

use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::hooks::{CrudOperation, HookRunner, LifecycleHook, TypeKind};
use crate::query::RelationQuery;
use crate::relation::TypeBridgeRelation;
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
/// ```ignore
/// let manager = RelationManager::<Employment>::new(&db);
/// manager.insert(&mut employment).await?;
/// let rels = manager.all().await?;
/// ```
pub struct RelationManager<'db, R: TypeBridgeRelation> {
    db: &'db Database,
    hooks: HookRunner,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationManager<'db, R> {
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

        let typeql = query_builder::build_relation_insert_with_iid::<R>(relation, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "INSERT RELATION");

        let result = self.db.execute_raw(&typeql, TxType::Write).await?;
        let iid = match result {
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
        }?;

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
        let typeql = query_builder::build_relation_fetch::<R>(filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "FETCH RELATION");

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

        let typeql = query_builder::build_relation_delete::<R>(relation, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "DELETE RELATION");
        self.db.execute_raw(&typeql, TxType::Write).await?;

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

    /// Count all relations of this type.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Create a chainable query builder for this relation type.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let recent = manager.query()
    ///     .filter(Expr::gte("start-date", AttributeValue::Date("2024-01-01".into())))
    ///     .order_by("start-date", SortDir::Desc)
    ///     .limit(5)
    ///     .execute().await?;
    /// ```
    pub fn query(&self) -> RelationQuery<'db, R> {
        RelationQuery::new(self.db)
    }

    /// Count relations matching the given filters.
    #[tracing::instrument(skip(self, filters), fields(relation_type = R::TYPE_NAME))]
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let typeql = query_builder::build_relation_count::<R>(filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "COUNT RELATION");
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
        let mut iids = Vec::with_capacity(relations.len());

        for relation in relations.iter_mut() {
            let typeql = query_builder::build_relation_insert_with_iid::<R>(relation, "$r")?;
            tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "INSERT RELATION BATCH");

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
        for relation in relations {
            let typeql = query_builder::build_relation_delete::<R>(relation, "$r")?;
            tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "DELETE RELATION BATCH");
            tx.query(&typeql).await?;
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
