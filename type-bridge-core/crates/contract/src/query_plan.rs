//! Reusable typed query plans: the first public V2 read vocabulary.
//!
//! A [`QueryPlan`] extends the minimal migration-assertion primitives into a
//! reusable, invocation-free read program: dense typed bindings, declared
//! typed inputs, one closed pattern conjunction, and an ordered pipeline of
//! V1-parity stages (`select`, `require`, `distinct`, `sort`, `offset`,
//! `limit`) ending in one explicit output. Later vocabulary — functions,
//! reductions, documents, reachability — stays reserved behind independent
//! capabilities and is absent from this format, not defaulted.
//!
//! The persisted Phase 4 assertion algebra keeps its exact meaning: this
//! module defines its own pattern vocabulary rather than widening
//! [`AssertionPattern`](crate::migration_assertion::AssertionPattern).

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::to_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, FunctionId, Label, RoleId, TypeId};
use crate::limits::StructuralLimits;
use crate::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use crate::value::{CanonicalValue, ValueTypeTag};

/// The exact wire discriminator for first-format query plans.
pub const QUERY_PLAN_FORMAT_V1: &str = "typebridge.query-plan/v1";
/// Domain separating query-plan fingerprints from every other digest.
pub const QUERY_PLAN_FINGERPRINT_DOMAIN: &str = "typebridge.query.plan";
/// Canonicalization version fingerprints commit to.
pub const QUERY_PLAN_CANONICALIZATION: &str = "typebridge.query-plan-c14n/v1";

const CAP_PLAN: &str = "query.plan";
const CAP_ISA: &str = "query.pattern.isa";
const CAP_ISA_SUBTYPES: &str = "query.pattern.isa-subtypes";
const CAP_HAS: &str = "query.pattern.has";
const CAP_LINKS: &str = "query.pattern.links";
const CAP_VALUE: &str = "query.pattern.value";
const CAP_NEGATION: &str = "query.pattern.negation";
const CAP_INPUT_COLUMNS: &str = "query.input.columns";
const CAP_STAGE_SELECT: &str = "query.stage.select";
const CAP_STAGE_REQUIRE: &str = "query.stage.require";
const CAP_STAGE_DISTINCT: &str = "query.stage.distinct";
const CAP_STAGE_SORT: &str = "query.stage.sort";
const CAP_STAGE_OFFSET: &str = "query.stage.offset";
const CAP_STAGE_LIMIT: &str = "query.stage.limit";
const CAP_OUTPUT_ROWS: &str = "query.output.rows";
const CAP_FUNCTION_CALL: &str = "query.pattern.function-call";
const CAP_STAGE_REDUCE: &str = "query.stage.reduce";
const CAP_TRY: &str = "query.pattern.try";
const CAP_OUTPUT_DOCUMENTS: &str = "query.output.documents";
const CAP_LOCAL_FUNCTIONS: &str = "query.function.local";
const CAP_REACHABLE: &str = "query.pattern.reachable";
const CAP_INPUT_GIVEN_ROWS: &str = "query.input.given-rows";

/// Return every capability the first query-plan vocabulary can require.
#[must_use]
pub fn query_plan_capability_vocabulary() -> CapabilitySet {
    [
        CAP_PLAN,
        CAP_ISA,
        CAP_ISA_SUBTYPES,
        CAP_HAS,
        CAP_LINKS,
        CAP_VALUE,
        CAP_NEGATION,
        CAP_INPUT_COLUMNS,
        CAP_STAGE_SELECT,
        CAP_STAGE_REQUIRE,
        CAP_STAGE_DISTINCT,
        CAP_STAGE_SORT,
        CAP_STAGE_OFFSET,
        CAP_STAGE_LIMIT,
        CAP_OUTPUT_ROWS,
        CAP_FUNCTION_CALL,
        CAP_STAGE_REDUCE,
        CAP_TRY,
        CAP_OUTPUT_DOCUMENTS,
        CAP_LOCAL_FUNCTIONS,
        CAP_REACHABLE,
    ]
    .into_iter()
    .map(|value| CapabilityId::new(value).expect("static capability id is canonical"))
    .collect()
}

