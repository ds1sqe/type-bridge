use serde::{Deserialize, Serialize};
use type_bridge_contract::value::CanonicalValue;

/// Entity or relation target kind in the canonical typed match AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedThingKind {
    /// An entity binding.
    Entity,
    /// A relation binding.
    Relation,
}

/// A typed literal accepted by canonical match lowering.
///
/// This representation cannot carry an arbitrary JSON shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TypedLiteral {
    /// TypeQL string.
    String(String),
    /// TypeQL signed 64-bit integer.
    Long(i64),
    /// TypeQL finite double.
    Double(f64),
    /// TypeQL boolean.
    Boolean(bool),
    /// Canonical TypeQL date spelling.
    Date(String),
    /// Canonical TypeQL timezone-free datetime spelling.
    DateTime(String),
    /// Canonical TypeQL timezone-aware datetime spelling.
    DateTimeTz(String),
    /// Canonical arbitrary-precision decimal spelling.
    Decimal(String),
    /// Canonical TypeQL duration spelling.
    Duration(String),
}

/// Typed comparison operators supported by canonical match predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedComparisonOperator {
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

/// One exact or subtype-inclusive thing target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedMatchTarget {
    /// Deterministic plan-local binding ordinal.
    pub binding: u16,
    /// Entity or relation target kind.
    pub kind: TypedThingKind,
    /// Validated TypeDB type label.
    pub type_name: String,
    /// Whether lowering uses strict `isa!` matching.
    pub exact: bool,
}

/// One owner-qualified field variable in a typed match statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedFieldBinding {
    /// Deterministic statement-local field ordinal.
    pub id: u16,
    /// Owner binding ordinal.
    pub owner: u16,
    /// Validated TypeDB attribute label.
    pub field_name: String,
}

/// Canonical typed predicate tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypedMatchPredicate {
    /// Compare one bound owner-qualified field with a typed literal.
    FieldValue {
        /// Field-binding ordinal.
        field: u16,
        /// Typed comparison operator.
        operator: TypedComparisonOperator,
        /// Typed literal operand.
        value: TypedLiteral,
    },
    /// Compare two owner-qualified field bindings.
    FieldComparison {
        /// Left field-binding ordinal.
        left: u16,
        /// Typed comparison operator.
        operator: TypedComparisonOperator,
        /// Right field-binding ordinal.
        right: u16,
    },
    /// Compare scalar operands after evaluating exact schema-function calls.
    FunctionComparison {
        /// Function calls in dependency-first order.
        calls: Vec<TypedFunctionCall>,
        /// Left scalar operand.
        left: TypedScalarOperand,
        /// Typed comparison operator.
        operator: TypedComparisonOperator,
        /// Right scalar operand.
        right: TypedScalarOperand,
    },
    /// Require an owner-qualified field to be present or absent.
    FieldPresence {
        /// Field-binding ordinal.
        field: u16,
        /// `true` requires at least one owned value; `false` requires none.
        present: bool,
    },
    /// Match one thing binding by its canonical provider IID.
    BindingIid {
        /// Thing-binding ordinal.
        binding: u16,
        /// Canonical TypeDB thing IID.
        iid: String,
    },
    /// Require one relation binding to link one player through a role.
    RoleEdge {
        /// Deterministic role-edge ordinal.
        edge: u16,
        /// Relation binding ordinal.
        relation: u16,
        /// Validated relation-role label.
        role_name: String,
        /// Player binding ordinal.
        player: u16,
    },
    /// Require a finite directed walk between two bound endpoints.
    Reachable {
        /// Exact validated relation type label used for every hop.
        relation_type: String,
        /// Ordered role played by each hop's source endpoint.
        role_from: String,
        /// Ordered role played by each hop's target endpoint.
        role_to: String,
        /// Bound walk source ordinal.
        source: u16,
        /// Bound walk target ordinal.
        target: u16,
        /// Inclusive minimum hop count. Zero is exact endpoint identity.
        min_depth: u8,
        /// Inclusive finite maximum hop count.
        max_depth: u8,
    },
    /// Require every child expression.
    And {
        /// Child predicates in canonical source order.
        expressions: Vec<TypedMatchPredicate>,
    },
    /// Require at least one child expression.
    Or {
        /// Child predicates in canonical source order.
        expressions: Vec<TypedMatchPredicate>,
    },
    /// Negate one correlated child expression.
    Not {
        /// Negated predicate.
        expression: Box<TypedMatchPredicate>,
    },
}

/// One typed schema-function argument after call dependencies are flattened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypedFunctionArgument {
    /// One existing thing binding.
    Binding {
        /// Existing thing-binding ordinal.
        binding: u16,
    },
    /// One canonical scalar supplied through `given`.
    Value {
        /// Exact canonical scalar value.
        value: CanonicalValue,
    },
    /// One prior call result in this predicate.
    CallResult {
        /// Prior call-result ordinal.
        call: u16,
    },
}

/// One exact scalar schema-function call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedFunctionCall {
    /// Dense predicate-local result ordinal.
    pub result: u16,
    /// Validated schema function label.
    pub function: String,
    /// Ordered exact-signature arguments.
    pub arguments: Vec<TypedFunctionArgument>,
}

/// One operand in a schema-function scalar comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypedScalarOperand {
    /// One bound owner-qualified field.
    Field {
        /// Bound-field ordinal.
        field: u16,
    },
    /// One canonical scalar supplied through `given`.
    Value {
        /// Exact canonical scalar value.
        value: CanonicalValue,
    },
    /// One function call result.
    CallResult {
        /// Call-result ordinal.
        call: u16,
    },
}

