//! Additive query-plan V2 compatibility contracts.
//!
//! The ordinary typed query vocabulary remains in [`super`]. This module owns
//! the extra closed algebra needed to losslessly adapt the released
//! model-oriented match request without accepting raw TypeQL or host-authored
//! JSON fragments.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::limits::QueryLimits;
use crate::migration_assertion::BindingId;
use crate::value::ValueTypeTag;

use super::model::{
    HydrationDescriptor, HydrationProjection, ModelComparator, ModelField, ModelOutput,
    ModelOutputSlot, ModelPredicate, ModelReductionKey, ModelReductionTerm, ModelStableOrder,
    deserialize_binding,
};
use super::{failure, insert_capability};
use crate::capability::CapabilitySet;

pub(crate) const CAP_PLAN_V2: &str = "query.plan.v2";
pub(crate) const CAP_DISJUNCTION: &str = "query.pattern.disjunction";
pub(crate) const CAP_STRING_OPERATORS: &str = "query.pattern.string-operators";
pub(crate) const CAP_LINKS_SUBTYPES: &str = "query.pattern.links-subtypes";
pub(crate) const CAP_IID: &str = "query.pattern.iid";
pub(crate) const CAP_CROSS_JOIN: &str = "query.topology.cross-join";
pub(crate) const CAP_OUTPUT_NAMED: &str = "query.output.named";
pub(crate) const CAP_OUTPUT_COLLECT: &str = "query.output.collect";
pub(crate) const CAP_OUTPUT_COLLECT_DISTINCT: &str = "query.output.collect-distinct";
pub(crate) const CAP_OUTPUT_HYDRATED: &str = "query.output.hydrated";
pub(crate) const CAP_EXACTLY_ONE: &str = "query.operation.exactly-one";
pub(crate) const CAP_COMPLETE_ROWS: &str = "query.operation.complete-rows";
pub(crate) const CAP_PAGE: &str = "query.operation.page";
pub(crate) const CAP_DISTINCT_COUNT: &str = "query.operation.distinct-count";
pub(crate) const CAP_DISTINCT_EXISTS: &str = "query.operation.distinct-exists";
pub(crate) const CAP_STABLE_SELECTED: &str = "query.order.stable-selected";
pub(crate) const CAP_STABLE_ROOT: &str = "query.order.stable-root";
pub(crate) const CAP_STABLE_COLLECTION: &str = "query.order.stable-collection";
pub(crate) const CAP_SAME_SNAPSHOT_HYDRATION: &str = "query.execution.same-snapshot-hydration";
pub(crate) const CAP_BATCH_IDENTITY_REBIND: &str = "query.execution.batch-identity-rebind";

/// One canonical explicit cross-join permission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryBindingPairV2 {
    #[serde(deserialize_with = "deserialize_binding")]
    left: BindingId,
    #[serde(deserialize_with = "deserialize_binding")]
    right: BindingId,
}

impl QueryBindingPairV2 {
    /// Construct a pair in ascending binding-ID order.
    #[must_use]
    pub const fn new(first: BindingId, second: BindingId) -> Self {
        if first.get() <= second.get() {
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

    /// Return the lower binding ID.
    #[must_use]
    pub const fn left(self) -> BindingId {
        self.left
    }

    /// Return the higher binding ID.
    #[must_use]
    pub const fn right(self) -> BindingId {
        self.right
    }
}

/// One bounded model-query result window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryWindowV2 {
    limit: u64,
    offset: u64,
}

impl QueryWindowV2 {
    /// Construct an exact offset and positive limit.
    #[must_use]
    pub const fn new(offset: u64, limit: u64) -> Self {
        Self { limit, offset }
    }

    /// Return the skipped distinct row/root count.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Return the maximum returned distinct row/root count.
    #[must_use]
    pub const fn limit(self) -> u64 {
        self.limit
    }
}

/// Cardinality required from a hydrated row query.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryRowCardinalityV2 {
    /// Require exactly one distinct selected-identity tuple.
    ExactlyOne,
    /// Return one bounded stable row window.
    BoundedMany,
    /// Return every distinct selected-identity tuple, or refuse when there
    /// are more than the window's limit. Nothing is cut off, so no order
    /// is required.
    Complete,
}

/// The released model-query terminal contract carried by a V2 plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelQueryV2 {
    /// Return hydrated selected rows.
    Rows {
        /// Required selected-row cardinality.
        cardinality: QueryRowCardinalityV2,
        /// Complete hydration authority.
        hydration: HydrationProjection,
        /// Stable order; absent only for exactly-one.
        order: Option<ModelStableOrder>,
        /// Public output shape.
        output: ModelOutput,
        /// Exact row window.
        window: QueryWindowV2,
    },
    /// Page by distinct root identity and hydrate in the same snapshot.
    Page {
        /// Complete hydration authority.
        hydration: HydrationProjection,
        /// Whether the same snapshot also returns a distinct-root total.
        include_total: bool,
        /// Stable distinct-root order.
        order: ModelStableOrder,
        /// Public output shape.
        output: ModelOutput,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
        /// Exact root window.
        window: QueryWindowV2,
    },
    /// Count distinct root identities losslessly.
    DistinctCount {
        /// Complete plan-side descriptor and predicate authority.
        hydration: HydrationProjection,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
    },
    /// Test distinct-root existence.
    DistinctExists {
        /// Complete plan-side descriptor and predicate authority.
        hydration: HydrationProjection,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
    },
    /// Reduce the distinct-root stream with optional typed grouping.
    Reduction {
        /// Complete plan-side descriptor and hydration authority.
        hydration: HydrationProjection,
        /// Root identity whose distinct stream feeds every reducer.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
        /// Optional exact grouping contract.
        group: Option<ModelReductionKey>,
        /// Ordered typed reducer terms.
        reducers: Vec<ModelReductionTerm>,
    },
}