/// Return the transport capability multi-row `given` invocations require.
///
/// Plans never require this capability — it is derived from invocation
/// cardinality by [`QueryInvocation::transport_capabilities`], so it is
/// not part of [`query_plan_capability_vocabulary`]. Executors advertise
/// it only when their provider can transport explicit input rows, which
/// makes multi-row admission truthful at preflight instead of failing
/// after a transaction exists.
#[must_use]
pub fn query_given_rows_capability() -> CapabilityId {
    CapabilityId::new(CAP_INPUT_GIVEN_ROWS).expect("static capability id is canonical")
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
    /// Bind an owned attribute through an effective ownership.
    Has {
        /// The bound attribute instance.
        attribute: BindingId,
        /// The exact attribute type.
        attribute_id: AttributeId,
        /// The owning binding.
        owner: BindingId,
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
    /// Holds when the target is reachable from the source in at most
    /// `max_depth` hops, each hop one relation instance from the `role_from`
    /// player to the `role_to` player. The bound is mandatory and finite;
    /// lowering unrolls it provider-side, never by repeated client queries.
    Reachable {
        /// The mandatory finite hop bound (at least one).
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
/// Reducers that can observe an empty stream (`max`, `min`, `mean`) are
/// admitted only under group bindings, where every group is witnessed by at
/// least one row; `count` and `sum` stay total on empty streams.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reducer {
    /// Count surviving rows.
    Count,
    /// The largest input value.
    Max,
    /// The arithmetic mean of input values.
    Mean,
    /// The smallest input value.
    Min,
    /// The total of input values.
    Sum,
}

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
    const fn ordinal(&self) -> u8 {
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

/// A reusable, invocation-free typed read program.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueryPlan {
    bindings: Vec<AssertionBinding>,
    format: String,
    functions: Vec<LocalFunction>,
    inputs: Vec<InputColumn>,
    managed_semantics: ManagedSemanticSchemaFingerprint,
    output: QueryOutput,
    pipeline: Vec<ReadStage>,
    required_capabilities: CapabilitySet,
}

impl QueryPlan {
    /// Validate and construct one canonical plan under fixed protocol limits.
    pub fn new(
        bindings: Vec<AssertionBinding>,
        inputs: Vec<InputColumn>,
        pipeline: Vec<ReadStage>,
        output: QueryOutput,
        managed_semantics: ManagedSemanticSchemaFingerprint,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_limits(
            bindings,
            Vec::new(),
            inputs,
            pipeline,
            output,
            managed_semantics,
            StructuralLimits::CANONICAL,
        )
    }

    /// Validate and construct one plan carrying plan-local functions.
    pub fn new_with_functions(
        bindings: Vec<AssertionBinding>,
        functions: Vec<LocalFunction>,
        inputs: Vec<InputColumn>,
        pipeline: Vec<ReadStage>,
        output: QueryOutput,
        managed_semantics: ManagedSemanticSchemaFingerprint,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_limits(
            bindings,
            functions,
            inputs,
            pipeline,
            output,
            managed_semantics,
            StructuralLimits::CANONICAL,
        )
    }

    fn new_with_limits(
        bindings: Vec<AssertionBinding>,
        functions: Vec<LocalFunction>,
        inputs: Vec<InputColumn>,
        pipeline: Vec<ReadStage>,
        output: QueryOutput,
        managed_semantics: ManagedSemanticSchemaFingerprint,
        limits: StructuralLimits,
    ) -> Result<Self, Diagnostic> {
        if bindings.is_empty() || !limits.allows_bindings(bindings.len()) {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_binding_limit",
                "plan binding count is empty or exceeds the structural ceiling",
            ));
        }
        let mut names = BTreeSet::new();
        for (index, binding) in bindings.iter().enumerate() {
            if usize::from(binding.id().get()) != index {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_bindings_not_dense",
                    "plan binding IDs must be ordered dense zero-based ordinals",
                ));
            }
            if !names.insert(binding.variable().clone()) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_variable",
                    "plan query variables must be unique",
                ));
            }
        }
        if !limits.allows_bindings(inputs.len().max(1)) {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_input_limit",
                "plan input column count exceeds the structural ceiling",
            ));
        }
        for (index, column) in inputs.iter().enumerate() {
            if usize::from(column.id().get()) != index {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_inputs_not_dense",
                    "input column IDs must be ordered dense zero-based ordinals",
                ));
            }
            if !names.insert(column.public_name().clone()) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_variable",
                    "input column names must not collide with query variables",
                ));
            }
        }

        validate_local_functions(&functions, limits)?;

        let (mandatory, optional, has_sort) =
            validate_pipeline(&pipeline, bindings.len(), inputs.len(), limits)?;
        let visible: BTreeSet<BindingId> = mandatory.union(&optional).copied().collect();

        match &output {
            QueryOutput::Rows { columns } => {
                if columns.is_empty() || !limits.allows_selected_slots(columns.len()) {
                    return Err(failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_plan_output_limit",
                        "output column count is empty or exceeds the structural ceiling",
                    ));
                }
                let mut seen = BTreeSet::new();
                for column in columns {
                    if !visible.contains(column) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_output_not_visible",
                            "output projects a binding outside the visible row environment",
                        ));
                    }
                    if !seen.insert(*column) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_duplicate_output_column",
                            "output projects one binding twice",
                        ));
                    }
                }
            }
            QueryOutput::Documents { fields } => {
                if fields.is_empty() || !limits.allows_selected_slots(fields.len()) {
                    return Err(failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_plan_output_limit",
                        "output column count is empty or exceeds the structural ceiling",
                    ));
                }
                let mut keys = BTreeSet::new();
                for field in fields {
                    if !keys.insert(field.key().clone()) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_duplicate_output_column",
                            "documents fetch one key twice",
                        ));
                    }
                    match field.source() {
                        DocumentSource::Binding { binding } => {
                            if !visible.contains(binding) {
                                return Err(failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_output_not_visible",
                                    "output projects a binding outside the visible row environment",
                                ));
                            }
                        }
                        DocumentSource::AttributeList { owner, .. } => {
                            // A list reaches through its owner per row;
                            // absence would have no list to fetch from.
                            if !mandatory.contains(owner) {
                                return Err(failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_output_not_visible",
                                    "attribute lists require a mandatory owner binding",
                                ));
                            }
                        }
                    }
                }
            }
        }
        // Offset and limit consume an ordered stream; without an explicit
        // sort there is no stable total order to consume. Fail closed rather
        // than inherit provider iteration order.
        if !has_sort
            && pipeline
                .iter()
                .any(|stage| matches!(stage, ReadStage::Offset { .. } | ReadStage::Limit { .. }))
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_unordered_truncation",
                "offset and limit require an explicit total sort order",
            ));
        }

        let required_capabilities = derive_capabilities(&pipeline, &functions, &inputs, &output)?;
        Ok(Self {
            bindings,
            format: QUERY_PLAN_FORMAT_V1.to_owned(),
            functions,
            inputs,
            managed_semantics,
            output,
            pipeline,
            required_capabilities,
        })
    }

    /// Return the exact wire format discriminator.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }

    /// Return dense binding declarations.
    #[must_use]
    pub fn bindings(&self) -> &[AssertionBinding] {
        &self.bindings
    }

    /// Return dense typed input column declarations.
    #[must_use]
    pub fn inputs(&self) -> &[InputColumn] {
        &self.inputs
    }

    /// Return the plan-local functions.
    #[must_use]
    pub fn functions(&self) -> &[LocalFunction] {
        &self.functions
    }

    /// Return the ordered read pipeline.
    pub fn pipeline(&self) -> &[ReadStage] {
        &self.pipeline
    }

    /// Return the explicit output category.
    #[must_use]
    pub const fn output(&self) -> &QueryOutput {
        &self.output
    }

    /// Return the exact managed semantics this plan was authored against.
    #[must_use]
    pub const fn managed_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantics
    }

    /// Return open capabilities derived from syntax, never named manually.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Encode exact canonical bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Compute the canonical domain-separated plan fingerprint.
    pub fn fingerprint(&self) -> Result<QueryPlanFingerprint, Diagnostic> {
        QueryPlanFingerprint::compute(self)
    }
}

