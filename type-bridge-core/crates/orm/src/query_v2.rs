//! Deterministic TypeQL lowering for validated V2 query plans.
//!
//! One validated plan plus one bound invocation lowers to exact TypeQL text.
//! Fully populated single-row inputs substitute canonical literals directly
//! into the pattern text. Multi-row batches and optional absence use the
//! native driver `given` transport; unsupported scalar domains reject during
//! provider-independent preflight, never through silent row-by-row emulation.

use std::fmt::Write as _;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    InputRow, QueryInvocation, QueryOperand, QueryOperation, QueryOutput, QueryPattern, QueryPlan,
    ReadStage,
};
use type_bridge_query::{
    DocumentColumnShape, DocumentSchema, OutputSchema, RowSchema, ValidatedQuery,
};

use crate::migration_assertion::{render_comparator, render_literal};

pub(crate) use crate::query_v2_model::{
    execute_validated_model_query, execute_validated_model_query_borrowed,
    execute_validated_model_query_with_statement_limit,
};

/// Exact provider text and typed shape for one lowered invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct LoweredQuery {
    given: Option<crate::session::backend::GivenRowsSpec>,
    operation: QueryOperation,
    output_schema: OutputSchema,
    typeql: String,
}

impl LoweredQuery {
    /// Return the deterministic provider query text.
    #[must_use]
    pub fn typeql(&self) -> &str {
        &self.typeql
    }

    /// Return the driver-transported input rows for a `given` lowering.
    #[must_use]
    pub const fn given_rows(&self) -> Option<&crate::session::backend::GivenRowsSpec> {
        self.given.as_ref()
    }

    /// Return the validated output shape.
    #[must_use]
    pub const fn output_schema(&self) -> &OutputSchema {
        &self.output_schema
    }

    /// Return the requested closed operation.
    #[must_use]
    pub const fn operation(&self) -> QueryOperation {
        self.operation
    }
}

/// Lower one validated plan and its bound invocation to exact TypeQL.
pub fn lower_validated_query(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
) -> Result<LoweredQuery, Diagnostic> {
    lower_validated_query_with_execution_limits(validated, invocation, None, None)
}

/// Lower one execution with finite provider-side answer sentinels.
///
/// These sentinels are deliberately execution-only: they are not part of plan
/// identity or the public deterministic lowering. The item bound clamps an
/// existing limit or is appended before `fetch`; every fetched list becomes a
/// subquery capped at its own `N + 1`. Together they let the consumer inspect
/// first over-budget evidence and still read the statement's terminal frame.
fn lower_validated_query_with_execution_limits(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    max_collection_members: Option<u64>,
    max_provider_items: Option<u64>,
) -> Result<LoweredQuery, Diagnostic> {
    let plan = validated.plan();
    preflight_lowered_structure(validated)?;
    preflight_invocation_transport(plan, invocation)?;
    if let Some(compatibility) = crate::query_v2_compatibility::lower_validated_compatibility_query(
        validated,
        invocation.operation(),
    )? {
        if compatibility.typeql().len() > MAX_CANONICAL_BYTES {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_v2_lowered_typeql_byte_limit",
                "lowered TypeQL exceeds the canonical protocol byte ceiling",
            ));
        }
        return Ok(LoweredQuery {
            given: None,
            operation: compatibility.operation(),
            output_schema: validated.output_schema().clone(),
            typeql: compatibility.typeql().to_owned(),
        });
    }
    let (row, given) = match invocation.inputs() {
        [] => (None, None),
        [row] if !invocation_requires_given(invocation) => (Some(row), None),
        rows => (None, Some(given_rows_spec(plan, rows)?)),
    };

    let mut typeql = String::new();
    for function in plan.functions() {
        write!(typeql, "with fun {}(", function.name().label())
            .expect("writing to String cannot fail");
        for (index, label) in function.parameters().iter().enumerate() {
            if index != 0 {
                typeql.push_str(", ");
            }
            write!(
                typeql,
                "${}: {}",
                local_variable(
                    function.bindings(),
                    BindingId::new(u16::try_from(index).expect("dense parameter ordinal"),)?
                )?,
                label.as_str(),
            )
            .expect("writing to String cannot fail");
        }
        writeln!(
            typeql,
            ") -> {}:",
            given_type_keyword(function.returns().value_type())
                .expect("contract admits integer and double local returns"),
        )
        .expect("writing to String cannot fail");
        typeql.push_str("match\n");
        render_scoped_patterns(
            &mut typeql,
            plan,
            function.bindings(),
            function.body(),
            None,
            0,
            None,
        )?;
        let keyword = match function.returns().reducer() {
            type_bridge_contract::query_plan::Reducer::Count => "count",
            type_bridge_contract::query_plan::Reducer::Sum => "sum",
            type_bridge_contract::query_plan::Reducer::Max => "max",
            type_bridge_contract::query_plan::Reducer::Mean => "mean",
            type_bridge_contract::query_plan::Reducer::Min => "min",
        };
        writeln!(
            typeql,
            "return {keyword}(${});",
            local_variable(function.bindings(), function.returns().input())?,
        )
        .expect("writing to String cannot fail");
    }
    if let Some(spec) = &given {
        typeql.push_str("given ");
        for (index, column) in plan.inputs().iter().enumerate() {
            if index != 0 {
                typeql.push_str(", ");
            }
            write!(
                typeql,
                "${}: {}",
                column.public_name().as_str(),
                given_type_keyword(column.value_type())
                    .expect("spec construction admitted this value type"),
            )
            .expect("writing to String cannot fail");
        }
        typeql.push_str(";\n");
        debug_assert_eq!(spec.variables.len(), plan.inputs().len());
    }
    typeql.push_str("match\n");
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_match_not_first",
            "a validated plan always opens with its match stage",
        ));
    };
    render_patterns(&mut typeql, validated, patterns, row, 0)?;

    // Reachable is an existential predicate over the declared root
    // environment, not a request for one row per proof path. TypeQL distinct
    // does not collapse equal projections produced by different disjunction
    // branches, so aggregate every proof by the complete visible environment
    // and then project the synthetic count away. This happens before any
    // user-authored select/reduce/sort/window stage, and rows that differ in
    // any declared visible binding remain distinct.
    let reachability_projection = contains_reachable(patterns);
    if reachability_projection {
        typeql.push_str("reduce $RreachableProofCount = count groupby ");
        render_variable_list(&mut typeql, plan, validated.root_visibility())?;
        typeql.push_str(";\nselect ");
        render_variable_list(&mut typeql, plan, validated.root_visibility())?;
        typeql.push_str(";\n");
    }

    // A binding established only inside a negation is a witness the
    // provider never returns as a column. When the plan carries no
    // explicit Select or Reduce and a witness narrows the environment,
    // project the validator-derived root visibility exactly, so implicit
    // projection never requests a column no provider row can carry.
    let has_projection_stage = plan
        .pipeline()
        .iter()
        .any(|stage| matches!(stage, ReadStage::Select { .. } | ReadStage::Reduce { .. }));
    if !reachability_projection
        && !has_projection_stage
        && validated.root_visibility().len() < plan.bindings().len()
    {
        typeql.push_str("select ");
        render_variable_list(&mut typeql, plan, validated.root_visibility())?;
        typeql.push_str(";\n");
    }

    let exists_probe = matches!(invocation.operation(), QueryOperation::Exists);
    let max_provider_items = if exists_probe {
        Some(1)
    } else {
        max_provider_items
    };
    let mut rendered_limit = false;
    for stage in &plan.pipeline()[1..] {
        match stage {
            ReadStage::Match { .. } => {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_match_not_first",
                    "a validated plan carries exactly one match stage",
                ));
            }
            ReadStage::Select { bindings } => {
                typeql.push_str("select ");
                render_variable_list(&mut typeql, plan, bindings)?;
                typeql.push_str(";\n");
            }
            ReadStage::Require { bindings } => {
                typeql.push_str("require ");
                render_variable_list(&mut typeql, plan, bindings)?;
                typeql.push_str(";\n");
            }
            ReadStage::Distinct => typeql.push_str("distinct;\n"),
            ReadStage::Reduce {
                assignments,
                groups,
            } => {
                typeql.push_str("reduce ");
                for (index, assignment) in assignments.iter().enumerate() {
                    if index != 0 {
                        typeql.push_str(", ");
                    }
                    let keyword = match assignment.reducer() {
                        type_bridge_contract::query_plan::Reducer::Count => "count",
                        type_bridge_contract::query_plan::Reducer::Max => "max",
                        type_bridge_contract::query_plan::Reducer::Mean => "mean",
                        type_bridge_contract::query_plan::Reducer::Min => "min",
                        type_bridge_contract::query_plan::Reducer::Sum => "sum",
                    };
                    write!(
                        typeql,
                        "${} = {keyword}",
                        variable(plan, assignment.assigned())?,
                    )
                    .expect("writing to String cannot fail");
                    if let Some(input) = assignment.input() {
                        write!(typeql, "(${})", variable(plan, input)?)
                            .expect("writing to String cannot fail");
                    }
                }
                if !groups.is_empty() {
                    typeql.push_str(" groupby ");
                    render_variable_list(&mut typeql, plan, groups)?;
                }
                typeql.push_str(";\n");
            }
            ReadStage::Sort { terms } => {
                typeql.push_str("sort ");
                for (index, term) in terms.iter().enumerate() {
                    if index != 0 {
                        typeql.push_str(", ");
                    }
                    write!(
                        typeql,
                        "${} {}",
                        variable(plan, term.binding())?,
                        match term.direction() {
                            type_bridge_contract::query_plan::OrderDirection::Ascending => "asc",
                            type_bridge_contract::query_plan::OrderDirection::Descending => "desc",
                        },
                    )
                    .expect("writing to String cannot fail");
                }
                typeql.push_str(";\n");
            }
            ReadStage::Offset { rows } => {
                writeln!(typeql, "offset {rows};").expect("writing to String cannot fail");
            }
            ReadStage::Limit { rows } => {
                let rows = max_provider_items.map_or(*rows, |maximum| (*rows).min(maximum));
                writeln!(typeql, "limit {rows};").expect("writing to String cannot fail");
                rendered_limit = true;
            }
        }
    }

    // The execution bound belongs in the TypeQL statement so the runtime can
    // consume its terminal frame instead of abandoning a resumable driver
    // stream. This must precede a document `fetch` output clause. Existing
    // limits are clamped above, so an explicit `limit 0` remains zero.
    if let Some(max_provider_items) = max_provider_items
        && !rendered_limit
    {
        writeln!(typeql, "limit {max_provider_items};").expect("writing to String cannot fail");
    }

    if let type_bridge_contract::query_plan::QueryOutput::Documents { fields } = plan.output() {
        typeql.push_str("fetch {\n");
        for (index, field) in fields.iter().enumerate() {
            write!(typeql, "    \"{}\": ", field.key().as_str())
                .expect("writing to String cannot fail");
            match field.source() {
                type_bridge_contract::query_plan::DocumentSource::Binding { binding } => {
                    write!(typeql, "${}", variable(plan, *binding)?)
                        .expect("writing to String cannot fail");
                }
                type_bridge_contract::query_plan::DocumentSource::AttributeList {
                    attribute,
                    owner,
                } => {
                    if let Some(max_collection_members) = max_collection_members {
                        let sentinel_limit = max_collection_members.saturating_add(1);
                        let fetch_variable = format!("FtbDocumentValue{index}");
                        write!(
                            typeql,
                            "[\n        match ${} has {} ${fetch_variable};\n        \
                             limit {sentinel_limit};\n        return {{ ${fetch_variable} }};\n    ]",
                            variable(plan, *owner)?,
                            attribute.label(),
                        )
                        .expect("writing to String cannot fail");
                    } else {
                        write!(
                            typeql,
                            "[ ${}.{} ]",
                            variable(plan, *owner)?,
                            attribute.label(),
                        )
                        .expect("writing to String cannot fail");
                    }
                }
            }
            typeql.push_str(if index + 1 == fields.len() {
                "\n"
            } else {
                ",\n"
            });
        }
        typeql.push_str("};\n");
    }

    if typeql.len() > MAX_CANONICAL_BYTES {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_v2_lowered_typeql_byte_limit",
            "lowered TypeQL exceeds the canonical protocol byte ceiling",
        ));
    }

    Ok(LoweredQuery {
        given,
        operation: invocation.operation(),
        output_schema: validated.output_schema().clone(),
        typeql,
    })
}

