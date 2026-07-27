//! Versioned, provider-neutral algebra for typed match requests.
//!
//! This module represents unvalidated input. Construction does not imply that
//! descriptors exist, members are compatible, topology is connected, or
//! structural limits are satisfied; the validator owns those guarantees.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::ids::{BindingId, BoundFieldId, DescriptorId, RoleEdgeId, RoleId};
use crate::value::AttributeValue;

/// Wire-format version for a match request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatchRequestVersion(u16);

impl MatchRequestVersion {
    /// The initial typed match-request contract.
    pub const V1: Self = Self(1);

    /// Preserve an unvalidated raw version from a serialized request.
    pub const fn from_raw(value: u16) -> Self {
        Self(value)
    }

    /// Return the raw wire version.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Version constant used by V1 request producers.
pub const MATCH_REQUEST_VERSION_V1: MatchRequestVersion = MatchRequestVersion::V1;

/// One complete typed match request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchRequest {
    /// Serialized algebra version.
    pub version: MatchRequestVersion,
    /// Binding and predicate graph.
    pub plan: MatchPlan,
    /// Requested terminal operation.
    pub operation: MatchOperation,
}

impl MatchRequest {
    /// Construct a V1 unvalidated request.
    pub fn v1(plan: MatchPlan, operation: MatchOperation) -> Self {
        Self {
            version: MatchRequestVersion::V1,
            plan,
            operation,
        }
    }
}

/// The binding and predicate graph shared by every terminal operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchPlan {
    /// Bindings in canonical selected-then-hidden order.
    pub bindings: Vec<MatchBinding>,
    /// Optional boolean predicate tree.
    pub predicate: Option<MatchExpr>,
    /// Explicit topology-level cross joins, stored in deterministic order.
    pub allowed_cross_joins: BTreeSet<BindingPair>,
}

/// One plan-local binding to a descriptor target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchBinding {
    /// Deterministic plan-local binding identity.
    pub id: BindingId,
    /// Deterministic registry descriptor identity.
    pub descriptor: DescriptorId,
    /// Entity or relation target kind used for capability negotiation.
    pub thing_kind: ThingKind,
    /// Exact or subtype-inclusive match behavior.
    pub match_mode: MatchMode,
}

/// TypeDB thing category targeted by a match binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThingKind {
    /// Entity type.
    Entity,
    /// Relation type.
    Relation,
}

/// Whether a binding targets one exact descriptor or includes its subtypes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Match only the exact target descriptor.
    Exact,
    /// Match the target descriptor and all compatible subtypes.
    Subtypes,
}

/// An explicitly permitted topology-level cross join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BindingPair {
    /// Lower canonical binding identity.
    pub left: BindingId,
    /// Higher canonical binding identity.
    pub right: BindingId,
}

impl BindingPair {
    /// Construct a pair in canonical binding-ID order.
    pub fn new(first: BindingId, second: BindingId) -> Self {
        if first <= second {
            Self {
                left: first,
                right: second,
            }
        } else {
            Self {
                left: second,
                right: first,
            }
        }
    }
}

/// A typed comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOp {
    /// Equality.
    Equal,
    /// Inequality.
    NotEqual,
    /// Strictly less than.
    LessThan,
    /// Less than or equal.
    LessThanOrEqual,
    /// Strictly greater than.
    GreaterThan,
    /// Greater than or equal.
    GreaterThanOrEqual,
    /// String containment.
    Contains,
    /// String prefix.
    StartsWith,
    /// String suffix.
    EndsWith,
    /// Regular-expression match.
    Regex,
}