/// One rectangular invocation input row.
///
/// Values are positional by dense input column ordinal. `None` is admitted
/// only where the declaring column is optional; there is no other
/// null-shaped default.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct InputRow {
    values: Vec<Option<CanonicalValue>>,
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

/// The closed operation vocabulary of the first public revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    /// Stream the projected rows.
    Rows,
    /// Count the projected rows.
    Count,
    /// Report whether at least one row exists.
    Exists,
}

/// One executable invocation of a reusable validated plan.
///
/// The invocation carries values and an operation; the plan carries all
/// reusable structure. Binding is by exact plan fingerprint, so a plan edit
/// invalidates every outstanding invocation instead of silently reshaping it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueryInvocation {
    inputs: Vec<InputRow>,
    operation: QueryOperation,
    plan_fingerprint: QueryPlanFingerprint,
}

impl QueryInvocation {
    /// Validate one rectangular input batch against its exact plan.
    pub fn new(
        plan: &QueryPlan,
        operation: QueryOperation,
        inputs: Vec<InputRow>,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_limits(plan, operation, inputs, StructuralLimits::CANONICAL)
    }

    fn new_with_limits(
        plan: &QueryPlan,
        operation: QueryOperation,
        inputs: Vec<InputRow>,
        limits: StructuralLimits,
    ) -> Result<Self, Diagnostic> {
        if plan.inputs().is_empty() {
            if !inputs.is_empty() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_invocation_unexpected_inputs",
                    "the plan declares no input columns yet the invocation carries rows",
                ));
            }
        } else {
            if inputs.is_empty() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_invocation_missing_inputs",
                    "the plan declares input columns and requires at least one row",
                ));
            }
            if !limits.allows_selected_slots(inputs.len()) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_invocation_row_limit",
                    "invocation input row count exceeds the structural ceiling",
                ));
            }
            for row in &inputs {
                if row.values().len() != plan.inputs().len() {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_invocation_row_arity",
                        "input row does not carry exactly the declared column set",
                    ));
                }
                for (column, value) in plan.inputs().iter().zip(row.values()) {
                    match value {
                        None if column.optional() => {}
                        None => {
                            return Err(failure(
                                DiagnosticCategory::InvalidContract,
                                "query_invocation_missing_value",
                                "a required input column carries no value",
                            ));
                        }
                        Some(value) if value.value_type() == column.value_type() => {}
                        Some(_) => {
                            return Err(failure(
                                DiagnosticCategory::InvalidContract,
                                "query_invocation_value_type",
                                "input value type differs from the declared column type",
                            ));
                        }
                    }
                }
            }
        }
        Ok(Self {
            inputs,
            operation,
            plan_fingerprint: plan.fingerprint()?,
        })
    }

    /// Return the rectangular validated input rows.
    #[must_use]
    pub fn inputs(&self) -> &[InputRow] {
        &self.inputs
    }

    /// Return the requested operation.
    #[must_use]
    pub const fn operation(&self) -> QueryOperation {
        self.operation
    }

    /// Return the exact plan fingerprint this invocation binds.
    #[must_use]
    pub const fn plan_fingerprint(&self) -> &QueryPlanFingerprint {
        &self.plan_fingerprint
    }

    /// Return whether this invocation still binds the supplied plan.
    pub fn binds(&self, plan: &QueryPlan) -> Result<bool, Diagnostic> {
        Ok(self.plan_fingerprint == plan.fingerprint()?)
    }

    /// Return capabilities this invocation's transport requires beyond
    /// the plan's own.
    ///
    /// A multi-row input batch rides the native `given` transport, so it
    /// requires [`query_given_rows_capability`]; empty and single-row
    /// invocations lower inline and require nothing extra. Both client
    /// preflight and executor admission check this set against the
    /// advertisement so untransportable invocations fail before any I/O
    /// or provider resource.
    #[must_use]
    pub fn transport_capabilities(&self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::new();
        if self.inputs.len() > 1 {
            capabilities.insert(query_given_rows_capability());
        }
        capabilities
    }
}