/// Charge schema-refined reachability lowering against the validated plan's
/// aggregate predicate-node budget before any provider call.
///
/// Context-free plan validation charges one clause for a zero-hop identity.
/// TypeDB's mixed identity/relation planner workaround instead emits one exact
/// typed identity branch per concrete source-domain type, so that
/// schema-dependent multiplicity must be charged after schema validation.
fn preflight_lowered_structure(validated: &ValidatedQuery) -> Result<(), Diagnostic> {
    let plan = validated.plan();
    let mut nodes = 0usize;
    for function in plan.functions() {
        for pattern in function.body() {
            charge_lowered_pattern(validated, pattern, &mut nodes)?;
        }
    }
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_match_not_first",
            "a validated plan always opens with its match stage",
        ));
    };
    for pattern in patterns {
        charge_lowered_pattern(validated, pattern, &mut nodes)?;
    }
    Ok(())
}

fn charge_lowered_pattern(
    validated: &ValidatedQuery,
    pattern: &QueryPattern,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    let charge = match pattern {
        QueryPattern::Reachable {
            min_depth,
            max_depth,
            source,
            ..
        } => {
            let first_positive = usize::from((*min_depth).max(1));
            let maximum = usize::from(*max_depth);
            let positive_hops = if first_positive <= maximum {
                (first_positive..=maximum).try_fold(0usize, usize::checked_add)
            } else {
                Some(0)
            };
            let zero_branches = if *min_depth == 0 {
                Some(
                    validated
                        .binding_domain(source)
                        .ok_or_else(|| {
                            failure(
                                DiagnosticCategory::Integrity,
                                "query_v2_reachable_source_domain",
                                "a validated reachable source has no schema-derived domain",
                            )
                        })?
                        .type_ids()
                        .len(),
                )
            } else {
                Some(0)
            };
            positive_hops.and_then(|hops| zero_branches.and_then(|zero| hops.checked_add(zero)))
        }
        QueryPattern::Or { branches } => {
            charge_nodes(validated, nodes, 1)?;
            for branch in branches {
                for child in branch {
                    charge_lowered_pattern(validated, child, nodes)?;
                }
            }
            return Ok(());
        }
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            charge_nodes(validated, nodes, 1)?;
            for child in patterns {
                charge_lowered_pattern(validated, child, nodes)?;
            }
            return Ok(());
        }
        QueryPattern::Isa { .. }
        | QueryPattern::Has { .. }
        | QueryPattern::Links { .. }
        | QueryPattern::Value { .. }
        | QueryPattern::FunctionCall { .. } => Some(1),
    }
    .ok_or_else(reachability_expansion_limit)?;
    charge_nodes(validated, nodes, charge)
}

fn charge_nodes(
    validated: &ValidatedQuery,
    nodes: &mut usize,
    charge: usize,
) -> Result<(), Diagnostic> {
    *nodes = nodes
        .checked_add(charge)
        .ok_or_else(reachability_expansion_limit)?;
    if !validated.structural_limits().allows_predicate_nodes(*nodes) {
        return Err(reachability_expansion_limit());
    }
    Ok(())
}

fn reachability_expansion_limit() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "query_v2_reachable_expansion_limit",
        "schema-refined reachability expansion exceeds the plan pattern-node ceiling",
    )
}

/// Build the driver-transported rows for one exact `given` invocation.
///
/// The portable given vocabulary transports absence and every scalar domain
/// the active driver can carry without narrowing. Provider-domain durations
/// use exact unsigned components after preflight rejects negative or wider
/// contract values.
fn given_rows_spec(
    plan: &QueryPlan,
    rows: &[type_bridge_contract::query_plan::InputRow],
) -> Result<crate::session::backend::GivenRowsSpec, Diagnostic> {
    let variables = plan
        .inputs()
        .iter()
        .map(|column| {
            if given_type_keyword(column.value_type()).is_none() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_v2_given_value_unsupported",
                    "this input value type has no proven given transport",
                ));
            }
            Ok(column.public_name().as_str().to_owned())
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let rows = rows
        .iter()
        .map(|row| {
            row.values()
                .iter()
                .map(|value| match value {
                    None => Ok(crate::session::backend::GivenValue::Empty),
                    Some(value) => given_value(value).ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_given_value_unsupported",
                            "this input value has no lossless given transport",
                        )
                    }),
                })
                .collect::<Result<Vec<_>, Diagnostic>>()
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(crate::session::backend::GivenRowsSpec { variables, rows })
}

const fn given_type_keyword(
    tag: type_bridge_contract::value::ValueTypeTag,
) -> Option<&'static str> {
    use type_bridge_contract::value::ValueTypeTag;
    match tag {
        ValueTypeTag::Boolean => Some("boolean"),
        ValueTypeTag::Long => Some("integer"),
        ValueTypeTag::Double => Some("double"),
        ValueTypeTag::String => Some("string"),
        ValueTypeTag::Date => Some("date"),
        ValueTypeTag::DateTime => Some("datetime"),
        ValueTypeTag::DateTimeTz => Some("datetime-tz"),
        ValueTypeTag::Decimal => Some("decimal"),
        // Provider-domain durations cross `given` as exact components after
        // shared temporal preflight has rejected negative or overflowing
        // values.
        ValueTypeTag::Duration => Some("duration"),
    }
}

fn given_value(
    value: &type_bridge_contract::value::CanonicalValue,
) -> Option<crate::session::backend::GivenValue> {
    use crate::session::backend::GivenValue;
    use type_bridge_contract::value::CanonicalValue;
    match value {
        CanonicalValue::Boolean(value) => Some(GivenValue::Boolean(*value)),
        CanonicalValue::Long(value) => Some(GivenValue::Integer(*value)),
        CanonicalValue::Double(value) => Some(GivenValue::Double(value.get())),
        CanonicalValue::String(value) => Some(GivenValue::String(value.as_str().to_owned())),
        CanonicalValue::Date(value) => Some(GivenValue::Date(value.to_string())),
        CanonicalValue::DateTime(value) => Some(GivenValue::Datetime(value.to_string())),
        CanonicalValue::DateTimeTz(value) => Some(GivenValue::DatetimeTzExact {
            local: value.local().to_string(),
            named_zone: match value.zone() {
                type_bridge_contract::temporal::TimeZoneDesignator::Named(name) => {
                    Some(name.clone())
                }
                type_bridge_contract::temporal::TimeZoneDesignator::Utc
                | type_bridge_contract::temporal::TimeZoneDesignator::OffsetSeconds(_) => None,
            },
            effective_offset_seconds: value.effective_offset_seconds(),
        }),
        CanonicalValue::Decimal(value) => Some(GivenValue::Decimal(value.as_str().to_owned())),
        CanonicalValue::Duration(value) => {
            let (negative, months, days, seconds, nanosecond) = value.components();
            let months = u32::try_from(months).ok()?;
            let days = u32::try_from(days).ok()?;
            let nanos = seconds
                .checked_mul(1_000_000_000)?
                .checked_add(u64::from(nanosecond))?;
            (!negative).then_some(GivenValue::Duration {
                months,
                days,
                nanos,
            })
        }
    }
}

/// Validate invocation-dependent transport before provider or transaction I/O.
pub(crate) fn preflight_invocation_transport(
    plan: &QueryPlan,
    invocation: &QueryInvocation,
) -> Result<(), Diagnostic> {
    if !invocation.binds(plan)? {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_invocation_plan_mismatch",
            "invocation does not bind the exact validated plan fingerprint",
        ));
    }
    for value in invocation
        .inputs()
        .iter()
        .flat_map(|row| row.values())
        .flatten()
    {
        type_bridge_schema::validate_provider_temporal_value(value)?;
    }
    if invocation_requires_given(invocation) {
        given_rows_spec(plan, invocation.inputs())?;
    }
    Ok(())
}

fn invocation_requires_given(invocation: &QueryInvocation) -> bool {
    invocation.inputs().len() > 1
        || invocation.inputs().first().is_some_and(|row| {
            row.values().iter().any(|value| {
                value.is_none()
                    || matches!(
                        value,
                        Some(type_bridge_contract::value::CanonicalValue::DateTimeTz(_))
                    )
            })
        })
}

fn render_patterns(
    output: &mut String,
    validated: &ValidatedQuery,
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    depth: usize,
) -> Result<(), Diagnostic> {
    let plan = validated.plan();
    render_scoped_patterns(
        output,
        plan,
        plan.bindings(),
        patterns,
        row,
        depth,
        Some(validated),
    )
}

