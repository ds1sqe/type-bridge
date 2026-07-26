//! Persistent, session-scoped builders for canonical match requests.
//!
//! Handles carry opaque process-local identity while a query is being built.
//! Terminal builders lower that identity to deterministic plan-local ordinals
//! and return an unvalidated [`MatchRequest`]; they never execute a query or
//! imply that canonical validation has succeeded.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::descriptor::{RoleDescriptor, TypeDescriptorRef};
use crate::error::{OrmError, Result};
use crate::registry::DescriptorRegistry;
use crate::value::AttributeValue;

use super::error::{MatchError, MatchErrorCategory};
use super::ids::{
    BindingId, BoundFieldId, DescriptorId, FieldId, RoleEdgeId, RoleId, SessionBindingToken,
    SessionId,
};
use super::limits::{MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES, MAX_SELECTED_SLOTS};
use super::model::{
    BindingPair, ComparisonOp, FetchShape, FetchSlot, MatchBinding, MatchExpr, MatchMode,
    MatchOperation, MatchOrder, MatchPlan, MatchRequest, MissingOrder, NamedFetchSlot,
    RowCardinality, SortDirection, ThingKind, Window,
};
use super::validation::{ValidatedMatchRequest, validate_match_request};

static NEXT_LIVE_TOKEN: AtomicU64 = AtomicU64::new(1);

const SESSION_TOKEN_DOMAIN: u64 = 0x7365_7373_696f_6e00;
const BINDING_TOKEN_DOMAIN: u64 = 0x6269_6e64_696e_6700;

fn next_token(domain: u64) -> [u8; 16] {
    let ordinal = NEXT_LIVE_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("process-local handle token space exhausted");
    let mut token = [0; 16];
    token[..8].copy_from_slice(&domain.to_be_bytes());
    token[8..].copy_from_slice(&ordinal.to_be_bytes());
    token
}

fn handle_error(code: &'static str, message: impl Into<String>) -> OrmError {
    MatchError::new(MatchErrorCategory::InvalidPlan, code, message).into()
}

#[derive(Debug)]
struct SessionState {
    id: SessionId,
    registry: Arc<DescriptorRegistry>,
}

/// Owner of one isolated handle-construction session.
///
/// Binding handles from different sessions cannot be combined, even when they
/// reference the same registry and descriptor.
#[derive(Clone)]
pub struct SessionHandle(Arc<SessionState>);

impl SessionHandle {
    /// Start a handle-construction session over a descriptor registry.
    pub fn new(registry: Arc<DescriptorRegistry>) -> Self {
        Self(Arc::new(SessionState {
            id: SessionId::new(next_token(SESSION_TOKEN_DOMAIN)),
            registry,
        }))
    }

    /// Create a fresh binding for a registered descriptor.
    ///
    /// Repeating this call intentionally creates a distinct binding handle.
    pub fn binding(&self, type_name: &str, match_mode: MatchMode) -> Result<BindingHandle> {
        let descriptor = self.0.registry.get(type_name).ok_or_else(|| {
            handle_error(
                "unknown_descriptor",
                format!("descriptor '{type_name}' is not registered in this session"),
            )
        })?;
        let descriptor_id = self.0.registry.descriptor_id(type_name).ok_or_else(|| {
            handle_error(
                "unknown_descriptor",
                format!("descriptor '{type_name}' is not registered in this session"),
            )
        })?;
        let thing_kind = match descriptor {
            TypeDescriptorRef::Entity(_) => ThingKind::Entity,
            TypeDescriptorRef::Relation(_) => ThingKind::Relation,
        };

        Ok(BindingHandle(Arc::new(BindingState {
            session: Arc::clone(&self.0),
            token: SessionBindingToken::new(next_token(BINDING_TOKEN_DOMAIN)),
            descriptor_id,
            thing_kind,
            match_mode,
        })))
    }

    /// Create a fresh exact-match binding for a registered descriptor.
    pub fn exact(&self, type_name: &str) -> Result<BindingHandle> {
        self.binding(type_name, MatchMode::Exact)
    }

    /// Create a fresh subtype-inclusive binding for a registered descriptor.
    pub fn subtypes(&self, type_name: &str) -> Result<BindingHandle> {
        self.binding(type_name, MatchMode::Subtypes)
    }

    /// Require a finite directed walk between two session-owned bindings.
    ///
    /// The walk uses `role_from -> role_to` on the exact relation type for
    /// every hop. Bounds are inclusive; a zero-hop branch requires exact
    /// source/target identity. Intermediate vertices are existential and are
    /// not exposed through the query shape.
    #[allow(clippy::too_many_arguments)]
    pub fn reachable(
        &self,
        relation_type: &str,
        role_from: &str,
        role_to: &str,
        source: &BindingHandle,
        target: &BindingHandle,
        min_depth: u8,
        max_depth: u8,
    ) -> Result<PredicateHandle> {
        self.require_session(source.session_id())?;
        self.require_session(target.session_id())?;
        validate_reachability_bounds(min_depth, max_depth)?;

        let relation_id = self
            .0
            .registry
            .descriptor_id(relation_type)
            .ok_or_else(|| {
                handle_error(
                    "unknown_descriptor",
                    format!("descriptor '{relation_type}' is not registered in this session"),
                )
            })?;
        let relation = match self.0.registry.get(relation_type) {
            Some(TypeDescriptorRef::Relation(relation)) => relation,
            Some(TypeDescriptorRef::Entity(_)) => {
                return Err(handle_error(
                    "reachable_relation_not_relation",
                    format!(
                        "bounded reachability relation '{relation_type}' is not a relation descriptor"
                    ),
                ));
            }
            None => {
                return Err(handle_error(
                    "unknown_descriptor",
                    format!("descriptor '{relation_type}' is not registered in this session"),
                ));
            }
        };
        let role_from_id = resolve_reachability_role(&self.0.registry, &relation_id, role_from)?;
        let role_to_id = resolve_reachability_role(&self.0.registry, &relation_id, role_to)?;
        let from_descriptor = relation
            .role(&role_from_id.name)
            .expect("registry-resolved role remains present");
        let to_descriptor = relation
            .role(&role_to_id.name)
            .expect("registry-resolved role remains present");
        require_reachable_endpoint(&self.0.registry, source, from_descriptor, "source")?;
        require_reachable_endpoint(&self.0.registry, target, to_descriptor, "target")?;

        Ok(PredicateHandle::new(
            self.0.id,
            HandleExpr::Reachable {
                relation: relation_id,
                role_from: role_from_id,
                role_to: role_to_id,
                source: source.token(),
                target: target.token(),
                min_depth,
                max_depth,
            },
        ))
    }

