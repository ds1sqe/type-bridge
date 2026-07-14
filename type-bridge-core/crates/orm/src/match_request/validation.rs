//! Fail-closed validation for canonical match requests.
//!
//! This module is the only constructor of [`ValidatedMatchRequest`]. It turns
//! serializable, forgeable input into an invocation-bound proof after checking
//! structural limits, descriptor ownership, expression topology, result shape,
//! stable ordering, and the request-relevant schema fingerprint.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use type_bridge_core_lib::decimal::parse_decimal;

use crate::attribute::ValueType;
use crate::descriptor::{OwnedAttributeDescriptor, TypeDescriptorRef};
use crate::entity::Annotation;
use crate::registry::{DescriptorFingerprintRoot, DescriptorRegistry};
use crate::value::AttributeValue;

use super::capability::{CapabilitySet, derive_required_capabilities};
use super::diagnostic::UnvalidatedMatchRequest;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPath, MatchErrorPathSegment};
use super::ids::{
    BindingId, BoundFieldId, DescriptorId, RequestToken, ResultShapeId, RoleEdgeId,
    SchemaFingerprint,
};
use super::limits::{
    CANONICAL_STRUCTURAL_LIMITS, MAX_ALLOWED_CROSS_JOINS, MAX_BINDINGS, MAX_BOOLEAN_TERMS,
    MAX_COLLECTION_ORDER_TERMS, MAX_ORDER_TERMS, MAX_OUTPUT_NAME_BYTES, MAX_PREDICATE_DEPTH,
    MAX_PREDICATE_NODES, MAX_SELECTED_SLOTS, MAX_SEMANTIC_ID_BYTES,
};
use super::model::{
    ComparisonOp, FetchShape, FetchSlot, MatchExpr, MatchMode, MatchOperation, MatchOrder,
    MatchRequest, MatchRequestVersion, RowCardinality, SortDirection, ThingKind,
};

static NEXT_REQUEST_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Construction authority held exclusively by canonical request validation.
///
/// The type is visible to the token implementation, but its field is private
/// so sibling modules cannot mint invocation evidence.
pub(super) struct RequestTokenIssuanceSeal(());

/// Whether a stable-order term was supplied publicly or synthesized from a
/// present unique scalar descriptor field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StableOrderOrigin {
    /// Caller-visible order term.
    Public,
    /// Canonical unique-key tie breaker.
    UniqueTieBreaker,
}

/// One term in the total order proven by request validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableOrderTerm {
    order: MatchOrder,
    origin: StableOrderOrigin,
}

impl StableOrderTerm {
    /// Return the canonical field, direction, and missing-value behavior.
    pub fn order(&self) -> &MatchOrder {
        &self.order
    }

    /// Return how this term entered the total order.
    pub const fn origin(&self) -> StableOrderOrigin {
        self.origin
    }
}

/// Complete stable row/root order derived during validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StableOrderSpec {
    terms: Vec<StableOrderTerm>,
}

impl StableOrderSpec {
    /// Return total-order terms in execution order.
    pub fn terms(&self) -> &[StableOrderTerm] {
        &self.terms
    }

    /// Return whether this operation has no ordering contract.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// A canonical request proven valid for one registry snapshot and invocation.
///
/// This value is deliberately non-serializable and has no public constructor.
#[derive(Debug)]
pub struct ValidatedMatchRequest {
    request: MatchRequest,
    request_token: RequestToken,
    schema_fingerprint: SchemaFingerprint,
    result_shape_id: ResultShapeId,
    stable_order: StableOrderSpec,
    collection_orders: BTreeMap<BindingId, StableOrderSpec>,
    required_capabilities: CapabilitySet,
}

impl ValidatedMatchRequest {
    /// Return the immutable canonical request.
    pub fn request(&self) -> &MatchRequest {
        &self.request
    }

    /// Return the request-relevant descriptor fingerprint proven at validation.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Return the exact validated result-shape identity.
    pub fn shape_id(&self) -> &ResultShapeId {
        &self.result_shape_id
    }

    /// Return the total stable order required by the operation.
    pub fn stable_order(&self) -> &StableOrderSpec {
        &self.stable_order
    }

    /// Return the validator-derived total order for one collected binding.
    ///
    /// The map is empty for row, count, and exists operations. Page executors
    /// must use this proof rather than inventing provider-side tie breakers.
    pub fn collection_order(&self, binding: BindingId) -> Option<&StableOrderSpec> {
        self.collection_orders.get(&binding)
    }

    /// Return the provider features derived by the normative capability matrix.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Recompute the request-relevant fingerprint immediately before execution.
    pub fn recheck_schema(&self, registry: &DescriptorRegistry) -> Result<(), MatchError> {
        let actual = fingerprint_for_request(registry, &self.request)?;
        if actual == self.schema_fingerprint {
            return Ok(());
        }
        Err(match_error(
            MatchErrorCategory::StaleSchema,
            "stale_schema",
            "request-relevant descriptors changed after request validation",
        )
        .at(MatchErrorPathSegment::Request)
        .with_detail("expected", self.schema_fingerprint.as_str())
        .with_detail("actual", actual.as_str()))
    }

    /// Fail provider preflight if any canonically required capability is absent.
    pub fn require_capabilities(&self, available: &CapabilitySet) -> Result<(), MatchError> {
        let missing = self.required_capabilities.missing_from(available);
        if missing.is_empty() {
            return Ok(());
        }
        Err(match_error(
            MatchErrorCategory::UnsupportedCapability,
            "missing_provider_capability",
            "provider does not advertise every capability required by this request",
        )
        .at(MatchErrorPathSegment::Operation)
        .with_detail(
            "missing",
            missing
                .iter()
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>(),
        ))
    }

