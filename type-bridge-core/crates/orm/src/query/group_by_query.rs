//! Group-by query builders for entities and relations.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::_entity::TypeBridgeEntity;
use crate::_manager::query_builder;
use crate::_relation::TypeBridgeRelation;
use crate::error::{OrmError, Result};
use crate::expr::{Agg, AggResult, GroupByResult};
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

/// A group-by query builder for entity types.
///
/// Created via [`EntityQuery::group_by()`](crate::query::EntityQuery::group_by).
/// Can only be finalized via `.aggregate()`.
///
/// # Example
///
/// This example is ignored because executing it requires a live TypeDB
/// service and generated `Person` fields supplied by a consumer package.
///
/// ```ignore
/// let results = manager.query()
///     .filter(Person::fields().age.gte(Age(18)))
///     .group_by("department")
///     .aggregate(&[Person::fields().age.mean()])
///     .await?;
/// ```
pub struct GroupByEntityQuery<'db, T: TypeBridgeEntity> {
    db: &'db Database,
    filters: Vec<crate::expr::Expr>,
    group_field: String,
    _marker: PhantomData<T>,
}

impl<'db, T: TypeBridgeEntity> GroupByEntityQuery<'db, T> {
    /// Create a new group-by query builder.
    pub fn new(db: &'db Database, filters: Vec<crate::expr::Expr>, group_field: String) -> Self {
        Self {
            db,
            filters,
            group_field,
            _marker: PhantomData,
        }
    }

    /// Execute the grouped aggregation and return results.
    #[tracing::instrument(skip(self, aggs), fields(entity_type = T::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<GroupByResult> {
        let typeql = query_builder::build_expr_group_by_aggregate::<T>(
            &self.filters,
            &self.group_field,
            aggs,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "GROUP BY AGGREGATE");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        parse_group_by_result::<T>(result)
    }
}

/// A group-by query builder for relation types.
///
/// Created via [`RelationQuery::group_by()`](crate::query::RelationQuery::group_by).
/// Can only be finalized via `.aggregate()`.
pub struct GroupByRelationQuery<'db, R: TypeBridgeRelation> {
    db: &'db Database,
    filters: Vec<crate::expr::Expr>,
    group_field: String,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> GroupByRelationQuery<'db, R> {
    /// Create a new group-by query builder.
    pub fn new(db: &'db Database, filters: Vec<crate::expr::Expr>, group_field: String) -> Self {
        Self {
            db,
            filters,
            group_field,
            _marker: PhantomData,
        }
    }

    /// Execute the grouped aggregation and return results.
    #[tracing::instrument(skip(self, aggs), fields(relation_type = R::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<GroupByResult> {
        let typeql = query_builder::build_relation_group_by_aggregate::<R>(
            &self.filters,
            &self.group_field,
            aggs,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "GROUP BY AGGREGATE RELATION");
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        parse_group_by_result::<R>(result)
    }
}

/// Parse grouped aggregation results from `QueryResult::Rows`.
fn parse_group_by_result<T: 'static>(result: QueryResult) -> Result<GroupByResult> {
    let type_name = std::any::type_name::<T>();
    match result {
        QueryResult::Rows(rows) => {
            let mut groups = Vec::new();
            for row in &rows {
                if let Some(obj) = row.as_object() {
                    let group_key = obj
                        .get("$group0")
                        .or_else(|| obj.get("$_group0"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let mut agg_map = HashMap::new();
                    for (k, v) in obj {
                        if k != "$group0" && k != "$_group0" {
                            agg_map.insert(k.clone(), v.clone());
                        }
                    }
                    groups.push((group_key, AggResult::new(agg_map)));
                }
            }
            Ok(GroupByResult::new(groups))
        }
        _ => Err(OrmError::Hydration {
            type_name: type_name.into(),
            message: "Expected Rows from group-by reduce query".into(),
        }),
    }
}
