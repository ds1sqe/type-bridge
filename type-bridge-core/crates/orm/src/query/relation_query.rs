//! Chainable query builder for relation types.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::_dynamic::DynamicSort;
use crate::_manager::DynamicRelationManager;
use crate::_manager::hydration::materialize_relation_row;
use crate::_manager::hydration::{extract_count, hydrate_relation};
use crate::_manager::query_builder;
use crate::_relation::TypeBridgeRelation;
use crate::error::{OrmError, Result};
use crate::expr::{Agg, AggResult, Expr, SortDir};
use crate::query::group_by_query::GroupByRelationQuery;
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

/// A chainable query builder for relation types.
///
/// Created via [`RelationManager::query()`](crate::_manager::RelationManager::query).
///
/// # Example
///
/// This example is ignored because executing it requires a live TypeDB
/// service and a generated relation manager supplied by a consumer package.
///
/// ```ignore
/// let recent = rel_manager.query()
///     .filter(Expr::gte("start-date", AttributeValue::Date("2024-01-01".into())))
///     .order_by("start-date", SortDir::Desc)
///     .limit(5)
///     .execute().await?;
/// ```
pub struct RelationQuery<'db, R: TypeBridgeRelation> {
    db: &'db Database,
    dynamic: Option<DynamicRelationManager<'db>>,
    filters: Vec<Expr>,
    sort_fields: Vec<(String, SortDir)>,
    limit_val: Option<u64>,
    offset_val: Option<u64>,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationQuery<'db, R> {
    /// Create a new query builder for the given database.
    pub fn new(db: &'db Database) -> Self {
        Self {
            db,
            dynamic: None,
            filters: Vec::new(),
            sort_fields: Vec::new(),
            limit_val: None,
            offset_val: None,
            _marker: PhantomData,
        }
    }

    pub(crate) fn with_manager(mut self, manager: DynamicRelationManager<'db>) -> Self {
        self.dynamic = Some(manager);
        self
    }

    /// Add a filter expression. Multiple filters are ANDed together.
    pub fn filter(mut self, expr: Expr) -> Self {
        self.filters.push(expr);
        self
    }

    /// Add a sort field.
    pub fn order_by(mut self, attr: impl Into<String>, dir: SortDir) -> Self {
        self.sort_fields.push((attr.into(), dir));
        self
    }

    /// Set the maximum number of results.
    pub fn limit(mut self, n: u64) -> Self {
        self.limit_val = Some(n);
        self
    }

    /// Set the number of results to skip.
    pub fn offset(mut self, n: u64) -> Self {
        self.offset_val = Some(n);
        self
    }

    /// Transition to a group-by query.
    ///
    /// Returns a [`GroupByRelationQuery`] that can only be finalized via
    /// `.aggregate()`.
    pub fn group_by(self, attr: impl Into<String>) -> GroupByRelationQuery<'db, R> {
        let query = GroupByRelationQuery::new(self.db, self.filters, attr.into());
        match self.dynamic {
            Some(manager) => query.with_manager(manager),
            None => query,
        }
    }

    /// Execute the query and return matching relations.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn execute(self) -> Result<Vec<R>> {
        if let Some(manager) = self.dynamic {
            let expressions = self
                .filters
                .into_iter()
                .map(Expr::into_dynamic)
                .collect::<Vec<_>>();
            let sorts = self
                .sort_fields
                .into_iter()
                .map(|(attr_name, direction)| DynamicSort::Attribute {
                    attr_name,
                    direction,
                })
                .collect::<Vec<_>>();
            return manager
                .get_with_query(&expressions, &sorts, self.limit_val, self.offset_val)
                .await?
                .into_iter()
                .map(materialize_relation_row::<R>)
                .collect();
        }

        let typeql = query_builder::build_relation_expr_fetch::<R>(
            &self.filters,
            &self.sort_fields,
            self.limit_val,
            self.offset_val,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "EXPR FETCH RELATION");

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

    /// Execute with limit(1) and return the first result, if any.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn first(self) -> Result<Option<R>> {
        let mut results = self.limit(1).execute().await?;
        Ok(if results.is_empty() {
            None
        } else {
            Some(results.swap_remove(0))
        })
    }

    /// Count matching relations.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn count(self) -> Result<u64> {
        if let Some(manager) = self.dynamic {
            let expressions = self
                .filters
                .into_iter()
                .map(Expr::into_dynamic)
                .collect::<Vec<_>>();
            return manager.count_with_query(&expressions).await;
        }

        let typeql = query_builder::build_relation_expr_count::<R>(&self.filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "EXPR COUNT RELATION");
        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Run aggregation queries.
    #[tracing::instrument(skip(self, aggs), fields(relation_type = R::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<AggResult> {
        if let Some(manager) = self.dynamic {
            let expressions = self
                .filters
                .into_iter()
                .map(Expr::into_dynamic)
                .collect::<Vec<_>>();
            let aggregates = aggs.iter().map(Agg::to_dynamic).collect::<Vec<_>>();
            let rows = manager
                .aggregate_with_query(&expressions, &aggregates)
                .await?;
            let row = rows.into_iter().next().ok_or_else(|| OrmError::Hydration {
                type_name: R::TYPE_NAME.into(),
                message: "Aggregation returned no rows".into(),
            })?;
            return Ok(AggResult::new(row.into_iter().collect()));
        }

        let typeql = query_builder::build_relation_expr_aggregate::<R>(&self.filters, aggs, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = R::TYPE_NAME, "EXPR AGGREGATE RELATION");
        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;

        match result {
            QueryResult::Rows(rows) => {
                let row = rows.first().ok_or_else(|| OrmError::Hydration {
                    type_name: R::TYPE_NAME.into(),
                    message: "Aggregation returned no rows".into(),
                })?;
                let map: HashMap<String, serde_json::Value> = row
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                Ok(AggResult::new(map))
            }
            _ => Err(OrmError::Hydration {
                type_name: R::TYPE_NAME.into(),
                message: "Expected Rows from reduce query".into(),
            }),
        }
    }
}