/// Concise alias used by remote response-shape contracts.
pub type ModelOutputV2 = ModelOutput;

/// The additive V2-only compatibility portion of one query plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryPlanV2Compatibility {
    allowed_cross_joins: Vec<QueryBindingPairV2>,
    model_query: Option<ModelQueryV2>,
    predicate: Option<ModelPredicate>,
}

impl QueryPlanV2Compatibility {
    /// Construct the empty compatibility portion used by native low-level V2 plans.
    #[must_use]
    pub const fn native() -> Self {
        Self {
            allowed_cross_joins: Vec::new(),
            model_query: None,
            predicate: None,
        }
    }

    /// Construct an adapter-authored compatibility contract.
    #[must_use]
    pub const fn new(
        predicate: Option<ModelPredicate>,
        allowed_cross_joins: Vec<QueryBindingPairV2>,
        model_query: Option<ModelQueryV2>,
    ) -> Self {
        Self {
            allowed_cross_joins,
            model_query,
            predicate,
        }
    }

    /// Return the closed released boolean predicate, if present.
    #[must_use]
    pub const fn predicate(&self) -> Option<&ModelPredicate> {
        self.predicate.as_ref()
    }

    /// Return canonical explicit cross-join permissions.
    #[must_use]
    pub fn allowed_cross_joins(&self) -> &[QueryBindingPairV2] {
        &self.allowed_cross_joins
    }

    /// Return the released model-query terminal contract, if present.
    #[must_use]
    pub const fn model_query(&self) -> Option<&ModelQueryV2> {
        self.model_query.as_ref()
    }

    pub(crate) fn validate(
        &self,
        binding_count: usize,
        limits: QueryLimits,
    ) -> Result<(), Diagnostic> {
        if self.allowed_cross_joins.len() > limits.structural.allowed_cross_joins {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_v2_cross_join_limit",
                "explicit cross joins exceed the structural ceiling",
            ));
        }
        let mut previous = None;
        for pair in &self.allowed_cross_joins {
            if pair.left >= pair.right {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_cross_join_pair",
                    "cross-join pairs must contain two distinct ascending binding IDs",
                ));
            }
            if previous.is_some_and(|previous| previous >= *pair) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_cross_joins_not_canonical",
                    "cross-join pairs must be strictly sorted and unique",
                ));
            }
            previous = Some(*pair);
            check_binding(pair.left, binding_count)?;
            check_binding(pair.right, binding_count)?;
        }

        if let Some(predicate) = &self.predicate {
            let mut nodes = 0usize;
            validate_pattern(predicate, 1, true, binding_count, limits, &mut nodes)?;
        }
        if let Some(model_query) = &self.model_query {
            validate_model_query(model_query, binding_count, limits)?;
        }
        if let Some(predicate) = &self.predicate {
            validate_pattern_authority(
                predicate,
                self.model_query.as_ref().and_then(model_hydration),
            )?;
        }
        Ok(())
    }

    pub(crate) fn add_capabilities(
        &self,
        capabilities: &mut CapabilitySet,
    ) -> Result<(), Diagnostic> {
        if let Some(predicate) = &self.predicate {
            add_pattern_capabilities(predicate, capabilities)?;
        }
        if !self.allowed_cross_joins.is_empty() {
            insert_capability(capabilities, CAP_CROSS_JOIN)?;
        }
        if let Some(model_query) = &self.model_query {
            add_model_query_capabilities(model_query, capabilities)?;
        }
        Ok(())
    }
}

const fn model_hydration(query: &ModelQueryV2) -> Option<&HydrationProjection> {
    match query {
        ModelQueryV2::Rows { hydration, .. }
        | ModelQueryV2::Page { hydration, .. }
        | ModelQueryV2::DistinctCount { hydration, .. }
        | ModelQueryV2::DistinctExists { hydration, .. }
        | ModelQueryV2::Reduction { hydration, .. } => Some(hydration),
    }
}

fn validate_pattern_authority(
    pattern: &ModelPredicate,
    hydration: Option<&HydrationProjection>,
) -> Result<(), Diagnostic> {
    match pattern {
        ModelPredicate::RoleEdge {
            include_relation_subtypes,
            player,
            relation,
            relation_type,
            role,
        } => validate_role_edge_authority(
            *include_relation_subtypes,
            *player,
            *relation,
            relation_type,
            role,
            hydration,
        ),
        ModelPredicate::And { patterns } | ModelPredicate::Or { patterns } => {
            for child in patterns {
                validate_pattern_authority(child, hydration)?;
            }
            Ok(())
        }
        ModelPredicate::Not { pattern } => validate_pattern_authority(pattern, hydration),
        ModelPredicate::FieldValue { field, .. } => {
            validate_predicate_field_authority(field, hydration)
        }
        ModelPredicate::FieldComparison { left, right, .. } => {
            validate_predicate_field_authority(left, hydration)?;
            validate_predicate_field_authority(right, hydration)
        }
        ModelPredicate::FieldPresence { field, .. } => {
            validate_predicate_field_authority(field, hydration)
        }
        ModelPredicate::BindingIid { .. } => Ok(()),
        ModelPredicate::Reachable { .. } => Ok(()),
    }
}