/// Fingerprint of exact canonical query-plan bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct QueryPlanFingerprint(Fingerprint);

impl QueryPlanFingerprint {
    /// Compute the fixed-domain fingerprint of a trusted plan.
    pub fn compute(plan: &QueryPlan) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(QUERY_PLAN_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_PLAN_CANONICALIZATION)?,
            None,
            &plan.canonical_bytes()?,
        )))
    }

    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Decode canonical bytes through private constructor-rebuilding wire types.
pub fn decode_query_plan(bytes: &[u8]) -> Result<QueryPlan, Diagnostic> {
    crate::query_plan_wire::decode_query_plan(bytes)
}

fn validate_local_functions(
    functions: &[LocalFunction],
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    if !limits.allows_bindings(functions.len().max(1)) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_local_function_limit",
            "plan-local function count exceeds the structural ceiling",
        ));
    }
    let mut names = BTreeSet::new();
    for function in functions {
        if !names.insert(function.name().clone()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_duplicate_local_function",
                "plan-local function names must be unique",
            ));
        }
        let bindings = function.bindings();
        if bindings.is_empty() || !limits.allows_bindings(bindings.len()) {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_binding_limit",
                "plan binding count is empty or exceeds the structural ceiling",
            ));
        }
        let mut local_names = BTreeSet::new();
        for (index, binding) in bindings.iter().enumerate() {
            if usize::from(binding.id().get()) != index {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_bindings_not_dense",
                    "plan binding IDs must be ordered dense zero-based ordinals",
                ));
            }
            if !local_names.insert(binding.variable().clone()) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_variable",
                    "plan query variables must be unique",
                ));
            }
        }
        if function.parameters().is_empty() || function.parameters().len() > bindings.len() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_parameters",
                "parameters must be a non-empty prefix of the local bindings",
            ));
        }
        if function.body().is_empty() || function.body().len() > limits.boolean_terms {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_pattern_limit",
                "plan root conjunction is empty or exceeds the term ceiling",
            ));
        }
        let mut nodes = 0usize;
        for pattern in function.body() {
            // The first local vocabulary keeps bodies flat.
            if matches!(
                pattern,
                QueryPattern::Not { .. }
                    | QueryPattern::Try { .. }
                    | QueryPattern::Reachable { .. }
                    | QueryPattern::FunctionCall { .. }
            ) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_local_function_body_unsupported",
                    "local bodies admit only isa, has, links, and value patterns",
                ));
            }
            inspect_pattern(pattern, 1, bindings.len(), 0, limits, &mut nodes)?;
        }
        let returns = function.returns();
        check_binding(returns.input(), bindings.len())?;
        if !returns.reducer().total_without_groups() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_return_partial",
                "local returns admit only reducers total on empty streams",
            ));
        }
        let declared_valid = match returns.reducer() {
            Reducer::Count => returns.value_type() == ValueTypeTag::Long,
            Reducer::Sum => matches!(
                returns.value_type(),
                ValueTypeTag::Long | ValueTypeTag::Double
            ),
            Reducer::Max | Reducer::Min | Reducer::Mean => false,
        };
        if !declared_valid {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_return_type",
                "declared return type does not fit the reducer",
            ));
        }
    }
    Ok(())
}