/// A typed boolean match expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchExpr {
    /// Compare a bound field with a typed literal value.
    FieldValue {
        /// Bound field being tested.
        field: BoundFieldId,
        /// Typed comparison operator.
        operator: ComparisonOp,
        /// Typed attribute value; never an untyped JSON value.
        value: AttributeValue,
    },
    /// Compare two bound fields.
    FieldComparison {
        /// Left-hand bound field.
        left: BoundFieldId,
        /// Typed comparison operator.
        operator: ComparisonOp,
        /// Right-hand bound field.
        right: BoundFieldId,
    },
    /// Require a relation binding to connect a player through one role.
    RoleEdge {
        /// Deterministic identity used by provider evidence.
        id: RoleEdgeId,
        /// Relation binding.
        relation: BindingId,
        /// Descriptor-qualified role.
        role: RoleId,
        /// Player binding.
        player: BindingId,
    },
    /// Require a finite directed walk between two bound endpoints.
    ///
    /// The relation and roles are exact schema identities. Intermediate
    /// vertices and relation instances are existential proof variables and
    /// never become part of the public result shape.
    Reachable {
        /// Exact relation descriptor used for every hop.
        relation: DescriptorId,
        /// Ordered role played by each hop's source endpoint.
        role_from: RoleId,
        /// Ordered role played by each hop's target endpoint.
        role_to: RoleId,
        /// Bound walk source.
        source: BindingId,
        /// Bound walk target.
        target: BindingId,
        /// Inclusive minimum hop count. Zero means exact endpoint identity.
        min_depth: u8,
        /// Inclusive finite maximum hop count.
        max_depth: u8,
    },
    /// Require every child expression.
    And {
        /// Child expressions in canonical source order.
        expressions: Vec<MatchExpr>,
    },
    /// Require at least one child expression.
    Or {
        /// Child expressions in canonical source order.
        expressions: Vec<MatchExpr>,
    },
    /// Negate one child expression.
    Not {
        /// Negated expression.
        expression: Box<MatchExpr>,
    },
}

/// One selected result slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FetchSlot {
    /// Select exactly one concept for the binding per row/root.
    One {
        /// Selected plan binding.
        binding: BindingId,
    },
    /// Collect all concepts for the binding per root.
    Collect {
        /// Collected plan binding.
        binding: BindingId,
        /// Whether collection multiplicity is removed by concept identity.
        distinct: bool,
        /// Stable public order for collection members.
        order: Vec<MatchOrder>,
    },
}

impl FetchSlot {
    /// Return the selected binding.
    pub const fn binding(&self) -> BindingId {
        match self {
            Self::One { binding } | Self::Collect { binding, .. } => *binding,
        }
    }

    /// Return whether this slot collects multiple values.
    pub const fn is_collection(&self) -> bool {
        matches!(self, Self::Collect { .. })
    }
}

/// One named selected output slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedFetchSlot {
    /// Public result name.
    pub name: String,
    /// Selected slot behavior.
    pub slot: FetchSlot,
}

/// Positional or named selected output shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FetchShape {
    /// Tuple-like selected output.
    Positional {
        /// Slots in public result order.
        slots: Vec<FetchSlot>,
    },
    /// Record-like selected output.
    Named {
        /// Named slots in public result order.
        slots: Vec<NamedFetchSlot>,
    },
}

impl FetchShape {
    /// Return the number of public selected slots.
    pub fn slot_count(&self) -> usize {
        match self {
            Self::Positional { slots } => slots.len(),
            Self::Named { slots } => slots.len(),
        }
    }

    /// Visit selected slots in public result order.
    pub fn for_each_slot(&self, mut visitor: impl FnMut(&FetchSlot)) {
        match self {
            Self::Positional { slots } => slots.iter().for_each(&mut visitor),
            Self::Named { slots } => slots.iter().for_each(|named| visitor(&named.slot)),
        }
    }
}

/// One stable public ordering term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchOrder {
    /// Bound scalar field used as the ordering key.
    pub field: BoundFieldId,
    /// Ascending or descending order.
    pub direction: SortDirection,
    /// Explicit behavior if a provider reports a missing key.
    pub missing: MissingOrder,
}

/// Sort direction for an ordering term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Ascending order.
    Ascending,
    /// Descending order.
    Descending,
}

/// Explicit ordering behavior for a missing or null key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingOrder {
    /// Treat a missing key as invalid provider evidence.
    Reject,
    /// Place a missing key before all present keys.
    First,
    /// Place a missing key after all present keys.
    Last,
}

/// A bounded result window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// Number of distinct rows or roots to skip.
    pub offset: u64,
    /// Maximum number of distinct rows or roots to return.
    pub limit: u64,
}