fn validate_predicate_field_authority(
    field: &ModelField,
    hydration: Option<&HydrationProjection>,
) -> Result<(), Diagnostic> {
    let hydration = hydration.ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_field_authority",
            "descriptor-qualified predicates require closed hydration authority",
        )
    })?;
    let binding = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == field.binding())
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_field_authority",
                "predicate field binding lacks hydration authority",
            )
        })?;
    if binding.concrete_descriptors().is_empty() {
        return Ok(());
    }
    let admitted = binding.concrete_descriptors().iter().any(|concrete| {
        hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
            .and_then(|descriptor| {
                descriptor.fields().iter().find(|projected| {
                    projected.reference_owners().contains(field.descriptor())
                        && projected.attribute() == field.attribute()
                        && projected.value_type() == field.value_type()
                })
            })
            .is_some()
    });
    if !admitted {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_field_authority",
            "predicate field owner, attribute, or type has no applicable concrete authority",
        ));
    }
    Ok(())
}

fn validate_role_edge_authority(
    include_relation_subtypes: bool,
    player: BindingId,
    relation: BindingId,
    relation_type: &TypeId,
    role: &RoleId,
    hydration: Option<&HydrationProjection>,
) -> Result<(), Diagnostic> {
    let exact_reference = role.declaring_relation() == relation_type.label();
    let Some(hydration) = hydration else {
        return if exact_reference && !include_relation_subtypes {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "inherited or subtype role edges require closed hydration authority",
            ))
        };
    };
    let Some(relation_binding) = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == relation)
    else {
        return if exact_reference && !include_relation_subtypes {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "role-edge relation binding lacks closed hydration authority",
            ))
        };
    };
    let Some(player_binding) = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == player)
    else {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_player_authority",
            "role-edge player binding lacks closed hydration authority",
        ));
    };
    if relation_binding.declared_descriptor() != relation_type {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_relation_authority",
            "role-edge relation descriptor contradicts its binding authority",
        ));
    }
    if relation_binding.concrete_descriptors().is_empty()
        || player_binding.concrete_descriptors().is_empty()
    {
        return Ok(());
    }
    let mut role_applicable = false;
    for concrete in relation_binding.concrete_descriptors() {
        let Some(descriptor) = hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
        else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "role-edge concrete relation lacks descriptor authority",
            ));
        };
        for projected_role in descriptor
            .roles()
            .iter()
            .filter(|projected| projected.reference_roles().contains(role))
        {
            role_applicable = true;
            // An exact subtype binding legitimately differs from a role's base
            // declaration; schema claims independently prove both closures.
            let player_admitted = projected_role.players().iter().any(|authority| {
                player_binding
                    .concrete_descriptors()
                    .iter()
                    .any(|concrete| authority.concrete_descriptors().contains(concrete))
            });
            if player_admitted {
                return Ok(());
            }
        }
    }
    if role_applicable {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_player_authority",
            "role player binding has no applicable declared-to-concrete role authority",
        ))
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_edge_authority",
            "role reference has no applicable concrete relation authority",
        ))
    }
}

fn check_binding(binding: BindingId, binding_count: usize) -> Result<(), Diagnostic> {
    if usize::from(binding.get()) < binding_count {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_unknown_binding",
            "V2 compatibility metadata references an undeclared binding",
        ))
    }
}

fn check_field(field: &ModelField, binding_count: usize) -> Result<(), Diagnostic> {
    check_binding(field.binding(), binding_count)?;
    if matches!(
        field.descriptor().kind(),
        TypeKind::Entity | TypeKind::Relation
    ) {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_invalid_field_owner",
            "a model field owner must be an entity or relation descriptor",
        ))
    }
}