fn validate_pipeline(
    pipeline: &[ReadStage],
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
) -> Result<(BTreeSet<BindingId>, BTreeSet<BindingId>, bool), Diagnostic> {
    let Some((first, rest)) = pipeline.split_first() else {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_empty_pipeline",
            "a read pipeline requires at least its match stage",
        ));
    };
    let ReadStage::Match { patterns } = first else {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_match_not_first",
            "the pattern conjunction must be the first pipeline stage",
        ));
    };
    if patterns.is_empty() || patterns.len() > limits.boolean_terms {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_pattern_limit",
            "plan root conjunction is empty or exceeds the term ceiling",
        ));
    }
    let mut stats = 0usize;
    for pattern in patterns {
        inspect_pattern(pattern, 1, binding_count, input_count, limits, &mut stats)?;
    }

    // The row environment starts as the pattern-referenced bindings: a
    // declared binding no pattern mentions has no row presence, and a
    // reduce assignment must stay outside the pattern conjunction.
    let mut pattern_bound = BTreeSet::new();
    for pattern in patterns {
        collect_pattern_bindings(pattern, &mut pattern_bound);
    }
    // Bindings established only inside a try body are optional: rows carry
    // them or an explicit absence. Each optional binding belongs to exactly
    // one try body; sharing one across bodies has no single presence source.
    let mut root_mandatory = BTreeSet::new();
    for pattern in patterns {
        if !matches!(pattern, QueryPattern::Try { .. }) {
            collect_pattern_bindings(pattern, &mut root_mandatory);
        }
    }
    let mut optional = BTreeSet::new();
    for pattern in patterns {
        let QueryPattern::Try { patterns } = pattern else {
            continue;
        };
        let mut body_refs = BTreeSet::new();
        for child in patterns {
            collect_pattern_bindings(child, &mut body_refs);
        }
        for reference in &body_refs {
            if optional.contains(reference) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_try_binding_shared",
                    "an optional binding belongs to exactly one try body",
                ));
            }
        }
        optional.extend(body_refs.difference(&root_mandatory).copied());
    }
    let mut mandatory: BTreeSet<BindingId> = pattern_bound.difference(&optional).copied().collect();
    let mut previous_ordinal = 0u8;
    let mut has_sort = false;
    for stage in rest {
        let ordinal = stage.ordinal();
        if ordinal <= previous_ordinal {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_stage_order",
                "pipeline stages must follow the canonical order exactly once each",
            ));
        }
        previous_ordinal = ordinal;
        match stage {
            ReadStage::Match { .. } => unreachable!("ordinal zero cannot follow"),
            ReadStage::Select { bindings } => {
                let union: BTreeSet<BindingId> = mandatory.union(&optional).copied().collect();
                let selected = canonical_stage_set(bindings, &union, "select")?;
                mandatory.retain(|id| selected.contains(id));
                optional.retain(|id| selected.contains(id));
            }
            ReadStage::Require { bindings } => {
                // The provider contract for requiring an optional binding is
                // unproven (TypeDB 3.12.1 stalls on it); the first optional
                // vocabulary reserves require to mandatory bindings.
                let union: BTreeSet<BindingId> = mandatory.union(&optional).copied().collect();
                let required = canonical_stage_set(bindings, &union, "require")?;
                if required.iter().any(|id| optional.contains(id)) {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_require_optional_reserved",
                        "requiring an optional binding is reserved in this vocabulary",
                    ));
                }
            }
            ReadStage::Distinct => {}
            ReadStage::Reduce {
                assignments,
                groups,
            } => {
                if assignments.is_empty()
                    || assignments.len() > limits.boolean_terms
                    || groups.len() > limits.boolean_terms
                {
                    return Err(failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_plan_reduce_term_limit",
                        "reduce has no assignments or exceeds the term ceiling",
                    ));
                }
                let mut previous = None;
                let mut next_visible = BTreeSet::new();
                for group in groups {
                    if previous.is_some_and(|previous: BindingId| previous >= *group) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_stage_set_not_canonical",
                            "stage binding sets must be strictly ascending",
                        ));
                    }
                    previous = Some(*group);
                    // Grouping by an optional binding would key groups on
                    // absence; group keys stay mandatory.
                    if !mandatory.contains(group) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_stage_unknown_binding",
                            "reduce groups a binding outside the mandatory row environment",
                        ));
                    }
                    next_visible.insert(*group);
                }
                for assignment in assignments {
                    check_binding(assignment.assigned(), binding_count)?;
                    if pattern_bound.contains(&assignment.assigned())
                        || !next_visible.insert(assignment.assigned())
                    {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_reduce_assigned_bound",
                            "reduce must assign a fresh binding free of patterns and groups",
                        ));
                    }
                    match assignment.input() {
                        Some(input) => {
                            if !mandatory.contains(&input) && !optional.contains(&input) {
                                return Err(failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_stage_unknown_binding",
                                    "reduce consumes a binding outside the visible row environment",
                                ));
                            }
                            // Count and sum skip absent inputs and stay
                            // total; max, min, and mean can observe a group
                            // whose optional input never matched.
                            if optional.contains(&input)
                                && !assignment.reducer().total_without_groups()
                            {
                                return Err(failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_reduce_optional_input",
                                    "this reducer is undefined over an optional input",
                                ));
                            }
                        }
                        None => {
                            if assignment.reducer().requires_input() {
                                return Err(failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_reduce_missing_input",
                                    "this reducer consumes an input binding",
                                ));
                            }
                        }
                    }
                    if groups.is_empty() && !assignment.reducer().total_without_groups() {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_reduce_requires_groups",
                            "reducers undefined on empty streams require group bindings",
                        ));
                    }
                }
                mandatory = next_visible;
                optional.clear();
            }
            ReadStage::Sort { terms } => {
                if terms.is_empty() || terms.len() > limits.boolean_terms {
                    return Err(failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_plan_sort_term_limit",
                        "sort has no terms or exceeds the term ceiling",
                    ));
                }
                let mut sorted = BTreeSet::new();
                for term in terms {
                    // Absence has no defined position in a total order;
                    // sort keys stay mandatory.
                    if !mandatory.contains(&term.binding()) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_stage_unknown_binding",
                            "sort references a binding outside the mandatory row environment",
                        ));
                    }
                    if !sorted.insert(term.binding()) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_duplicate_sort_binding",
                            "sort references one binding twice",
                        ));
                    }
                }
                has_sort = true;
            }
            ReadStage::Offset { .. } | ReadStage::Limit { .. } => {}
        }
    }
    Ok((mandatory, optional, has_sort))
}