    /// Return the invocation token only to trusted ORM executor/result code.
    pub(crate) const fn request_token(&self) -> RequestToken {
        self.request_token
    }
}

/// Validate one diagnostic-decoded request against the current registry.
impl UnvalidatedMatchRequest {
    /// Consume untrusted diagnostic input and run the complete canonical validator.
    pub fn validate(
        self,
        registry: &DescriptorRegistry,
    ) -> Result<ValidatedMatchRequest, MatchError> {
        validate_match_request(registry, self.into_parts().0)
    }
}

impl MatchRequest {
    /// Consume this forgeable request and run canonical validation.
    pub fn validate(
        self,
        registry: &DescriptorRegistry,
    ) -> Result<ValidatedMatchRequest, MatchError> {
        validate_match_request(registry, self)
    }
}

/// Validate one untrusted match request and issue an invocation-local proof.
pub fn validate_match_request(
    registry: &DescriptorRegistry,
    request: MatchRequest,
) -> Result<ValidatedMatchRequest, MatchError> {
    validate_version_and_structure(&request)?;
    let bindings = resolve_bindings(registry, &request)?;
    validate_expression(registry, &request, &bindings)?;
    validate_topology(&request)?;
    let (stable_order, collection_orders) = validate_operation(registry, &request, &bindings)?;
    let schema_fingerprint = fingerprint_for_request(registry, &request)?;
    let required_capabilities = derive_required_capabilities(&request);
    let result_shape_id = result_shape_id(&request)?;

    Ok(ValidatedMatchRequest {
        request,
        request_token: next_request_token(),
        schema_fingerprint,
        result_shape_id,
        stable_order,
        collection_orders,
        required_capabilities,
    })
}

fn validate_version_and_structure(request: &MatchRequest) -> Result<(), MatchError> {
    if request.version != MatchRequestVersion::V1 {
        return Err(invalid(
            "unsupported_request_version",
            "unsupported match request version",
        )
        .at(MatchErrorPathSegment::Request)
        .with_detail("actual", u64::from(request.version.get()))
        .with_detail("supported", u64::from(MatchRequestVersion::V1.get())));
    }

    let binding_count = request.plan.bindings.len();
    if binding_count == 0 {
        return Err(invalid(
            "empty_bindings",
            "match plan must declare at least one binding",
        )
        .at(MatchErrorPathSegment::Plan));
    }
    check_limit(
        "bindings",
        binding_count,
        MAX_BINDINGS,
        MatchErrorPathSegment::Plan,
    )?;
    check_limit(
        "allowed_cross_joins",
        request.plan.allowed_cross_joins.len(),
        MAX_ALLOWED_CROSS_JOINS,
        MatchErrorPathSegment::Plan,
    )?;

    for (index, binding) in request.plan.bindings.iter().enumerate() {
        let expected = u16::try_from(index).map_err(|_| {
            resource(
                "too_many_bindings",
                "binding ordinal exceeds the canonical ID range",
            )
        })?;
        if binding.id != BindingId::new(expected) {
            return Err(invalid(
                "non_canonical_binding_id",
                "binding IDs must be contiguous in canonical plan order",
            )
            .at(MatchErrorPathSegment::Binding(binding.id))
            .with_detail("expected", u64::from(expected))
            .with_detail("actual", u64::from(binding.id.get())));
        }
        validate_semantic_id("descriptor", binding.descriptor.as_str())?;
    }

    for pair in &request.plan.allowed_cross_joins {
        if pair.left >= pair.right {
            return Err(invalid(
                "non_canonical_cross_join",
                "cross-join pairs must contain two distinct bindings in ascending order",
            )
            .at(MatchErrorPathSegment::Plan));
        }
        require_binding(request, pair.left)?;
        require_binding(request, pair.right)?;
    }

    if let Some(predicate) = &request.plan.predicate {
        let mut stats = PredicateStats::default();
        inspect_predicate_structure(predicate, 1, &mut stats)?;
        check_limit(
            "predicate_nodes",
            stats.nodes,
            MAX_PREDICATE_NODES,
            MatchErrorPathSegment::Predicate,
        )?;
        check_limit(
            "predicate_depth",
            stats.max_depth,
            MAX_PREDICATE_DEPTH,
            MatchErrorPathSegment::Predicate,
        )?;
        let expected: Vec<_> = (0..stats.role_edge_ids.len())
            .map(|index| RoleEdgeId::new(u16::try_from(index).unwrap_or(u16::MAX)))
            .collect();
        if stats.role_edge_ids != expected {
            return Err(invalid(
                "non_canonical_role_edge_ids",
                "role-edge IDs must be unique and contiguous from zero",
            )
            .at(MatchErrorPathSegment::Predicate));
        }
    }

    validate_operation_structure(request)?;
    Ok(())
}

#[derive(Default)]
struct PredicateStats {
    nodes: usize,
    max_depth: usize,
    role_edge_ids: Vec<RoleEdgeId>,
    seen_role_edge_ids: BTreeSet<RoleEdgeId>,
}

fn inspect_predicate_structure(
    expression: &MatchExpr,
    depth: usize,
    stats: &mut PredicateStats,
) -> Result<(), MatchError> {
    stats.nodes += 1;
    stats.max_depth = stats.max_depth.max(depth);
    if stats.nodes > MAX_PREDICATE_NODES || depth > MAX_PREDICATE_DEPTH {
        return Err(resource(
            "predicate_limit_exceeded",
            "predicate tree exceeds canonical size or depth limits",
        )
        .at(MatchErrorPathSegment::Predicate));
    }
    match expression {
        MatchExpr::RoleEdge { id, role, .. } => {
            if !stats.seen_role_edge_ids.insert(*id) {
                return Err(
                    invalid("duplicate_role_edge_id", "role-edge IDs must be unique")
                        .at(MatchErrorPathSegment::RoleEdge(*id)),
                );
            }
            stats.role_edge_ids.push(*id);
            validate_qualified_member("role", role.owner.as_str(), &role.name)?;
        }
        MatchExpr::FieldValue { field, value, .. } => {
            validate_bound_field_id(field)?;
            validate_safe_value(value)?;
        }
        MatchExpr::FieldComparison { left, right, .. } => {
            validate_bound_field_id(left)?;
            validate_bound_field_id(right)?;
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            if expressions.is_empty() {
                return Err(invalid(
                    "empty_boolean_expression",
                    "and/or expressions must contain at least one child",
                )
                .at(MatchErrorPathSegment::Predicate));
            }
            check_limit(
                "boolean_terms",
                expressions.len(),
                MAX_BOOLEAN_TERMS,
                MatchErrorPathSegment::Predicate,
            )?;
            for child in expressions {
                inspect_predicate_structure(child, depth + 1, stats)?;
            }
        }
        MatchExpr::Not { expression } => inspect_predicate_structure(expression, depth + 1, stats)?,
    }
    Ok(())
}

fn validate_operation_structure(request: &MatchRequest) -> Result<(), MatchError> {
    let (output, order, window) = match &request.operation {
        MatchOperation::FetchRows {
            output,
            order,
            window,
            cardinality,
        } => {
            if *cardinality == RowCardinality::ExactlyOne
                && (window.offset != 0 || window.limit != 1)
            {
                return Err(invalid(
                    "invalid_exactly_one_window",
                    "exactly-one fetches require offset zero and limit one",
                )
                .at(MatchErrorPathSegment::Operation));
            }
            (Some(output), Some(order), Some(window))
        }
        MatchOperation::PageBy {
            output,
            order,
            window,
            ..
        } => (Some(output), Some(order), Some(window)),
        MatchOperation::CountBy { root } | MatchOperation::ExistsBy { root } => {
            require_binding(request, *root)?;
            (None, None, None)
        }
    };

    if let Some(window) = window {
        if window.limit == 0 {
            return Err(
                invalid("zero_window_limit", "row and page limits must be positive")
                    .at(MatchErrorPathSegment::Operation),
            );
        }
        window.offset.checked_add(window.limit).ok_or_else(|| {
            invalid(
                "window_overflow",
                "window offset plus limit exceeds the canonical unsigned range",
            )
            .at(MatchErrorPathSegment::Operation)
        })?;
    }
    if let Some(order) = order {
        check_limit(
            "order_terms",
            order.len(),
            MAX_ORDER_TERMS,
            MatchErrorPathSegment::Operation,
        )?;
        for term in order {
            validate_bound_field_id(&term.field)?;
        }
    }
    if let Some(output) = output {
        let count = output.slot_count();
        if count == 0 {
            return Err(invalid(
                "empty_output",
                "fetch output must select at least one binding",
            )
            .at(MatchErrorPathSegment::Output));
        }
        if count > MAX_SELECTED_SLOTS {
            return Err(invalid(
                "selection_cap_exceeded",
                "fetch output exceeds the canonical sixteen-slot ceiling",
            )
            .at(MatchErrorPathSegment::Output)
            .with_detail("actual", usize_detail(count))
            .with_detail("maximum", usize_detail(MAX_SELECTED_SLOTS)));
        }
        let mut selected = BTreeSet::new();
        output.for_each_slot(|slot| {
            selected.insert(slot.binding());
        });
        if selected.len() != count {
            return Err(invalid(
                "duplicate_selection",
                "one binding cannot occupy more than one output slot",
            )
            .at(MatchErrorPathSegment::Output));
        }
        output.for_each_slot(|slot| {
            if let FetchSlot::Collect { order, .. } = slot {
                // The actual failure is produced in the fallible pass below.
                let _ = order;
            }
        });
        for (index, slot) in output_slots(output).enumerate() {
            require_binding(request, slot.binding())
                .map_err(|error| error.at(MatchErrorPathSegment::OutputSlot(index)))?;
            if let FetchSlot::Collect { order, .. } = slot {
                check_limit(
                    "collection_order_terms",
                    order.len(),
                    MAX_COLLECTION_ORDER_TERMS,
                    MatchErrorPathSegment::OutputSlot(index),
                )?;
                for term in order {
                    validate_bound_field_id(&term.field)?;
                }
            }
        }
        if let FetchShape::Named { slots } = output {
            let mut names = BTreeSet::new();
            for (index, named) in slots.iter().enumerate() {
                if named.name.is_empty()
                    || named.name.len() > MAX_OUTPUT_NAME_BYTES
                    || named.name.chars().any(char::is_control)
                {
                    return Err(invalid(
                        "invalid_output_name",
                        "named output members must be non-empty and within the byte ceiling",
                    )
                    .at(MatchErrorPathSegment::OutputSlot(index)));
                }
                if !names.insert(named.name.as_str()) {
                    return Err(invalid(
                        "duplicate_output_name",
                        "named output members must be unique",
                    )
                    .at(MatchErrorPathSegment::OutputName(named.name.clone())));
                }
            }
        }
    }
    Ok(())
}

fn resolve_bindings(
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<BTreeMap<BindingId, TypeDescriptorRef>, MatchError> {
    request
        .plan
        .bindings
        .iter()
        .map(|binding| {
            let descriptor = resolve_descriptor(registry, &binding.descriptor)
                .map_err(|error| error.at(MatchErrorPathSegment::Binding(binding.id)))?;
            let actual_kind = descriptor_kind(&descriptor);
            if actual_kind != binding.thing_kind {
                return Err(invalid(
                    "descriptor_kind_mismatch",
                    "binding kind does not match its registered descriptor",
                )
                .at(MatchErrorPathSegment::Binding(binding.id)));
            }
            Ok((binding.id, descriptor))
        })
        .collect()
}

fn validate_expression(
    registry: &DescriptorRegistry,
    request: &MatchRequest,
    bindings: &BTreeMap<BindingId, TypeDescriptorRef>,
) -> Result<(), MatchError> {
    let Some(predicate) = &request.plan.predicate else {
        return Ok(());
    };
    validate_expr_node(registry, predicate, bindings)?;
    validate_or_exports(predicate)?;
    Ok(())
}

fn validate_expr_node(
    registry: &DescriptorRegistry,
    expression: &MatchExpr,
    bindings: &BTreeMap<BindingId, TypeDescriptorRef>,
) -> Result<(), MatchError> {
    match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => {
            let attribute = resolve_bound_field(registry, field, bindings)?;
            if attribute.value_type.as_str() != value.value_type_name() {
                return Err(invalid(
                    "literal_type_mismatch",
                    "literal value type does not match the descriptor field",
                )
                .at(MatchErrorPathSegment::Field(field.field.clone())));
            }
            validate_operator(*operator, attribute.value_type, true)
                .map_err(|error| error.at(MatchErrorPathSegment::Field(field.field.clone())))?;
            if *operator == ComparisonOp::Regex {
                let AttributeValue::String(pattern) = value else {
                    unreachable!("value-type validation requires a string regex literal")
                };
                regex::Regex::new(pattern).map_err(|error| {
                    invalid(
                        "invalid_regex",
                        "regex predicate must contain a syntactically valid regular expression",
                    )
                    .at(MatchErrorPathSegment::Field(field.field.clone()))
                    .with_detail("cause", error.to_string())
                })?;
            }
        }
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => {
            let left_attr = resolve_bound_field(registry, left, bindings)?;
            let right_attr = resolve_bound_field(registry, right, bindings)?;
            if left_attr.value_type != right_attr.value_type {
                return Err(invalid(
                    "field_type_mismatch",
                    "field comparison requires identical descriptor value types",
                )
                .at(MatchErrorPathSegment::Field(right.field.clone())));
            }
            validate_operator(*operator, left_attr.value_type, false)
                .map_err(|error| error.at(MatchErrorPathSegment::Field(left.field.clone())))?;
        }
        MatchExpr::RoleEdge {
            id,
            relation,
            role,
            player,
        } => {
            let relation_descriptor = bindings.get(relation).ok_or_else(|| {
                invalid(
                    "unknown_binding",
                    "role edge references an undeclared relation binding",
                )
                .at(MatchErrorPathSegment::RoleEdge(*id))
            })?;
            let TypeDescriptorRef::Relation(relation_descriptor) = relation_descriptor else {
                return Err(invalid(
                    "role_owner_not_relation",
                    "role-edge owner binding must target a relation",
                )
                .at(MatchErrorPathSegment::RoleEdge(*id)));
            };
            let relation_id = registry
                .descriptor_id(&relation_descriptor.type_name)
                .expect("resolved descriptor remains registered");
            if !registry.is_same_or_subtype(&relation_id, &role.owner) {
                return Err(invalid(
                    "cross_owner_role",
                    "role identity is not owned by the relation binding descriptor or one of its ancestors",
                )
                .at(MatchErrorPathSegment::Role(role.clone())));
            }
            let canonical_role = registry.role_id(&role.owner, &role.name).ok_or_else(|| {
                invalid(
                    "unknown_role",
                    "role is not present on the relation descriptor",
                )
                .at(MatchErrorPathSegment::Role(role.clone()))
            })?;
            if canonical_role != *role {
                return Err(
                    invalid("non_canonical_role", "role identity is not canonical")
                        .at(MatchErrorPathSegment::Role(role.clone())),
                );
            }
            if !registry.role_reference_is_compatible(&relation_id, &role.owner, &role.name) {
                return Err(invalid(
                    "cross_owner_role",
                    "role identity does not denote an effective inherited role on the relation binding descriptor",
                )
                .at(MatchErrorPathSegment::Role(role.clone())));
            }
            let player_descriptor = bindings.get(player).ok_or_else(|| {
                invalid(
                    "unknown_binding",
                    "role edge references an undeclared player binding",
                )
                .at(MatchErrorPathSegment::RoleEdge(*id))
            })?;
            let player_name = player_descriptor.type_name();
            let descriptor_role = relation_descriptor.role(&role.name).expect("resolved role");
            if !descriptor_role
                .player_type_names
                .iter()
                .any(|allowed| is_type_or_subtype(registry, player_name, allowed))
            {
                return Err(invalid(
                    "incompatible_role_player",
                    "player binding is not compatible with the relation role",
                )
                .at(MatchErrorPathSegment::RoleEdge(*id)));
            }
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for child in expressions {
                validate_expr_node(registry, child, bindings)?;
            }
        }
        MatchExpr::Not { expression } => validate_expr_node(registry, expression, bindings)?,
    }
    Ok(())
}

fn validate_or_exports(expression: &MatchExpr) -> Result<BTreeSet<BindingId>, MatchError> {
    match expression {
        MatchExpr::FieldValue { field, .. } => Ok(BTreeSet::from([field.binding])),
        MatchExpr::FieldComparison { left, right, .. } => {
            Ok(BTreeSet::from([left.binding, right.binding]))
        }
        MatchExpr::RoleEdge {
            relation, player, ..
        } => Ok(BTreeSet::from([*relation, *player])),
        MatchExpr::And { expressions } => {
            let mut definite = BTreeSet::new();
            for child in expressions {
                definite.extend(validate_or_exports(child)?);
            }
            Ok(definite)
        }
        MatchExpr::Or { expressions } => {
            let mut children = expressions.iter();
            let first = validate_or_exports(children.next().expect("structure checked"))?;
            for child in children {
                let branch = validate_or_exports(child)?;
                if branch != first {
                    return Err(invalid(
                        "partial_or_binding",
                        "every OR branch must definitely bind the same variables",
                    )
                    .at(MatchErrorPathSegment::Predicate));
                }
            }
            Ok(first)
        }
        MatchExpr::Not { expression } => {
            validate_or_exports(expression)?;
            Ok(BTreeSet::new())
        }
    }
}

fn validate_topology(request: &MatchRequest) -> Result<(), MatchError> {
    if request.plan.bindings.len() <= 1 {
        return Ok(());
    }
    let mut adjacency: BTreeMap<BindingId, BTreeSet<BindingId>> = request
        .plan
        .bindings
        .iter()
        .map(|binding| (binding.id, BTreeSet::new()))
        .collect();
    if let Some(predicate) = &request.plan.predicate {
        for (left, right) in definite_positive_edges(predicate, false) {
            adjacency.entry(left).or_default().insert(right);
            adjacency.entry(right).or_default().insert(left);
        }
    }
    for pair in &request.plan.allowed_cross_joins {
        adjacency.entry(pair.left).or_default().insert(pair.right);
        adjacency.entry(pair.right).or_default().insert(pair.left);
    }

    let start = request.plan.bindings[0].id;
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(binding) = queue.pop_front() {
        if visited.insert(binding) {
            queue.extend(adjacency[&binding].iter().copied());
        }
    }
    if visited.len() != request.plan.bindings.len() {
        return Err(invalid(
            "disconnected_plan",
            "positive match graph is disconnected without explicit cross-join permission",
        )
        .at(MatchErrorPathSegment::Plan)
        .with_detail(
            "unreachable_bindings",
            request
                .plan
                .bindings
                .iter()
                .filter(|binding| !visited.contains(&binding.id))
                .map(|binding| binding.id.to_string())
                .collect::<Vec<_>>(),
        ));
    }
    Ok(())
}

fn definite_positive_edges(
    expression: &MatchExpr,
    negated: bool,
) -> BTreeSet<(BindingId, BindingId)> {
    if negated {
        return BTreeSet::new();
    }
    match expression {
        MatchExpr::FieldValue { .. } => BTreeSet::new(),
        MatchExpr::FieldComparison { left, right, .. } => {
            BTreeSet::from([canonical_edge(left.binding, right.binding)])
        }
        MatchExpr::RoleEdge {
            relation, player, ..
        } => BTreeSet::from([canonical_edge(*relation, *player)]),
        MatchExpr::And { expressions } => expressions
            .iter()
            .flat_map(|child| definite_positive_edges(child, false))
            .collect(),
        MatchExpr::Or { expressions } => {
            let mut children = expressions.iter();
            let Some(first) = children.next() else {
                return BTreeSet::new();
            };
            children.fold(definite_positive_edges(first, false), |common, child| {
                common
                    .intersection(&definite_positive_edges(child, false))
                    .copied()
                    .collect()
            })
        }
        MatchExpr::Not { expression } => definite_positive_edges(expression, true),
    }
}

fn validate_operation(
    registry: &DescriptorRegistry,
    request: &MatchRequest,
    bindings: &BTreeMap<BindingId, TypeDescriptorRef>,
) -> Result<(StableOrderSpec, BTreeMap<BindingId, StableOrderSpec>), MatchError> {
    match &request.operation {
        MatchOperation::FetchRows {
            output,
            order,
            cardinality,
            ..
        } => {
            for (index, slot) in output_slots(output).enumerate() {
                if slot.is_collection() {
                    return Err(invalid(
                        "collection_requires_page_root",
                        "collected output is valid only for distinct-root page operations",
                    )
                    .at(MatchErrorPathSegment::OutputSlot(index)));
                }
            }
            let selected: BTreeSet<_> = output_slots(output).map(FetchSlot::binding).collect();
            validate_order_scope(registry, order, bindings, &selected, "selected")?;
            if *cardinality == RowCardinality::ExactlyOne {
                Ok((StableOrderSpec::default(), BTreeMap::new()))
            } else {
                Ok((
                    derive_stable_order(registry, order, bindings, selected.iter().copied())?,
                    BTreeMap::new(),
                ))
            }
        }
        MatchOperation::PageBy {
            root,
            output,
            order,
            ..
        } => {
            let mut root_slots = 0;
            let mut collection_orders = BTreeMap::new();
            for (index, slot) in output_slots(output).enumerate() {
                if slot.binding() == *root {
                    if slot.is_collection() {
                        return Err(invalid(
                            "collected_page_root",
                            "page root must be a singular output slot",
                        )
                        .at(MatchErrorPathSegment::OutputSlot(index)));
                    }
                    root_slots += 1;
                } else if !slot.is_collection() {
                    return Err(invalid(
                        "singular_non_root_page_slot",
                        "every non-root page output must be collected",
                    )
                    .at(MatchErrorPathSegment::OutputSlot(index)));
                }
                if let FetchSlot::Collect { binding, order, .. } = slot {
                    validate_order_scope(
                        registry,
                        order,
                        bindings,
                        &BTreeSet::from([*binding]),
                        "collection",
                    )?;
                    collection_orders.insert(
                        *binding,
                        derive_stable_order(registry, order, bindings, [*binding])?,
                    );
                }
            }
            if root_slots != 1 {
                return Err(invalid(
                    "missing_page_root",
                    "page root must appear exactly once as a singular output slot",
                )
                .at(MatchErrorPathSegment::Output));
            }
            validate_order_scope(registry, order, bindings, &BTreeSet::from([*root]), "root")?;
            Ok((
                derive_stable_order(registry, order, bindings, [*root])?,
                collection_orders,
            ))
        }
        MatchOperation::CountBy { .. } | MatchOperation::ExistsBy { .. } => {
            Ok((StableOrderSpec::default(), BTreeMap::new()))
        }
    }
}

fn validate_order_scope(
    registry: &DescriptorRegistry,
    order: &[MatchOrder],
    bindings: &BTreeMap<BindingId, TypeDescriptorRef>,
    allowed: &BTreeSet<BindingId>,
    scope: &'static str,
) -> Result<(), MatchError> {
    let mut fields = BTreeSet::new();
    for term in order {
        if !allowed.contains(&term.field.binding) {
            return Err(invalid(
                "order_scope_mismatch",
                "order field is outside the operation's permitted binding scope",
            )
            .at(MatchErrorPathSegment::Field(term.field.field.clone()))
            .with_detail("scope", scope));
        }
        let attribute = resolve_bound_field(registry, &term.field, bindings)?;
        if attribute.is_ordered
            || attribute
                .cardinality()
                .is_some_and(|(_, max)| max != Some(1))
        {
            return Err(invalid(
                "non_scalar_order_field",
                "order fields must be scalar descriptor ownerships",
            )
            .at(MatchErrorPathSegment::Field(term.field.field.clone())));
        }
        if !fields.insert(term.field.clone()) {
            return Err(invalid(
                "duplicate_order_field",
                "one order field cannot appear twice",
            )
            .at(MatchErrorPathSegment::Field(term.field.field.clone())));
        }
    }
    Ok(())
}

fn derive_stable_order(
    registry: &DescriptorRegistry,
    public: &[MatchOrder],
    bindings: &BTreeMap<BindingId, TypeDescriptorRef>,
    identity_bindings: impl IntoIterator<Item = BindingId>,
) -> Result<StableOrderSpec, MatchError> {
    let mut terms: Vec<_> = public
        .iter()
        .cloned()
        .map(|order| StableOrderTerm {
            order,
            origin: StableOrderOrigin::Public,
        })
        .collect();
    let public_fields: BTreeSet<_> = public.iter().map(|term| term.field.clone()).collect();

    for binding in identity_bindings {
        let descriptor = bindings.get(&binding).expect("operation binding resolved");
        let mut candidates: Vec<_> = descriptor_attributes(descriptor)
            .iter()
            .filter(|attribute| is_stable_unique_scalar(attribute))
            .collect();
        candidates.sort_by(|left, right| left.field_name.cmp(&right.field_name));
        let candidate = candidates.first().ok_or_else(|| {
            invalid(
                "missing_stable_unique_key",
                "bounded result identity requires a present unique scalar descriptor field",
            )
            .at(MatchErrorPathSegment::Binding(binding))
        })?;
        let descriptor_id = registry
            .descriptor_id(descriptor.type_name())
            .expect("resolved descriptor remains registered");
        let bound = BoundFieldId::new(
            binding,
            registry
                .field_id(&descriptor_id, &candidate.field_name)
                .expect("candidate belongs to descriptor"),
        );
        if !public_fields.contains(&bound) {
            terms.push(StableOrderTerm {
                order: MatchOrder {
                    field: bound,
                    direction: SortDirection::Ascending,
                    missing: super::model::MissingOrder::Reject,
                },
                origin: StableOrderOrigin::UniqueTieBreaker,
            });
        }
    }
    Ok(StableOrderSpec { terms })
}

fn resolve_bound_field<'a>(
    registry: &DescriptorRegistry,
    field: &BoundFieldId,
    bindings: &'a BTreeMap<BindingId, TypeDescriptorRef>,
) -> Result<&'a OwnedAttributeDescriptor, MatchError> {
    let descriptor = bindings.get(&field.binding).ok_or_else(|| {
        invalid("unknown_binding", "field references an undeclared binding")
            .at(MatchErrorPathSegment::Field(field.field.clone()))
    })?;
    let descriptor_id = registry
        .descriptor_id(descriptor.type_name())
        .expect("resolved descriptor remains registered");
    if !registry.is_same_or_subtype(&descriptor_id, &field.field.owner) {
        return Err(invalid(
            "cross_owner_field",
            "field identity is not owned by its bound descriptor or one of its ancestors",
        )
        .at(MatchErrorPathSegment::Field(field.field.clone())));
    }
    let canonical = registry
        .field_id(&field.field.owner, &field.field.name)
        .ok_or_else(|| {
            invalid(
                "unknown_field",
                "field is not present on its reference owner descriptor",
            )
            .at(MatchErrorPathSegment::Field(field.field.clone()))
        })?;
    if canonical != field.field {
        return Err(
            invalid("non_canonical_field", "field identity is not canonical")
                .at(MatchErrorPathSegment::Field(field.field.clone())),
        );
    }
    if !registry.field_reference_is_compatible(
        &descriptor_id,
        &field.field.owner,
        &field.field.name,
    ) {
        return Err(invalid(
            "cross_owner_field",
            "field identity does not denote an effective inherited field on its bound descriptor",
        )
        .at(MatchErrorPathSegment::Field(field.field.clone())));
    }
    descriptor_attributes(descriptor)
        .iter()
        .find(|attribute| attribute.field_name == field.field.name)
        .ok_or_else(|| invalid("unknown_field", "canonical field lookup failed closed"))
}