fn validate_pattern(
    pattern: &ModelPredicate,
    depth: usize,
    positive_root: bool,
    binding_count: usize,
    limits: QueryLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    *nodes = nodes.saturating_add(1);
    if !limits.structural.allows_predicate_nodes(*nodes) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_pattern_node_limit",
            "V2 compatibility predicate nodes exceed the structural ceiling",
        ));
    }
    if !limits.structural.allows_predicate_depth(depth) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_pattern_depth_limit",
            "V2 compatibility predicate depth exceeds the structural ceiling",
        ));
    }
    match pattern {
        ModelPredicate::FieldValue {
            field,
            comparator,
            value,
        } => {
            check_field(field, binding_count)?;
            value.validate()?;
            value.check_artifact_bytes(limits)?;
            if field.value_type() != value.value_type() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_literal_type",
                    "field and literal scalar domains must match exactly",
                ));
            }
            if comparator.is_string_operator() && field.value_type() != ValueTypeTag::String {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_string_operator_type",
                    "string operators require a canonical string literal",
                ));
            }
            refuse_duration_range(*comparator, field.value_type())?;
            if field.value_type() == ValueTypeTag::Boolean
                && !matches!(
                    comparator,
                    ModelComparator::Equal | ModelComparator::NotEqual
                )
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_boolean_operator",
                    "boolean fields admit only equality and inequality",
                ));
            }
            Ok(())
        }
        ModelPredicate::FieldComparison {
            left,
            comparator,
            right,
        } => {
            check_field(left, binding_count)?;
            check_field(right, binding_count)?;
            if left.value_type() != right.value_type() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_comparison_type",
                    "field-to-field comparisons require one exact scalar domain",
                ));
            }
            if comparator.is_string_operator() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_string_operator",
                    "released field-to-field comparisons do not admit string operators",
                ));
            }
            refuse_duration_range(*comparator, left.value_type())?;
            if left.value_type() == ValueTypeTag::Boolean
                && !matches!(
                    comparator,
                    ModelComparator::Equal | ModelComparator::NotEqual
                )
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_boolean_operator",
                    "boolean fields admit only equality and inequality",
                ));
            }
            Ok(())
        }
        ModelPredicate::FieldPresence { field, .. } => check_field(field, binding_count),
        ModelPredicate::BindingIid { binding, iid } => {
            check_binding(*binding, binding_count)?;
            if !is_canonical_thing_iid(iid) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_invalid_iid",
                    "IID predicates require a canonical TypeDB thing IID",
                ));
            }
            Ok(())
        }
        ModelPredicate::RoleEdge {
            player,
            relation,
            relation_type,
            ..
        } => {
            check_binding(*relation, binding_count)?;
            check_binding(*player, binding_count)?;
            if relation_type.kind() != TypeKind::Relation {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_role_edge_kind",
                    "role edges require a relation descriptor",
                ));
            }
            Ok(())
        }
        ModelPredicate::Reachable {
            min_depth,
            max_depth,
            relation,
            role_from,
            role_to,
            source,
            target,
        } => {
            if !positive_root {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_reachable_not_root_positive",
                    "bounded reachability is admitted only in the positive root conjunction",
                ));
            }
            if relation.kind() != TypeKind::Relation
                || role_from.declaring_relation() != relation.label()
                || role_to.declaring_relation() != relation.label()
                || min_depth > max_depth
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_reachable_contract",
                    "reachability requires relation-owned roles and canonical inclusive bounds",
                ));
            }
            if !limits
                .structural
                .allows_predicate_depth(usize::from(*max_depth))
            {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_v2_reachable_depth",
                    "reachability exceeds the finite depth ceiling",
                ));
            }
            let first_positive = usize::from((*min_depth).max(1));
            let bound = usize::from(*max_depth);
            let expanded_hops = if first_positive <= bound {
                (first_positive..=bound).fold(0usize, usize::saturating_add)
            } else {
                0
            };
            let expanded_clauses = expanded_hops.saturating_add(usize::from(*min_depth == 0));
            *nodes = nodes.saturating_add(expanded_clauses.saturating_sub(1));
            if !limits.structural.allows_predicate_nodes(*nodes) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_v2_reachable_expansion_limit",
                    "reachability expansion exceeds the predicate-node ceiling",
                ));
            }
            check_binding(*source, binding_count)?;
            check_binding(*target, binding_count)
        }
        ModelPredicate::And { patterns } => {
            validate_boolean_children(patterns, depth, positive_root, binding_count, limits, nodes)
        }
        ModelPredicate::Or { patterns } => {
            validate_boolean_children(patterns, depth, false, binding_count, limits, nodes)
        }
        ModelPredicate::Not { pattern } => {
            validate_pattern(pattern, depth + 1, false, binding_count, limits, nodes)
        }
    }
}

fn validate_boolean_children(
    patterns: &[ModelPredicate],
    depth: usize,
    positive_root: bool,
    binding_count: usize,
    limits: QueryLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    if patterns.is_empty() || patterns.len() > limits.structural.boolean_terms {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_boolean_term_limit",
            "boolean branches are empty or exceed the term ceiling",
        ));
    }
    for child in patterns {
        validate_pattern(
            child,
            depth + 1,
            positive_root,
            binding_count,
            limits,
            nodes,
        )?;
    }
    Ok(())
}