fn canonical_stage_set(
    bindings: &[BindingId],
    visible: &BTreeSet<BindingId>,
    stage: &'static str,
) -> Result<BTreeSet<BindingId>, Diagnostic> {
    if bindings.is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_empty_stage_set",
            stage,
        ));
    }
    let mut set = BTreeSet::new();
    let mut previous = None;
    for binding in bindings {
        if previous.is_some_and(|previous: BindingId| previous >= *binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_stage_set_not_canonical",
                "stage binding sets must be strictly ascending",
            ));
        }
        previous = Some(*binding);
        if !visible.contains(binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_stage_unknown_binding",
                "stage references a binding outside the visible row environment",
            ));
        }
        set.insert(*binding);
    }
    Ok(set)
}

fn inspect_pattern(
    pattern: &QueryPattern,
    depth: usize,
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    *nodes += 1;
    if !limits.allows_predicate_nodes(*nodes) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_pattern_node_limit",
            "plan pattern count exceeds the structural ceiling",
        ));
    }
    if !limits.allows_predicate_depth(depth) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_pattern_depth_limit",
            "plan pattern depth exceeds the structural ceiling",
        ));
    }
    match pattern {
        QueryPattern::Isa { binding, .. } => check_binding(*binding, binding_count),
        QueryPattern::Has {
            owner, attribute, ..
        } => {
            check_binding(*owner, binding_count)?;
            check_binding(*attribute, binding_count)
        }
        QueryPattern::Links {
            relation, players, ..
        } => {
            check_binding(*relation, binding_count)?;
            if players.is_empty() || players.len() > limits.boolean_terms {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_role_player_limit",
                    "links pattern has no players or exceeds the term ceiling",
                ));
            }
            for player in players {
                check_binding(player.player(), binding_count)?;
            }
            Ok(())
        }
        QueryPattern::Value { left, right, .. } => {
            check_operand(left, binding_count, input_count)?;
            check_operand(right, binding_count, input_count)
        }
        QueryPattern::Not { patterns } => {
            if patterns.is_empty() || patterns.len() > limits.boolean_terms {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_negation_term_limit",
                    "negation is empty or exceeds the boolean-term ceiling",
                ));
            }
            for child in patterns {
                inspect_pattern(child, depth + 1, binding_count, input_count, limits, nodes)?;
            }
            Ok(())
        }
        QueryPattern::Try { patterns } => {
            if depth > 1 {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_try_not_root",
                    "optional blocks are admitted only in the root conjunction",
                ));
            }
            if patterns.is_empty() || patterns.len() > limits.boolean_terms {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_try_term_limit",
                    "optional block is empty or exceeds the boolean-term ceiling",
                ));
            }
            for child in patterns {
                if matches!(
                    child,
                    QueryPattern::Not { .. }
                        | QueryPattern::Try { .. }
                        | QueryPattern::Reachable { .. }
                        | QueryPattern::FunctionCall { .. }
                ) {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_try_body_unsupported",
                        "the first optional vocabulary admits only isa, has, links, and value patterns",
                    ));
                }
                inspect_pattern(child, depth + 1, binding_count, input_count, limits, nodes)?;
            }
            Ok(())
        }
        QueryPattern::Reachable {
            max_depth,
            source,
            target,
            ..
        } => {
            if depth > 1 {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_reachable_not_root",
                    "bounded reachability is admitted only in the root conjunction",
                ));
            }
            if *max_depth == 0 || !limits.allows_predicate_depth(usize::from(*max_depth)) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_reachable_depth",
                    "reachability requires a finite hop bound within the depth ceiling",
                ));
            }
            // Lowering unrolls one branch per path length, so a bound of d
            // expands into d(d+1)/2 relation clauses. Charge that expansion
            // against the shared node budget: a compact plan cannot smuggle
            // hundreds of thousands of emitted clauses past the ceiling by
            // spelling them as one pattern.
            let bound = usize::from(*max_depth);
            let expanded_hops = bound * (bound + 1) / 2;
            *nodes = nodes.saturating_add(expanded_hops.saturating_sub(1));
            if !limits.allows_predicate_nodes(*nodes) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_reachable_expansion_limit",
                    "reachability expansion exceeds the plan pattern-node ceiling",
                ));
            }
            check_binding(*source, binding_count)?;
            check_binding(*target, binding_count)
        }
        QueryPattern::FunctionCall {
            arguments,
            assigned,
            ..
        } => {
            if depth > 1 {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_function_in_negation",
                    "function calls are admitted only in the root conjunction",
                ));
            }
            if arguments.len() > limits.boolean_terms {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_function_argument_limit",
                    "function call arguments exceed the term ceiling",
                ));
            }
            for argument in arguments {
                check_operand(argument, binding_count, input_count)?;
            }
            check_binding(*assigned, binding_count)
        }
    }
}

