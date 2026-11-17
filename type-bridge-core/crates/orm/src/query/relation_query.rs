//! Chainable query builder for relation types.

use std::marker::PhantomData;

use crate::_dynamic::DynamicSort;
use crate::_manager::DynamicRelationManager;
use crate::_manager::hydration::materialize_relation_row;
use crate::_relation::TypeBridgeRelation;
use crate::error::{OrmError, Result};
use crate::expr::{Agg, AggResult, Expr, SortDir};
use crate::query::group_by_query::GroupByRelationQuery;

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
    dynamic: DynamicRelationManager<'db>,
    filters: Vec<Expr>,
    sort_fields: Vec<(String, SortDir)>,
    limit_val: Option<u64>,
    offset_val: Option<u64>,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> RelationQuery<'db, R> {
    pub(crate) fn from_manager(manager: DynamicRelationManager<'db>) -> Self {
        Self {
            dynamic: manager,
            filters: Vec::new(),
            sort_fields: Vec::new(),
            limit_val: None,
            offset_val: None,
            _marker: PhantomData,
        }
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
        GroupByRelationQuery::from_manager(self.dynamic, self.filters, attr.into())
    }

    /// Execute the query and return matching relations.
    #[tracing::instrument(skip(self), fields(relation_type = R::TYPE_NAME))]
    pub async fn execute(self) -> Result<Vec<R>> {
        let manager = self.dynamic;
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
        let manager = self.dynamic;
        let expressions = self
            .filters
            .into_iter()
            .map(Expr::into_dynamic)
            .collect::<Vec<_>>();
        return manager.count_with_query(&expressions).await;
    }

    /// Run aggregation queries.
    #[tracing::instrument(skip(self, aggs), fields(relation_type = R::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<AggResult> {
        let manager = self.dynamic;
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
}