fn render_scoped_patterns(
    output: &mut String,
    plan: &QueryPlan,
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    depth: usize,
    validated: Option<&ValidatedQuery>,
) -> Result<(), Diagnostic> {
    let indent = "    ".repeat(depth);
    for (pattern_index, pattern) in patterns.iter().enumerate() {
        match pattern {
            QueryPattern::Isa {
                binding,
                include_subtypes,
                type_id,
            } => {
                writeln!(
                    output,
                    "{indent}${} isa{} {};",
                    local_variable(bindings, *binding)?,
                    if *include_subtypes { "" } else { "!" },
                    type_id.label()
                )
                .expect("writing to String cannot fail");
            }
            QueryPattern::Has {
                attribute,
                attribute_id,
                owner,
            } => {
                writeln!(
                    output,
                    "{indent}${} has {} ${};",
                    local_variable(bindings, *owner)?,
                    attribute_id.label(),
                    local_variable(bindings, *attribute)?,
                )
                .expect("writing to String cannot fail");
                writeln!(
                    output,
                    "{indent}${} isa! {};",
                    local_variable(bindings, *attribute)?,
                    attribute_id.label(),
                )
                .expect("writing to String cannot fail");
            }
            QueryPattern::Links {
                players,
                relation,
                relation_id,
            } => {
                write!(
                    output,
                    "{indent}${} isa! {}, links (",
                    local_variable(bindings, *relation)?,
                    relation_id.label(),
                )
                .expect("writing to String cannot fail");
                for (index, player) in players.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    write!(
                        output,
                        "{}: ${}",
                        player.role().label(),
                        local_variable(bindings, player.player())?,
                    )
                    .expect("writing to String cannot fail");
                }
                output.push_str(");\n");
            }
            QueryPattern::Value {
                comparator,
                left,
                right,
            } => {
                writeln!(
                    output,
                    "{indent}{} {} {};",
                    render_operand(plan, bindings, left, row)?,
                    render_comparator(*comparator),
                    render_operand(plan, bindings, right, row)?,
                )
                .expect("writing to String cannot fail");
            }
            QueryPattern::Or { branches } => {
                for (index, branch) in branches.iter().enumerate() {
                    writeln!(output, "{indent}{}{{", if index == 0 { "" } else { "or " },)
                        .expect("writing to String cannot fail");
                    render_scoped_patterns(
                        output,
                        plan,
                        bindings,
                        branch,
                        row,
                        depth + 1,
                        validated,
                    )?;
                    writeln!(
                        output,
                        "{indent}}}{}",
                        if index + 1 == branches.len() { ";" } else { "" },
                    )
                    .expect("writing to String cannot fail");
                }
            }
            QueryPattern::Not { patterns } => {
                writeln!(output, "{indent}not {{").expect("writing to String cannot fail");
                render_scoped_patterns(
                    output,
                    plan,
                    bindings,
                    patterns,
                    row,
                    depth + 1,
                    validated,
                )?;
                writeln!(output, "{indent}}};").expect("writing to String cannot fail");
            }
            QueryPattern::Try { patterns } => {
                writeln!(output, "{indent}try {{").expect("writing to String cannot fail");
                render_scoped_patterns(
                    output,
                    plan,
                    bindings,
                    patterns,
                    row,
                    depth + 1,
                    validated,
                )?;
                writeln!(output, "{indent}}};").expect("writing to String cannot fail");
            }
            QueryPattern::Reachable {
                min_depth,
                max_depth,
                relation,
                role_from,
                role_to,
                source,
                target,
            } => {
                // Uppercase hop intermediates cannot collide with the
                // lowercase plan variable space; the pattern index keeps
                // them unique across root reachability patterns.
                let source_id = *source;
                let source = local_variable(bindings, source_id)?;
                let target = local_variable(bindings, *target)?;
                let mut branches = Vec::with_capacity(
                    usize::from(*max_depth)
                        .saturating_sub(usize::from(*min_depth))
                        .saturating_add(1),
                );
                for length in usize::from(*min_depth)..=usize::from(*max_depth) {
                    if length == 0 {
                        let validated = validated.ok_or_else(|| {
                            failure(
                                DiagnosticCategory::Integrity,
                                "query_v2_reachable_scope",
                                "a validated reachable pattern appears outside the root query scope",
                            )
                        })?;
                        let source_domain =
                            validated.binding_domain(&source_id).ok_or_else(|| {
                                failure(
                                    DiagnosticCategory::Integrity,
                                    "query_v2_reachable_source_domain",
                                    "a validated reachable source has no schema-derived domain",
                                )
                            })?;
                        if source_domain.type_ids().is_empty() {
                            return Err(failure(
                                DiagnosticCategory::Integrity,
                                "query_v2_reachable_source_domain",
                                "a validated reachable source has no schema-derived domain",
                            ));
                        }
                        // TypeDB 3.12 cannot plan a bare identity branch next
                        // to relation branches. One exact typed witness per
                        // validator-derived concrete source type preserves the
                        // full source domain without widening it.
                        for (type_ordinal, type_id) in source_domain.type_ids().iter().enumerate() {
                            // Every disjunction branch owns its complete local
                            // witness environment. Reusing one variable across
                            // mutually exclusive concrete-type branches makes
                            // TypeDB require that witness in sibling positive
                            // path branches (REP44).
                            let zero_var = format!("R{pattern_index}z{type_ordinal}");
                            branches.push(format!(
                                "${zero_var} isa! {}; ${source} is ${zero_var}; ${target} is ${zero_var};",
                                type_id.label()
                            ));
                        }
                        continue;
                    }
                    let mut branch = String::new();
                    let hop_var = |hop: usize| format!("R{pattern_index}l{length}h{hop}");
                    for hop in 1..=length {
                        if hop != 1 {
                            branch.push(' ');
                        }
                        let from = if hop == 1 {
                            source.to_owned()
                        } else {
                            hop_var(hop - 1)
                        };
                        let to = if hop == length {
                            target.to_owned()
                        } else {
                            hop_var(hop)
                        };
                        // The Reachable contract promises the exact relation
                        // type for every hop; `isa!` keeps subtype instances
                        // out exactly like every other exact-type pattern.
                        write!(
                            branch,
                            "({}: ${from}, {}: ${to}) isa! {};",
                            role_from.label(),
                            role_to.label(),
                            relation.label(),
                        )
                        .expect("writing to String cannot fail");
                    }
                    branches.push(branch);
                }
                if branches.len() == 1 {
                    writeln!(output, "{indent}{}", branches[0])
                        .expect("writing to String cannot fail");
                } else {
                    let joined = branches
                        .iter()
                        .map(|branch| format!("{{ {branch} }}"))
                        .collect::<Vec<_>>()
                        .join(" or ");
                    writeln!(output, "{indent}{joined};").expect("writing to String cannot fail");
                }
            }
            QueryPattern::FunctionCall {
                arguments,
                assigned,
                function,
            } => {
                write!(
                    output,
                    "{indent}let ${} = {}(",
                    local_variable(bindings, *assigned)?,
                    function.label(),
                )
                .expect("writing to String cannot fail");
                for (index, argument) in arguments.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&render_operand(plan, bindings, argument, row)?);
                }
                output.push_str(");\n");
            }
        }
    }
    Ok(())
}

fn contains_reachable(patterns: &[QueryPattern]) -> bool {
    patterns.iter().any(|pattern| match pattern {
        QueryPattern::Reachable { .. } => true,
        QueryPattern::Or { branches } => branches.iter().any(|branch| contains_reachable(branch)),
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            contains_reachable(patterns)
        }
        QueryPattern::Isa { .. }
        | QueryPattern::Has { .. }
        | QueryPattern::Links { .. }
        | QueryPattern::Value { .. }
        | QueryPattern::FunctionCall { .. } => false,
    })
}

fn render_variable_list(
    output: &mut String,
    plan: &QueryPlan,
    bindings: &[BindingId],
) -> Result<(), Diagnostic> {
    for (index, binding) in bindings.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push('$');
        output.push_str(variable(plan, *binding)?);
    }
    Ok(())
}

fn render_operand(
    plan: &QueryPlan,
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    operand: &QueryOperand,
    row: Option<&InputRow>,
) -> Result<String, Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => {
            Ok(format!("${}", local_variable(bindings, *binding)?))
        }
        QueryOperand::Literal { value } => render_literal(value),
        QueryOperand::Input { column } => match row {
            Some(row) => {
                let value = row
                    .values()
                    .get(usize::from(column.get()))
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_missing_input_value",
                            "pattern reads an input column absent from the bound row",
                        )
                    })?;
                render_literal(value)
            }
            // Given lowering: the column is a driver-bound row variable.
            None => {
                let column = plan
                    .inputs()
                    .get(usize::from(column.get()))
                    .ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_missing_input_value",
                            "pattern reads an undeclared input column",
                        )
                    })?;
                Ok(format!("${}", column.public_name().as_str()))
            }
        },
    }
}

fn variable(plan: &QueryPlan, binding: BindingId) -> Result<&str, Diagnostic> {
    local_variable(plan.bindings(), binding)
}

fn local_variable(
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    binding: BindingId,
) -> Result<&str, Diagnostic> {
    bindings
        .get(usize::from(binding.get()))
        .map(|binding| binding.variable().as_str())
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "query_v2_unknown_binding",
                "validated plan references an unknown binding",
            )
        })
}

pub(crate) fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static query lowering diagnostic code"),
        message,
    )
}

/// Preserve the stable resource diagnostics emitted by the bounded provider
/// seam while redacting arbitrary driver text for every other provider error.
pub(crate) fn provider_diagnostic(
    error: &crate::error::OrmError,
    generic_code: &'static str,
    generic_message: &'static str,
) -> Diagnostic {
    let crate::error::OrmError::Match(error) = error else {
        return failure(DiagnosticCategory::Integrity, generic_code, generic_message);
    };
    if error.category() != crate::match_request::MatchErrorCategory::ResourceLimit {
        return failure(DiagnosticCategory::Integrity, generic_code, generic_message);
    }
    let (code, message) = match error.code().as_str() {
        "provider_cancelled" => (
            "provider_cancelled",
            "provider answer processing was cancelled",
        ),
        "transaction_deadline_exceeded" => (
            "transaction_deadline_exceeded",
            "provider transaction deadline expired",
        ),
        "processed_item_limit" => (
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        "response_byte_limit" => (
            "response_byte_limit",
            "provider answer exceeded the response-byte ceiling",
        ),
        "processed_item_counter_overflow" => (
            "processed_item_counter_overflow",
            "processed provider item counter overflowed",
        ),
        "answer_byte_counter_overflow" => (
            "answer_byte_counter_overflow",
            "provider answer byte counter overflowed",
        ),
        "query_v2_document_member_limit" => (
            "query_v2_document_member_limit",
            "document lists exceed the aggregate member ceiling",
        ),
        _ => return failure(DiagnosticCategory::Integrity, generic_code, generic_message),
    };
    failure(DiagnosticCategory::ResourceLimit, code, message)
}

fn query_v2_provider_resource_error(
    code: &'static str,
    message: impl Into<String>,
) -> crate::error::OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::ResourceLimit,
        code,
        message,
    )
    .at(crate::match_request::MatchErrorPathSegment::ProviderEvidence)
    .into()
}

fn query_v2_result_validation_error(diagnostic: Diagnostic) -> QueryV2ExecutionError {
    if diagnostic.category() == DiagnosticCategory::ResourceLimit
        && diagnostic.code().as_str() == "query_v2_document_member_limit"
    {
        return QueryV2ExecutionError::Provider(query_v2_provider_resource_error(
            "query_v2_document_member_limit",
            diagnostic.message().to_owned(),
        ));
    }
    QueryV2ExecutionError::Validation(diagnostic)
}

/// Return the output projection columns of one validated plan.
///
/// The provider `select` keeps the plan's visible environment; the exact
/// projection to output columns is enforced in Rust during result
/// validation, mirroring the released V1 executor discipline.
#[must_use]
pub fn output_columns(plan: &QueryPlan) -> &[BindingId] {
    match plan.output() {
        QueryOutput::Rows { columns } => columns,
        QueryOutput::Documents { .. } => &[],
    }
}

/// One typed, evidence-validated output value.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryRowValue {
    /// An entity or relation reference: exact runtime type plus identity.
    Thing {
        /// The validated runtime type.
        type_id: type_bridge_contract::id::TypeId,
        /// The provider instance identity.
        iid: String,
    },
    /// An attribute instance with its parsed canonical value.
    Attribute {
        /// The validated runtime attribute type.
        type_id: type_bridge_contract::id::TypeId,
        /// The exact typed scalar value.
        value: type_bridge_contract::value::CanonicalValue,
    },
    /// A pure typed value produced by a schema-function assignment.
    Value {
        /// The exact typed scalar value.
        value: type_bridge_contract::value::CanonicalValue,
    },
    /// An explicit absence in an optional output column.
    Absent,
}