fn collect_pattern_bindings(pattern: &QueryPattern, bindings: &mut BTreeSet<BindingId>) {
    let mut operand = |operand: &QueryOperand| {
        if let QueryOperand::Binding { binding } = operand {
            bindings.insert(*binding);
        }
    };
    match pattern {
        QueryPattern::Isa { binding, .. } => {
            bindings.insert(*binding);
        }
        QueryPattern::Has {
            owner, attribute, ..
        } => {
            bindings.insert(*owner);
            bindings.insert(*attribute);
        }
        QueryPattern::Links {
            relation, players, ..
        } => {
            bindings.insert(*relation);
            for player in players {
                bindings.insert(player.player());
            }
        }
        QueryPattern::Value { left, right, .. } => {
            operand(left);
            operand(right);
        }
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            for child in patterns {
                collect_pattern_bindings(child, bindings);
            }
        }
        QueryPattern::Reachable { source, target, .. } => {
            bindings.insert(*source);
            bindings.insert(*target);
        }
        QueryPattern::FunctionCall {
            arguments,
            assigned,
            ..
        } => {
            for argument in arguments {
                operand(argument);
            }
            bindings.insert(*assigned);
        }
    }
}

fn check_operand(
    operand: &QueryOperand,
    binding_count: usize,
    input_count: usize,
) -> Result<(), Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => check_binding(*binding, binding_count),
        QueryOperand::Literal { .. } => Ok(()),
        QueryOperand::Input { column } => {
            if usize::from(column.get()) < input_count {
                Ok(())
            } else {
                Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_unknown_input_column",
                    "pattern references an undeclared input column",
                ))
            }
        }
    }
}