fn resolve_descriptor(
    registry: &DescriptorRegistry,
    id: &DescriptorId,
) -> Result<TypeDescriptorRef, MatchError> {
    let Some((kind, type_name)) = id.as_str().split_once(':') else {
        return Err(invalid(
            "malformed_descriptor_id",
            "descriptor identity must contain a kind-qualified type name",
        ));
    };
    if type_name.is_empty() || !matches!(kind, "entity" | "relation") {
        return Err(invalid(
            "malformed_descriptor_id",
            "descriptor identity has an unsupported kind or empty type name",
        ));
    }
    let descriptor = registry.get(type_name).ok_or_else(|| {
        invalid(
            "unknown_descriptor",
            "binding descriptor is not registered in the current schema",
        )
        .with_detail("descriptor", id.as_str())
    })?;
    if registry.descriptor_id(type_name).as_ref() != Some(id) {
        return Err(invalid(
            "descriptor_kind_mismatch",
            "descriptor identity kind does not match the registered descriptor",
        ));
    }
    Ok(descriptor)
}

fn descriptor_kind(descriptor: &TypeDescriptorRef) -> ThingKind {
    match descriptor {
        TypeDescriptorRef::Entity(_) => ThingKind::Entity,
        TypeDescriptorRef::Relation(_) => ThingKind::Relation,
    }
}

