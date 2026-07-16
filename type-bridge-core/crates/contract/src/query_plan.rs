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
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain,
};
use crate::id::{AttributeId, TypeId};
use crate::limits::StructuralLimits;
use crate::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable,
    ValueComparator,
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
    ]
    .into_iter()
    .map(|value| CapabilityId::new(value).expect("static capability id is canonical"))
    .collect()
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

/// One ordered read stage of the first public vocabulary.
///
/// The canonical stage order is fixed: one `match`, then at most one each of
/// `select`, `require`, `distinct`, `sort`, `offset`, and `limit`, in that
/// order. Later stage kinds (reductions, documents) are reserved.
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
            Self::Sort { .. } => 4,
            Self::Offset { .. } => 5,
            Self::Limit { .. } => 6,
        }
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
}

/// A reusable, invocation-free typed read program.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueryPlan {
    bindings: Vec<AssertionBinding>,
    format: String,
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
            inputs,
            pipeline,
            output,
            managed_semantics,
            StructuralLimits::CANONICAL,
        )
    }

    fn new_with_limits(
        bindings: Vec<AssertionBinding>,
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

        let (visible, has_sort) =
            validate_pipeline(&pipeline, bindings.len(), inputs.len(), limits)?;

        let QueryOutput::Rows { columns } = &output;
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
        // Offset and limit consume an ordered stream; without an explicit
        // sort there is no stable total order to consume. Fail closed rather
        // than inherit provider iteration order.
        if !has_sort
            && pipeline.iter().any(|stage| {
                matches!(stage, ReadStage::Offset { .. } | ReadStage::Limit { .. })
            })
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_unordered_truncation",
                "offset and limit require an explicit total sort order",
            ));
        }

        let required_capabilities = derive_capabilities(&pipeline, &inputs)?;
        Ok(Self {
            bindings,
            format: QUERY_PLAN_FORMAT_V1.to_owned(),
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

    /// Return the ordered read pipeline.
    #[must_use]
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

fn validate_pipeline(
    pipeline: &[ReadStage],
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
) -> Result<(BTreeSet<BindingId>, bool), Diagnostic> {
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

    let mut visible: BTreeSet<BindingId> = (0..binding_count)
        .map(|index| BindingId::new(u16::try_from(index).expect("dense ordinal")))
        .collect::<Result<_, _>>()?;
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
                let selected =
                    canonical_stage_set(bindings, &visible, "select")?;
                visible = selected;
            }
            ReadStage::Require { bindings } => {
                canonical_stage_set(bindings, &visible, "require")?;
            }
            ReadStage::Distinct => {}
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
                    if !visible.contains(&term.binding()) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_stage_unknown_binding",
                            "sort references a binding outside the visible row environment",
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
    Ok((visible, has_sort))
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
        QueryPattern::Has { owner, attribute, .. } => {
            check_binding(*owner, binding_count)?;
            check_binding(*attribute, binding_count)
        }
        QueryPattern::Links { relation, players, .. } => {
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
                inspect_pattern(
                    child,
                    depth + 1,
                    binding_count,
                    input_count,
                    limits,
                    nodes,
                )?;
            }
            Ok(())
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
    inputs: &[InputColumn],
) -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = CapabilitySet::new();
    insert_capability(&mut capabilities, CAP_PLAN)?;
    insert_capability(&mut capabilities, CAP_OUTPUT_ROWS)?;
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
        QueryPattern::Isa { include_subtypes, .. } => {
            insert_capability(capabilities, CAP_ISA)?;
            if *include_subtypes {
                insert_capability(capabilities, CAP_ISA_SUBTYPES)?;
            }
        }
        QueryPattern::Has { .. } => insert_capability(capabilities, CAP_HAS)?,
        QueryPattern::Links { .. } => insert_capability(capabilities, CAP_LINKS)?,
        QueryPattern::Value { .. } => insert_capability(capabilities, CAP_VALUE)?,
        QueryPattern::Not { patterns } => {
            insert_capability(capabilities, CAP_NEGATION)?;
            for child in patterns {
                collect_pattern_capabilities(child, capabilities)?;
            }
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