fn validate_model_query(
    query: &ModelQueryV2,
    binding_count: usize,
    limits: QueryLimits,
) -> Result<(), Diagnostic> {
    match query {
        ModelQueryV2::Rows {
            cardinality,
            hydration,
            order,
            output,
            window,
        } => {
            validate_window(*window)?;
            validate_model_output(output, hydration, binding_count, limits)?;
            let selected_identity = output_identity_tuple(output);
            if selected_identity.is_empty() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_missing_selected_identity",
                    "row queries require at least one singular public identity slot",
                ));
            }
            match cardinality {
                QueryRowCardinalityV2::ExactlyOne => {
                    if window.offset() != 0 || window.limit() != 1 || order.is_some() {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_exactly_one_contract",
                            "exactly-one requires offset zero, limit one, and no row order",
                        ));
                    }
                }
                QueryRowCardinalityV2::BoundedMany => {
                    let order = order.as_ref().ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_rows_missing_order",
                            "bounded rows require a validator-proven stable total order",
                        )
                    })?;
                    validate_stable_order(order, binding_count, limits.structural.order_terms)?;
                    let exposed = selected_identity.iter().copied().collect();
                    validate_order_against_hydration(order, hydration, &exposed)?;
                    if order.identity_tiebreakers() != selected_identity {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_selected_order_tiebreaker",
                            "bounded row ordering must use the selected identity tuple",
                        ));
                    }
                }
                QueryRowCardinalityV2::Complete => {
                    if window.offset() != 0 || order.is_some() {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_complete_rows_contract",
                            "complete rows require offset zero and no row order",
                        ));
                    }
                }
            }
        }
        ModelQueryV2::Page {
            hydration,
            order,
            output,
            root,
            window,
            ..
        } => {
            check_binding(*root, binding_count)?;
            validate_window(*window)?;
            validate_model_output(output, hydration, binding_count, limits)?;
            let mut root_is_singular = false;
            output.visit_slots(|_, slot| {
                if matches!(slot, ModelOutputSlot::One { binding, .. } if binding == root) {
                    root_is_singular = true;
                }
            });
            if !root_is_singular {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_page_root_not_singular",
                    "the page root must be one singular public output slot",
                ));
            }
            validate_stable_order(order, binding_count, limits.structural.order_terms)?;
            validate_order_against_hydration(order, hydration, &BTreeSet::from([*root]))?;
            if order.identity_tiebreakers() != [*root] {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_root_order_tiebreaker",
                    "page ordering must use the exact root identity tie breaker",
                ));
            }
        }
        ModelQueryV2::DistinctCount { hydration, root }
        | ModelQueryV2::DistinctExists { hydration, root } => {
            check_binding(*root, binding_count)?;
            validate_hydration(hydration, binding_count, limits)?;
            if !hydration
                .bindings()
                .iter()
                .any(|binding| binding.binding() == *root)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_root_not_hydrated",
                    "distinct root operations require root descriptor authority",
                ));
            }
        }
        ModelQueryV2::Reduction {
            hydration,
            root,
            group,
            reducers,
        } => {
            check_binding(*root, binding_count)?;
            validate_hydration(hydration, binding_count, limits)?;
            if !hydration
                .bindings()
                .iter()
                .any(|binding| binding.binding() == *root)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_root_not_hydrated",
                    "reduction roots require descriptor authority",
                ));
            }
            if reducers.is_empty() || !limits.structural.allows_selected_slots(reducers.len()) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_v2_reducer_limit",
                    "typed reductions require a bounded non-empty reducer list",
                ));
            }
            for term in reducers {
                if term.reduction().requires_input() != term.input().is_some() {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_v2_reducer_input",
                        "count forbids an input and every numeric reducer requires one",
                    ));
                }
                if let Some(field) = term.input() {
                    validate_reduction_field(field, hydration, binding_count, true)?;
                }
            }
            match group {
                None => {}
                Some(ModelReductionKey::Binding { binding }) => {
                    check_binding(*binding, binding_count)?;
                    if binding == root {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_reduction_group_is_root",
                            "the reduction group binding must differ from the reduced root",
                        ));
                    }
                    if !hydration
                        .bindings()
                        .iter()
                        .any(|authority| authority.binding() == *binding)
                    {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_reduction_group_not_hydrated",
                            "thing grouping requires descriptor authority for its binding",
                        ));
                    }
                }
                Some(ModelReductionKey::Field { field }) => {
                    validate_reduction_field(field, hydration, binding_count, false)?;
                }
                Some(ModelReductionKey::Fields { fields }) => {
                    if fields.len() < 2 {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_reduction_group_tuple",
                            "tuple-field grouping requires at least two fields",
                        ));
                    }
                    if !limits.structural.allows_selected_slots(fields.len()) {
                        return Err(failure(
                            DiagnosticCategory::ResourceLimit,
                            "query_plan_v2_reduction_group_limit",
                            "tuple-field grouping exceeds the bounded field ceiling",
                        ));
                    }
                    let mut unique = BTreeSet::new();
                    for field in fields {
                        validate_reduction_field(field, hydration, binding_count, false)?;
                        if !unique.insert((field.binding(), field.descriptor(), field.attribute()))
                        {
                            return Err(failure(
                                DiagnosticCategory::InvalidContract,
                                "query_plan_v2_reduction_group_duplicate",
                                "tuple-field grouping fields must be unique",
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_reduction_field(
    field: &ModelField,
    hydration: &HydrationProjection,
    binding_count: usize,
    numeric_scalar: bool,
) -> Result<(), Diagnostic> {
    check_field(field, binding_count)?;
    validate_predicate_field_authority(field, Some(hydration))?;
    if numeric_scalar
        && !matches!(
            field.value_type(),
            ValueTypeTag::Long | ValueTypeTag::Double
        )
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_reducer_domain",
            "reducer inputs require the long or double scalar domain",
        ));
    }
    if numeric_scalar {
        let binding = hydration
            .bindings()
            .iter()
            .find(|binding| binding.binding() == field.binding())
            .expect("field authority validation proved the binding");
        for concrete in binding.concrete_descriptors() {
            let Some(projected) = hydration
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.descriptor() == concrete)
                .and_then(|descriptor| {
                    descriptor.fields().iter().find(|projected| {
                        projected.reference_owners().contains(field.descriptor())
                            && projected.attribute() == field.attribute()
                            && projected.value_type() == field.value_type()
                    })
                })
            else {
                continue;
            };
            if projected.ordered() || projected.cardinality().max() != Some(1) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_reducer_non_scalar",
                    "reducer inputs must be scalar descriptor ownerships",
                ));
            }
        }
    }
    Ok(())
}

fn validate_window(window: QueryWindowV2) -> Result<(), Diagnostic> {
    if window.limit() == 0 || window.offset().checked_add(window.limit()).is_none() {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_window",
            "model-query windows require a positive non-overflowing limit",
        ))
    } else {
        Ok(())
    }
}

/// TypeDB 3.14 refuses range comparisons of durations; refuse them on every
/// profile at build time.
fn refuse_duration_range(
    comparator: ModelComparator,
    value_type: ValueTypeTag,
) -> Result<(), Diagnostic> {
    if value_type == ValueTypeTag::Duration
        && matches!(
            comparator,
            ModelComparator::Less
                | ModelComparator::LessOrEqual
                | ModelComparator::Greater
                | ModelComparator::GreaterOrEqual
        )
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_duration_range",
            "durations admit only equality comparison",
        ));
    }
    Ok(())
}