    /// Create a positional public output shape.
    pub fn positional(
        &self,
        slots: impl IntoIterator<Item = SelectionHandle>,
    ) -> Result<ShapeHandle> {
        let slots: Vec<_> = slots.into_iter().collect();
        validate_shape_arity(slots.len())?;
        for slot in &slots {
            self.require_session(slot.session_id())?;
        }
        Ok(ShapeHandle(Arc::new(ShapeState {
            session: Arc::clone(&self.0),
            kind: ShapeKind::Positional(slots),
        })))
    }

    /// Create a named public output shape in declaration order.
    pub fn named<I, N>(&self, slots: I) -> Result<ShapeHandle>
    where
        I: IntoIterator<Item = (N, SelectionHandle)>,
        N: Into<String>,
    {
        let slots: Vec<_> = slots
            .into_iter()
            .map(|(name, slot)| (name.into(), slot))
            .collect();
        validate_shape_arity(slots.len())?;
        let mut names = BTreeSet::new();
        for (name, slot) in &slots {
            self.require_session(slot.session_id())?;
            if !names.insert(name.clone()) {
                return Err(handle_error(
                    "duplicate_output_name",
                    format!("named output member '{name}' is declared more than once"),
                ));
            }
        }
        Ok(ShapeHandle(Arc::new(ShapeState {
            session: Arc::clone(&self.0),
            kind: ShapeKind::Named(slots),
        })))
    }

    /// Create a named shape whose language-level declaration has been checked
    /// against the selected descriptor and cardinality of every slot.
    ///
    /// Native bindings normalize each resolved annotation to an ordered
    /// `(field name, registered model type name, is collection)` triple. This
    /// method keeps the correspondence proof in Rust while leaving Python- or
    /// TypeScript-specific reflection outside the canonical request algebra.
    #[doc(hidden)]
    pub fn named_checked(
        &self,
        declarations: impl IntoIterator<Item = (String, String, bool)>,
        slots: impl IntoIterator<Item = (String, SelectionHandle)>,
    ) -> Result<ShapeHandle> {
        let declarations: Vec<_> = declarations.into_iter().collect();
        let slots: Vec<_> = slots.into_iter().collect();
        validate_shape_arity(slots.len())?;
        if declarations.len() != slots.len() {
            return Err(handle_error(
                "named_declaration_length_mismatch",
                "named row declaration and selected output must have equal length",
            ));
        }

        for ((declared_name, declared_type, declared_collection), (actual_name, selection)) in
            declarations.iter().zip(&slots)
        {
            self.require_session(selection.session_id())?;
            if declared_name != actual_name {
                return Err(handle_error(
                    "named_declaration_name_mismatch",
                    format!(
                        "declared named row field '{declared_name}' does not match selected field '{actual_name}'"
                    ),
                ));
            }
            let declared_descriptor = self.0.registry.descriptor_id(declared_type).ok_or_else(|| {
                handle_error(
                    "unknown_declared_descriptor",
                    format!(
                        "named row annotation references unregistered descriptor '{declared_type}'"
                    ),
                )
            })?;
            if declared_descriptor != selection.0.binding.0.descriptor_id {
                return Err(handle_error(
                    "named_declaration_descriptor_mismatch",
                    format!(
                        "named row field '{declared_name}' annotation does not match its selected descriptor"
                    ),
                ));
            }
            let actual_collection = matches!(selection.0.kind, SelectionKind::Collect { .. });
            if *declared_collection != actual_collection {
                return Err(handle_error(
                    "named_declaration_cardinality_mismatch",
                    format!(
                        "named row field '{declared_name}' annotation does not match its selected cardinality"
                    ),
                ));
            }
        }

        self.named(slots)
    }

    /// Begin a persistent query lineage from a selected output shape.
    pub fn query(&self, shape: ShapeHandle) -> Result<QueryHandle> {
        self.require_session(shape.session_id())?;
        let selected = shape.selected_bindings();
        let mut seen = BTreeSet::new();
        for binding in &selected {
            if !seen.insert(binding.token()) {
                return Err(handle_error(
                    "duplicate_selection",
                    "one binding handle cannot occupy multiple selected output slots",
                ));
            }
        }

        Ok(QueryHandle(Arc::new(QueryState {
            session: Arc::clone(&self.0),
            shape,
            selected,
            hidden: Vec::new(),
            predicate: None,
            allowed_cross_joins: BTreeSet::new(),
        })))
    }