fn descriptor_attributes(descriptor: &TypeDescriptorRef) -> &[OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

fn is_type_or_subtype(registry: &DescriptorRegistry, actual: &str, expected: &str) -> bool {
    let mut current = Some(actual.to_owned());
    let mut visited = BTreeSet::new();
    while let Some(type_name) = current {
        if type_name == expected {
            return true;
        }
        if !visited.insert(type_name.clone()) {
            return false;
        }
        current = registry
            .get(&type_name)
            .and_then(|descriptor| match descriptor {
                TypeDescriptorRef::Entity(descriptor) => descriptor.parent_type.clone(),
                TypeDescriptorRef::Relation(descriptor) => descriptor.parent_type.clone(),
            });
    }
    false
}

fn validate_operator(
    operator: ComparisonOp,
    value_type: ValueType,
    literal_rhs: bool,
) -> Result<(), MatchError> {
    let valid = match value_type {
        ValueType::String if literal_rhs => true,
        ValueType::String => !matches!(
            operator,
            ComparisonOp::Contains
                | ComparisonOp::StartsWith
                | ComparisonOp::EndsWith
                | ComparisonOp::Regex
        ),
        ValueType::Boolean => matches!(operator, ComparisonOp::Equal | ComparisonOp::NotEqual),
        _ => !matches!(
            operator,
            ComparisonOp::Contains
                | ComparisonOp::StartsWith
                | ComparisonOp::EndsWith
                | ComparisonOp::Regex
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "invalid_operator_for_type",
            "comparison operator is not supported by the descriptor value type",
        ))
    }
}

