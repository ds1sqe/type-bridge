//! Chainable query builders for entities and relations.
//!
//! Provides [`EntityQuery`] and [`RelationQuery`] with fluent APIs for
//! filtering, sorting, pagination, and aggregation.
//! [`GroupByEntityQuery`] and [`GroupByRelationQuery`] add grouped
//! aggregation support.

pub mod entity_query;
pub mod group_by_query;
pub mod relation_query;

pub use entity_query::EntityQuery;
pub use group_by_query::{GroupByEntityQuery, GroupByRelationQuery};
pub use relation_query::RelationQuery;
