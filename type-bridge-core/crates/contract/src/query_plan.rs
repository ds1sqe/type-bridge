//! Native typed query plans and their shared vocabulary.

use std::collections::BTreeSet;

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::to_canonical_json_with_limits;
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, RoleId, TypeId};
use crate::limits::{QueryLimits, StructuralLimits};
use crate::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use crate::value::{CanonicalValue, ValueTypeTag};

#[path = "query_plan_types.rs"]
mod types;
pub use types::{
    DocumentField, DocumentSource, InputColumn, InputColumnId, InputRow, LocalFunction,
    LocalReturn, OrderDirection, OrderTerm, QueryOperand, QueryOutput, QueryPattern,
    QueryPlanFingerprint, ReadStage, ReduceAssignment, Reducer,
};

/// Native read/write plans.
#[path = "query_plan_v3.rs"]
pub mod v3;
pub use v3::{
    QUERY_PLAN_CANONICALIZATION_V3, QUERY_PLAN_FORMAT_V3, QueryPlanV3, decode_query_plan_v3,
};

#[path = "query_model.rs"]
mod model;
pub use model::{
    HydrationBinding, HydrationDescriptor, HydrationField, HydrationPlayer, HydrationProjection,
    HydrationRole, ModelComparator, ModelField, ModelMissingOrder, ModelNamedOutputSlot,
    ModelOrderDirection, ModelOrderTerm, ModelOutput, ModelOutputSlot, ModelPredicate,
    ModelReductionKey, ModelReductionKind, ModelReductionTerm, ModelStableOrder, ModelValue,
    ModelValueKind,
};

/// Domain separating query-plan fingerprints from every other digest.
pub const QUERY_PLAN_FINGERPRINT_DOMAIN: &str = "typebridge.query.plan";
const CAP_PLAN: &str = "query.plan";
const CAP_ISA: &str = "query.pattern.isa";
const CAP_ISA_SUBTYPES: &str = "query.pattern.isa-subtypes";
const CAP_HAS: &str = "query.pattern.has";
const CAP_LINKS: &str = "query.pattern.links";
const CAP_VALUE: &str = "query.pattern.value";
const CAP_NEGATION: &str = "query.pattern.negation";
const CAP_DISJUNCTION: &str = "query.pattern.disjunction";
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

/// Return the transport capability exact `given` invocations require.
///
/// Native exact-row inputs require this provider transport capability.
#[must_use]
pub fn query_given_rows_capability() -> CapabilityId {
    CapabilityId::new(CAP_INPUT_GIVEN_ROWS).expect("static capability id is canonical")
}

fn validate_local_functions(
    functions: &[LocalFunction],
    limits: StructuralLimits,
    nodes: &mut usize,
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
            validate_query_name_limit(
                binding.variable(),
                limits,
                "a local query variable name exceeds the structural ceiling",
            )?;
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
        for pattern in function.body() {
            // The first local vocabulary keeps bodies flat.
            if matches!(
                pattern,
                QueryPattern::Or { .. }
                    | QueryPattern::Not { .. }
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
            // The predicate-node budget is one aggregate ceiling for the
            // whole plan: local bodies charge the same counter the root
            // conjunction does instead of resetting per function.
            inspect_pattern(pattern, 1, bindings.len(), 0, limits, nodes, false)?;
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
            Reducer::Max | Reducer::Min | Reducer::Mean | Reducer::Median | Reducer::Std => false,
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

fn validate_declarations(
    bindings: &[AssertionBinding],
    inputs: &[InputColumn],
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
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
        validate_query_name_limit(
            binding.variable(),
            limits,
            "a query variable name exceeds the structural ceiling",
        )?;
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
        validate_query_name_limit(
            column.public_name(),
            limits,
            "an input column name exceeds the structural ceiling",
        )?;
        if !names.insert(column.public_name().clone()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_duplicate_variable",
                "input column names must not collide with query variables",
            ));
        }
    }

    Ok(())
}