    fn require_session(&self, actual: SessionId) -> Result<()> {
        if self.0.id == actual {
            Ok(())
        } else {
            Err(handle_error(
                "cross_session_handle",
                "handles from different construction sessions cannot be combined",
            ))
        }
    }
}

fn validate_shape_arity(count: usize) -> Result<()> {
    if count == 0 {
        return Err(handle_error(
            "empty_output",
            "typed queries must select at least one output binding",
        ));
    }
    if count > MAX_SELECTED_SLOTS {
        return Err(handle_error(
            "selection_cap_exceeded",
            "typed query output exceeds the canonical sixteen-slot ceiling",
        ));
    }
    Ok(())
}

fn validate_reachability_bounds(min_depth: u8, max_depth: u8) -> Result<()> {
    if min_depth > max_depth {
        return Err(handle_error(
            "reachable_bounds",
            "reachability minimum depth must not exceed its maximum depth",
        ));
    }
    if usize::from(max_depth) > MAX_PREDICATE_DEPTH {
        return Err(handle_error(
            "reachable_depth_limit",
            "reachability maximum depth exceeds the canonical predicate-depth ceiling",
        ));
    }
    let expanded = reachability_expanded_clauses(min_depth, max_depth);
    if expanded > MAX_PREDICATE_NODES {
        return Err(handle_error(
            "reachable_expansion_limit",
            "reachability expansion exceeds the canonical predicate-node ceiling",
        ));
    }
    Ok(())
}

fn reachability_expanded_clauses(min_depth: u8, max_depth: u8) -> usize {
    let first_positive = usize::from(min_depth.max(1));
    let maximum = usize::from(max_depth);
    let positive_hops = if first_positive <= maximum {
        (first_positive..=maximum).fold(0_usize, usize::saturating_add)
    } else {
        0
    };
    positive_hops.saturating_add(usize::from(min_depth == 0))
}

fn resolve_reachability_role(
    registry: &DescriptorRegistry,
    relation: &DescriptorId,
    role_name: &str,
) -> Result<RoleId> {
    registry.role_id(relation, role_name).ok_or_else(|| {
        handle_error(
            "unknown_role",
            format!("relation descriptor '{relation}' has no registered role '{role_name}'"),
        )
    })
}

fn require_reachable_endpoint(
    registry: &DescriptorRegistry,
    binding: &BindingHandle,
    role: &RoleDescriptor,
    endpoint: &str,
) -> Result<()> {
    let compatible = role.player_type_names.iter().any(|allowed_name| {
        let Some(allowed) = registry.descriptor_id(allowed_name) else {
            return false;
        };
        registry.is_same_or_subtype(&binding.0.descriptor_id, &allowed)
            || (binding.0.match_mode == MatchMode::Subtypes
                && registry.is_same_or_subtype(&allowed, &binding.0.descriptor_id))
    });
    if compatible {
        Ok(())
    } else {
        Err(handle_error(
            "incompatible_reachable_endpoint",
            format!(
                "reachability {endpoint} binding '{}' cannot play role '{}'",
                binding.0.descriptor_id, role.role_name
            ),
        ))
    }
}

