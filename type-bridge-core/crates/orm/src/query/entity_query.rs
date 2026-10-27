//! Chainable query builder for entity types.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::_dynamic::DynamicSort;
use crate::_entity::TypeBridgeEntity;
use crate::_manager::DynamicEntityManager;
use crate::_manager::hydration::materialize_entity_row;
use crate::_manager::hydration::{extract_count, hydrate_entity};
use crate::_manager::query_builder;
use crate::error::{OrmError, Result};
use crate::expr::{Agg, AggResult, Expr, SortDir};
use crate::query::group_by_query::GroupByEntityQuery;
use crate::session::Database;
use crate::session::backend::{QueryResult, TxType};

/// A chainable query builder for entity types.
///
/// Created via [`EntityManager::query()`](crate::_manager::EntityManager::query).
///
/// # Example
///
/// This example is ignored because executing it requires a live TypeDB
/// service and a generated entity manager supplied by a consumer package.
///
/// ```ignore
/// let adults = manager.query()
///     .filter(Expr::gte("age", AttributeValue::Long(18)))
///     .order_by("name", SortDir::Asc)
///     .limit(10)
///     .offset(20)
///     .execute().await?;
/// ```
pub struct EntityQuery<'db, T: TypeBridgeEntity> {
    db: &'db Database,
    dynamic: Option<DynamicEntityManager<'db>>,
    filters: Vec<Expr>,
    sort_fields: Vec<(String, SortDir)>,
    limit_val: Option<u64>,
    offset_val: Option<u64>,
    _marker: PhantomData<T>,
}

impl<'db, T: TypeBridgeEntity> EntityQuery<'db, T> {
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

    pub(crate) fn with_manager(mut self, manager: DynamicEntityManager<'db>) -> Self {
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
    /// Returns a [`GroupByEntityQuery`] that can only be finalized via
    /// `.aggregate()`.
    pub fn group_by(self, attr: impl Into<String>) -> GroupByEntityQuery<'db, T> {
        let query = GroupByEntityQuery::new(self.db, self.filters, attr.into());
        match self.dynamic {
            Some(manager) => query.with_manager(manager),
            None => query,
        }
    }

    /// Execute the query and return matching entities.
    #[tracing::instrument(skip(self), fields(entity_type = T::TYPE_NAME))]
    pub async fn execute(self) -> Result<Vec<T>> {
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
                .map(materialize_entity_row::<T>)
                .collect();
        }

        let typeql = query_builder::build_expr_fetch::<T>(
            &self.filters,
            &self.sort_fields,
            self.limit_val,
            self.offset_val,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "EXPR FETCH");

        // raw remainder: no registry authority; removed in 06
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

    /// Execute with limit(1) and return the first result, if any.
    #[tracing::instrument(skip(self), fields(entity_type = T::TYPE_NAME))]
    pub async fn first(self) -> Result<Option<T>> {
        let mut results = self.limit(1).execute().await?;
        Ok(if results.is_empty() {
            None
        } else {
            Some(results.swap_remove(0))
        })
    }

    /// Count matching entities.
    #[tracing::instrument(skip(self), fields(entity_type = T::TYPE_NAME))]
    pub async fn count(self) -> Result<u64> {
        if let Some(manager) = self.dynamic {
            let expressions = self
                .filters
                .into_iter()
                .map(Expr::into_dynamic)
                .collect::<Vec<_>>();
            return manager.count_with_query(&expressions).await;
        }

        let typeql = query_builder::build_expr_count::<T>(&self.filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "EXPR COUNT");
        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Run aggregation queries.
    #[tracing::instrument(skip(self, aggs), fields(entity_type = T::TYPE_NAME))]
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
                type_name: T::TYPE_NAME.into(),
                message: "Aggregation returned no rows".into(),
            })?;
            return Ok(AggResult::new(row.into_iter().collect()));
        }

        let typeql = query_builder::build_expr_aggregate::<T>(&self.filters, aggs, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = T::TYPE_NAME, "EXPR AGGREGATE");
        // raw remainder: no registry authority; removed in 06
        let result = self.db.execute_raw(&typeql, TxType::Read).await?;

        match result {
            QueryResult::Rows(rows) => {
                let row = rows.first().ok_or_else(|| OrmError::Hydration {
                    type_name: T::TYPE_NAME.into(),
                    message: "Aggregation returned no rows".into(),
                })?;
                let map: HashMap<String, serde_json::Value> = row
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                Ok(AggResult::new(map))
            }
            _ => Err(OrmError::Hydration {
                type_name: T::TYPE_NAME.into(),
                message: "Expected Rows from reduce query".into(),
            }),
        }
    }
}