fn validate_output(
    output: &QueryOutput,
    mandatory: &BTreeSet<BindingId>,
    optional: &BTreeSet<BindingId>,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    let visible: BTreeSet<BindingId> = mandatory.union(optional).copied().collect();

    match output {
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
                validate_query_name_limit(
                    field.key(),
                    limits,
                    "a document output key exceeds the structural ceiling",
                )?;
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
    Ok(())
}

fn validate_query_name_limit(
    name: &QueryVariable,
    limits: StructuralLimits,
    message: &'static str,
) -> Result<(), Diagnostic> {
    if name.as_str().len() > limits.output_name_bytes {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_name_limit",
            message,
        ));
    }
    Ok(())
}

fn validate_native_pipeline(
    pipeline: &[ReadStage],
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
) -> Result<(BTreeSet<BindingId>, BTreeSet<BindingId>, bool), Diagnostic> {
    validate_pipeline_inner(pipeline, binding_count, input_count, limits, nodes, true)
}

fn validate_pipeline_inner(
    pipeline: &[ReadStage],
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
    native_optional_union: bool,
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
    for pattern in patterns {
        inspect_pattern(
            pattern,
            1,
            binding_count,
            input_count,
            limits,
            nodes,
            native_optional_union,
        )?;
    }

    // Keep every pattern reference for freshness checks, but derive the row
    // environment only from bindings positively established in the root
    // conjunction. Negation-local witnesses never become row columns merely
    // because their binding IDs are referenced in a nested scope.
    let mut pattern_bound = BTreeSet::new();
    for pattern in patterns {
        collect_pattern_bindings(pattern, &mut pattern_bound);
    }
    // Bindings established only inside a try body are optional: rows carry
    // them or an explicit absence. Each optional binding belongs to exactly
    // one try body; sharing one across bodies has no single presence source.
    let mut root_mandatory = BTreeSet::new();
    let mut scoped_positive = BTreeSet::new();
    for pattern in patterns {
        collect_direct_positive_bindings(pattern, &mut root_mandatory);
        collect_negation_positive_bindings(pattern, &mut scoped_positive);
    }
    let mut optional = BTreeSet::new();
    for pattern in patterns {
        let QueryPattern::Try { patterns } = pattern else {
            continue;
        };
        let mut body_positive = BTreeSet::new();
        for child in patterns {
            collect_direct_positive_bindings(child, &mut body_positive);
        }
        for local in body_positive.difference(&root_mandatory) {
            if !optional.insert(*local) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_try_binding_shared",
                    "an optional binding belongs to exactly one try body",
                ));
            }
        }
    }
    if !scoped_positive.is_disjoint(&optional) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_try_binding_shared",
            "an optional binding cannot also be a negation-local witness",
        ));
    }
    validate_read_stages(
        rest,
        root_mandatory,
        optional,
        &pattern_bound,
        binding_count,
        limits,
    )
}

