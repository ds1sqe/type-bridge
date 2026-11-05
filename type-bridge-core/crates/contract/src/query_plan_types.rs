//! Input, pattern, pipeline, output and identity vocabulary shared by native plans.

use crate::fingerprint::Fingerprint;
use crate::id::{AttributeId, FunctionId, Label, RoleId, TypeId};
use crate::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use crate::value::{CanonicalValue, ValueTypeTag};
use serde::Serialize;
use std::fmt;

/// Explicit TypeQL type matching semantics for an ownership pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum TypeMatchMode {
    /// Match the declared type and its subtypes (`isa`).
    #[serde(rename = "isa")]
    Isa,
    /// Match only the declared type (`isa!`).
    #[serde(rename = "isa!")]
    IsaExact,
}
impl TypeMatchMode {
    /// Parse an explicit TypeQL type-match form; no implicit default exists.
    pub fn from_typeql(value: &str) -> Result<Self, crate::diagnostic::Diagnostic> {
        match value {
            "isa" => Ok(Self::Isa),
            "isa!" => Ok(Self::IsaExact),
            _ => Err(crate::diagnostic::Diagnostic::new(
                crate::diagnostic::DiagnosticCategory::InvalidContract,
                crate::diagnostic::DiagnosticCode::new("query_plan_type_match_mode")
                    .expect("static code"),
                "type-match mode must be explicit: isa or isa!",
            )),
        }
    }
}

/// One dense typed input column identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct InputColumnId(u16);

impl InputColumnId {
    /// Construct a dense input column ordinal.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Return the dense ordinal.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for InputColumnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One typed input column declaration owned by the reusable plan.
///
/// Input declarations belong to the plan; input rows belong to the
/// invocation. Changing an input value never creates a new plan identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InputColumn {
    id: InputColumnId,
    optional: bool,
    public_name: QueryVariable,
    value_type: ValueTypeTag,
}

impl InputColumn {
    /// Construct one typed input column.
    #[must_use]
    pub const fn new(
        id: InputColumnId,
        public_name: QueryVariable,
        value_type: ValueTypeTag,
        optional: bool,
    ) -> Self {
        Self {
            id,
            optional,
            public_name,
            value_type,
        }
    }

    /// Return the dense column identity.
    #[must_use]
    pub const fn id(&self) -> InputColumnId {
        self.id
    }

    /// Return the binding-facing public column name.
    #[must_use]
    pub const fn public_name(&self) -> &QueryVariable {
        &self.public_name
    }

    /// Return the exact scalar type every row value must carry.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }

    /// Return whether invocation rows may omit this column's value.
    #[must_use]
    pub const fn optional(&self) -> bool {
        self.optional
    }
}

/// One typed operand inside a query value comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryOperand {
    /// Read the scalar value of an attribute binding.
    Binding {
        /// The referenced dense binding.
        binding: BindingId,
    },
    /// Compare with an exact canonical literal.
    Literal {
        /// The canonical scalar literal.
        value: CanonicalValue,
    },
    /// Read one declared invocation input column.
    Input {
        /// The referenced dense input column.
        column: InputColumnId,
    },
}