fn validate_safe_value(value: &AttributeValue) -> Result<(), MatchError> {
    let valid = match value {
        AttributeValue::Double(value) => value.is_finite(),
        AttributeValue::Date(value) => valid_date(value),
        AttributeValue::DateTime(value) => valid_datetime(value, false),
        AttributeValue::DateTimeTZ(value) => valid_datetime(value, true),
        AttributeValue::Decimal(value) => parse_decimal(value).is_some(),
        AttributeValue::Duration(value) => valid_duration(value),
        AttributeValue::String(_) | AttributeValue::Long(_) | AttributeValue::Boolean(_) => true,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "unsafe_literal",
            "literal contains a non-finite number or malformed canonical temporal/decimal value",
        )
        .at(MatchErrorPathSegment::Predicate))
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day >= 1 && day <= days[(month - 1) as usize]
}

fn valid_datetime(value: &str, timezone_required: bool) -> bool {
    let Some((date, time)) = value.split_once('T') else {
        return false;
    };
    if !valid_date(date) {
        return false;
    }
    let (clock, has_timezone) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, true)
    } else if let Some(index) = time
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))
    {
        let (clock, offset) = time.split_at(index);
        (clock, valid_clock(&offset[1..]))
    } else {
        (time, false)
    };
    valid_clock(clock) && has_timezone == timezone_required
}

