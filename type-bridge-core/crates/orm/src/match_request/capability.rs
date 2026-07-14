//! Normative request-feature to provider-capability derivation.
//!
//! Capability derivation is pure and deterministic. It reports what a request
//! requires; provider negotiation and unsupported-capability errors belong to
//! validation and execution layers.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::model::{
    FetchShape, FetchSlot, MatchExpr, MatchMode, MatchOperation, MatchPlan, MatchRequest,
    RowCardinality, ThingKind,
};

/// One provider behavior required by a validated match request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    /// Provider streams within explicit resource bounds.
    ResourceBoundedStreaming,
    /// Exact entity target matching.
    ExactEntityTarget,
    /// Exact relation target matching.
    ExactRelationTarget,
    /// Entity target matching including subtypes.
    SubtypeEntityTarget,
    /// Relation target matching including subtypes.
    SubtypeRelationTarget,
    /// Field-to-field comparisons.
    FieldComparison,
    /// Nested `and`, `or`, or `not` patterns.
    BooleanPattern,
    /// Distinctness by selected tuple identity.
    SelectedTupleDistinct,
    /// Stable order over selected tuples.
    StableSelectedOrder,
    /// Distinct root identity selection.
    DistinctRootSelection,
    /// Stable order over distinct roots.
    StableRootOrder,
    /// Root selection and output hydration in the same transaction snapshot.
    SameTransactionRehydration,
    /// Batched rebind by concept identity.
    BatchIdentityRebind,
    /// Count distinct root identities.
    DistinctRootCount,
    /// Test existence by distinct root identity.
    DistinctRootExists,
    /// Preserve collection multiplicity.
    Collect,
    /// Remove collection multiplicity by concept identity.
    CollectDistinct,
    /// Stable order over collection members.
    StableCollectionOrder,
}

impl Capability {
    /// Complete provider capability vocabulary for exhaustive implementations.
    pub const ALL: [Self; 18] = [
        Self::ResourceBoundedStreaming,
        Self::ExactEntityTarget,
        Self::ExactRelationTarget,
        Self::SubtypeEntityTarget,
        Self::SubtypeRelationTarget,
        Self::FieldComparison,
        Self::BooleanPattern,
        Self::SelectedTupleDistinct,
        Self::StableSelectedOrder,
        Self::DistinctRootSelection,
        Self::StableRootOrder,
        Self::SameTransactionRehydration,
        Self::BatchIdentityRebind,
        Self::DistinctRootCount,
        Self::DistinctRootExists,
        Self::Collect,
        Self::CollectDistinct,
        Self::StableCollectionOrder,
    ];
}

/// A deterministically ordered provider-capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl CapabilitySet {
    /// Construct an empty set.
    pub const fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Construct a provider inventory containing the complete vocabulary.
    pub fn all() -> Self {
        Self::from_iter(Capability::ALL)
    }

    /// Derive every capability required by one unvalidated request shape.
    ///
    /// Validation must still prove that the request itself is well formed
    /// before this set can be used for provider negotiation.
    pub fn for_request(request: &MatchRequest) -> Self {
        derive_required_capabilities(request)
    }

    /// Insert one capability.
    pub fn insert(&mut self, capability: Capability) -> bool {
        self.0.insert(capability)
    }

    /// Return whether this set contains `capability`.
    pub fn contains(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }

    /// Return the number of capabilities.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Return whether no capabilities are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate in deterministic capability order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Capability> {
        self.0.iter()
    }

    /// Return required capabilities that are absent from `available`.
    pub fn missing_from(&self, available: &Self) -> Self {
        Self(self.0.difference(&available.0).copied().collect())
    }

    /// Consume the wrapper and return its ordered set.
    pub fn into_inner(self) -> BTreeSet<Capability> {
        self.0
    }
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self::new()
    }
}