impl fmt::Debug for SessionHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionHandle")
            .field("id", &self.0.id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct BindingState {
    session: Arc<SessionState>,
    token: SessionBindingToken,
    descriptor_id: DescriptorId,
    thing_kind: ThingKind,
    match_mode: MatchMode,
}

/// One fresh, session-owned occurrence of a registered descriptor.
#[derive(Clone)]
pub struct BindingHandle(Arc<BindingState>);

impl BindingHandle {
    /// Return the stable registry descriptor identity.
    pub fn descriptor_id(&self) -> &DescriptorId {
        &self.0.descriptor_id
    }

    /// Return the descriptor's entity/relation category.
    pub fn thing_kind(&self) -> ThingKind {
        self.0.thing_kind
    }

    /// Return this binding's exact/subtypes match behavior.
    pub fn match_mode(&self) -> MatchMode {
        self.0.match_mode
    }

    /// Resolve a descriptor-owned scalar field.
    pub fn field(&self, field_name: &str) -> Result<FieldHandle> {
        let field_id = self.resolve_field(&self.0.descriptor_id, field_name)?;
        Ok(FieldHandle(Arc::new(FieldState {
            binding: self.clone(),
            field_id,
        })))
    }

    /// Resolve a scalar field while preserving the model type that supplied
    /// the language-level field reference.
    ///
    /// The reference owner must be this binding's descriptor or a registered
    /// ancestor whose field remains identical in the binding's flattened
    /// descriptor. Unrelated same-label fields and subtype shadows fail closed.
    pub fn field_owned_by(&self, owner_type: &str, field_name: &str) -> Result<FieldHandle> {
        let owner_id = self.reference_owner_id(owner_type, "field")?;
        let field_id = self.resolve_field(&owner_id, field_name)?;
        if !self.0.session.registry.field_reference_is_compatible(
            &self.0.descriptor_id,
            &owner_id,
            &field_id.name,
        ) {
            return Err(handle_error(
                "cross_owner_field",
                format!(
                    "field '{}.{}' does not belong to binding descriptor '{}'",
                    owner_id, field_id.name, self.0.descriptor_id
                ),
            ));
        }
        Ok(FieldHandle(Arc::new(FieldState {
            binding: self.clone(),
            field_id,
        })))
    }

    /// Resolve a role owned by this relation descriptor.
    pub fn role(&self, role_name: &str) -> Result<RoleHandle> {
        if self.0.thing_kind != ThingKind::Relation {
            return Err(handle_error(
                "role_on_entity",
                format!(
                    "entity descriptor '{}' cannot own relation roles",
                    self.0.descriptor_id
                ),
            ));
        }
        let role_id = self
            .0
            .session
            .registry
            .role_id(&self.0.descriptor_id, role_name)
            .ok_or_else(|| {
                handle_error(
                    "unknown_role",
                    format!(
                        "relation descriptor '{}' has no registered role '{role_name}'",
                        self.0.descriptor_id
                    ),
                )
            })?;
        Ok(RoleHandle(Arc::new(RoleState {
            relation: self.clone(),
            role_id,
        })))
    }

    /// Resolve a role while preserving the relation type that supplied the
    /// language-level role reference.
    pub fn role_owned_by(&self, owner_type: &str, role_name: &str) -> Result<RoleHandle> {
        if self.0.thing_kind != ThingKind::Relation {
            return Err(handle_error(
                "role_on_entity",
                format!(
                    "entity descriptor '{}' cannot own relation roles",
                    self.0.descriptor_id
                ),
            ));
        }
        let owner_id = self.reference_owner_id(owner_type, "role")?;
        let role_id = self
            .0
            .session
            .registry
            .role_id(&owner_id, role_name)
            .ok_or_else(|| {
                handle_error(
                    "unknown_role",
                    format!(
                        "relation descriptor '{owner_id}' has no registered role '{role_name}'"
                    ),
                )
            })?;
        if !self.0.session.registry.role_reference_is_compatible(
            &self.0.descriptor_id,
            &owner_id,
            &role_id.name,
        ) {
            return Err(handle_error(
                "cross_owner_role",
                format!(
                    "role '{}.{}' does not belong to binding descriptor '{}'",
                    owner_id, role_id.name, self.0.descriptor_id
                ),
            ));
        }
        Ok(RoleHandle(Arc::new(RoleState {
            relation: self.clone(),
            role_id,
        })))
    }

    /// Select this binding as one concept per row/root.
    pub fn one(&self) -> SelectionHandle {
        SelectionHandle(Arc::new(SelectionState {
            binding: self.clone(),
            kind: SelectionKind::One,
        }))
    }

    /// Select this binding as a concept collection per root.
    pub fn collect(&self) -> SelectionHandle {
        SelectionHandle(Arc::new(SelectionState {
            binding: self.clone(),
            kind: SelectionKind::Collect {
                distinct: false,
                order: Vec::new(),
            },
        }))
    }

    fn session_id(&self) -> SessionId {
        self.0.session.id
    }

    fn token(&self) -> SessionBindingToken {
        self.0.token
    }

    fn reference_owner_id(&self, owner_type: &str, member_kind: &str) -> Result<DescriptorId> {
        self.0
            .session
            .registry
            .descriptor_id(owner_type)
            .ok_or_else(|| {
                handle_error(
                    "unknown_descriptor",
                    format!(
                        "{member_kind} reference owner '{owner_type}' is not registered in this session"
                    ),
                )
            })
    }

    fn resolve_field(&self, owner_id: &DescriptorId, field_name: &str) -> Result<FieldId> {
        self.0
            .session
            .registry
            .field_id(owner_id, field_name)
            .ok_or_else(|| {
                handle_error(
                    "unknown_field",
                    format!("descriptor '{owner_id}' has no registered field '{field_name}'"),
                )
            })
    }
}

impl PartialEq for BindingHandle {
    fn eq(&self, other: &Self) -> bool {
        self.session_id() == other.session_id() && self.token() == other.token()
    }
}

impl Eq for BindingHandle {}

impl fmt::Debug for BindingHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BindingHandle")
            .field("token", &self.0.token)
            .field("descriptor_id", &self.0.descriptor_id)
            .field("thing_kind", &self.0.thing_kind)
            .field("match_mode", &self.0.match_mode)
            .finish()
    }
}

#[derive(Debug)]
struct FieldState {
    binding: BindingHandle,
    field_id: FieldId,
}

/// A scalar field resolved against one binding occurrence.
#[derive(Clone, Debug)]
pub struct FieldHandle(Arc<FieldState>);

impl FieldHandle {
    /// Compare this field with a typed literal.
    pub fn compare_value(&self, operator: ComparisonOp, value: AttributeValue) -> PredicateHandle {
        PredicateHandle::new(
            self.session_id(),
            HandleExpr::FieldValue {
                binding: self.binding_token(),
                field: self.0.field_id.clone(),
                operator,
                value,
            },
        )
    }

    /// Compare this field with another bound field.
    pub fn compare_field(
        &self,
        operator: ComparisonOp,
        other: &FieldHandle,
    ) -> Result<PredicateHandle> {
        require_same_session(self.session_id(), other.session_id())?;
        Ok(PredicateHandle::new(
            self.session_id(),
            HandleExpr::FieldComparison {
                left_binding: self.binding_token(),
                left: self.0.field_id.clone(),
                operator,
                right_binding: other.binding_token(),
                right: other.0.field_id.clone(),
            },
        ))
    }

    /// Use this field as a stable public ordering key.
    pub fn order(&self, direction: SortDirection, missing: MissingOrder) -> OrderHandle {
        OrderHandle(Arc::new(OrderState {
            field: self.clone(),
            direction,
            missing,
        }))
    }

    /// Return the descriptor-qualified field identity.
    pub fn field_id(&self) -> &FieldId {
        &self.0.field_id
    }

    fn session_id(&self) -> SessionId {
        self.0.binding.session_id()
    }