/// One evidence-validated output row, positional by output column.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryResultRow {
    values: Vec<QueryRowValue>,
}

impl QueryResultRow {
    pub(crate) fn from_values(values: Vec<QueryRowValue>) -> Self {
        Self { values }
    }

    /// Return positional typed values in output-column order.
    #[must_use]
    pub fn values(&self) -> &[QueryRowValue] {
        &self.values
    }
}

/// One typed, evidence-validated document field value.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentFieldValue {
    /// One exact typed scalar.
    Scalar(type_bridge_contract::value::CanonicalValue),
    /// An explicit absence in an optional scalar field.
    Absent,
    /// A typed list of every owned attribute value.
    List(Vec<type_bridge_contract::value::CanonicalValue>),
}

/// One evidence-validated fetched document, positional by document column.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryResultDocument {
    values: Vec<DocumentFieldValue>,
}

impl QueryResultDocument {
    pub(crate) fn from_values(values: Vec<DocumentFieldValue>) -> Self {
        Self { values }
    }

    /// Return field values in validated document-column order.
    pub fn values(&self) -> &[DocumentFieldValue] {
        &self.values
    }
}

/// The validated terminal result of one executed invocation.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryV2Outcome {
    /// Evidence-validated projected rows in provider order.
    Rows(Vec<QueryResultRow>),
    /// Evidence-validated fetched documents in provider order.
    Documents(Vec<QueryResultDocument>),
    /// The exact number of returned rows.
    Count(u64),
    /// Whether at least one row exists.
    Exists(bool),
}

/// Failure executing one lowered invocation.
#[derive(Debug, thiserror::Error)]
pub enum QueryV2ExecutionError {
    /// The provider call failed.
    #[error("query execution provider call failed: {0}")]
    Provider(crate::error::OrmError),
    /// Lowering, preflight, or result-evidence validation failed.
    #[error("query execution validation failed: {0}")]
    Validation(Diagnostic),
}

/// Internal observer invoked after evidence validation and before retention.
pub(crate) trait QueryV2ValidatedItemObserver: Send {
    fn observe_row(&mut self, row: &QueryResultRow) -> Result<(), Diagnostic>;

    fn observe_document(&mut self, document: &QueryResultDocument) -> Result<(), Diagnostic>;
}

/// Execute one validated invocation through an open read transaction.
///
/// Rows are streamed under the caller's explicit bounded limits and each
/// provider row is evidence-validated against the derived row schema before
/// projection; count and exists reuse the same validated stream and are
/// decided in Rust, mirroring the released V1 executor discipline.
pub async fn execute_validated_query(
    transaction: &mut crate::session::transaction::Transaction,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: crate::session::backend::QueryV2AnswerLimits,
) -> Result<QueryV2Outcome, QueryV2ExecutionError> {
    preflight_invocation_transport(validated.plan(), invocation)
        .map_err(QueryV2ExecutionError::Validation)?;
    let transaction = transaction
        .provider_mut()
        .map_err(QueryV2ExecutionError::Provider)?;
    let mut provider = crate::migration_assertion::TransactionAssertionProvider { transaction };
    execute_with_provider(&mut provider, validated, invocation, limits).await
}

pub(crate) async fn execute_with_provider<
    P: crate::migration_assertion::AssertionProviderCall + ?Sized,
>(
    provider: &mut P,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: crate::session::backend::QueryV2AnswerLimits,
) -> Result<QueryV2Outcome, QueryV2ExecutionError> {
    execute_with_provider_observer(provider, validated, invocation, limits, None).await
}

pub(crate) async fn execute_with_provider_observer<
    P: crate::migration_assertion::AssertionProviderCall + ?Sized,
>(
    provider: &mut P,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: crate::session::backend::QueryV2AnswerLimits,
    mut observer: Option<&mut dyn QueryV2ValidatedItemObserver>,
) -> Result<QueryV2Outcome, QueryV2ExecutionError> {
    let mut semantic_limits = limits;
    semantic_limits.max_collection_members = semantic_limits
        .max_collection_members
        .min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).expect("canonical limit fits u64"));
    let exists_probe = matches!(invocation.operation(), QueryOperation::Exists);
    let provider_item_sentinel = if exists_probe {
        1
    } else {
        semantic_limits.answer.max_items.saturating_add(1)
    };
    let lowered = lower_validated_query_with_execution_limits(
        validated,
        invocation,
        Some(semantic_limits.max_collection_members),
        Some(provider_item_sentinel),
    )
    .map_err(QueryV2ExecutionError::Validation)?;
    let allow_input_echo = lowered.given_rows().is_some();

    // The statement itself is finite at the semantic item/list sentinels, and
    // the provider/session seam independently retains the finite raw-answer
    // byte and aggregate-member ceilings. The consumer repeats those checks on
    // the projected semantic evidence; neither layer may become unbounded just
    // because a remote caller meters a different canonical wire encoding.
    let mut provider_limits = semantic_limits.clone();
    provider_limits.answer.max_items = provider_item_sentinel;

    let mut rows: Vec<QueryResultRow> = Vec::new();
    let mut documents: Vec<QueryResultDocument> = Vec::new();
    let max_items = semantic_limits.answer.max_items;
    let max_bytes = semantic_limits.answer.max_bytes;
    let max_collection_members = semantic_limits.max_collection_members;
    let mut processed_items = 0_u64;
    let mut response_bytes = 0_u64;
    let mut collection_members = 0_u64;
    let mut deferred_error: Option<QueryV2ExecutionError> = None;
    let mut consumer = |item| {
        // Once a deterministic first failure is known, keep draining the
        // finite statement without parsing or retaining any later evidence.
        if deferred_error.is_some() {
            return Ok(crate::session::backend::AnswerControl::Continue);
        }

        let Some(next_items) = processed_items.checked_add(1) else {
            deferred_error = Some(QueryV2ExecutionError::Provider(
                query_v2_provider_resource_error(
                    "processed_item_counter_overflow",
                    "processed provider item counter overflowed",
                ),
            ));
            return Ok(crate::session::backend::AnswerControl::Continue);
        };
        if next_items > max_items {
            deferred_error = Some(QueryV2ExecutionError::Provider(
                query_v2_provider_resource_error(
                    "processed_item_limit",
                    "provider answer exceeded the processed-item ceiling",
                ),
            ));
            return Ok(crate::session::backend::AnswerControl::Continue);
        }

        let value = match &item {
            crate::session::backend::AnswerItem::Row(value)
            | crate::session::backend::AnswerItem::Document(value) => value,
        };
        let encoded = match serde_json::to_vec(value) {
            Ok(encoded) => encoded,
            Err(error) => {
                deferred_error = Some(QueryV2ExecutionError::Provider(
                    crate::error::OrmError::QueryExecution(format!("Answer encode: {error}")),
                ));
                return Ok(crate::session::backend::AnswerControl::Continue);
            }
        };
        let Ok(encoded_bytes) = u64::try_from(encoded.len()) else {
            deferred_error = Some(QueryV2ExecutionError::Provider(
                query_v2_provider_resource_error(
                    "answer_byte_counter_overflow",
                    "encoded provider answer length exceeds the counter range",
                ),
            ));
            return Ok(crate::session::backend::AnswerControl::Continue);
        };
        let Some(next_bytes) = response_bytes.checked_add(encoded_bytes) else {
            deferred_error = Some(QueryV2ExecutionError::Provider(
                query_v2_provider_resource_error(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                ),
            ));
            return Ok(crate::session::backend::AnswerControl::Continue);
        };
        if next_bytes > max_bytes {
            deferred_error = Some(QueryV2ExecutionError::Provider(
                query_v2_provider_resource_error(
                    "response_byte_limit",
                    "provider answer exceeded the response-byte ceiling",
                ),
            ));
            return Ok(crate::session::backend::AnswerControl::Continue);
        }
        processed_items = next_items;
        response_bytes = next_bytes;

        let validated_item = match (validated.output_schema(), item) {
            (OutputSchema::Rows(schema), crate::session::backend::AnswerItem::Row(row)) => {
                validate_result_row(&row, validated, schema, allow_input_echo).and_then(|values| {
                    let row = QueryResultRow { values };
                    if let Some(observer) = observer.as_deref_mut() {
                        observer.observe_row(&row)?;
                    }
                    rows.push(row);
                    Ok(())
                })
            }
            (
                OutputSchema::Documents(schema),
                crate::session::backend::AnswerItem::Document(document),
            ) => validate_result_document(
                &document,
                schema,
                &mut collection_members,
                max_collection_members,
            )
            .and_then(|values| {
                let document = QueryResultDocument { values };
                if let Some(observer) = observer.as_deref_mut() {
                    observer.observe_document(&document)?;
                }
                documents.push(document);
                Ok(())
            }),
            (OutputSchema::Rows(_), _) => Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_v2_result_not_row",
                "provider returned a document for a selected-row query",
            )),
            (OutputSchema::Documents(_), _) => Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_v2_result_not_document",
                "provider returned a row for a fetched-document query",
            )),
        };
        match validated_item {
            Ok(()) => Ok(crate::session::backend::AnswerControl::Continue),
            Err(diagnostic) => {
                deferred_error = Some(query_v2_result_validation_error(diagnostic));
                Ok(crate::session::backend::AnswerControl::Continue)
            }
        }
    };
    let provider_result = match lowered.given_rows() {
        Some(spec) => {
            if !provider.supports_given_rows() {
                return Err(QueryV2ExecutionError::Validation(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_v2_given_transport_unsupported",
                    "this invocation requires the native given transport capability",
                )));
            }
            provider
                .query_v2_with_rows_bounded(
                    lowered.typeql(),
                    spec.clone(),
                    provider_limits,
                    &mut consumer,
                )
                .await
        }
        None => {
            provider
                .query_v2_bounded(lowered.typeql(), provider_limits, &mut consumer)
                .await
        }
    };
    // A provider failure observed while draining follows the semantic or
    // evidence failure that made draining necessary. Preserve that first
    // stable failure just as the former immediate-return path did.
    if let Some(error) = deferred_error {
        return Err(error);
    }
    let stats = provider_result.map_err(QueryV2ExecutionError::Provider)?;
    if stats.stopped_early {
        return Err(QueryV2ExecutionError::Validation(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_provider_stream_not_exhausted",
            "provider stopped before the bounded query statement reached its terminal frame",
        )));
    }

    let answers = rows.len() + documents.len();
    Ok(match lowered.operation() {
        QueryOperation::Rows => match validated.output_schema() {
            OutputSchema::Rows(_) => QueryV2Outcome::Rows(rows),
            OutputSchema::Documents(_) => QueryV2Outcome::Documents(documents),
        },
        QueryOperation::Count => QueryV2Outcome::Count(answers as u64),
        QueryOperation::Exists => QueryV2Outcome::Exists(answers != 0),
    })
}

