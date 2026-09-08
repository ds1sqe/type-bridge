use serde::{Deserialize, Serialize};

/// A value expression in TypeQL.
///
/// Represents any expression that evaluates to a value, including literals,
/// function calls, arithmetic operations, and variable references.
/// Uses tagged JSON serialization (`"type"` + `"data"`) for portable interchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Value {
    /// A typed literal value (e.g. `"Alice"`, `30`, `true`).
    Literal(LiteralValue),
    /// A function invocation (e.g. `count($x)`, `max($salary)`).
    FunctionCall(FunctionCallValue),
    /// A binary arithmetic expression (e.g. `$base + $bonus`).
    Arithmetic(ArithmeticValue),
    /// A reference to a query variable (e.g. `$x`).
    Variable(String),
}

/// A typed literal value in TypeQL.
///
/// Stores the raw JSON representation of a value together with its TypeQL
/// value type name (e.g. `"string"`, `"long"`, `"double"`, `"boolean"`,
/// `"datetime"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiteralValue {
    /// The literal value as a JSON-compatible representation.
    pub value: serde_json::Value,
    /// The TypeQL value type name (e.g. `"string"`, `"long"`, `"double"`).
    pub value_type: String,
}

/// A function invocation value in TypeQL.
///
/// Represents calling a built-in or user-defined function with a list of
/// argument expressions, such as `count($x)` or `max($salary)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCallValue {
    /// The name of the function being called (e.g. `"count"`, `"max"`, `"min"`).
    pub function: String,
    /// The argument expressions passed to the function.
    pub args: Vec<Value>,
}

/// A binary arithmetic expression in TypeQL.
///
/// Represents an infix arithmetic operation such as `$base + $bonus` or
/// `$price * $quantity`. The operands are boxed `Value` nodes to allow
/// arbitrary nesting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArithmeticValue {
    /// The left-hand operand of the arithmetic expression.
    pub left: Box<Value>,
    /// The arithmetic operator (e.g. `"+"`, `"-"`, `"*"`, `"/"`).
    pub operator: String,
    /// The right-hand operand of the arithmetic expression.
    pub right: Box<Value>,
}

/// A participant in a TypeQL relation.
///
/// Binds a role name to a player variable, representing one side of a
/// relation pattern such as `friendship:friend($alice)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RolePlayer {
    /// The role name within the relation (e.g. `"friend"`, `"employee"`).
    pub role: String,
    /// The variable name of the entity playing this role (e.g. `"$alice"`).
    pub player_var: String,
}

/// A constraint applied to a pattern variable.
///
/// Constraints narrow the results of a match clause by requiring an IID,
/// an attribute ownership, or a type assertion on a variable.
/// Uses tagged JSON serialization (`"type"` + `"data"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Constraint {
    /// An internal identifier (IID) lookup constraint.
    Iid(String),
    /// A has-attribute constraint requiring the variable to own an attribute
    /// with the given name and value.
    Has {
        /// The attribute type name (e.g. `"name"`, `"age"`).
        attr_name: String,
        /// The expected value of the attribute.
        value: Value,
    },
    /// A type assertion constraint (`isa` or `isa!`).
    Isa {
        /// The type name to assert (e.g. `"person"`, `"company"`).
        type_name: String,
        /// Whether to use strict type matching (`isa!`) rather than inclusive (`isa`).
        strict: bool,
    },
}