fn validate_stable_order(
    order: &ModelStableOrder,
    binding_count: usize,
    term_limit: usize,
) -> Result<(), Diagnostic> {
    if order.terms().len() > term_limit || order.identity_tiebreakers().is_empty() {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_order_limit",
            "stable order terms are unbounded or lack an identity tie breaker",
        ));
    }
    let mut fields = BTreeSet::new();
    for term in order.terms() {
        check_field(term.field(), binding_count)?;
        // TypeDB 3.14 refuses sorts on unordered value types (3.13 panicked).
        if term.field().value_type() == ValueTypeTag::Duration {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_sort_unordered_value",
                "order terms require an ordered value type; durations have no total order",
            ));
        }
        if !fields.insert((
            term.field().binding(),
            term.field().descriptor(),
            term.field().attribute(),
        )) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_duplicate_order_field",
                "stable order fields must be unique",
            ));
        }
    }
    let mut previous = None;
    for binding in order.identity_tiebreakers() {
        check_binding(*binding, binding_count)?;
        if previous.is_some_and(|previous| previous >= *binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_tiebreakers_not_canonical",
                "identity tie breakers must be strictly ascending and unique",
            ));
        }
        previous = Some(*binding);
    }
    Ok(())
}

fn validate_model_output(
    output: &ModelOutput,
    hydration: &HydrationProjection,
    binding_count: usize,
    limits: QueryLimits,
) -> Result<(), Diagnostic> {
    if output.slot_count() == 0 || !limits.structural.allows_selected_slots(output.slot_count()) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_output_limit",
            "model output is empty or exceeds the selected-slot ceiling",
        ));
    }
    validate_hydration(hydration, binding_count, limits)?;
    let hydrated = hydration
        .bindings()
        .iter()
        .map(|binding| (binding.binding(), binding.declared_descriptor()))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut error = None;
    output.visit_slots(|name, slot| {
        if error.is_some() {
            return;
        }
        if let Some(name) = name
            && (name.is_empty()
                || name.len() > limits.structural.output_name_bytes
                || name.chars().any(char::is_control)
                || !names.insert(name.to_owned()))
        {
            error = Some(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_output_name",
                "named output members must be bounded, non-control, and unique",
            ));
            return;
        }
        let binding = slot.binding();
        if check_binding(binding, binding_count).is_err()
            || !selected.insert(binding)
            || hydrated.get(&binding).copied() != Some(slot.declared())
        {
            error = Some(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_output_binding",
                "model output bindings must be declared, unique, and hydrated",
            ));
            return;
        }
        if let ModelOutputSlot::Collect { binding, order, .. } = slot {
            if let Err(order_error) = validate_stable_order(
                order,
                binding_count,
                limits.structural.collection_order_terms,
            )
            .map_err(|diagnostic| limits.annotate_limit(diagnostic, "collection_order_terms"))
            {
                error = Some(order_error);
            } else if let Err(order_error) =
                validate_order_against_hydration(order, hydration, &BTreeSet::from([*binding]))
            {
                error = Some(order_error);
            } else if order.identity_tiebreakers() != [*binding] {
                error = Some(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_collection_order_tiebreaker",
                    "collection order must use the collected identity tie breaker",
                ));
            }
        }
    });
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

fn output_identity_tuple(output: &ModelOutput) -> Vec<BindingId> {
    let mut bindings = output
        .slots()
        .into_iter()
        .filter_map(|slot| match slot {
            ModelOutputSlot::One { binding, .. } => Some(*binding),
            ModelOutputSlot::Collect { .. } => None,
        })
        .collect::<Vec<_>>();
    bindings.sort_unstable();
    bindings
}

fn validate_order_against_hydration(
    order: &ModelStableOrder,
    hydration: &HydrationProjection,
    exposed_bindings: &BTreeSet<BindingId>,
) -> Result<(), Diagnostic> {
    let mut stable_unique_bindings = BTreeSet::new();
    for term in order.terms() {
        let field = term.field();
        if !exposed_bindings.contains(&field.binding()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_term_not_exposed",
                "stable order terms must refer to the response slot they order",
            ));
        }
        let Some(binding) = hydration
            .bindings()
            .iter()
            .find(|binding| binding.binding() == field.binding())
        else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_field_not_hydrated",
                "stable order fields require hydration authority for their binding",
            ));
        };
        let mut unique_on_every_concrete = true;
        for concrete in binding.concrete_descriptors() {
            let Some(descriptor) = hydration
                .descriptors()
                .iter()
                .find(|projection| projection.descriptor() == concrete)
            else {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_order_field_not_hydrated",
                    "stable order field concrete descriptor lacks hydration authority",
                ));
            };
            let Some(projected) = descriptor.fields().iter().find(|projected| {
                projected.reference_owners().contains(field.descriptor())
                    && projected.attribute() == field.attribute()
                    && projected.value_type() == field.value_type()
            }) else {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_order_field_claim",
                    "stable order field is absent or differently typed on an admitted concrete descriptor",
                ));
            };
            if projected.ordered() || projected.cardinality().max() != Some(1) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_non_scalar_order_field",
                    "stable order fields must be scalar descriptor ownerships",
                ));
            }
            unique_on_every_concrete &= projected.unique()
                && projected.cardinality().min() >= 1
                && projected.cardinality().max() == Some(1)
                && !projected.ordered();
        }
        if unique_on_every_concrete {
            stable_unique_bindings.insert(field.binding());
        }
    }
    for binding in order.identity_tiebreakers() {
        if !stable_unique_bindings.contains(binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_unproven_identity_tiebreaker",
                "each identity tie breaker requires one present unique scalar order field",
            ));
        }
    }
    Ok(())
}