fn check_binding(binding: BindingId, binding_count: usize) -> Result<(), Diagnostic> {
    if usize::from(binding.get()) < binding_count {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_unknown_binding",
            "pattern references an undeclared binding",
        ))
    }
}

fn derive_capabilities(
    pipeline: &[ReadStage],
    functions: &[LocalFunction],
    inputs: &[InputColumn],
    output: &QueryOutput,
) -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = CapabilitySet::new();
    insert_capability(&mut capabilities, CAP_PLAN)?;
    if !functions.is_empty() {
        insert_capability(&mut capabilities, CAP_LOCAL_FUNCTIONS)?;
        for function in functions {
            for pattern in function.body() {
                collect_pattern_capabilities(pattern, &mut capabilities)?;
            }
        }
    }
    match output {
        QueryOutput::Rows { .. } => {
            insert_capability(&mut capabilities, CAP_OUTPUT_ROWS)?;
        }
        QueryOutput::Documents { .. } => {
            insert_capability(&mut capabilities, CAP_OUTPUT_DOCUMENTS)?;
        }
    }
    if !inputs.is_empty() {
        insert_capability(&mut capabilities, CAP_INPUT_COLUMNS)?;
    }
    for stage in pipeline {
        match stage {
            ReadStage::Match { patterns } => {
                for pattern in patterns {
                    collect_pattern_capabilities(pattern, &mut capabilities)?;
                }
            }
            ReadStage::Select { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_SELECT)?;
            }
            ReadStage::Require { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_REQUIRE)?;
            }
            ReadStage::Distinct => {
                insert_capability(&mut capabilities, CAP_STAGE_DISTINCT)?;
            }
            ReadStage::Reduce { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_REDUCE)?;
            }
            ReadStage::Sort { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_SORT)?;
            }
            ReadStage::Offset { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_OFFSET)?;
            }
            ReadStage::Limit { .. } => {
                insert_capability(&mut capabilities, CAP_STAGE_LIMIT)?;
            }
        }
    }
    Ok(capabilities)
}

fn collect_pattern_capabilities(
    pattern: &QueryPattern,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match pattern {
        QueryPattern::Isa {
            include_subtypes, ..
        } => {
            insert_capability(capabilities, CAP_ISA)?;
            if *include_subtypes {
                insert_capability(capabilities, CAP_ISA_SUBTYPES)?;
            }
        }
        QueryPattern::Has { .. } => insert_capability(capabilities, CAP_HAS)?,
        QueryPattern::Links { .. } => insert_capability(capabilities, CAP_LINKS)?,
        QueryPattern::Value { .. } => insert_capability(capabilities, CAP_VALUE)?,
        QueryPattern::Try { patterns } => {
            insert_capability(capabilities, CAP_TRY)?;
            for child in patterns {
                collect_pattern_capabilities(child, capabilities)?;
            }
        }
        QueryPattern::Reachable { .. } => {
            insert_capability(capabilities, CAP_REACHABLE)?;
        }
        QueryPattern::Not { patterns } => {
            insert_capability(capabilities, CAP_NEGATION)?;
            for child in patterns {
                collect_pattern_capabilities(child, capabilities)?;
            }
        }
        QueryPattern::FunctionCall { .. } => {
            insert_capability(capabilities, CAP_FUNCTION_CALL)?;
        }
    }
    Ok(())
}

fn insert_capability(
    capabilities: &mut CapabilitySet,
    value: &'static str,
) -> Result<(), Diagnostic> {
    capabilities.insert(CapabilityId::new(value)?);
    Ok(())
}

pub(crate) fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static query-plan diagnostic code"),
        message,
    )
}