/// Sort direction in a typed selected-row statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedSortDirection {
    /// Ascending order.
    Ascending,
    /// Descending order.
    Descending,
}

/// Validated missing-value policy retained through provider lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedMissingOrder {
    /// Missing evidence is invalid.
    Reject,
    /// Missing values sort first.
    First,
    /// Missing values sort last.
    Last,
}

/// One validated total-order term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedMatchOrder {
    /// Field-binding ordinal.
    pub field: u16,
    /// Sort direction.
    pub direction: TypedSortDirection,
    /// Missing-value policy.
    pub missing: TypedMissingOrder,
}

/// A complete typed `FetchRows` provider statement.
///
/// The compiler derives every TypeQL variable from numeric IDs; the lowerer
/// never constructs variable names or query fragments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedFetchRows {
    /// Exact/subtype entity/relation targets in plan order.
    pub targets: Vec<TypedMatchTarget>,
    /// Owner-qualified fields in deterministic ordinal order.
    pub fields: Vec<TypedFieldBinding>,
    /// Optional nested predicate tree.
    pub predicate: Option<TypedMatchPredicate>,
    /// Selected binding ordinals in public output order.
    pub projection: Vec<u16>,
    /// Whether selected identity tuples are distinct.
    pub distinct: bool,
    /// Validator-derived total ordering.
    pub order: Vec<TypedMatchOrder>,
    /// Distinct selected tuples to skip.
    pub offset: u64,
    /// Maximum distinct selected tuples to return.
    pub limit: u64,
}

/// One typed distinct-root stream over the complete validated match graph.
///
/// Count and exists omit ordering/window state. Page selection carries only
/// the validator-derived root order and applies its offset/limit after root
/// identity distinctness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedRootScan {
    /// Exact/subtype entity/relation targets in plan order.
    pub targets: Vec<TypedMatchTarget>,
    /// Owner-qualified fields used by predicates or root ordering.
    pub fields: Vec<TypedFieldBinding>,
    /// Optional complete original predicate tree.
    pub predicate: Option<TypedMatchPredicate>,
    /// Page/count/exists root binding ordinal.
    pub root: u16,
    /// Validator-derived total root order; empty for count/exists.
    pub order: Vec<TypedMatchOrder>,
    /// Distinct roots to skip; absent for count/exists.
    pub offset: Option<u64>,
    /// Distinct roots to retain; absent for count and one for exists.
    pub limit: Option<u64>,
}

/// Validator-derived deterministic order for one collected binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedCollectionOrder {
    /// Collected binding ordinal.
    pub binding: u16,
    /// Binding-local total ordering terms.
    pub order: Vec<TypedMatchOrder>,
}

/// One original-graph re-match restricted to an exact root IID batch.
///
/// The provider returns one completely hydrated document per matching graph
/// solution. All positive bindings remain present so canonical predicate and
/// collection validation never relies on selected-only evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedPageRematch {
    /// Exact/subtype entity/relation targets in plan order.
    pub targets: Vec<TypedMatchTarget>,
    /// Owner-qualified fields used by the original predicate.
    pub fields: Vec<TypedFieldBinding>,
    /// Optional complete original predicate tree.
    pub predicate: Option<TypedMatchPredicate>,
    /// Page root binding ordinal.
    pub root: u16,
    /// Exact selected root IID set in stable page order.
    pub root_concept_ids: Vec<String>,
    /// Validator-derived binding-local collection orders.
    pub collection_orders: Vec<TypedCollectionOrder>,
}

/// One descriptor-qualified attribute requested during complete hydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedHydrationField {
    /// Binding-facing descriptor field name.
    pub field_name: String,
    /// TypeDB attribute type label.
    pub attribute_type: String,
    /// TypeDB value type label.
    pub value_type: String,
}

/// One descriptor-qualified relation role requested during complete hydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedHydrationRole {
    /// Relation role label.
    pub role_name: String,
    /// Compatible declared player type labels.
    pub player_types: Vec<String>,
}

/// Complete metadata for one possible concrete hydration descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedHydrationDescriptor {
    /// Concrete TypeDB type label.
    pub type_name: String,
    /// Entity/relation kind.
    pub kind: TypedThingKind,
    /// Complete effective owned-attribute metadata.
    pub fields: Vec<TypedHydrationField>,
    /// Complete effective role metadata; empty for entities.
    pub roles: Vec<TypedHydrationRole>,
}

/// All provider concept identities assigned to one plan-local binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedHydrationTarget {
    /// Plan-local binding ordinal.
    pub binding: u16,
    /// Declared target type label.
    pub declared_type: String,
    /// Declared entity/relation kind.
    pub kind: TypedThingKind,
    /// Deterministically ordered unique provider IIDs.
    pub concept_ids: Vec<String>,
    /// Declared descriptor and every currently registered compatible subtype.
    pub concrete_descriptors: Vec<TypedHydrationDescriptor>,
}

/// One kind-homogeneous batched same-transaction hydration statement.
///
/// The statement deliberately carries every entity or every relation identity
/// in its partition rather than exposing a per-binding query API. Keeping kinds
/// separate guarantees entity queries cannot execute relation-side `links`.
/// Provider adapters return complete attributes and, for relation batches,
/// complete role-player evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedHydrateThings {
    /// Deterministically ordered binding batches with complete schema metadata.
    pub targets: Vec<TypedHydrationTarget>,
}