/// Validate one provider document against the derived document schema.
fn validate_result_document(
    document: &serde_json::Value,
    schema: &DocumentSchema,
    collection_members: &mut u64,
    max_collection_members: u64,
) -> Result<Vec<DocumentFieldValue>, Diagnostic> {
    let object = document.as_object().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_result_document_malformed",
            "provider document must be a JSON object keyed by fetch keys",
        )
    })?;
    if object.len() != schema.columns().len()
        || schema
            .columns()
            .iter()
            .any(|column| !object.contains_key(column.key().as_str()))
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_result_column_mismatch",
            "provider document keys do not exactly match the fetch schema",
        ));
    }
    let mut values = Vec::with_capacity(schema.columns().len());
    for column in schema.columns() {
        let value = &object[column.key().as_str()];
        values.push(match column.shape() {
            DocumentColumnShape::Scalar {
                value_type,
                optional,
            } => {
                if value.is_null() {
                    if !optional {
                        return Err(failure(
                            DiagnosticCategory::Integrity,
                            "query_v2_result_type_mismatch",
                            "mandatory document field carries an explicit absence",
                        ));
                    }
                    DocumentFieldValue::Absent
                } else {
                    DocumentFieldValue::Scalar(crate::migration_assertion::parse_provider_value(
                        value,
                        *value_type,
                    )?)
                }
            }
            DocumentColumnShape::List { element_type, .. } => {
                let elements = value.as_array().ok_or_else(|| {
                    failure(
                        DiagnosticCategory::Integrity,
                        "query_v2_result_type_mismatch",
                        "attribute list field is not a JSON array",
                    )
                })?;
                let field_members = u64::try_from(elements.len()).map_err(|_| {
                    failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_v2_document_member_limit",
                        "document list member count exceeds the supported counter range",
                    )
                })?;
                let next_members =
                    collection_members
                        .checked_add(field_members)
                        .ok_or_else(|| {
                            failure(
                                DiagnosticCategory::ResourceLimit,
                                "query_v2_document_member_limit",
                                "document list member counter overflowed",
                            )
                        })?;
                if next_members > max_collection_members {
                    return Err(failure(
                        DiagnosticCategory::ResourceLimit,
                        "query_v2_document_member_limit",
                        "document lists exceed the aggregate member ceiling",
                    ));
                }
                *collection_members = next_members;
                DocumentFieldValue::List(
                    elements
                        .iter()
                        .map(|element| {
                            crate::migration_assertion::parse_provider_value(element, *element_type)
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
        });
    }
    Ok(values)
}

/// Validate one provider row and project it to typed output values.
fn validate_result_row(
    row: &serde_json::Value,
    validated: &ValidatedQuery,
    schema: &RowSchema,
    allow_input_echo: bool,
) -> Result<Vec<QueryRowValue>, Diagnostic> {
    use type_bridge_contract::id::{TypeId, TypeKind};

    let object = row.as_object().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_result_row_malformed",
            "provider row must be a JSON object keyed by selected variables",
        )
    })?;
    let visible = visible_variables(validated);
    // Given lowerings echo the driver-bound input variables in every row;
    // input names are contract-unique against binding names, so tolerating
    // exactly them stays closed.
    let inputs: std::collections::BTreeSet<&str> = if allow_input_echo {
        validated
            .plan()
            .inputs()
            .iter()
            .map(|column| column.public_name().as_str())
            .collect()
    } else {
        std::collections::BTreeSet::new()
    };
    if visible.iter().any(|name| !object.contains_key(*name))
        || object
            .keys()
            .any(|key| !visible.contains(&key.as_str()) && !inputs.contains(key.as_str()))
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_result_column_mismatch",
            "provider row columns do not exactly match the visible row environment",
        ));
    }

    let mut values = Vec::with_capacity(schema.columns().len());
    for column in schema.columns() {
        let domain = column.domain();
        // Optional columns carry an explicit null when their try body did
        // not match; mandatory columns never do.
        if column.optional()
            && object
                .get(column.variable().as_str())
                .is_some_and(serde_json::Value::is_null)
        {
            values.push(QueryRowValue::Absent);
            continue;
        }
        let concept = object
            .get(column.variable().as_str())
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "query_v2_result_concept_malformed",
                    "output column is not a provider concept object",
                )
            })?;
        // A pure value binding (empty thing domain, exact scalar) carries a
        // value concept. Drivers may include their informational value label,
        // but it is not a schema type identity.
        if domain.type_ids().is_empty() {
            let Some(expected) = domain.value_type() else {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "output column has neither a thing domain nor a scalar domain",
                ));
            };
            let category =
                crate::migration_assertion::string_field(concept, "category", column.variable())?;
            if !matches!(category, "value" | "Value") {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "value column evidence is not a provider value concept",
                ));
            }
            reject_unexpected_provider_concept_fields(concept, column.variable(), category)?;
            if concept.get("label").is_some_and(|label| !label.is_string()) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_v2_result_concept_malformed",
                    "provider value concept carries an invalid label",
                ));
            }
            let actual =
                crate::migration_assertion::string_field(concept, "value_type", column.variable())?;
            if crate::migration_assertion::provider_value_type(actual) != Some(expected) {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "provider value type differs from the validated scalar domain",
                ));
            }
            let value = crate::migration_assertion::parse_provider_value(
                concept.get("value").ok_or_else(|| {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "query_v2_result_concept_malformed",
                        "value concept carries no value",
                    )
                })?,
                expected,
            )?;
            values.push(QueryRowValue::Value { value });
            continue;
        }
        let category =
            crate::migration_assertion::string_field(concept, "category", column.variable())?;
        reject_unexpected_provider_concept_fields(concept, column.variable(), category)?;
        let label = crate::migration_assertion::string_field(concept, "label", column.variable())?;
        let kind =
            crate::migration_assertion::provider_concept_kind(category).ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "provider concept category is outside the validated domain",
                )
            })?;
        let type_id = TypeId::new(kind, label).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_v2_result_concept_malformed",
                "provider concept label is not a canonical type identity",
            )
        })?;
        if !domain.type_ids().contains(&type_id) {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_v2_result_type_mismatch",
                "provider concept type is outside the validated domain",
            ));
        }
        values.push(match (kind, domain.value_type()) {
            (TypeKind::Entity | TypeKind::Relation, None) => {
                let iid =
                    crate::migration_assertion::string_field(concept, "iid", column.variable())?;
                if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_v2_result_concept_malformed",
                        if iid.is_empty() {
                            "thing concept carries an empty instance identity"
                        } else {
                            "thing concept carries a malformed instance identity"
                        },
                    ));
                }
                QueryRowValue::Thing {
                    type_id,
                    iid: iid.to_owned(),
                }
            }
            (TypeKind::Attribute, Some(expected)) => {
                if let Some(iid) = concept.get("iid") {
                    let Some(iid) = iid.as_str() else {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_result_concept_malformed",
                            "attribute concept carries an invalid instance identity",
                        ));
                    };
                    if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_result_concept_malformed",
                            if iid.is_empty() {
                                "attribute concept carries an empty instance identity"
                            } else {
                                "attribute concept carries a malformed instance identity"
                            },
                        ));
                    }
                }
                let actual = crate::migration_assertion::string_field(
                    concept,
                    "value_type",
                    column.variable(),
                )?;
                if crate::migration_assertion::provider_value_type(actual) != Some(expected) {
                    return Err(failure(
                        DiagnosticCategory::Integrity,
                        "query_v2_result_type_mismatch",
                        "provider value type differs from the validated scalar domain",
                    ));
                }
                let value = crate::migration_assertion::parse_provider_value(
                    concept.get("value").ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_result_concept_malformed",
                            "attribute concept carries no value",
                        )
                    })?,
                    expected,
                )?;
                QueryRowValue::Attribute { type_id, value }
            }
            _ => {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "provider concept shape disagrees with the validated domain",
                ));
            }
        });
    }
    Ok(values)
}

fn reject_unexpected_provider_concept_fields(
    concept: &serde_json::Map<String, serde_json::Value>,
    binding: &type_bridge_contract::migration_assertion::QueryVariable,
    category: &str,
) -> Result<(), Diagnostic> {
    let allowed: &[&str] = match category {
        "value" | "Value" => &["category", "label", "value", "value_type"],
        "entity" | "Entity" | "relation" | "Relation" => &["category", "iid", "label"],
        "attribute" | "Attribute" => &["category", "iid", "label", "value", "value_type"],
        // The caller reports an unknown category as a domain mismatch. Keep
        // that stable diagnostic instead of letting an incidental field win.
        _ => return Ok(()),
    };
    let mut unexpected = concept
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unexpected.sort();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_result_concept_malformed",
        "provider concept evidence contains unexpected typed fields",
    )
    .with_detail("binding", binding.as_str())
    .with_detail("unexpected_fields", unexpected))
}