    fn binding_token(&self) -> SessionBindingToken {
        self.0.binding.token()
    }
}

#[derive(Debug)]
struct RoleState {
    relation: BindingHandle,
    role_id: RoleId,
}

/// A relation role resolved against one relation binding occurrence.
#[derive(Clone, Debug)]
pub struct RoleHandle(Arc<RoleState>);

impl RoleHandle {
    /// Require this relation role to connect the supplied player binding.
    pub fn connects(&self, player: &BindingHandle) -> Result<PredicateHandle> {
        require_same_session(self.session_id(), player.session_id())?;
        Ok(PredicateHandle::new(
            self.session_id(),
            HandleExpr::RoleEdge {
                relation: self.0.relation.token(),
                role: self.0.role_id.clone(),
                player: player.token(),
            },
        ))
    }

    /// Return the descriptor-qualified role identity.
    pub fn role_id(&self) -> &RoleId {
        &self.0.role_id
    }

    fn session_id(&self) -> SessionId {
        self.0.relation.session_id()
    }
}

#[derive(Clone, Debug)]
enum HandleExpr {
    FieldValue {
        binding: SessionBindingToken,
        field: FieldId,
        operator: ComparisonOp,
        value: AttributeValue,
    },
    FieldComparison {
        left_binding: SessionBindingToken,
        left: FieldId,
        operator: ComparisonOp,
        right_binding: SessionBindingToken,
        right: FieldId,
    },
    RoleEdge {
        relation: SessionBindingToken,
        role: RoleId,
        player: SessionBindingToken,
    },
    Reachable {
        relation: DescriptorId,
        role_from: RoleId,
        role_to: RoleId,
        source: SessionBindingToken,
        target: SessionBindingToken,
        min_depth: u8,
        max_depth: u8,
    },
    And(Vec<Arc<HandleExpr>>),
    Or(Vec<Arc<HandleExpr>>),
    Not(Arc<HandleExpr>),
}

/// An immutable predicate tree over session-owned binding handles.
#[derive(Clone, Debug)]
pub struct PredicateHandle {
    session_id: SessionId,
    expression: Arc<HandleExpr>,
    bindings: BTreeSet<SessionBindingToken>,
}

impl PredicateHandle {
    fn new(session_id: SessionId, expression: HandleExpr) -> Self {
        let expression = Arc::new(expression);
        let mut bindings = BTreeSet::new();
        collect_expr_bindings(&expression, &mut bindings);
        Self {
            session_id,
            expression,
            bindings,
        }
    }

    /// Combine two predicates with conjunction in source order.
    pub fn and(&self, other: &Self) -> Result<Self> {
        require_same_session(self.session_id, other.session_id)?;
        Ok(Self::new(
            self.session_id,
            HandleExpr::And(vec![
                Arc::clone(&self.expression),
                Arc::clone(&other.expression),
            ]),
        ))
    }

    /// Combine two predicates with disjunction in source order.
    pub fn or(&self, other: &Self) -> Result<Self> {
        require_same_session(self.session_id, other.session_id)?;
        Ok(Self::new(
            self.session_id,
            HandleExpr::Or(vec![
                Arc::clone(&self.expression),
                Arc::clone(&other.expression),
            ]),
        ))
    }

    /// Negate this predicate without mutating its lineage.
    pub fn not(&self) -> Self {
        Self::new(
            self.session_id,
            HandleExpr::Not(Arc::clone(&self.expression)),
        )
    }
}

fn collect_expr_bindings(expression: &HandleExpr, bindings: &mut BTreeSet<SessionBindingToken>) {
    match expression {
        HandleExpr::FieldValue { binding, .. } => {
            bindings.insert(*binding);
        }
        HandleExpr::FieldComparison {
            left_binding,
            right_binding,
            ..
        } => {
            bindings.insert(*left_binding);
            bindings.insert(*right_binding);
        }
        HandleExpr::RoleEdge {
            relation, player, ..
        } => {
            bindings.insert(*relation);
            bindings.insert(*player);
        }
        HandleExpr::Reachable { source, target, .. } => {
            bindings.insert(*source);
            bindings.insert(*target);
        }
        HandleExpr::And(expressions) | HandleExpr::Or(expressions) => {
            for expression in expressions {
                collect_expr_bindings(expression, bindings);
            }
        }
        HandleExpr::Not(expression) => collect_expr_bindings(expression, bindings),
    }
}

#[derive(Debug)]
struct OrderState {
    field: FieldHandle,
    direction: SortDirection,
    missing: MissingOrder,
}

/// One immutable public ordering term.
#[derive(Clone, Debug)]
pub struct OrderHandle(Arc<OrderState>);

impl OrderHandle {
    fn session_id(&self) -> SessionId {
        self.0.field.session_id()
    }

    fn binding_token(&self) -> SessionBindingToken {
        self.0.field.binding_token()
    }
}

#[derive(Debug, Clone)]
enum SelectionKind {
    One,
    Collect {
        distinct: bool,
        order: Vec<OrderHandle>,
    },
}

#[derive(Debug)]
struct SelectionState {
    binding: BindingHandle,
    kind: SelectionKind,
}

/// One immutable selected output slot.
#[derive(Clone, Debug)]
pub struct SelectionHandle(Arc<SelectionState>);

impl SelectionHandle {
    /// Return a collection selection with the requested identity distinctness.
    pub fn distinct(&self, distinct: bool) -> Result<Self> {
        let SelectionKind::Collect { order, .. } = &self.0.kind else {
            return Err(handle_error(
                "distinct_on_singular_selection",
                "distinctness applies only to collection selections",
            ));
        };
        Ok(Self(Arc::new(SelectionState {
            binding: self.0.binding.clone(),
            kind: SelectionKind::Collect {
                distinct,
                order: order.clone(),
            },
        })))
    }