/// A match-clause pattern in TypeQL.
///
/// Patterns describe the shape of data to match in the database. They can
/// represent entities, relations, subtypes, attributes, has-bindings,
/// comparisons, negations, disjunctions, IID lookups, or raw TypeQL strings.
/// Uses tagged JSON serialization (`"type"` + `"data"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Pattern {
    /// An entity pattern matching a typed entity with optional constraints.
    Entity {
        /// The query variable bound to this entity (e.g. `"$p"`).
        variable: String,
        /// The entity type name (e.g. `"person"`).
        type_name: String,
        /// Additional constraints on the entity (has-attribute, IID, etc.).
        constraints: Vec<Constraint>,
        /// Whether to use strict type matching (`isa!`) instead of inclusive (`isa`).
        is_strict: bool,
    },
    /// A relation pattern matching a typed relation with role players.
    Relation {
        /// The query variable bound to this relation (e.g. `"$r"`).
        variable: String,
        /// The relation type name (e.g. `"friendship"`).
        type_name: String,
        /// The role players participating in this relation.
        role_players: Vec<RolePlayer>,
        /// Additional constraints on the relation.
        constraints: Vec<Constraint>,
    },
    /// A subtype pattern matching variables whose type is a subtype of a parent.
    SubType {
        /// The query variable bound to the subtype.
        variable: String,
        /// The parent type name to match subtypes of.
        parent_type: String,
    },
    /// An attribute pattern matching a typed attribute with an optional value.
    Attribute {
        /// The query variable bound to this attribute.
        variable: String,
        /// The attribute type name (e.g. `"name"`, `"age"`).
        type_name: String,
        /// An optional value to constrain the attribute to.
        value: Option<Value>,
    },
    /// A has-binding pattern linking a thing variable to an attribute variable.
    Has {
        /// The variable of the thing that owns the attribute.
        thing_var: String,
        /// The attribute type name.
        attr_type: String,
        /// The variable bound to the attribute instance.
        attr_var: String,
    },
    /// A value comparison pattern (e.g. `$age > 18`).
    ValueComparison {
        /// The variable being compared.
        var: String,
        /// The comparison operator (e.g. `">"`, `"<"`, `"=="`, `"!="`, `">="`, `"<="`).
        operator: String,
        /// The value to compare against.
        value: Value,
    },
    /// A negation pattern — none of the inner patterns may match.
    Not(Vec<Pattern>),
    /// An optional pattern — the inner patterns are matched when present but
    /// do not exclude a row when absent. Variables bound only inside are
    /// absent (not present in the row) when the block does not match.
    Try(Vec<Pattern>),
    /// A disjunction pattern — at least one branch of patterns must match.
    /// Each inner `Vec<Pattern>` represents one branch of the `or`.
    Or(Vec<Vec<Pattern>>),
    /// An IID lookup pattern matching a specific internal identifier.
    Iid {
        /// The query variable bound to the thing with this IID.
        variable: String,
        /// The internal identifier value.
        iid: String,
    },
    /// A raw TypeQL string pattern, passed through without further processing.
    Raw(String),
}

/// A write-clause statement in TypeQL.
///
/// Statements describe mutations to perform in insert, delete, or update
/// clauses. They can assign attributes, assert types, create relations,
/// delete things, or pass through raw TypeQL strings.
/// Uses tagged JSON serialization (`"type"` + `"data"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Statement {
    /// A has-attribute statement assigning an attribute value to a subject.
    Has {
        /// The variable of the subject that will own the attribute.
        subject_var: String,
        /// The attribute type name (e.g. `"name"`, `"email"`).
        attr_name: String,
        /// The value to assign to the attribute.
        value: Value,
    },
    /// A type assertion statement (`isa`) declaring a variable's type.
    Isa {
        /// The query variable being typed.
        variable: String,
        /// The type name to assert (e.g. `"person"`, `"company"`).
        type_name: String,
    },
    /// A relation creation statement with role players and optional inline attributes.
    Relation {
        /// The query variable bound to the new relation.
        variable: String,
        /// The relation type name (e.g. `"employment"`).
        type_name: String,
        /// The role players participating in this relation.
        role_players: Vec<RolePlayer>,
        /// Whether to include the variable assignment in the generated TypeQL.
        include_variable: bool,
        /// Inline has-attribute statements on the relation (recursive `Statement::Has`).
        attributes: Vec<Statement>,
    },
    /// A delete statement removing a thing by its variable name.
    DeleteThing(String),
    /// A raw TypeQL string statement, passed through without further processing.
    Raw(String),
}