fn validate_hydration(
    hydration: &HydrationProjection,
    binding_count: usize,
    limits: QueryLimits,
) -> Result<(), Diagnostic> {
    if hydration.bindings().is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_empty_hydration",
            "hydrated model output requires binding authority",
        ));
    }
    let mut previous_binding = None;
    let mut admitted = BTreeSet::new();
    for binding in hydration.bindings() {
        check_binding(binding.binding(), binding_count)?;
        if previous_binding.is_some_and(|previous| previous >= binding.binding()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_bindings_not_canonical",
                "hydration bindings must be strictly ascending and unique",
            ));
        }
        previous_binding = Some(binding.binding());
        if !matches!(
            binding.declared_descriptor().kind(),
            TypeKind::Entity | TypeKind::Relation
        ) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_binding",
                "hydration bindings require a thing descriptor",
            ));
        }
        let mut previous = None;
        for descriptor in binding.concrete_descriptors() {
            if descriptor.kind() != binding.declared_descriptor().kind()
                || previous.is_some_and(|previous: &TypeId| previous >= descriptor)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_concrete_descriptors_not_canonical",
                    "concrete descriptors must match kind and be strictly sorted",
                ));
            }
            previous = Some(descriptor);
            admitted.insert(descriptor.clone());
        }
    }

    let mut projections = BTreeMap::new();
    let mut previous_descriptor = None;
    for descriptor in hydration.descriptors() {
        if previous_descriptor
            .as_ref()
            .is_some_and(|previous| previous >= descriptor.descriptor())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_descriptors_not_canonical",
                "hydration descriptors must be strictly sorted and unique",
            ));
        }
        previous_descriptor = Some(descriptor.descriptor().clone());
        validate_hydration_descriptor(descriptor, limits)?;
        projections.insert(descriptor.descriptor().clone(), descriptor);
    }

    let full = admitted.clone();
    for concrete in &full {
        let Some(descriptor) = projections.get(concrete) else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_missing_hydration_descriptor",
                "every admitted concrete descriptor requires complete hydration authority",
            ));
        };
        for role in descriptor.roles() {
            for player in role.players() {
                for concrete in player.concrete_descriptors() {
                    if !projections.contains_key(concrete) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_missing_role_player_descriptor",
                            "every admitted concrete role player requires hydration authority",
                        ));
                    }
                    admitted.insert(concrete.clone());
                }
            }
        }
    }
    if projections
        .keys()
        .any(|descriptor| !admitted.contains(descriptor))
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_unadmitted_hydration_descriptor",
            "hydration descriptor is absent from binding or role-player closure",
        ));
    }
    Ok(())
}

fn validate_hydration_descriptor(
    descriptor: &HydrationDescriptor,
    limits: QueryLimits,
) -> Result<(), Diagnostic> {
    if !matches!(
        descriptor.descriptor().kind(),
        TypeKind::Entity | TypeKind::Relation
    ) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_hydration_descriptor_kind",
            "hydration descriptors must be entities or relations",
        ));
    }
    let mut aliases = BTreeSet::new();
    let mut attributes = BTreeSet::new();
    let mut previous_alias: Option<&str> = None;
    for field in descriptor.fields() {
        if field.reference_owners().is_empty()
            || field
                .reference_owners()
                .iter()
                .any(|owner| owner.kind() != descriptor.descriptor().kind())
            || field
                .reference_owners()
                .windows(2)
                .any(|owners| owners[0] >= owners[1])
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_field_owners_not_canonical",
                "hydration field reference owners must be nonempty sorted unique thing descriptors",
            ));
        }
        if field.distinct() && !field.ordered() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_field_distinct_requires_ordered",
                "distinct ownership requires ordered-list authority",
            ));
        }
        if field.alias().is_empty()
            || field.alias().len() > limits.structural.output_name_bytes
            || field.alias().chars().any(char::is_control)
            || previous_alias.is_some_and(|previous| previous >= field.alias())
            || !aliases.insert(field.alias())
            || !attributes.insert(field.attribute())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_fields_not_canonical",
                "hydration fields require unique sorted aliases and provider attributes",
            ));
        }
        previous_alias = Some(field.alias());
    }
    if descriptor.descriptor().kind() == TypeKind::Entity && !descriptor.roles().is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_entity_roles",
            "entity hydration projections cannot declare roles",
        ));
    }
    let mut previous_role = None;
    let mut admitted_role_references = BTreeSet::new();
    for role in descriptor.roles() {
        if role.distinct() && !role.ordered() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_distinct_requires_ordered",
                "distinct role players require ordered-list authority",
            ));
        }
        if previous_role
            .as_ref()
            .is_some_and(|previous| previous >= role.role())
            || role.role().declaring_relation() != descriptor.descriptor().label()
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_roles_not_canonical",
                "hydration roles must be owned, sorted, and unique",
            ));
        }
        previous_role = Some(role.role().clone());
        if role.reference_roles().is_empty()
            || !role.reference_roles().contains(role.role())
            || role
                .reference_roles()
                .iter()
                .any(|reference| reference.label() != role.role().label())
            || role
                .reference_roles()
                .windows(2)
                .any(|roles| roles[0] >= roles[1])
            || role
                .reference_roles()
                .iter()
                .any(|reference| !admitted_role_references.insert(reference.clone()))
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_references_not_canonical",
                "role references must include the provider role and be sorted, unique, same-label, and unambiguous",
            ));
        }
        let mut previous_declared = None;
        for player in role.players() {
            if !matches!(
                player.declared_descriptor().kind(),
                TypeKind::Entity | TypeKind::Relation
            ) || previous_declared
                .as_ref()
                .is_some_and(|previous| previous >= player.declared_descriptor())
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_role_players_not_canonical",
                    "role-player declarations must be sorted unique thing descriptors",
                ));
            }
            previous_declared = Some(player.declared_descriptor().clone());
            let mut previous_concrete = None;
            for concrete in player.concrete_descriptors() {
                if concrete.kind() != player.declared_descriptor().kind()
                    || previous_concrete
                        .as_ref()
                        .is_some_and(|previous| previous >= concrete)
                {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_v2_role_player_concretes_not_canonical",
                        "role-player concrete descriptors must be sorted unique and match the declared thing kind",
                    ));
                }
                previous_concrete = Some(concrete.clone());
            }
        }
    }
    Ok(())
}