/// The closed typed pattern algebra of the first public vocabulary.
///
/// Exactly the V1-parity graph shapes: typed `isa` with optional subtype
/// inclusion, effective-ownership `has`, role-qualified `links`, typed value
/// comparison, and negation over a closed conjunction. No variant accepts an
/// arbitrary TypeQL fragment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryPattern {
    /// Constrain a binding to one schema type, optionally including subtypes.
    Isa {
        /// The constrained binding.
        binding: BindingId,
        /// Whether transitive subtypes are admitted.
        include_subtypes: bool,
        /// The exact schema type.
        type_id: TypeId,
    },
    /// Constrain a positively typed thing by canonical IID, including nested scopes.
    /// Admitted only by the capability-fenced native plan vocabulary.
    Iid {
        /// Thing binding established in this lexical scope or an outer one.
        binding: BindingId,
        /// Canonical IID string literal or declared string input.
        iid: QueryOperand,
    },
    /// Bind an owned attribute through an effective ownership.
    Has {
        /// The bound attribute instance.
        attribute: BindingId,
        /// The exact attribute type.
        attribute_id: AttributeId,
        /// The owning binding.
        owner: BindingId,
    },
    /// Bind an ownership with explicit TypeQL attribute type matching.
    HasType {
        /// The bound attribute instance.
        attribute: BindingId,
        /// Attribute label, including abstract supertypes in `isa` mode.
        attribute_id: AttributeId,
        /// The owning binding.
        owner: BindingId,
        /// Required TypeQL form; never inferred from the schema.
        mode: TypeMatchMode,
    },
    /// Bind a relation and role-qualified players.
    Links {
        /// Role-qualified player bindings.
        players: Vec<AssertionRolePlayer>,
        /// The bound relation instance.
        relation: BindingId,
        /// The exact relation type.
        relation_id: TypeId,
    },
    /// Compare two exact typed scalar operands.
    Value {
        /// The closed comparator.
        comparator: ValueComparator,
        /// The left operand.
        left: QueryOperand,
        /// The right operand.
        right: QueryOperand,
    },
    /// Require at least one child pattern.
    ///
    /// A binding is positively established after this pattern only when every
    /// branch establishes it. Branch-local bindings never leak into the outer
    /// row environment.
    Or {
        /// Non-empty alternative conjunctions in source order.
        branches: Vec<Vec<QueryPattern>>,
    },
    /// Negate a closed nested conjunction.
    Not {
        /// The negated conjunction.
        patterns: Vec<QueryPattern>,
    },
    /// Optionally match a nested conjunction without filtering rows.
    ///
    /// Bindings established only inside the body survive as optional: rows
    /// where the body matched carry them, other rows carry an explicit
    /// absence. The first optional vocabulary admits `isa`, `has`, `links`,
    /// and value comparisons in the body, at the root conjunction only.
    Try {
        /// The optional conjunction.
        patterns: Vec<QueryPattern>,
    },
    /// Existential bounded reachability along one role-directed relation.
    ///
    /// Holds when the target is reachable from the source in
    /// `min_depth..=max_depth` hops, each hop one relation instance from the
    /// `role_from` player to the `role_to` player. A zero-hop branch is exact
    /// concept identity. Positive branches are finite directed walks: repeated
    /// vertices or relation instances are admitted, so cycles cannot make
    /// execution unbounded. Proof-path identity is existential and never
    /// enters the selected row. Lowering unrolls every admitted length
    /// provider-side, never by repeated client queries.
    Reachable {
        /// The inclusive minimum hop count. Zero admits source/target identity.
        min_depth: u8,
        /// The inclusive finite maximum hop count.
        max_depth: u8,
        /// The exact relation type of every hop.
        relation: TypeId,
        /// The role the hop starts from.
        role_from: RoleId,
        /// The role the hop arrives at.
        role_to: RoleId,
        /// The established start binding.
        source: BindingId,
        /// The established end binding.
        target: BindingId,
    },
    /// Assign one scalar schema-function result to a binding.
    ///
    /// The first function vocabulary admits scalar, non-optional returns
    /// only; tuple and stream returns stay reserved behind later
    /// capabilities.
    FunctionCall {
        /// Ordered call arguments.
        arguments: Vec<QueryOperand>,
        /// The binding assigned from the scalar return.
        assigned: BindingId,
        /// The exact schema function identity.
        function: FunctionId,
    },
}

/// The sort direction of one order term.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

crate::enum_spellings!(OrderDirection, "order_direction", canonical {
    "ascending" => Ascending,
    "descending" => Descending,
});

/// One typed sort key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrderTerm {
    binding: BindingId,
    direction: OrderDirection,
}

impl OrderTerm {
    /// Construct one sort key.
    #[must_use]
    pub const fn new(binding: BindingId, direction: OrderDirection) -> Self {
        Self { binding, direction }
    }

    /// Return the sorted binding.
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the sort direction.
    #[must_use]
    pub const fn direction(&self) -> OrderDirection {
        self.direction
    }
}

/// The closed reducer vocabulary of the first reduce stage.
///
/// Reducers that can observe an empty stream (`max`, `min`, `mean`,
/// `median`, `std`) are admitted only under group bindings, where every
/// group is witnessed by at least one row; `count` and `sum` stay total on
/// empty streams.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reducer {
    /// Count surviving rows.
    Count,
    /// The largest input value.
    Max,
    /// The arithmetic mean of input values.
    Mean,
    /// The statistical median of input values.
    Median,
    /// The smallest input value.
    Min,
    /// The sample standard deviation of input values.
    Std,
    /// The total of input values.
    Sum,
}