    /// Append a stable collection-member order term.
    pub fn order_by(&self, order: OrderHandle) -> Result<Self> {
        require_same_session(self.session_id(), order.session_id())?;
        if self.0.binding.token() != order.binding_token() {
            return Err(handle_error(
                "collection_order_binding_mismatch",
                "collection ordering must reference the collected binding",
            ));
        }
        let SelectionKind::Collect {
            distinct,
            order: existing,
        } = &self.0.kind
        else {
            return Err(handle_error(
                "order_on_singular_selection",
                "collection-member ordering applies only to collection selections",
            ));
        };
        let mut orders = existing.clone();
        orders.push(order);
        Ok(Self(Arc::new(SelectionState {
            binding: self.0.binding.clone(),
            kind: SelectionKind::Collect {
                distinct: *distinct,
                order: orders,
            },
        })))
    }

    fn session_id(&self) -> SessionId {
        self.0.binding.session_id()
    }
}

#[derive(Debug)]
enum ShapeKind {
    Positional(Vec<SelectionHandle>),
    Named(Vec<(String, SelectionHandle)>),
}

#[derive(Debug)]
struct ShapeState {
    session: Arc<SessionState>,
    kind: ShapeKind,
}

/// An immutable positional or named selected output shape.
#[derive(Clone, Debug)]
pub struct ShapeHandle(Arc<ShapeState>);

impl ShapeHandle {
    fn session_id(&self) -> SessionId {
        self.0.session.id
    }

    fn selected_bindings(&self) -> Vec<BindingHandle> {
        match &self.0.kind {
            ShapeKind::Positional(slots) => {
                slots.iter().map(|slot| slot.0.binding.clone()).collect()
            }
            ShapeKind::Named(slots) => slots
                .iter()
                .map(|(_, slot)| slot.0.binding.clone())
                .collect(),
        }
    }
}

#[derive(Debug)]
struct QueryState {
    session: Arc<SessionState>,
    shape: ShapeHandle,
    selected: Vec<BindingHandle>,
    hidden: Vec<BindingHandle>,
    predicate: Option<PredicateHandle>,
    allowed_cross_joins: BTreeSet<(SessionBindingToken, SessionBindingToken)>,
}

/// One immutable point in a persistent match-query lineage.
///
/// Every transition returns a new handle and leaves all ancestors usable.
#[derive(Clone, Debug)]
pub struct QueryHandle(Arc<QueryState>);

impl QueryHandle {
    /// Canonically validate one selected-row terminal without serializing its
    /// semantic request. Intended for native language-binding entry seams.
    #[doc(hidden)]
    pub fn validate_fetch_rows(
        &self,
        order: &[OrderHandle],
        window: Window,
        cardinality: RowCardinality,
    ) -> Result<ValidatedMatchRequest> {
        let request = self.fetch_rows(order, window, cardinality)?;
        Ok(validate_match_request(&self.0.session.registry, request)?)
    }

    /// Canonically validate one distinct-root page terminal without a
    /// diagnostic serialization round-trip.
    #[doc(hidden)]
    pub fn validate_page_by(
        &self,
        root: &BindingHandle,
        order: &[OrderHandle],
        window: Window,
        include_total: bool,
    ) -> Result<ValidatedMatchRequest> {
        let request = self.page_by(root, order, window, include_total)?;
        Ok(validate_match_request(&self.0.session.registry, request)?)
    }

    /// Canonically validate one distinct-root count terminal.
    #[doc(hidden)]
    pub fn validate_count_by(&self, root: &BindingHandle) -> Result<ValidatedMatchRequest> {
        let request = self.count_by(root)?;
        Ok(validate_match_request(&self.0.session.registry, request)?)
    }

    /// Canonically validate one distinct-root existence terminal.
    #[doc(hidden)]
    pub fn validate_exists_by(&self, root: &BindingHandle) -> Result<ValidatedMatchRequest> {
        let request = self.exists_by(root)?;
        Ok(validate_match_request(&self.0.session.registry, request)?)
    }

    /// Return this opaque query lineage's descriptor registry to a trusted
    /// native execution seam.
    #[doc(hidden)]
    pub fn registry_arc(&self) -> Arc<DescriptorRegistry> {
        Arc::clone(&self.0.session.registry)
    }

    /// Declare a non-selected binding after all selected bindings.
    pub fn add_hidden(&self, binding: BindingHandle) -> Result<Self> {
        self.require_session(binding.session_id())?;
        if self.is_attached(binding.token()) {
            return Err(handle_error(
                "duplicate_binding",
                "one binding handle cannot be declared more than once",
            ));
        }
        let mut hidden = self.0.hidden.clone();
        hidden.push(binding);
        Ok(self.with_state(
            hidden,
            self.0.predicate.clone(),
            self.0.allowed_cross_joins.clone(),
        ))
    }

    /// Attach a predicate whose referenced bindings are already declared.
    ///
    /// Repeated calls form a conjunction in call order.
    pub fn where_predicate(&self, predicate: PredicateHandle) -> Result<Self> {
        self.require_session(predicate.session_id)?;
        if predicate
            .bindings
            .iter()
            .any(|token| !self.is_attached(*token))
        {
            return Err(handle_error(
                "unattached_binding",
                "predicate references a binding not attached to this query",
            ));
        }
        let predicate = match &self.0.predicate {
            Some(existing) => existing.and(&predicate)?,
            None => predicate,
        };
        Ok(self.with_state(
            self.0.hidden.clone(),
            Some(predicate),
            self.0.allowed_cross_joins.clone(),
        ))
    }