fn add_pattern_capabilities(
    pattern: &ModelPredicate,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match pattern {
        ModelPredicate::FieldValue { comparator, .. } => {
            insert_capability(capabilities, super::CAP_VALUE)?;
            if comparator.is_string_operator() {
                insert_capability(capabilities, CAP_STRING_OPERATORS)?;
            }
        }
        ModelPredicate::FieldComparison { .. } => {
            insert_capability(capabilities, super::CAP_VALUE)?;
        }
        ModelPredicate::FieldPresence { .. } => {
            insert_capability(capabilities, super::CAP_HAS)?;
            // Both absence and existential presence lower through closed
            // negation patterns so cardinality-many values cannot multiply an
            // owner row.
            insert_capability(capabilities, super::CAP_NEGATION)?;
        }
        ModelPredicate::BindingIid { .. } => {
            insert_capability(capabilities, CAP_IID)?;
        }
        ModelPredicate::RoleEdge {
            include_relation_subtypes,
            ..
        } => {
            insert_capability(capabilities, super::CAP_LINKS)?;
            if *include_relation_subtypes {
                insert_capability(capabilities, CAP_LINKS_SUBTYPES)?;
            }
        }
        ModelPredicate::Reachable { .. } => {
            insert_capability(capabilities, super::CAP_REACHABLE)?;
        }
        ModelPredicate::And { patterns } => {
            for child in patterns {
                add_pattern_capabilities(child, capabilities)?;
            }
        }
        ModelPredicate::Or { patterns } => {
            insert_capability(capabilities, CAP_DISJUNCTION)?;
            for child in patterns {
                add_pattern_capabilities(child, capabilities)?;
            }
        }
        ModelPredicate::Not { pattern } => {
            insert_capability(capabilities, super::CAP_NEGATION)?;
            add_pattern_capabilities(pattern, capabilities)?;
        }
    }
    Ok(())
}

fn add_model_query_capabilities(
    query: &ModelQueryV2,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match query {
        ModelQueryV2::Rows {
            cardinality,
            output,
            ..
        } => {
            add_hydrated_output_capabilities(output, capabilities)?;
            match cardinality {
                QueryRowCardinalityV2::ExactlyOne => {
                    insert_capability(capabilities, CAP_EXACTLY_ONE)?;
                }
                QueryRowCardinalityV2::BoundedMany => {
                    insert_capability(capabilities, CAP_STABLE_SELECTED)?;
                }
                QueryRowCardinalityV2::Complete => {
                    insert_capability(capabilities, CAP_COMPLETE_ROWS)?;
                }
            }
        }
        ModelQueryV2::Page {
            include_total,
            output,
            ..
        } => {
            add_hydrated_output_capabilities(output, capabilities)?;
            insert_capability(capabilities, CAP_PAGE)?;
            insert_capability(capabilities, CAP_STABLE_ROOT)?;
            if *include_total {
                insert_capability(capabilities, CAP_DISTINCT_COUNT)?;
            }
        }
        ModelQueryV2::DistinctCount { .. } => {
            insert_capability(capabilities, CAP_DISTINCT_COUNT)?;
        }
        ModelQueryV2::DistinctExists { .. } => {
            insert_capability(capabilities, CAP_DISTINCT_EXISTS)?;
        }
        ModelQueryV2::Reduction { .. } => {
            insert_capability(capabilities, super::CAP_STAGE_REDUCE)?;
        }
    }
    Ok(())
}

fn add_hydrated_output_capabilities(
    output: &ModelOutput,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    insert_capability(capabilities, CAP_OUTPUT_HYDRATED)?;
    insert_capability(capabilities, CAP_SAME_SNAPSHOT_HYDRATION)?;
    insert_capability(capabilities, CAP_BATCH_IDENTITY_REBIND)?;
    if matches!(output, ModelOutput::Named { .. }) {
        insert_capability(capabilities, CAP_OUTPUT_NAMED)?;
    }
    output.visit_slots(|_, slot| {
        if let ModelOutputSlot::Collect { distinct, .. } = slot {
            let capability = if *distinct {
                CAP_OUTPUT_COLLECT_DISTINCT
            } else {
                CAP_OUTPUT_COLLECT
            };
            // All IDs above are fixed valid constants.
            capabilities.insert(
                crate::capability::CapabilityId::new(capability)
                    .expect("static V2 query capability is canonical"),
            );
            capabilities.insert(
                crate::capability::CapabilityId::new(CAP_STABLE_COLLECTION)
                    .expect("static V2 query capability is canonical"),
            );
        }
    });
    Ok(())
}

impl QueryRowCardinalityV2 {
    /// Return whether exactly one distinct selected tuple is required.
    #[must_use]
    pub const fn is_exactly_one(self) -> bool {
        matches!(self, Self::ExactlyOne)
    }
}