fn valid_clock(value: &str) -> bool {
    let main = value.split_once('.').map_or(value, |(main, fraction)| {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            ""
        } else {
            main
        }
    });
    let parts: Vec<_> = main.split(':').collect();
    if parts.len() != 3 && parts.len() != 2 {
        return false;
    }
    let Ok(hour) = parts[0].parse::<u32>() else {
        return false;
    };
    let Ok(minute) = parts[1].parse::<u32>() else {
        return false;
    };
    let second = if parts.len() == 3 {
        let Ok(second) = parts[2].parse::<u32>() else {
            return false;
        };
        second
    } else {
        0
    };
    hour <= 23 && minute <= 59 && second <= 59
}

fn valid_duration(value: &str) -> bool {
    value.starts_with('P')
        && value.len() > 1
        && value.bytes().skip(1).all(|byte| {
            byte.is_ascii_digit() || matches!(byte, b'Y' | b'M' | b'D' | b'T' | b'H' | b'S' | b'.')
        })
        && value.bytes().skip(1).any(|byte| byte.is_ascii_digit())
}

fn is_stable_unique_scalar(attribute: &OwnedAttributeDescriptor) -> bool {
    let unique = attribute
        .annotations
        .iter()
        .any(|annotation| matches!(annotation, Annotation::Key | Annotation::Unique));
    let scalar = !attribute.is_ordered
        && attribute
            .cardinality()
            .map_or(!attribute.is_optional, |(minimum, maximum)| {
                minimum >= 1 && maximum == Some(1)
            });
    unique && scalar && !attribute.is_optional
}