    /// Explicitly permit a topology-level cross join between attached bindings.
    pub fn allow_cross_join(&self, left: &BindingHandle, right: &BindingHandle) -> Result<Self> {
        self.require_session(left.session_id())?;
        self.require_session(right.session_id())?;
        if !self.is_attached(left.token()) || !self.is_attached(right.token()) {
            return Err(handle_error(
                "unattached_binding",
                "cross-join permission references a binding not attached to this query",
            ));
        }
        if left.token() == right.token() {
            return Err(handle_error(
                "self_cross_join",
                "cross-join permission requires two distinct binding handles",
            ));
        }
        let pair = if left.token() < right.token() {
            (left.token(), right.token())
        } else {
            (right.token(), left.token())
        };
        let mut allowed = self.0.allowed_cross_joins.clone();
        allowed.insert(pair);
        Ok(self.with_state(self.0.hidden.clone(), self.0.predicate.clone(), allowed))
    }

    /// Lower this lineage to an unvalidated row-fetch request.
    pub fn fetch_rows(
        &self,
        order: &[OrderHandle],
        window: Window,
        cardinality: RowCardinality,
    ) -> Result<MatchRequest> {
        let lowered = self.lower()?;
        Ok(MatchRequest::v1(
            lowered.plan,
            MatchOperation::FetchRows {
                output: lowered.output,
                order: self.lower_orders(order, &lowered.binding_ids)?,
                window,
                cardinality,
            },
        ))
    }

    /// Lower this lineage to an unvalidated distinct-root page request.
    pub fn page_by(
        &self,
        root: &BindingHandle,
        order: &[OrderHandle],
        window: Window,
        include_total: bool,
    ) -> Result<MatchRequest> {
        let lowered = self.lower()?;
        let root = self.require_attached(root, &lowered.binding_ids)?;
        Ok(MatchRequest::v1(
            lowered.plan,
            MatchOperation::PageBy {
                root,
                output: lowered.output,
                order: self.lower_orders(order, &lowered.binding_ids)?,
                window,
                include_total,
            },
        ))
    }

    /// Lower this lineage to an unvalidated distinct-root count request.
    pub fn count_by(&self, root: &BindingHandle) -> Result<MatchRequest> {
        let lowered = self.lower()?;
        let root = self.require_attached(root, &lowered.binding_ids)?;
        Ok(MatchRequest::v1(
            lowered.plan,
            MatchOperation::CountBy { root },
        ))
    }

    /// Lower this lineage to an unvalidated distinct-root existence request.
    pub fn exists_by(&self, root: &BindingHandle) -> Result<MatchRequest> {
        let lowered = self.lower()?;
        let root = self.require_attached(root, &lowered.binding_ids)?;
        Ok(MatchRequest::v1(
            lowered.plan,
            MatchOperation::ExistsBy { root },
        ))
    }

    fn with_state(
        &self,
        hidden: Vec<BindingHandle>,
        predicate: Option<PredicateHandle>,
        allowed_cross_joins: BTreeSet<(SessionBindingToken, SessionBindingToken)>,
    ) -> Self {
        Self(Arc::new(QueryState {
            session: Arc::clone(&self.0.session),
            shape: self.0.shape.clone(),
            selected: self.0.selected.clone(),
            hidden,
            predicate,
            allowed_cross_joins,
        }))
    }

    fn require_session(&self, actual: SessionId) -> Result<()> {
        require_same_session(self.0.session.id, actual)
    }

    fn is_attached(&self, token: SessionBindingToken) -> bool {
        self.0
            .selected
            .iter()
            .chain(&self.0.hidden)
            .any(|binding| binding.token() == token)
    }

    fn require_attached(
        &self,
        binding: &BindingHandle,
        binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
    ) -> Result<BindingId> {
        self.require_session(binding.session_id())?;
        binding_ids.get(&binding.token()).copied().ok_or_else(|| {
            handle_error(
                "unattached_binding",
                "terminal root binding is not attached to this query",
            )
        })
    }

    fn lower(&self) -> Result<LoweredQuery> {
        let declared: Vec<_> = self.0.selected.iter().chain(&self.0.hidden).collect();
        let mut binding_ids = BTreeMap::new();
        let mut bindings = Vec::with_capacity(declared.len());
        for (index, binding) in declared.into_iter().enumerate() {
            let ordinal = u16::try_from(index).map_err(|_| {
                handle_error(
                    "binding_ordinal_overflow",
                    "query contains more bindings than canonical ordinals can represent",
                )
            })?;
            let id = BindingId::new(ordinal);
            binding_ids.insert(binding.token(), id);
            bindings.push(MatchBinding {
                id,
                descriptor: binding.0.descriptor_id.clone(),
                thing_kind: binding.0.thing_kind,
                match_mode: binding.0.match_mode,
            });
        }

        let mut next_role_edge = 0_usize;
        let predicate = self
            .0
            .predicate
            .as_ref()
            .map(|predicate| {
                lower_expression(&predicate.expression, &binding_ids, &mut next_role_edge)
            })
            .transpose()?;

        let allowed_cross_joins = self
            .0
            .allowed_cross_joins
            .iter()
            .map(|(left, right)| {
                Ok(BindingPair::new(
                    lookup_binding_id(*left, &binding_ids)?,
                    lookup_binding_id(*right, &binding_ids)?,
                ))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let output = lower_shape(&self.0.shape, &binding_ids)?;

        Ok(LoweredQuery {
            plan: MatchPlan {
                bindings,
                predicate,
                allowed_cross_joins,
            },
            output,
            binding_ids,
        })
    }

    fn lower_orders(
        &self,
        orders: &[OrderHandle],
        binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
    ) -> Result<Vec<MatchOrder>> {
        orders
            .iter()
            .map(|order| {
                self.require_session(order.session_id())?;
                lower_order(order, binding_ids)
            })
            .collect()
    }
}

struct LoweredQuery {
    plan: MatchPlan,
    output: FetchShape,
    binding_ids: BTreeMap<SessionBindingToken, BindingId>,
}

fn require_same_session(expected: SessionId, actual: SessionId) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(handle_error(
            "cross_session_handle",
            "handles from different construction sessions cannot be combined",
        ))
    }
}