/// Cardinality expected from a row-fetch operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowCardinality {
    /// Require exactly one selected row.
    ExactlyOne,
    /// Return a resource-bounded ordered sequence of selected rows.
    BoundedMany,
}

/// Terminal operation over a typed match plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchOperation {
    /// Fetch selected distinct tuples.
    FetchRows {
        /// Public selected output shape.
        output: FetchShape,
        /// Stable selected-tuple order.
        order: Vec<MatchOrder>,
        /// Result window.
        window: Window,
        /// Required row cardinality.
        cardinality: RowCardinality,
    },
    /// Page by distinct root identity and rehydrate selected output.
    PageBy {
        /// Singular root binding that defines page identity.
        root: BindingId,
        /// Public selected output shape.
        output: FetchShape,
        /// Stable distinct-root order.
        order: Vec<MatchOrder>,
        /// Root page window.
        window: Window,
        /// Whether the provider must also return the total distinct-root count.
        include_total: bool,
    },
    /// Count distinct matching root identities.
    CountBy {
        /// Root binding whose identities are counted.
        root: BindingId,
    },
    /// Test whether any distinct matching root identity exists.
    ExistsBy {
        /// Root binding whose existence is tested.
        root: BindingId,
    },
}

impl MatchOperation {
    /// Return selected output when the operation has one.
    pub const fn output(&self) -> Option<&FetchShape> {
        match self {
            Self::FetchRows { output, .. } | Self::PageBy { output, .. } => Some(output),
            Self::CountBy { .. } | Self::ExistsBy { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(id: u16, descriptor: &str, thing_kind: ThingKind) -> MatchBinding {
        MatchBinding {
            id: BindingId::new(id),
            descriptor: DescriptorId::new(descriptor),
            thing_kind,
            match_mode: MatchMode::Exact,
        }
    }

    fn one(binding: u16) -> FetchSlot {
        FetchSlot::One {
            binding: BindingId::new(binding),
        }
    }

    #[test]
    fn cross_join_pair_constructor_is_canonical() {
        assert_eq!(
            BindingPair::new(BindingId::new(9), BindingId::new(2)),
            BindingPair {
                left: BindingId::new(2),
                right: BindingId::new(9),
            }
        );
    }

    #[test]
    fn btree_cross_joins_serialize_independently_of_insertion_order() {
        let pair_a = BindingPair::new(BindingId::new(2), BindingId::new(3));
        let pair_b = BindingPair::new(BindingId::new(0), BindingId::new(1));
        let make_request = |pairs: [BindingPair; 2]| {
            let mut allowed_cross_joins = BTreeSet::new();
            allowed_cross_joins.extend(pairs);
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![
                        binding(0, "entity:person", ThingKind::Entity),
                        binding(1, "relation:employment", ThingKind::Relation),
                    ],
                    predicate: None,
                    allowed_cross_joins,
                },
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            )
        };

        let forward = serde_json::to_vec(&make_request([pair_a, pair_b])).unwrap();
        let reverse = serde_json::to_vec(&make_request([pair_b, pair_a])).unwrap();
        assert_eq!(forward, reverse);
    }

    #[test]
    fn every_operation_variant_roundtrips_without_untyped_values() {
        let output = FetchShape::Positional {
            slots: vec![one(0)],
        };
        let operations = [
            MatchOperation::FetchRows {
                output: output.clone(),
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
            MatchOperation::PageBy {
                root: BindingId::new(0),
                output,
                order: Vec::new(),
                window: Window {
                    offset: 10,
                    limit: 20,
                },
                include_total: true,
            },
            MatchOperation::CountBy {
                root: BindingId::new(0),
            },
            MatchOperation::ExistsBy {
                root: BindingId::new(0),
            },
        ];

        for operation in operations {
            let encoded = serde_json::to_vec(&operation).unwrap();
            let decoded: MatchOperation = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, operation);
        }
    }

    #[test]
    fn unknown_versions_remain_unvalidated_input() {
        let encoded = serde_json::to_string(&MatchRequestVersion::from_raw(99)).unwrap();
        let decoded: MatchRequestVersion = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.get(), 99);
        assert_ne!(decoded, MatchRequestVersion::V1);
    }
}