/// A top-level query clause in TypeQL.
///
/// Clauses are the building blocks of a complete TypeQL query. A query is
/// composed of one or more clauses chained together (e.g. match then fetch,
/// match then insert, match then delete).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Clause {
    /// A match clause containing patterns to match against the database.
    Match(Vec<Pattern>),
    /// A match-let clause with variable bindings evaluated during matching.
    MatchLet(Vec<LetAssignment>),
    /// An insert clause containing statements that create new data.
    Insert(Vec<Statement>),
    /// A put clause containing statements for idempotent insert (upsert).
    ///
    /// Semantics: insert if not exists, otherwise return the existing match.
    /// Uses the same statement syntax as `Insert`.
    Put(Vec<Statement>),
    /// A delete clause containing statements that remove existing data.
    Delete(Vec<Statement>),
    /// An update clause containing statements that modify existing data.
    Update(Vec<Statement>),
    /// A fetch clause specifying which attributes and values to return.
    Fetch(Vec<FetchItem>),
    /// A reduce clause performing aggregations over matched results.
    Reduce {
        /// The aggregation assignments (e.g. `$total = count($x)`).
        assignments: Vec<ReduceAssignment>,
        /// An optional variable to group results by before aggregating.
        group_by: Option<String>,
    },
    /// A sort clause ordering results by one or more variables.
    Sort(Vec<SortField>),
    /// A limit clause restricting the number of results.
    Limit(u64),
    /// An offset clause skipping a number of results.
    Offset(u64),
}

/// A variable binding in a match-let clause.
///
/// Assigns the result of an expression to one or more variables during
/// the match phase, optionally as a stream of values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetAssignment {
    /// The variable names to bind the expression result to.
    pub variables: Vec<String>,
    /// The expression whose result is assigned to the variables.
    pub expression: Value,
    /// Whether this assignment produces a stream of values rather than a single value.
    pub is_stream: bool,
}

/// An item in a fetch clause specifying what data to return.
///
/// Fetch items describe the shape of the query response, selecting specific
/// attributes, variables, attribute lists, function results, or wildcards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FetchItem {
    /// Fetch a single attribute value from a variable, keyed in the response.
    Attribute {
        /// The key name in the response object.
        key: String,
        /// The variable that owns the attribute.
        var: String,
        /// The attribute type name to fetch.
        attr_name: String,
    },
    /// Fetch a variable's value directly, keyed in the response.
    Variable {
        /// The key name in the response object.
        key: String,
        /// The variable to fetch.
        var: String,
    },
    /// Fetch all values of a multi-valued attribute as a list.
    AttributeList {
        /// The key name in the response object.
        key: String,
        /// The variable that owns the attribute.
        var: String,
        /// The attribute type name to fetch as a list.
        attr_name: String,
    },
    /// Fetch the result of applying a function to a variable.
    Function {
        /// The key name in the response object.
        key: String,
        /// The function name to apply (e.g. `"count"`, `"sum"`).
        func_name: String,
        /// The variable to pass as the function argument.
        var: String,
    },
    /// Fetch all attributes of a variable (wildcard expansion).
    Wildcard {
        /// The key name in the response object.
        key: String,
        /// The variable whose attributes are fetched.
        var: String,
    },
    /// Fetch all attributes of a variable and its nested owned things.
    NestedWildcard {
        /// The key name in the response object.
        key: String,
        /// The variable whose attributes and nested things are fetched.
        var: String,
    },
}

/// A sort field specifying a variable and sort direction.
///
/// Used within [`Clause::Sort`] to define the ordering of query results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    /// The variable to sort by (e.g. `"$age"`, `"$name"`).
    pub variable: String,
    /// Whether to sort in ascending order (`true`) or descending (`false`).
    pub ascending: bool,
}

/// An aggregation assignment in a reduce clause.
///
/// Binds the result of an aggregation expression to a variable name,
/// such as `$total = count($x)` or `$avg_salary = mean($s)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReduceAssignment {
    /// The variable name to bind the aggregation result to.
    pub variable: String,
    /// The aggregation expression (typically a `Value::FunctionCall`).
    pub expression: Value,
}

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
/// Unlike the legacy portable [`LiteralValue`], this representation cannot
/// carry an arbitrary JSON shape.
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