crate::enum_spellings!(Reducer, "plan_reducer", canonical {
    "count" => Count,
    "max" => Max,
    "mean" => Mean,
    "median" => Median,
    "min" => Min,
    "std" => Std,
    "sum" => Sum,
});

impl Reducer {
    /// Whether this reducer yields a defined result on an empty stream.
    #[must_use]
    pub const fn total_without_groups(self) -> bool {
        matches!(self, Self::Count | Self::Sum)
    }

    /// Whether this reducer consumes an input binding.
    #[must_use]
    pub const fn requires_input(self) -> bool {
        !matches!(self, Self::Count)
    }
}

/// One reduce-stage assignment producing a fresh value binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReduceAssignment {
    assigned: BindingId,
    input: Option<BindingId>,
    reducer: Reducer,
}

impl ReduceAssignment {
    /// Assign one reducer result to a fresh declared binding.
    #[must_use]
    pub const fn new(assigned: BindingId, reducer: Reducer, input: Option<BindingId>) -> Self {
        Self {
            assigned,
            input,
            reducer,
        }
    }

    /// Return the fresh binding the reducer result is assigned to.
    #[must_use]
    pub const fn assigned(&self) -> BindingId {
        self.assigned
    }

    /// Return the reduced input binding, absent for bare `count`.
    #[must_use]
    pub const fn input(&self) -> Option<BindingId> {
        self.input
    }

    /// Return the reducer applied within each group.
    #[must_use]
    pub const fn reducer(&self) -> Reducer {
        self.reducer
    }
}

/// One ordered read stage of the first public vocabulary.
///
/// The canonical stage order is fixed: one `match`, then at most one each of
/// `select`, `require`, `distinct`, `reduce`, `sort`, `offset`, and `limit`,
/// in that order. Later stage kinds (documents) are reserved.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadStage {
    /// The single pattern conjunction producing the row environment.
    Match {
        /// The closed positive/negative conjunction.
        patterns: Vec<QueryPattern>,
    },
    /// Restrict the visible row environment to these bindings.
    Select {
        /// Canonical-sorted visible bindings.
        bindings: Vec<BindingId>,
    },
    /// Require optional bindings to be present in every surviving row.
    Require {
        /// Canonical-sorted required bindings.
        bindings: Vec<BindingId>,
    },
    /// Deduplicate the visible row environment.
    Distinct,
    /// Collapse rows into grouped reducer results.
    ///
    /// The surviving row environment is exactly the group bindings plus the
    /// assigned reducer results; every other binding ends here.
    Reduce {
        /// Reducer assignments, each producing a fresh value binding.
        assignments: Vec<ReduceAssignment>,
        /// Canonical-sorted group-key bindings; empty for a global reduce.
        groups: Vec<BindingId>,
    },
    /// Impose an explicit total row order.
    Sort {
        /// Ordered sort keys.
        terms: Vec<OrderTerm>,
    },
    /// Skip an exact number of ordered rows.
    Offset {
        /// The number of rows skipped.
        rows: u64,
    },
    /// Truncate to an exact number of ordered rows.
    Limit {
        /// The maximum number of rows returned.
        rows: u64,
    },
}

impl ReadStage {
    pub(super) const fn ordinal(&self) -> u8 {
        match self {
            Self::Match { .. } => 0,
            Self::Select { .. } => 1,
            Self::Require { .. } => 2,
            Self::Distinct => 3,
            Self::Reduce { .. } => 4,
            Self::Sort { .. } => 5,
            Self::Offset { .. } => 6,
            Self::Limit { .. } => 7,
        }
    }
}

/// The declared scalar return of one plan-local function.
///
/// The first local vocabulary admits only reducers that stay total on an
/// empty body stream (`count`, `sum`), so every call yields a value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalReturn {
    input: BindingId,
    reducer: Reducer,
    value_type: ValueTypeTag,
}

