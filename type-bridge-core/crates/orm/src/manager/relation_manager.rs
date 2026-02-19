//! Generic CRUD manager for TypeDB relations.
//!
//! [`RelationManager`] provides typed insert, fetch, delete, and count
//! operations for relation types, parallel to [`EntityManager`](super::EntityManager).

use std::marker::PhantomData;

use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::relation::TypeBridgeRelation;
use crate::session::backend::{QueryResult, TxType};
use crate::session::Database;

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
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationManager<'db, R> {
    /// Create a new manager for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self {
            db,
            _marker: PhantomData,
        }
    }

    /// Insert a relation and return the assigned IID.
    ///
    /// The relation's IID is also set in-place via [`TypeBridgeRelation::set_iid`].
    pub async fn insert(&self, relation: &mut R) -> Result<String> {
        let typeql = query_builder::build_relation_insert_with_iid::<R>(relation, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "INSERT RELATION");

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
        }
    }

    /// Fetch relations matching the given filters.
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
    pub async fn all(&self) -> Result<Vec<R>> {
        self.get(&[]).await
    }

    /// Delete a specific relation instance.
    pub async fn delete(&self, relation: &R) -> Result<()> {
        let typeql = query_builder::build_relation_delete::<R>(relation, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "DELETE RELATION");
        self.db.execute_raw(&typeql, TxType::Write).await?;
        Ok(())
    }

    /// Count all relations of this type.
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Count relations matching the given filters.
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let typeql = query_builder::build_relation_count::<R>(filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "COUNT RELATION");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
    }
}