fn lookup_binding_id(
    token: SessionBindingToken,
    binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
) -> Result<BindingId> {
    binding_ids.get(&token).copied().ok_or_else(|| {
        handle_error(
            "unattached_binding",
            "handle references a binding not attached to this query",
        )
    })
}

fn lower_expression(
    expression: &HandleExpr,
    binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
    next_role_edge: &mut usize,
) -> Result<MatchExpr> {
    match expression {
        HandleExpr::FieldValue {
            binding,
            field,
            operator,
            value,
        } => Ok(MatchExpr::FieldValue {
            field: BoundFieldId::new(lookup_binding_id(*binding, binding_ids)?, field.clone()),
            operator: *operator,
            value: value.clone(),
        }),
        HandleExpr::FieldComparison {
            left_binding,
            left,
            operator,
            right_binding,
            right,
        } => Ok(MatchExpr::FieldComparison {
            left: BoundFieldId::new(lookup_binding_id(*left_binding, binding_ids)?, left.clone()),
            operator: *operator,
            right: BoundFieldId::new(
                lookup_binding_id(*right_binding, binding_ids)?,
                right.clone(),
            ),
        }),
        HandleExpr::RoleEdge {
            relation,
            role,
            player,
        } => {
            let ordinal = u16::try_from(*next_role_edge).map_err(|_| {
                handle_error(
                    "role_edge_ordinal_overflow",
                    "predicate contains more role edges than canonical ordinals can represent",
                )
            })?;
            *next_role_edge += 1;
            Ok(MatchExpr::RoleEdge {
                id: RoleEdgeId::new(ordinal),
                relation: lookup_binding_id(*relation, binding_ids)?,
                role: role.clone(),
                player: lookup_binding_id(*player, binding_ids)?,
            })
        }
        HandleExpr::Reachable {
            relation,
            role_from,
            role_to,
            source,
            target,
            min_depth,
            max_depth,
        } => Ok(MatchExpr::Reachable {
            relation: relation.clone(),
            role_from: role_from.clone(),
            role_to: role_to.clone(),
            source: lookup_binding_id(*source, binding_ids)?,
            target: lookup_binding_id(*target, binding_ids)?,
            min_depth: *min_depth,
            max_depth: *max_depth,
        }),
        HandleExpr::And(expressions) => Ok(MatchExpr::And {
            expressions: expressions
                .iter()
                .map(|expression| lower_expression(expression, binding_ids, next_role_edge))
                .collect::<Result<_>>()?,
        }),
        HandleExpr::Or(expressions) => Ok(MatchExpr::Or {
            expressions: expressions
                .iter()
                .map(|expression| lower_expression(expression, binding_ids, next_role_edge))
                .collect::<Result<_>>()?,
        }),
        HandleExpr::Not(expression) => Ok(MatchExpr::Not {
            expression: Box::new(lower_expression(expression, binding_ids, next_role_edge)?),
        }),
    }
}

fn lower_order(
    order: &OrderHandle,
    binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
) -> Result<MatchOrder> {
    Ok(MatchOrder {
        field: BoundFieldId::new(
            lookup_binding_id(order.binding_token(), binding_ids)?,
            order.0.field.0.field_id.clone(),
        ),
        direction: order.0.direction,
        missing: order.0.missing,
    })
}

fn lower_selection(
    selection: &SelectionHandle,
    binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
) -> Result<FetchSlot> {
    let binding = lookup_binding_id(selection.0.binding.token(), binding_ids)?;
    match &selection.0.kind {
        SelectionKind::One => Ok(FetchSlot::One { binding }),
        SelectionKind::Collect { distinct, order } => Ok(FetchSlot::Collect {
            binding,
            distinct: *distinct,
            order: order
                .iter()
                .map(|order| lower_order(order, binding_ids))
                .collect::<Result<_>>()?,
        }),
    }
}

fn lower_shape(
    shape: &ShapeHandle,
    binding_ids: &BTreeMap<SessionBindingToken, BindingId>,
) -> Result<FetchShape> {
    match &shape.0.kind {
        ShapeKind::Positional(slots) => Ok(FetchShape::Positional {
            slots: slots
                .iter()
                .map(|slot| lower_selection(slot, binding_ids))
                .collect::<Result<_>>()?,
        }),
        ShapeKind::Named(slots) => Ok(FetchShape::Named {
            slots: slots
                .iter()
                .map(|(name, slot)| {
                    Ok(NamedFetchSlot {
                        name: name.clone(),
                        slot: lower_selection(slot, binding_ids)?,
                    })
                })
                .collect::<Result<_>>()?,
        }),
    }
}