impl LocalReturn {
    /// Declare one total reducer return over a body binding.
    #[must_use]
    pub const fn new(reducer: Reducer, input: BindingId, value_type: ValueTypeTag) -> Self {
        Self {
            input,
            reducer,
            value_type,
        }
    }

    /// Return the reduced body binding.
    #[must_use]
    pub const fn input(&self) -> BindingId {
        self.input
    }

    /// Return the reducer.
    #[must_use]
    pub const fn reducer(&self) -> Reducer {
        self.reducer
    }

    /// Return the declared scalar result type.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }
}

/// One plan-local function defined from the closed pattern algebra.
///
/// The function owns a private dense binding space; its parameters are the
/// leading bindings, each declared against one schema type label. The first
/// local vocabulary keeps bodies flat (`isa`, `has`, `links`, value) and
/// returns one total reducer result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalFunction {
    bindings: Vec<AssertionBinding>,
    body: Vec<QueryPattern>,
    name: FunctionId,
    parameters: Vec<Label>,
    returns: LocalReturn,
}

impl LocalFunction {
    /// Declare one plan-local function.
    #[must_use]
    pub const fn new(
        name: FunctionId,
        bindings: Vec<AssertionBinding>,
        parameters: Vec<Label>,
        body: Vec<QueryPattern>,
        returns: LocalReturn,
    ) -> Self {
        Self {
            bindings,
            body,
            name,
            parameters,
            returns,
        }
    }

    /// Return the function name.
    #[must_use]
    pub const fn name(&self) -> &FunctionId {
        &self.name
    }

    /// Return the private dense binding table.
    #[must_use]
    pub fn bindings(&self) -> &[AssertionBinding] {
        &self.bindings
    }

    /// Return the schema type label of each leading parameter binding.
    #[must_use]
    pub fn parameters(&self) -> &[Label] {
        &self.parameters
    }

    /// Return the closed body conjunction.
    #[must_use]
    pub fn body(&self) -> &[QueryPattern] {
        &self.body
    }

    /// Return the declared reducer result.
    #[must_use]
    pub const fn returns(&self) -> &LocalReturn {
        &self.returns
    }
}

/// One value source of a fetched document field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocumentSource {
    /// The scalar value of one visible binding; optional bindings
    /// fetch as an explicit JSON null where absent.
    Binding {
        /// The projected scalar binding.
        binding: BindingId,
    },
    /// Every value of one attribute on one mandatory owner binding,
    /// fetched as a typed list (empty where the owner has none).
    AttributeList {
        /// The listed attribute.
        attribute: AttributeId,
        /// The mandatory owner binding.
        owner: BindingId,
    },
}

/// One key-value field of a fetched document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DocumentField {
    key: QueryVariable,
    source: DocumentSource,
}

impl DocumentField {
    /// Bind one document key to its value source.
    #[must_use]
    pub const fn new(key: QueryVariable, source: DocumentSource) -> Self {
        Self { key, source }
    }

    /// Return the document key.
    #[must_use]
    pub const fn key(&self) -> &QueryVariable {
        &self.key
    }

    /// Return the value source.
    #[must_use]
    pub const fn source(&self) -> &DocumentSource {
        &self.source
    }
}

/// The explicit output category of the first public vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryOutput {
    /// Project ordered typed row columns.
    Rows {
        /// The visible bindings projected, in output order.
        columns: Vec<BindingId>,
    },
    /// Fetch one flat typed JSON document per surviving row.
    Documents {
        /// The document fields, in output order.
        fields: Vec<DocumentField>,
    },
}

/// One rectangular invocation input row.
///
/// Values are positional by dense input column ordinal. `None` is admitted
/// only where the declaring column is optional; there is no other
/// null-shaped default.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct InputRow {
    pub(super) values: Vec<Option<CanonicalValue>>,
}

impl InputRow {
    /// Construct one positional input row.
    #[must_use]
    pub const fn new(values: Vec<Option<CanonicalValue>>) -> Self {
        Self { values }
    }

    /// Return positional values by dense column ordinal.
    #[must_use]
    pub fn values(&self) -> &[Option<CanonicalValue>] {
        &self.values
    }
}

/// Fingerprint of exact canonical query-plan bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct QueryPlanFingerprint(pub(super) Fingerprint);

impl QueryPlanFingerprint {
    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}