fn fingerprint_for_request(
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<SchemaFingerprint, MatchError> {
    let roots = request
        .plan
        .bindings
        .iter()
        .map(|binding| {
            DescriptorFingerprintRoot::new(
                binding.descriptor.clone(),
                binding.match_mode == MatchMode::Subtypes,
            )
        })
        .collect::<Vec<_>>();
    registry
        .request_relevant_fingerprint(&roots)
        .map_err(|error| {
            invalid(
                "schema_fingerprint_failed",
                "request-relevant descriptor closure could not be fingerprinted",
            )
            .at(MatchErrorPathSegment::Request)
            .with_detail("cause", error.to_string())
        })
}

fn result_shape_id(request: &MatchRequest) -> Result<ResultShapeId, MatchError> {
    let encoded = serde_json::to_vec(&request.operation).map_err(|error| {
        invalid(
            "shape_encode_failed",
            "validated result shape could not be canonically encoded",
        )
        .with_detail("cause", error.to_string())
    })?;
    let digest = Sha256::digest(encoded);
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ResultShapeId::new(format!("shape-sha256-v1:{digest}")))
}

fn next_request_token() -> RequestToken {
    let ordinal = NEXT_REQUEST_TOKEN.fetch_add(1, Ordering::Relaxed);
    let mut bytes = [0_u8; 16];
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    RequestToken::issue(bytes, RequestTokenIssuanceSeal(()))
}

#[cfg(test)]
pub(super) const fn request_token_for_test(bytes: [u8; 16]) -> RequestToken {
    RequestToken::issue(bytes, RequestTokenIssuanceSeal(()))
}

fn validate_bound_field_id(field: &BoundFieldId) -> Result<(), MatchError> {
    validate_qualified_member("field", field.field.owner.as_str(), &field.field.name)
}

fn validate_qualified_member(
    label: &'static str,
    owner: &str,
    member: &str,
) -> Result<(), MatchError> {
    validate_semantic_id("member_owner", owner)?;
    validate_semantic_id(label, member)?;
    let actual = owner
        .len()
        .checked_add(1)
        .and_then(|length| length.checked_add(member.len()))
        .unwrap_or(usize::MAX);
    if actual > MAX_SEMANTIC_ID_BYTES {
        return Err(invalid(
            "invalid_semantic_id",
            "qualified semantic identity exceeds the canonical byte ceiling",
        )
        .with_detail("identity_kind", label)
        .with_detail("actual_bytes", usize_detail(actual)));
    }
    Ok(())
}

fn validate_semantic_id(label: &'static str, value: &str) -> Result<(), MatchError> {
    if value.is_empty()
        || value.len() > MAX_SEMANTIC_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "invalid_semantic_id",
            "semantic identities must be non-empty, bounded UTF-8 without control characters",
        )
        .with_detail("identity_kind", label)
        .with_detail("actual_bytes", usize_detail(value.len())));
    }
    Ok(())
}

fn require_binding(request: &MatchRequest, binding: BindingId) -> Result<(), MatchError> {
    if request
        .plan
        .bindings
        .iter()
        .any(|candidate| candidate.id == binding)
    {
        Ok(())
    } else {
        Err(invalid(
            "unknown_binding",
            "request references an undeclared binding",
        )
        .at(MatchErrorPathSegment::Binding(binding)))
    }
}

fn output_slots(output: &FetchShape) -> Box<dyn Iterator<Item = &FetchSlot> + '_> {
    match output {
        FetchShape::Positional { slots } => Box::new(slots.iter()),
        FetchShape::Named { slots } => Box::new(slots.iter().map(|named| &named.slot)),
    }
}

fn canonical_edge(left: BindingId, right: BindingId) -> (BindingId, BindingId) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn check_limit(
    label: &'static str,
    actual: usize,
    maximum: usize,
    path: MatchErrorPathSegment,
) -> Result<(), MatchError> {
    if actual <= maximum {
        return Ok(());
    }
    Err(resource(
        "structural_limit_exceeded",
        "request structure exceeds a canonical protocol ceiling",
    )
    .with_path(MatchErrorPath::from_segments([path]))
    .with_detail("limit", label)
    .with_detail("actual", usize_detail(actual))
    .with_detail("maximum", usize_detail(maximum)))
}

fn invalid(code: &'static str, message: &'static str) -> MatchError {
    match_error(MatchErrorCategory::InvalidPlan, code, message)
}