/// Return the visible variable names after select and reduce stages.
fn visible_variables(validated: &ValidatedQuery) -> Vec<&str> {
    let plan = validated.plan();
    // A reduce stage replaces the whole row environment with its group
    // keys and assigned results, superseding any earlier select.
    for stage in plan.pipeline() {
        if let ReadStage::Reduce {
            assignments,
            groups,
        } = stage
        {
            return groups
                .iter()
                .copied()
                .chain(assignments.iter().map(|assignment| assignment.assigned()))
                .filter_map(|binding| {
                    plan.bindings()
                        .get(usize::from(binding.get()))
                        .map(|binding| binding.variable().as_str())
                })
                .collect();
        }
    }
    for stage in plan.pipeline() {
        if let ReadStage::Select { bindings } = stage {
            return bindings
                .iter()
                .filter_map(|binding| {
                    plan.bindings()
                        .get(usize::from(binding.get()))
                        .map(|binding| binding.variable().as_str())
                })
                .collect();
        }
    }
    // No Select or Reduce: the environment is the validator-derived root
    // visibility, which lowering also projects explicitly whenever a
    // negation witness narrows it below the declared binding set.
    validated
        .root_visibility()
        .iter()
        .filter_map(|binding| {
            plan.bindings()
                .get(usize::from(binding.get()))
                .map(|binding| binding.variable().as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, QueryVariable, ValueComparator,
    };
    use type_bridge_contract::query_plan::{
        DocumentField, DocumentSource, InputColumn, InputColumnId, OrderDirection, OrderTerm,
        QueryOutput as PlanOutput,
    };
    use type_bridge_contract::schema::{
        AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
        DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId,
        SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId,
        TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
    use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

    use super::*;
    use crate::migration_assertion::AssertionProviderCall;
    use crate::session::backend::{
        AnswerCancellation, AnswerConsumer, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
        BoundedAnswerStats, BoxFuture, QueryV2AnswerLimits,
    };

    struct ScriptedProvider {
        documents: bool,
        rows: Vec<serde_json::Value>,
    }

    impl AssertionProviderCall for ScriptedProvider {
        fn query_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
            Box::pin(async move {
                let mut processed = 0u64;
                for row in &self.rows {
                    if processed >= limits.max_items {
                        break;
                    }
                    processed += 1;
                    let answer = if self.documents {
                        AnswerItem::Document(row.clone())
                    } else {
                        AnswerItem::Row(row.clone())
                    };
                    if consumer.accept(answer)? == crate::session::backend::AnswerControl::Stop {
                        break;
                    }
                }
                Ok(BoundedAnswerStats {
                    processed_items: processed,
                    response_bytes: 0,
                    stopped_early: false,
                })
            })
        }
    }

    struct DrainProbe {
        documents: bool,
        rows: Vec<serde_json::Value>,
        observed_typeql: String,
        observed_limits: Option<(u64, u64, u64)>,
        controls: Vec<crate::session::backend::AnswerControl>,
        forge_early_stop: bool,
        fail_after_rows: bool,
    }

    impl DrainProbe {
        fn rows(rows: Vec<serde_json::Value>) -> Self {
            Self {
                documents: false,
                rows,
                observed_typeql: String::new(),
                observed_limits: None,
                controls: Vec::new(),
                forge_early_stop: false,
                fail_after_rows: false,
            }
        }

        fn documents(rows: Vec<serde_json::Value>) -> Self {
            Self {
                documents: true,
                ..Self::rows(rows)
            }
        }
    }

    impl AssertionProviderCall for DrainProbe {
        fn query_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            _limits: BoundedAnswerLimits,
            _consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
            panic!("V2 execution must use the additive V2 provider seam")
        }

        fn query_v2_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            limits: QueryV2AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
            Box::pin(async move {
                self.observed_typeql = typeql.to_owned();
                self.observed_limits = Some((
                    limits.answer.max_items,
                    limits.answer.max_bytes,
                    limits.max_collection_members,
                ));
                let mut processed_items = 0_u64;
                let mut response_bytes = 0_u64;
                let mut stopped_early = self.forge_early_stop;
                for row in &self.rows {
                    processed_items += 1;
                    response_bytes +=
                        u64::try_from(serde_json::to_vec(row).expect("JSON value encodes").len())
                            .expect("encoded test value length fits u64");
                    let item = if self.documents {
                        AnswerItem::Document(row.clone())
                    } else {
                        AnswerItem::Row(row.clone())
                    };
                    let control = consumer.accept(item)?;
                    self.controls.push(control);
                    stopped_early |= control == crate::session::backend::AnswerControl::Stop;
                }
                if self.fail_after_rows {
                    return Err(crate::error::OrmError::Transaction(
                        "later drain failure".into(),
                    ));
                }
                Ok(BoundedAnswerStats {
                    processed_items,
                    response_bytes,
                    stopped_early,
                })
            })
        }
    }

    struct RawLimitProbe {
        row: serde_json::Value,
        observed_max_bytes: Option<u64>,
    }

    impl AssertionProviderCall for RawLimitProbe {
        fn query_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            _limits: BoundedAnswerLimits,
            _consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
            panic!("V2 execution must use the additive V2 provider seam")
        }

        fn query_v2_bounded<'a>(
            &'a mut self,
            _typeql: &'a str,
            limits: QueryV2AnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
            Box::pin(async move {
                self.observed_max_bytes = Some(limits.answer.max_bytes);
                let mut reader = BoundedAnswerReader::new(limits.answer);
                reader.accept(AnswerItem::Row(self.row.clone()), consumer)?;
                Ok(reader.stats())
            })
        }
    }

    fn binding_id(id: u16) -> BindingId {
        BindingId::new(id).expect("binding id")
    }

    fn fixture_with_output(output: PlanOutput) -> (ValidatedQuery, QueryPlan) {
        fixture_with_output_and_tail(output, Vec::new())
    }

    fn fixture_with_output_and_tail(
        output: PlanOutput,
        tail: Vec<ReadStage>,
    ) -> (ValidatedQuery, QueryPlan) {
        let person = TypeId::new(TypeKind::Entity, "person").expect("type");
        let name = AttributeId::new("name").expect("attribute");
        let facts = vec![
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SchemaFact::Type(
                TypeFact::new(TypeId::new(TypeKind::Attribute, "name").expect("type"))
                    .expect("type fact"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(name.clone()),
                ValueTypeTag::String,
            )),
            SchemaFact::Owns(OwnsFact::new(
                OwnsFactId::new(person.clone(), name.clone()).expect("owns id"),
            )),
            SchemaFact::Annotation(
                AnnotationFact::new(
                    AnnotationFactId::new(
                        AnnotationSubjectId::Owns(
                            OwnsFactId::new(person.clone(), name).expect("owns id"),
                        ),
                        AnnotationKindId::Unique,
                    ),
                    SchemaAnnotationValue::Presence,
                )
                .expect("unique annotation"),
            ),
        ];
        let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
            let byte = u64::try_from(index).expect("byte");
            let line = u32::try_from(index + 1).expect("line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("query-v2-executor-fixture").expect("document"),
                    byte,
                    byte + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("span"),
            )
        });
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-executor-scope").expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        );
        let managed = managed_schema_state(&declared, &context).expect("managed state");
        let resolved = resolve(&declared, &profile).expect("resolved schema");

        let mut pipeline = vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: false,
                    type_id: TypeId::new(TypeKind::Entity, "person").expect("type"),
                },
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::GreaterOrEqual,
                    left: QueryOperand::Binding {
                        binding: binding_id(1),
                    },
                    right: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                },
            ],
        }];
        pipeline.extend(tail);
        let plan = QueryPlan::new(
            vec![
                AssertionBinding::new(
                    binding_id(0),
                    QueryVariable::new("person").expect("variable"),
                ),
                AssertionBinding::new(binding_id(1), QueryVariable::new("name").expect("variable")),
            ],
            vec![InputColumn::new(
                InputColumnId::new(0),
                QueryVariable::new("minimum_name").expect("input name"),
                ValueTypeTag::String,
                false,
            )],
            pipeline,
            output,
            managed.managed_semantic_schema().clone(),
        )
        .expect("query plan");
        let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
        let validated =
            validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
                .expect("validated query");
        (validated, plan)
    }

    fn fixture() -> (ValidatedQuery, QueryPlan) {
        fixture_with_output(PlanOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        })
    }

    fn document_fixture() -> (ValidatedQuery, QueryPlan) {
        fixture_with_output(PlanOutput::Documents {
            fields: vec![DocumentField::new(
                QueryVariable::new("names").expect("document key"),
                DocumentSource::AttributeList {
                    attribute: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
            )],
        })
    }

    fn invocation(plan: &QueryPlan, operation: QueryOperation) -> QueryInvocation {
        QueryInvocation::new(
            plan,
            operation,
            vec![InputRow::new(vec![Some(CanonicalValue::String(
                CanonicalString::new("a").expect("canonical string"),
            ))])],
        )
        .expect("invocation")
    }

    fn person_row(iid: &str, name: &str) -> serde_json::Value {
        json!({
            "person": {"category": "entity", "label": "person", "iid": iid},
            "name": {
                "category": "attribute",
                "label": "name",
                "value": name,
                "value_type": "string"
            },
        })
    }

    #[test]
    fn result_rows_allow_input_echoes_only_for_actual_given_lowerings() {
        let (validated, _) = fixture();
        let OutputSchema::Rows(schema) = validated.output_schema() else {
            panic!("fixture has row output")
        };
        let mut row = person_row("0x1", "ada");
        row.as_object_mut()
            .expect("row object")
            .insert("minimum_name".into(), serde_json::json!("provider-forgery"));

        let error = validate_result_row(&row, &validated, schema, false)
            .expect_err("inline lowering cannot produce an echoed input column");
        assert_eq!(error.code().as_str(), "query_v2_result_column_mismatch");

        validate_result_row(&row, &validated, schema, true)
            .expect("given lowerings may echo their driver-bound input columns");
    }

    fn limits() -> QueryV2AnswerLimits {
        QueryV2AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: 100,
                max_bytes: 1 << 20,
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
            max_collection_members: 1 << 16,
        }
    }

    #[tokio::test]
    async fn schema_refined_zero_branches_are_rejected_before_provider_call() {
        let node = TypeId::new(TypeKind::Entity, "node").expect("node");
        let child = TypeId::new(TypeKind::Entity, "node-child").expect("child");
        let edge = TypeId::new(TypeKind::Relation, "directed-edge").expect("edge");
        let origin = RoleId::new(edge.label().as_str(), "origin").expect("origin");
        let destination = RoleId::new(edge.label().as_str(), "destination").expect("destination");
        let facts = vec![
            SchemaFact::Type(TypeFact::new(node.clone()).expect("node type")),
            SchemaFact::Type(TypeFact::new(child.clone()).expect("child type")),
            SchemaFact::Sub(SubFact::new(
                SubFactId::new(child, node.clone()).expect("node subtype"),
            )),
            SchemaFact::Type(TypeFact::new(edge.clone()).expect("edge type")),
            SchemaFact::Relates(
                RelatesFact::new(
                    RelatesFactId::new(edge.clone(), origin.clone()).expect("origin role"),
                    None,
                )
                .expect("origin relates"),
            ),
            SchemaFact::Relates(
                RelatesFact::new(
                    RelatesFactId::new(edge.clone(), destination.clone())
                        .expect("destination role"),
                    None,
                )
                .expect("destination relates"),
            ),
            SchemaFact::Plays(PlaysFact::new(
                PlaysFactId::new(node.clone(), origin.clone()).expect("origin plays"),
            )),
            SchemaFact::Plays(PlaysFact::new(
                PlaysFactId::new(node, destination.clone()).expect("destination plays"),
            )),
        ];
        let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
            let byte = u64::try_from(index).expect("byte");
            let line = u32::try_from(index + 1).expect("line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("query-v2-reach-limit").expect("document"),
                    byte,
                    byte + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("span"),
            )
        });
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let managed = managed_schema_state(
            &declared,
            &ManagedDeltaContext::new(
                ManagedScopeId::new("query-v2-reach-limit").expect("scope"),
                profile.clone(),
                CapabilitySet::new(),
            ),
        )
        .expect("managed state");
        let resolved = resolve(&declared, &profile).expect("resolved schema");
        let plan = QueryPlan::new_v2(
            vec![
                AssertionBinding::new(binding_id(0), QueryVariable::new("source").expect("source")),
                AssertionBinding::new(binding_id(1), QueryVariable::new("target").expect("target")),
            ],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![QueryPattern::Reachable {
                    min_depth: 0,
                    max_depth: 0,
                    relation: edge,
                    role_from: origin,
                    role_to: destination,
                    source: binding_id(0),
                    target: binding_id(1),
                }],
            }],
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            managed.managed_semantic_schema().clone(),
        )
        .expect("zero-hop plan");
        let structural_limits = StructuralLimits {
            predicate_nodes: 1,
            ..StructuralLimits::CANONICAL
        };
        let validated = validate_query_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&resolved, &managed),
            structural_limits,
        )
        .expect("schema-valid zero-hop plan");
        assert_eq!(
            validated
                .binding_domain(&binding_id(0))
                .expect("source domain")
                .type_ids()
                .len(),
            2
        );

        let invocation =
            QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
        let canonical_validated = validate_query_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&resolved, &managed),
            StructuralLimits::CANONICAL,
        )
        .expect("canonical zero-hop plan");
        let lowered = lower_validated_query(&canonical_validated, &invocation)
            .expect("schema-refined zero-hop lowering");
        assert!(
            lowered
                .typeql()
                .contains("$R0z0 isa! node; $source is $R0z0; $target is $R0z0;")
        );
        assert!(
            lowered
                .typeql()
                .contains("$R0z1 isa! node-child; $source is $R0z1; $target is $R0z1;")
        );
        assert!(!lowered.typeql().contains("$R0z isa!"));

        let mut provider = DrainProbe::rows(Vec::new());
        let error = execute_with_provider(&mut provider, &validated, &invocation, limits())
            .await
            .expect_err("two emitted identity branches exceed the one-node validation budget");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("lowering must reject before the provider seam");
        };
        assert_eq!(
            diagnostic.code().as_str(),
            "query_v2_reachable_expansion_limit"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
        assert!(provider.observed_typeql.is_empty());
    }

    #[test]
    fn provider_resource_mapping_is_specific_and_other_errors_are_redacted() {
        for code in [
            "provider_cancelled",
            "transaction_deadline_exceeded",
            "processed_item_limit",
            "response_byte_limit",
            "processed_item_counter_overflow",
            "answer_byte_counter_overflow",
            "query_v2_document_member_limit",
        ] {
            let provider_error: crate::error::OrmError = crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::ResourceLimit,
                code,
                "provider-private detail",
            )
            .into();
            let diagnostic = provider_diagnostic(
                &provider_error,
                "query_remote_provider_failed",
                "the executor provider call failed",
            );
            assert_eq!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
            assert_eq!(diagnostic.code().as_str(), code);
            assert!(!diagnostic.message().contains("provider-private"));
        }

        let unknown_resource: crate::error::OrmError = crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "future_provider_resource_code",
            "secret resource detail",
        )
        .into();
        for provider_error in [
            unknown_resource,
            crate::error::OrmError::Transaction("secret transaction detail".into()),
        ] {
            let diagnostic = provider_diagnostic(
                &provider_error,
                "query_remote_provider_failed",
                "the executor provider call failed",
            );
            assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
            assert_eq!(diagnostic.code().as_str(), "query_remote_provider_failed");
            assert_eq!(diagnostic.message(), "the executor provider call failed");
        }
    }

    #[tokio::test]
    async fn rows_count_and_exists_share_one_validated_stream() {
        let (validated, plan) = fixture();
        let mut provider = ScriptedProvider {
            documents: false,
            rows: vec![person_row("0x1", "ada"), person_row("0x2", "grace")],
        };
        let outcome = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            limits(),
        )
        .await
        .expect("rows outcome");
        let QueryV2Outcome::Rows(rows) = outcome else {
            panic!("rows operation returns rows");
        };
        assert_eq!(rows.len(), 2);
        let QueryRowValue::Thing { type_id, iid } = &rows[0].values()[0] else {
            panic!("first column is the person reference");
        };
        assert_eq!(type_id.label().as_str(), "person");
        assert_eq!(iid, "0x1");
        let QueryRowValue::Attribute { value, .. } = &rows[1].values()[1] else {
            panic!("second column is the name attribute");
        };
        assert_eq!(
            value,
            &CanonicalValue::String(CanonicalString::new("grace").expect("canonical string")),
        );

        let outcome = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Count),
            limits(),
        )
        .await
        .expect("count outcome");
        assert_eq!(outcome, QueryV2Outcome::Count(2));

        let outcome = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Exists),
            limits(),
        )
        .await
        .expect("exists outcome");
        assert_eq!(outcome, QueryV2Outcome::Exists(true));

        let mut empty = ScriptedProvider {
            documents: false,
            rows: Vec::new(),
        };
        let outcome = execute_with_provider(
            &mut empty,
            &validated,
            &invocation(&plan, QueryOperation::Exists),
            limits(),
        )
        .await
        .expect("empty exists outcome");
        assert_eq!(outcome, QueryV2Outcome::Exists(false));
    }

    #[tokio::test]
    async fn row_item_budget_uses_n_plus_one_sentinel_and_drains_to_eof() {
        let (validated, plan) = fixture();
        let mut provider =
            DrainProbe::rows(vec![person_row("0x1", "ada"), person_row("0x2", "grace")]);
        let mut bounded = limits();
        bounded.answer.max_items = 1;
        let expected_max_bytes = bounded.answer.max_bytes;
        let expected_max_collection_members = bounded.max_collection_members;

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            bounded,
        )
        .await
        .expect_err("the sentinel row proves the semantic item budget was exceeded");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("the released provider resource-error surface is preserved");
        };
        assert_eq!(
            error.category(),
            crate::match_request::MatchErrorCategory::ResourceLimit
        );
        assert_eq!(error.code().as_str(), "processed_item_limit");
        assert_eq!(
            error.path().segments(),
            &[crate::match_request::MatchErrorPathSegment::ProviderEvidence]
        );
        assert!(provider.observed_typeql.ends_with("limit 2;\n"));
        assert_eq!(
            provider.observed_limits,
            Some((2, expected_max_bytes, expected_max_collection_members,))
        );
        assert_eq!(
            provider.controls,
            vec![
                crate::session::backend::AnswerControl::Continue,
                crate::session::backend::AnswerControl::Continue,
            ]
        );
    }

    #[tokio::test]
    async fn deferred_budget_failure_precedes_a_later_provider_drain_failure() {
        let (validated, plan) = fixture();
        let mut provider =
            DrainProbe::rows(vec![person_row("0x1", "ada"), person_row("0x2", "grace")]);
        provider.fail_after_rows = true;
        let mut bounded = limits();
        bounded.answer.max_items = 1;

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            bounded,
        )
        .await
        .expect_err("the first semantic failure wins over a later drain failure");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("the item-budget resource error must not be overwritten");
        };
        assert_eq!(error.code().as_str(), "processed_item_limit");
        assert_eq!(
            provider.controls,
            vec![
                crate::session::backend::AnswerControl::Continue,
                crate::session::backend::AnswerControl::Continue,
            ]
        );
    }

    #[tokio::test]
    async fn document_byte_budget_drains_to_eof_before_preserving_resource_error() {
        let (validated, plan) = document_fixture();
        let document = json!({"names": ["ada"]});
        let encoded_bytes = u64::try_from(
            serde_json::to_vec(&document)
                .expect("JSON value encodes")
                .len(),
        )
        .expect("encoded test value length fits u64");
        let mut provider = DrainProbe::documents(vec![document]);
        let mut bounded = limits();
        bounded.answer.max_bytes = encoded_bytes - 1;
        bounded.max_collection_members = 1;

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            bounded,
        )
        .await
        .expect_err("the first document exceeds the semantic byte budget");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("the released provider resource-error surface is preserved");
        };
        assert_eq!(error.code().as_str(), "response_byte_limit");
        assert!(provider.observed_typeql.contains("limit 101;\nfetch {\n"));
        assert!(provider.observed_typeql.contains("        limit 2;\n"));
        assert_eq!(provider.observed_limits, Some((101, encoded_bytes - 1, 1)));
        assert_eq!(
            provider.controls,
            vec![crate::session::backend::AnswerControl::Continue]
        );
    }

    #[tokio::test]
    async fn raw_provider_bytes_stay_finite_when_projected_output_would_be_small() {
        let (validated, plan) = fixture();
        let mut row = person_row("0x1", "ada");
        row.as_object_mut().expect("row object").insert(
            "ignored-provider-overhead".to_owned(),
            json!("x".repeat(4096)),
        );
        let mut provider = RawLimitProbe {
            row,
            observed_max_bytes: None,
        };
        let mut bounded = limits();
        bounded.answer.max_bytes = 512;

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            bounded,
        )
        .await
        .expect_err("raw ignored provider data must still meet the finite provider ceiling");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("provider byte ceiling retains the stable resource surface");
        };
        assert_eq!(error.code().as_str(), "response_byte_limit");
        assert_eq!(provider.observed_max_bytes, Some(512));
    }

    #[test]
    fn exists_lowering_owns_a_final_semantic_limit_before_document_output() {
        let (validated, plan) = fixture();
        let lowered = lower_validated_query(&validated, &invocation(&plan, QueryOperation::Exists))
            .expect("exists lowering");
        assert!(lowered.typeql().ends_with("limit 1;\n"));

        let (limited, limited_plan) = fixture_with_output_and_tail(
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            vec![
                ReadStage::Sort {
                    terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
                },
                ReadStage::Limit { rows: 7 },
            ],
        );
        let lowered =
            lower_validated_query(&limited, &invocation(&limited_plan, QueryOperation::Exists))
                .expect("limited exists lowering");
        assert!(lowered.typeql().ends_with("limit 1;\n"));

        let (zero, zero_plan) = fixture_with_output_and_tail(
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            vec![
                ReadStage::Sort {
                    terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
                },
                ReadStage::Limit { rows: 0 },
            ],
        );
        let lowered = lower_validated_query(&zero, &invocation(&zero_plan, QueryOperation::Exists))
            .expect("zero-limit exists lowering");
        assert!(lowered.typeql().ends_with("limit 0;\n"));

        let (documents, document_plan) = document_fixture();
        let lowered = lower_validated_query(
            &documents,
            &invocation(&document_plan, QueryOperation::Exists),
        )
        .expect("document exists lowering");
        assert!(lowered.typeql().contains("limit 1;\nfetch {\n"));
    }

    #[test]
    fn execution_item_sentinel_is_additive_and_clamps_existing_limits() {
        let (validated, plan) = fixture();
        let public = lower_validated_query(&validated, &invocation(&plan, QueryOperation::Rows))
            .expect("public lowering");
        assert!(!public.typeql().contains("\nlimit "));

        let bounded = lower_validated_query_with_execution_limits(
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            None,
            Some(2),
        )
        .expect("execution lowering");
        assert!(bounded.typeql().ends_with("limit 2;\n"));

        let (limited, limited_plan) = fixture_with_output_and_tail(
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            vec![
                ReadStage::Sort {
                    terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
                },
                ReadStage::Limit { rows: 7 },
            ],
        );
        let bounded = lower_validated_query_with_execution_limits(
            &limited,
            &invocation(&limited_plan, QueryOperation::Rows),
            None,
            Some(2),
        )
        .expect("clamped execution lowering");
        assert!(bounded.typeql().ends_with("limit 2;\n"));

        let (documents, document_plan) = document_fixture();
        let bounded = lower_validated_query_with_execution_limits(
            &documents,
            &invocation(&document_plan, QueryOperation::Rows),
            Some(3),
            Some(2),
        )
        .expect("bounded document lowering");
        assert!(bounded.typeql().contains("limit 2;\nfetch {\n"));
        assert!(bounded.typeql().contains("        limit 4;\n"));
    }

    #[tokio::test]
    async fn exists_continues_to_provider_eof_and_rejects_false_exhaustion_proof() {
        struct ExhaustionProbe {
            row: serde_json::Value,
            observed_typeql: String,
            control: Option<crate::session::backend::AnswerControl>,
            forge_early_stop: bool,
        }

        impl AssertionProviderCall for ExhaustionProbe {
            fn query_bounded<'a>(
                &'a mut self,
                typeql: &'a str,
                _limits: BoundedAnswerLimits,
                consumer: &'a mut dyn AnswerConsumer,
            ) -> BoxFuture<'a, Result<BoundedAnswerStats, crate::error::OrmError>> {
                Box::pin(async move {
                    self.observed_typeql = typeql.to_owned();
                    let control = consumer.accept(AnswerItem::Row(self.row.clone()))?;
                    self.control = Some(control);
                    Ok(BoundedAnswerStats {
                        processed_items: 1,
                        response_bytes: 0,
                        stopped_early: self.forge_early_stop
                            || control == crate::session::backend::AnswerControl::Stop,
                    })
                })
            }
        }

        let (validated, plan) = fixture();
        let invocation = invocation(&plan, QueryOperation::Exists);
        let mut provider = ExhaustionProbe {
            row: person_row("0x1", "ada"),
            observed_typeql: String::new(),
            control: None,
            forge_early_stop: false,
        };
        let outcome = execute_with_provider(&mut provider, &validated, &invocation, limits())
            .await
            .expect("bounded exists reaches EOF");
        assert_eq!(outcome, QueryV2Outcome::Exists(true));
        assert!(provider.observed_typeql.ends_with("limit 1;\n"));
        assert_eq!(
            provider.control,
            Some(crate::session::backend::AnswerControl::Continue)
        );

        provider.forge_early_stop = true;
        let error = execute_with_provider(&mut provider, &validated, &invocation, limits())
            .await
            .expect_err("early-stop stats cannot prove bounded existence");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("false exhaustion proof is a validation failure")
        };
        assert_eq!(
            diagnostic.code().as_str(),
            "query_v2_provider_stream_not_exhausted"
        );
    }

    #[tokio::test]
    async fn zero_item_exists_distinguishes_empty_from_first_over_budget_row() {
        let (validated, plan) = fixture();
        let mut zero = limits();
        zero.answer.max_items = 0;

        let mut empty = DrainProbe::rows(Vec::new());
        let outcome = execute_with_provider(
            &mut empty,
            &validated,
            &invocation(&plan, QueryOperation::Exists),
            zero.clone(),
        )
        .await
        .expect("an empty result satisfies a zero-item budget");
        assert_eq!(outcome, QueryV2Outcome::Exists(false));
        assert!(empty.observed_typeql.ends_with("limit 1;\n"));

        let (limited, limited_plan) = fixture_with_output_and_tail(
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            vec![
                ReadStage::Sort {
                    terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
                },
                ReadStage::Limit { rows: 0 },
            ],
        );
        let mut no_rows = DrainProbe::rows(Vec::new());
        let outcome = execute_with_provider(
            &mut no_rows,
            &limited,
            &invocation(&limited_plan, QueryOperation::Exists),
            zero.clone(),
        )
        .await
        .expect("an explicit zero limit proves false without an answer item");
        assert_eq!(outcome, QueryV2Outcome::Exists(false));
        assert!(no_rows.observed_typeql.ends_with("limit 0;\n"));

        let mut matching = DrainProbe::rows(vec![person_row("0x1", "ada")]);
        let error = execute_with_provider(
            &mut matching,
            &validated,
            &invocation(&plan, QueryOperation::Exists),
            zero,
        )
        .await
        .expect_err("the first matching row exceeds a zero-item budget");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("the stable provider item-limit surface is preserved");
        };
        assert_eq!(error.code().as_str(), "processed_item_limit");
        assert_eq!(
            matching.controls,
            vec![crate::session::backend::AnswerControl::Continue]
        );
    }

    #[tokio::test]
    async fn malformed_row_evidence_records_first_failure_and_still_drains() {
        let (validated, plan) = fixture();
        let malformed = json!({
            "person": {"category": "entity", "label": "company", "iid": "0x1"},
            "name": {
                "category": "attribute",
                "label": "name",
                "value": "ada",
                "value_type": "string"
            },
        });
        let mut provider = DrainProbe::rows(vec![malformed, person_row("0x2", "grace")]);

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            limits(),
        )
        .await
        .expect_err("foreign evidence must fail after the finite stream is exhausted");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("forged evidence remains a validation failure");
        };
        assert_eq!(diagnostic.code().as_str(), "query_v2_result_type_mismatch");
        assert_eq!(
            provider.controls,
            vec![
                crate::session::backend::AnswerControl::Continue,
                crate::session::backend::AnswerControl::Continue,
            ]
        );
    }

    #[test]
    fn local_row_evidence_rejects_noncanonical_thing_iids() {
        let (validated, _) = fixture();
        let OutputSchema::Rows(schema) = validated.output_schema() else {
            panic!("row fixture derives a row schema");
        };
        let oversized = format!(
            "0x{}",
            "a".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS + 1)
        );
        for malformed in ["0x1; delete $x;", oversized.as_str()] {
            let error =
                validate_result_row(&person_row(malformed, "ada"), &validated, schema, false)
                    .expect_err("malformed Thing IID cannot construct a local typed row");
            assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
        }
    }

    #[test]
    fn provider_concept_field_allowlists_cover_every_category_variant() {
        let binding = QueryVariable::new("result").expect("binding");

        for category in ["entity", "Entity", "relation", "Relation"] {
            for (field, value) in [
                ("value", json!("smuggled")),
                ("value_type", json!("string")),
            ] {
                let mut concept = json!({
                    "category": category,
                    "iid": "0x01",
                    "label": "person"
                })
                .as_object()
                .expect("concept object")
                .clone();
                concept.insert(field.to_owned(), value);
                let error = reject_unexpected_provider_concept_fields(&concept, &binding, category)
                    .expect_err("thing concepts must reject scalar-only fields");
                assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
            }
        }

        for category in ["value", "Value"] {
            let mut concept = json!({
                "category": category,
                "label": "string",
                "value": "ada",
                "value_type": "string"
            })
            .as_object()
            .expect("concept object")
            .clone();
            reject_unexpected_provider_concept_fields(&concept, &binding, category)
                .expect("the provider's value label is legitimate evidence");
            concept.insert("iid".to_owned(), json!("0x01"));
            let error = reject_unexpected_provider_concept_fields(&concept, &binding, category)
                .expect_err("value concepts must not smuggle thing identity");
            assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
        }

        for category in ["attribute", "Attribute"] {
            let mut concept = json!({
                "category": category,
                "iid": "0x01",
                "label": "name",
                "value": "ada",
                "value_type": "string"
            })
            .as_object()
            .expect("concept object")
            .clone();
            reject_unexpected_provider_concept_fields(&concept, &binding, category)
                .expect("attribute identity is an optional provider field");
            concept.insert("entity_type".to_owned(), json!("person"));
            let error = reject_unexpected_provider_concept_fields(&concept, &binding, category)
                .expect_err("attributes must reject fields outside their exact allowlist");
            assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
        }
    }

    #[test]
    fn local_row_validation_rejects_entity_scalar_fields_and_checks_attribute_iid() {
        let (validated, _) = fixture();
        let OutputSchema::Rows(schema) = validated.output_schema() else {
            panic!("row fixture derives a row schema");
        };

        for (field, value) in [
            ("value", json!("smuggled")),
            ("value_type", json!("string")),
        ] {
            let mut row = person_row("0x01", "ada");
            row["person"]
                .as_object_mut()
                .expect("person concept")
                .insert(field.to_owned(), value);
            let error = validate_result_row(&row, &validated, schema, false)
                .expect_err("entity scalar fields must fail closed");
            assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
        }

        let mut identified_attribute = person_row("0x01", "ada");
        identified_attribute["name"]
            .as_object_mut()
            .expect("attribute concept")
            .insert("iid".to_owned(), json!("0x02"));
        validate_result_row(&identified_attribute, &validated, schema, false)
            .expect("a canonical optional attribute IID remains admissible");

        identified_attribute["name"]
            .as_object_mut()
            .expect("attribute concept")
            .insert("iid".to_owned(), json!("not-an-iid"));
        let error = validate_result_row(&identified_attribute, &validated, schema, false)
            .expect_err("a present attribute IID must itself be valid evidence");
        assert_eq!(error.code().as_str(), "query_v2_result_concept_malformed");
    }

    #[tokio::test]
    async fn every_operation_rejects_a_forged_early_stop_stat() {
        let (validated, plan) = fixture();
        let mut provider = DrainProbe::rows(vec![person_row("0x1", "ada")]);
        provider.forge_early_stop = true;

        let error = execute_with_provider(
            &mut provider,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            limits(),
        )
        .await
        .expect_err("an unterminated rows stream is not complete evidence");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("false exhaustion proof is a validation failure");
        };
        assert_eq!(
            diagnostic.code().as_str(),
            "query_v2_provider_stream_not_exhausted"
        );
        assert_eq!(
            provider.controls,
            vec![crate::session::backend::AnswerControl::Continue]
        );
    }

    #[tokio::test]
    async fn forged_and_malformed_provider_rows_fail_closed() {
        let (validated, plan) = fixture();
        let mut forged = ScriptedProvider {
            documents: false,
            rows: vec![json!({
                "person": {"category": "entity", "label": "company", "iid": "0x1"},
                "name": {
                    "category": "attribute",
                    "label": "name",
                    "value": "ada",
                    "value_type": "string"
                },
            })],
        };
        let error = execute_with_provider(
            &mut forged,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            limits(),
        )
        .await
        .expect_err("a foreign concept type is not evidence");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("forged evidence must surface as validation failure");
        };
        assert_eq!(diagnostic.code().as_str(), "query_v2_result_type_mismatch");

        let mut sparse = ScriptedProvider {
            documents: false,
            rows: vec![json!({
                "person": {"category": "entity", "label": "person", "iid": "0x1"},
            })],
        };
        let error = execute_with_provider(
            &mut sparse,
            &validated,
            &invocation(&plan, QueryOperation::Rows),
            limits(),
        )
        .await
        .expect_err("a sparse row is not evidence");
        let QueryV2ExecutionError::Validation(diagnostic) = error else {
            panic!("sparse evidence must surface as validation failure");
        };
        assert_eq!(
            diagnostic.code().as_str(),
            "query_v2_result_column_mismatch"
        );
    }

    #[tokio::test]
    async fn document_lists_are_rejected_before_member_materialization() {
        let (validated, plan) = document_fixture();
        let invocation = invocation(&plan, QueryOperation::Rows);
        let zero_limit =
            lower_validated_query_with_execution_limits(&validated, &invocation, Some(0), None)
                .expect("bounded document lowering");
        assert!(
            zero_limit
                .typeql()
                .contains("match $person has name $FtbDocumentValue0;\n        limit 1;\n        return { $FtbDocumentValue0 };")
        );
        let canonical_limit = lower_validated_query_with_execution_limits(
            &validated,
            &invocation,
            Some(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).expect("canonical limit")),
            None,
        )
        .expect("canonical document lowering");
        assert!(
            canonical_limit
                .typeql()
                .contains(&format!("limit {};", MAX_CANONICAL_COLLECTION_LEN + 1)),
        );

        let mut provider = DrainProbe::documents(vec![json!({"names": ["ada", "grace"]})]);
        provider.fail_after_rows = true;
        let mut bounded = limits();
        bounded.max_collection_members = 1;
        let error = execute_with_provider(&mut provider, &validated, &invocation, bounded)
            .await
            .expect_err("aggregate list members exceed the caller ceiling");
        let QueryV2ExecutionError::Provider(crate::error::OrmError::Match(error)) = error else {
            panic!("member budget rejection preserves the bounded-provider surface");
        };
        assert_eq!(
            error.category(),
            crate::match_request::MatchErrorCategory::ResourceLimit
        );
        assert_eq!(error.code().as_str(), "query_v2_document_member_limit");
        assert_eq!(
            error.message(),
            "document lists exceed the aggregate member ceiling"
        );
        assert_eq!(
            error.path().segments(),
            &[crate::match_request::MatchErrorPathSegment::ProviderEvidence]
        );
        assert_eq!(
            provider.controls,
            vec![crate::session::backend::AnswerControl::Continue]
        );
    }
}