impl FromIterator<Capability> for CapabilitySet {
    fn from_iter<T: IntoIterator<Item = Capability>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for CapabilitySet {
    type Item = Capability;
    type IntoIter = std::collections::btree_set::IntoIter<Capability>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Derive the normative provider capabilities for one request.
pub fn derive_required_capabilities(request: &MatchRequest) -> CapabilitySet {
    let mut required = CapabilitySet::new();
    required.insert(Capability::ResourceBoundedStreaming);
    add_plan_capabilities(&request.plan, &mut required);
    add_operation_capabilities(&request.operation, &mut required);
    required
}

fn add_plan_capabilities(plan: &MatchPlan, required: &mut CapabilitySet) {
    for binding in &plan.bindings {
        let capability = match (binding.thing_kind, binding.match_mode) {
            (ThingKind::Entity, MatchMode::Exact) => Capability::ExactEntityTarget,
            (ThingKind::Relation, MatchMode::Exact) => Capability::ExactRelationTarget,
            (ThingKind::Entity, MatchMode::Subtypes) => Capability::SubtypeEntityTarget,
            (ThingKind::Relation, MatchMode::Subtypes) => Capability::SubtypeRelationTarget,
        };
        required.insert(capability);
    }

    if let Some(predicate) = &plan.predicate {
        add_expression_capabilities(predicate, required);
    }
}

fn add_expression_capabilities(expression: &MatchExpr, required: &mut CapabilitySet) {
    match expression {
        MatchExpr::FieldValue { .. } | MatchExpr::RoleEdge { .. } => {}
        MatchExpr::FieldComparison { .. } => {
            required.insert(Capability::FieldComparison);
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            required.insert(Capability::BooleanPattern);
            for child in expressions {
                add_expression_capabilities(child, required);
            }
        }
        MatchExpr::Not { expression } => {
            required.insert(Capability::BooleanPattern);
            add_expression_capabilities(expression, required);
        }
    }
}

fn add_operation_capabilities(operation: &MatchOperation, required: &mut CapabilitySet) {
    match operation {
        MatchOperation::FetchRows {
            output,
            cardinality,
            ..
        } => {
            required.insert(Capability::SelectedTupleDistinct);
            if *cardinality == RowCardinality::BoundedMany {
                required.insert(Capability::StableSelectedOrder);
            }
            add_output_capabilities(output, required);
        }
        MatchOperation::PageBy {
            output,
            include_total,
            ..
        } => {
            required.insert(Capability::DistinctRootSelection);
            required.insert(Capability::StableRootOrder);
            required.insert(Capability::SameTransactionRehydration);
            required.insert(Capability::BatchIdentityRebind);
            if *include_total {
                required.insert(Capability::DistinctRootCount);
            }
            add_output_capabilities(output, required);
        }
        MatchOperation::CountBy { .. } => {
            required.insert(Capability::DistinctRootCount);
        }
        MatchOperation::ExistsBy { .. } => {
            required.insert(Capability::DistinctRootExists);
        }
    }
}

fn add_output_capabilities(output: &FetchShape, required: &mut CapabilitySet) {
    output.for_each_slot(|slot| {
        if let FetchSlot::Collect { distinct, .. } = slot {
            required.insert(if *distinct {
                Capability::CollectDistinct
            } else {
                Capability::Collect
            });
            required.insert(Capability::StableCollectionOrder);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_request::ids::{BindingId, BoundFieldId, DescriptorId, FieldId};
    use crate::match_request::model::{
        MatchBinding, MatchOrder, MissingOrder, SortDirection, Window,
    };
    use std::collections::BTreeSet;

    fn binding(
        id: u16,
        descriptor: &str,
        thing_kind: ThingKind,
        match_mode: MatchMode,
    ) -> MatchBinding {
        MatchBinding {
            id: BindingId::new(id),
            descriptor: DescriptorId::new(descriptor),
            thing_kind,
            match_mode,
        }
    }

    fn field(binding: u16, owner: &str, name: &str) -> BoundFieldId {
        BoundFieldId::new(
            BindingId::new(binding),
            FieldId::new(DescriptorId::new(owner), name),
        )
    }

    fn request(predicate: Option<MatchExpr>, operation: MatchOperation) -> MatchRequest {
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    binding(0, "entity:person", ThingKind::Entity, MatchMode::Exact),
                    binding(
                        1,
                        "relation:employment",
                        ThingKind::Relation,
                        MatchMode::Subtypes,
                    ),
                ],
                predicate,
                allowed_cross_joins: BTreeSet::new(),
            },
            operation,
        )
    }

    fn positional(slots: Vec<FetchSlot>) -> FetchShape {
        FetchShape::Positional { slots }
    }

    #[test]
    fn target_boolean_comparison_and_bounded_row_features_are_derived() {
        let predicate = MatchExpr::Not {
            expression: Box::new(MatchExpr::FieldComparison {
                left: field(0, "entity:person", "name"),
                operator: super::super::model::ComparisonOp::Equal,
                right: field(0, "entity:person", "legal_name"),
            }),
        };
        let operation = MatchOperation::FetchRows {
            output: positional(vec![FetchSlot::One {
                binding: BindingId::new(0),
            }]),
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 20,
            },
            cardinality: RowCardinality::BoundedMany,
        };

        let required = derive_required_capabilities(&request(Some(predicate), operation));

        for capability in [
            Capability::ResourceBoundedStreaming,
            Capability::ExactEntityTarget,
            Capability::SubtypeRelationTarget,
            Capability::FieldComparison,
            Capability::BooleanPattern,
            Capability::SelectedTupleDistinct,
            Capability::StableSelectedOrder,
        ] {
            assert!(required.contains(capability), "missing {capability:?}");
        }
    }

    #[test]
    fn page_and_collection_features_are_derived() {
        let order = MatchOrder {
            field: field(1, "relation:employment", "start_date"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        };
        let operation = MatchOperation::PageBy {
            root: BindingId::new(0),
            output: positional(vec![FetchSlot::Collect {
                binding: BindingId::new(1),
                distinct: true,
                order: vec![order],
            }]),
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 10,
            },
            include_total: true,
        };

        let required = CapabilitySet::for_request(&request(None, operation));

        for capability in [
            Capability::DistinctRootSelection,
            Capability::StableRootOrder,
            Capability::SameTransactionRehydration,
            Capability::BatchIdentityRebind,
            Capability::DistinctRootCount,
            Capability::CollectDistinct,
            Capability::StableCollectionOrder,
        ] {
            assert!(required.contains(capability), "missing {capability:?}");
        }
        assert!(!required.contains(Capability::Collect));

        let without_total = MatchOperation::PageBy {
            root: BindingId::new(0),
            output: positional(vec![FetchSlot::One {
                binding: BindingId::new(0),
            }]),
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 10,
            },
            include_total: false,
        };
        assert!(
            !CapabilitySet::for_request(&request(None, without_total))
                .contains(Capability::DistinctRootCount)
        );
    }

    #[test]
    fn count_and_exists_require_distinct_root_semantics() {
        let count = CapabilitySet::for_request(&request(
            None,
            MatchOperation::CountBy {
                root: BindingId::new(0),
            },
        ));
        let exists = CapabilitySet::for_request(&request(
            None,
            MatchOperation::ExistsBy {
                root: BindingId::new(0),
            },
        ));

        assert!(count.contains(Capability::DistinctRootCount));
        assert!(exists.contains(Capability::DistinctRootExists));
    }

    #[test]
    fn capability_set_serialization_and_missing_order_are_deterministic() {
        let required = CapabilitySet::from_iter([
            Capability::Collect,
            Capability::ResourceBoundedStreaming,
            Capability::BooleanPattern,
        ]);
        let available = CapabilitySet::from_iter([Capability::ResourceBoundedStreaming]);

        assert_eq!(
            serde_json::to_string(&required).unwrap(),
            r#"["RESOURCE_BOUNDED_STREAMING","BOOLEAN_PATTERN","COLLECT"]"#
        );
        assert_eq!(
            required
                .missing_from(&available)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![Capability::BooleanPattern, Capability::Collect]
        );
    }
}