fn resource(code: &'static str, message: &'static str) -> MatchError {
    match_error(MatchErrorCategory::ResourceLimit, code, message)
}

fn match_error(
    category: MatchErrorCategory,
    code: &'static str,
    message: &'static str,
) -> MatchError {
    MatchError::new(category, code, message)
}

fn usize_detail(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[allow(dead_code)]
const _: () = {
    // Keep the struct-level canonical limits auditable alongside the constants
    // used above; a mismatch is a compile-time failure.
    assert!(CANONICAL_STRUCTURAL_LIMITS.selected_slots == MAX_SELECTED_SLOTS);
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::entity::Annotation;
    use crate::match_request::ids::FieldId;
    use crate::match_request::model::{FetchSlot, MissingOrder, Window};

    fn person_registry() -> DescriptorRegistry {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "person-name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn one_person(
        operation: impl FnOnce(BindingId, BoundFieldId) -> MatchOperation,
    ) -> (DescriptorRegistry, MatchRequest) {
        let registry = person_registry();
        let descriptor = registry.descriptor_id("person").unwrap();
        let binding = BindingId::new(0);
        let field = BoundFieldId::new(binding, FieldId::new(descriptor.clone(), "name"));
        let request = MatchRequest::v1(
            super::super::model::MatchPlan {
                bindings: vec![super::super::model::MatchBinding {
                    id: binding,
                    descriptor,
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            operation(binding, field),
        );
        (registry, request)
    }

    #[test]
    fn bounded_rows_append_present_unique_scalar_tie_breaker() {
        let (registry, request) = one_person(|binding, _| MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![FetchSlot::One { binding }],
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 10,
            },
            cardinality: RowCardinality::BoundedMany,
        });

        let validated = validate_match_request(&registry, request).unwrap();
        assert_eq!(validated.stable_order().terms().len(), 1);
        assert_eq!(
            validated.stable_order().terms()[0].origin(),
            StableOrderOrigin::UniqueTieBreaker
        );
        assert_eq!(
            validated.stable_order().terms()[0].order().field.field.name,
            "name"
        );
        assert!(
            validated
                .shape_id()
                .as_str()
                .starts_with("shape-sha256-v1:")
        );
    }

    #[test]
    fn page_persists_validator_derived_collection_orders() {
        let registry = person_registry();
        let descriptor = registry.descriptor_id("person").unwrap();
        let root = BindingId::new(0);
        let collected = BindingId::new(1);
        let request = MatchRequest::v1(
            super::super::model::MatchPlan {
                bindings: vec![
                    super::super::model::MatchBinding {
                        id: root,
                        descriptor: descriptor.clone(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    super::super::model::MatchBinding {
                        id: collected,
                        descriptor,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([super::super::model::BindingPair::new(
                    root, collected,
                )]),
            },
            MatchOperation::PageBy {
                root,
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One { binding: root },
                        FetchSlot::Collect {
                            binding: collected,
                            distinct: true,
                            order: Vec::new(),
                        },
                    ],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 10,
                },
                include_total: false,
            },
        );

        let validated = validate_match_request(&registry, request).unwrap();
        let order = validated.collection_order(collected).unwrap();
        assert_eq!(order.terms().len(), 1);
        assert_eq!(
            order.terms()[0].origin(),
            StableOrderOrigin::UniqueTieBreaker
        );
        assert_eq!(order.terms()[0].order().field.binding, collected);
        assert!(validated.collection_order(root).is_none());
    }

    #[test]
    fn schema_recheck_ignores_unrelated_registration() {
        let (registry, request) =
            one_person(|binding, _| MatchOperation::CountBy { root: binding });
        let validated = validate_match_request(&registry, request).unwrap();
        registry
            .register_entity(EntityDescriptor {
                type_name: "skill".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        validated.recheck_schema(&registry).unwrap();
    }

    #[test]
    fn relevant_subtype_stales_polymorphic_request() {
        let (registry, mut request) =
            one_person(|binding, _| MatchOperation::CountBy { root: binding });
        request.plan.bindings[0].match_mode = MatchMode::Subtypes;
        let validated = validate_match_request(&registry, request).unwrap();
        registry
            .register_entity(EntityDescriptor {
                type_name: "employee".into(),
                is_abstract: false,
                parent_type: Some("person".into()),
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();

        let error = validated.recheck_schema(&registry).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::StaleSchema);
        assert_eq!(error.code().as_str(), "stale_schema");
    }

    #[test]
    fn wrong_owner_and_non_finite_literal_fail_before_validation() {
        let (registry, mut request) = one_person(|binding, field| MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![FetchSlot::One { binding }],
            },
            order: vec![MatchOrder {
                field,
                direction: SortDirection::Ascending,
                missing: MissingOrder::Reject,
            }],
            window: Window {
                offset: 0,
                limit: 10,
            },
            cardinality: RowCardinality::BoundedMany,
        });
        let wrong = BoundFieldId::new(
            BindingId::new(0),
            FieldId::new(DescriptorId::new("entity:company"), "name"),
        );
        request.plan.predicate = Some(MatchExpr::FieldValue {
            field: wrong,
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("Alice".into()),
        });
        assert_eq!(
            validate_match_request(&registry, request)
                .unwrap_err()
                .code()
                .as_str(),
            "cross_owner_field"
        );

        let (registry, mut request) =
            one_person(|binding, _| MatchOperation::CountBy { root: binding });
        let descriptor = registry.descriptor_id("person").unwrap();
        request.plan.predicate = Some(MatchExpr::FieldValue {
            field: BoundFieldId::new(BindingId::new(0), FieldId::new(descriptor, "name")),
            operator: ComparisonOp::Equal,
            value: AttributeValue::Double(f64::NAN),
        });
        assert_eq!(
            validate_match_request(&registry, request)
                .unwrap_err()
                .code()
                .as_str(),
            "unsafe_literal"
        );
    }

    #[test]
    fn missing_capability_has_structured_error() {
        let (registry, request) =
            one_person(|binding, _| MatchOperation::CountBy { root: binding });
        let validated = validate_match_request(&registry, request).unwrap();
        let error = validated
            .require_capabilities(&CapabilitySet::new())
            .unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
        assert_eq!(error.code().as_str(), "missing_provider_capability");
    }
}
