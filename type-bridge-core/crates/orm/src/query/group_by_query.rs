//! Group-by query builders for entities and relations.

use std::marker::PhantomData;

use crate::_entity::TypeBridgeEntity;
use crate::_relation::TypeBridgeRelation;
use crate::error::Result;
use crate::expr::{Agg, AggResult, GroupByResult};

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
    dynamic: crate::_manager::DynamicEntityManager<'db>,
    filters: Vec<crate::expr::Expr>,
    group_field: String,
    _marker: PhantomData<T>,
}

impl<'db, T: TypeBridgeEntity> GroupByEntityQuery<'db, T> {
    pub(crate) fn from_manager(
        manager: crate::_manager::DynamicEntityManager<'db>,
        filters: Vec<crate::expr::Expr>,
        group_field: String,
    ) -> Self {
        Self {
            dynamic: manager,
            filters,
            group_field,
            _marker: PhantomData,
        }
    }

    /// Execute the grouped aggregation and return results.
    #[tracing::instrument(skip(self, aggs), fields(entity_type = T::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<GroupByResult> {
        let manager = self.dynamic;
        let expressions = self
            .filters
            .into_iter()
            .map(crate::expr::Expr::into_dynamic)
            .collect::<Vec<_>>();
        let aggregates = aggs.iter().map(Agg::to_dynamic).collect::<Vec<_>>();
        let rows = manager
            .group_by_aggregate_with_query(&expressions, &[self.group_field], &aggregates)
            .await?;
        return Ok(GroupByResult::new(
            rows.into_iter()
                .map(|mut row| {
                    let group = row.remove("group0").unwrap_or(serde_json::Value::Null);
                    (group, AggResult::new(row.into_iter().collect()))
                })
                .collect(),
        ));
    }
}

/// A group-by query builder for relation types.
///
/// Created via [`RelationQuery::group_by()`](crate::query::RelationQuery::group_by).
/// Can only be finalized via `.aggregate()`.
pub struct GroupByRelationQuery<'db, R: TypeBridgeRelation> {
    dynamic: crate::_manager::DynamicRelationManager<'db>,
    filters: Vec<crate::expr::Expr>,
    group_field: String,
    _marker: PhantomData<R>,
}

impl<'db, R: TypeBridgeRelation> GroupByRelationQuery<'db, R> {
    pub(crate) fn from_manager(
        manager: crate::_manager::DynamicRelationManager<'db>,
        filters: Vec<crate::expr::Expr>,
        group_field: String,
    ) -> Self {
        Self {
            dynamic: manager,
            filters,
            group_field,
            _marker: PhantomData,
        }
    }

    /// Execute the grouped aggregation and return results.
    #[tracing::instrument(skip(self, aggs), fields(relation_type = R::TYPE_NAME))]
    pub async fn aggregate(self, aggs: &[Agg]) -> Result<GroupByResult> {
        let manager = self.dynamic;
        let expressions = self
            .filters
            .into_iter()
            .map(crate::expr::Expr::into_dynamic)
            .collect::<Vec<_>>();
        let aggregates = aggs.iter().map(Agg::to_dynamic).collect::<Vec<_>>();
        let rows = manager
            .group_by_aggregate_with_query(&expressions, &[self.group_field], &aggregates)
            .await?;
        return Ok(GroupByResult::new(
            rows.into_iter()
                .map(|mut row| {
                    let group = row.remove("group0").unwrap_or(serde_json::Value::Null);
                    (group, AggResult::new(row.into_iter().collect()))
                })
                .collect(),
        ));
    }
}