fn validate_read_stages(
    rest: &[ReadStage],
    mut mandatory: BTreeSet<BindingId>,
    mut optional: BTreeSet<BindingId>,
    pattern_bound: &BTreeSet<BindingId>,
    binding_count: usize,
    limits: StructuralLimits,
) -> Result<(BTreeSet<BindingId>, BTreeSet<BindingId>, bool), Diagnostic> {
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
                if terms.is_empty() || !limits.allows_order_terms(terms.len()) {
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

fn supported_optional_body(pattern: &QueryPattern, unions: bool) -> bool {
    match pattern {
        QueryPattern::Or { branches } => {
            unions
                && branches
                    .iter()
                    .flatten()
                    .all(|child| supported_optional_body(child, unions))
        }
        QueryPattern::Not { .. }
        | QueryPattern::Try { .. }
        | QueryPattern::Reachable { .. }
        | QueryPattern::FunctionCall { .. } => false,
        _ => true,
    }
}

fn inspect_pattern(
    pattern: &QueryPattern,
    depth: usize,
    binding_count: usize,
    input_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
    native_optional_union: bool,
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
        QueryPattern::Iid { binding, iid } => {
            check_binding(*binding, binding_count)?;
            check_operand(iid, binding_count, input_count)?;
            if let QueryOperand::Literal { value } = iid
                && !matches!(value, CanonicalValue::String(value) if crate::id::is_canonical_thing_iid(value.as_str()))
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_invalid_iid",
                    "IID patterns require a canonical TypeDB thing IID",
                ));
            }
            Ok(())
        }
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
        QueryPattern::Or { branches } => {
            if branches.is_empty()
                || branches.len() > limits.boolean_terms
                || branches
                    .iter()
                    .any(|branch| branch.is_empty() || branch.len() > limits.boolean_terms)
            {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_disjunction_term_limit",
                    "disjunction branches are empty or exceed the boolean-term ceiling",
                ));
            }
            for branch in branches {
                for child in branch {
                    inspect_pattern(
                        child,
                        depth + 1,
                        binding_count,
                        input_count,
                        limits,
                        nodes,
                        native_optional_union,
                    )?;
                }
            }
            Ok(())
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
                    native_optional_union,
                )?;
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
                if !supported_optional_body(child, native_optional_union) {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_try_body_unsupported",
                        "the first optional vocabulary admits only isa, has, links, and value patterns",
                    ));
                }
                inspect_pattern(
                    child,
                    depth + 1,
                    binding_count,
                    input_count,
                    limits,
                    nodes,
                    native_optional_union,
                )?;
            }
            Ok(())
        }
        QueryPattern::Reachable {
            min_depth,
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
            if *min_depth == 1 && *max_depth == 0 {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_reachable_depth",
                    "reachability requires a finite hop bound within the depth ceiling",
                ));
            }
            if min_depth > max_depth {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_reachable_bounds",
                    "reachability minimum depth must not exceed its maximum depth",
                ));
            }
            if !limits.allows_predicate_depth(usize::from(*max_depth)) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_reachable_depth",
                    "reachability requires a finite hop bound within the depth ceiling",
                ));
            }
            // Lowering unrolls one branch per admitted path length. Charge
            // every emitted hop against the shared node budget. The zero-hop
            // identity branch is one clause, so a compact plan cannot smuggle
            // hundreds of thousands of emitted clauses past the ceiling.
            let first_positive = usize::from((*min_depth).max(1));
            let bound = usize::from(*max_depth);
            let expanded_hops = if first_positive <= bound {
                (first_positive..=bound).fold(0usize, usize::saturating_add)
            } else {
                0
            };
            let expanded_clauses = expanded_hops.saturating_add(usize::from(*min_depth == 0));
            *nodes = nodes.saturating_add(expanded_clauses.saturating_sub(1));
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
        QueryPattern::Iid { binding, iid } => {
            operand(iid);
            bindings.insert(*binding);
        }
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
        QueryPattern::Or { branches } => {
            for branch in branches {
                for child in branch {
                    collect_pattern_bindings(child, bindings);
                }
            }
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

/// Collect bindings positively established by one pattern in its current
/// lexical scope. References used only as value/function operands are not
/// producers, and nested scopes are deliberately not traversed.
fn collect_direct_positive_bindings(pattern: &QueryPattern, bindings: &mut BTreeSet<BindingId>) {
    match pattern {
        QueryPattern::Isa { binding, .. } => {
            bindings.insert(*binding);
        }
        QueryPattern::Has {
            owner, attribute, ..
        } => {
            bindings.extend([*owner, *attribute]);
        }
        QueryPattern::Links {
            relation, players, ..
        } => {
            bindings.insert(*relation);
            bindings.extend(players.iter().map(AssertionRolePlayer::player));
        }
        QueryPattern::Reachable { source, target, .. } => {
            bindings.extend([*source, *target]);
        }
        QueryPattern::FunctionCall { assigned, .. } => {
            bindings.insert(*assigned);
        }
        QueryPattern::Or { branches } => {
            let mut branches = branches.iter().map(|patterns| {
                let mut branch = BTreeSet::new();
                for pattern in patterns {
                    collect_direct_positive_bindings(pattern, &mut branch);
                }
                branch
            });
            if let Some(mut intersection) = branches.next() {
                for branch in branches {
                    intersection.retain(|binding| branch.contains(binding));
                }
                bindings.extend(intersection);
            }
        }
        QueryPattern::Iid { .. }
        | QueryPattern::Value { .. }
        | QueryPattern::Not { .. }
        | QueryPattern::Try { .. } => {}
    }
}

/// Collect bindings established only inside negation scopes, including nested
/// negations. Optional blocks cannot occur below a negation in this vocabulary,
/// so every nested positive producer is a scoped witness.
fn collect_negation_positive_bindings(pattern: &QueryPattern, bindings: &mut BTreeSet<BindingId>) {
    match pattern {
        QueryPattern::Not { patterns } => {
            for child in patterns {
                collect_direct_positive_bindings(child, bindings);
                collect_negation_positive_bindings(child, bindings);
            }
        }
        QueryPattern::Or { branches } => {
            for branch in branches {
                for child in branch {
                    collect_negation_positive_bindings(child, bindings);
                }
            }
        }
        QueryPattern::Isa { .. }
        | QueryPattern::Has { .. }
        | QueryPattern::Links { .. }
        | QueryPattern::Iid { .. }
        | QueryPattern::Value { .. }
        | QueryPattern::Try { .. }
        | QueryPattern::Reachable { .. }
        | QueryPattern::FunctionCall { .. } => {}
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
                collect_pattern_capabilities(pattern, &mut capabilities, false)?;
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
                    collect_pattern_capabilities(pattern, &mut capabilities, false)?;
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
    negative: bool,
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
        QueryPattern::Iid { iid, .. } => {
            insert_capability(capabilities, "query.pattern.nested-iid")?;
            if negative && !matches!(iid, QueryOperand::Literal { .. }) {
                insert_capability(capabilities, v3::CAP_NEGATED_GIVEN_IID)?;
            }
        }
        QueryPattern::Has { .. } => insert_capability(capabilities, CAP_HAS)?,
        QueryPattern::Links { .. } => insert_capability(capabilities, CAP_LINKS)?,
        QueryPattern::Value { comparator, .. } => {
            insert_capability(capabilities, CAP_VALUE)?;
            if matches!(
                comparator,
                ValueComparator::Contains
                    | ValueComparator::StartsWith
                    | ValueComparator::EndsWith
                    | ValueComparator::Like
            ) {
                insert_capability(capabilities, "query.pattern.string-operators")?;
            }
        }
        QueryPattern::Or { branches } => {
            insert_capability(capabilities, CAP_DISJUNCTION)?;
            for branch in branches {
                for child in branch {
                    collect_pattern_capabilities(child, capabilities, negative)?;
                }
            }
        }
        QueryPattern::Try { patterns } => {
            insert_capability(capabilities, CAP_TRY)?;
            for child in patterns {
                collect_pattern_capabilities(child, capabilities, negative)?;
            }
        }
        QueryPattern::Reachable { .. } => {
            insert_capability(capabilities, CAP_REACHABLE)?;
        }
        QueryPattern::Not { patterns } => {
            insert_capability(capabilities, CAP_NEGATION)?;
            for child in patterns {
                collect_pattern_capabilities(child, capabilities, true)?;
            }
        }
        QueryPattern::FunctionCall { .. } => {
            insert_capability(capabilities, CAP_FUNCTION_CALL)?;
        }
    }
    Ok(())
}

pub(crate) fn insert_capability(
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

fn validate_input_rows(
    columns: &[InputColumn],
    inputs: &[InputRow],
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    validate_input_rows_deferred(columns, inputs, limits, None)
}

pub(crate) fn validate_input_rows_deferred(
    columns: &[InputColumn],
    inputs: &[InputRow],
    limits: StructuralLimits,
    deferred: Option<&BTreeSet<InputColumnId>>,
) -> Result<(), Diagnostic> {
    if columns.is_empty() {
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
        if !limits.allows_input_rows(inputs.len()) {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_invocation_row_limit",
                "invocation input row count exceeds the structural ceiling",
            ));
        }
        let input_bytes = serde_json::to_vec(inputs).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_invocation_inputs_unencodable",
                "invocation input rows cannot be encoded",
            )
        })?;
        if !limits.allows_input_bytes(input_bytes.len()) {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_invocation_input_byte_limit",
                "invocation input rows exceed the structural byte ceiling",
            ));
        }
        for row in inputs {
            if row.values().len() != columns.len() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_invocation_row_arity",
                    "input row does not carry exactly the declared column set",
                ));
            }
            for (column, value) in columns.iter().zip(row.values()) {
                match value {
                    None if column.optional()
                        || deferred.is_some_and(|columns| columns.contains(&column.id())) => {}
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
    Ok(())
}
