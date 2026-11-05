//! Shared deterministic TypeQL lowering for query plans.

use crate::migration_assertion::render_comparator;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    InputColumn, InputRow, LocalFunction, QueryOperand, QueryPattern,
};
use type_bridge_core_lib::__internal as tql;

pub(crate) fn lower_functions(
    inputs: &[InputColumn],
    functions: &[LocalFunction],
) -> Result<Vec<tql::FunctionDefinition>, Diagnostic> {
    let mut output = Vec::new();
    for function in functions {
        let arguments = function
            .parameters()
            .iter()
            .enumerate()
            .map(|(index, label)| {
                Ok((
                    local_variable(
                        function.bindings(),
                        BindingId::new(u16::try_from(index).expect("dense parameter ordinal"))?,
                    )?,
                    label.as_str(),
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let body = lower_scoped_patterns(inputs, function.bindings(), function.body(), None)?;
        let reducer = ast(tql::reducer(
            reducer_keyword(function.returns().reducer()),
            Some(local_variable(
                function.bindings(),
                function.returns().input(),
            )?),
        ))?;
        output.push(ast(tql::function_definition(
            function.name().label().as_str(),
            &arguments,
            function.returns().value_type(),
            vec![tql::match_stage(body)],
            reducer,
        ))?);
    }
    Ok(output)
}

/// Build the driver-transported rows for one exact `given` invocation.
///
/// The portable given vocabulary transports absence and every scalar domain
/// the active driver can carry without narrowing. Provider-domain durations
/// use exact unsigned components after preflight rejects negative or wider
/// contract values.
pub(crate) fn given_rows_spec(
    inputs: &[InputColumn],
    rows: &[type_bridge_contract::query_plan::InputRow],
) -> Result<crate::session::backend::GivenRowsSpec, Diagnostic> {
    let variables = inputs
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

pub(crate) fn ast<T>(result: Result<T, tql::TqlError>) -> Result<T, Diagnostic> {
    result.map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_v2_ast_lowering",
            "validated query cannot be represented by the TypeQL adapter",
        )
    })
}

pub(crate) fn reducer_keyword(reducer: type_bridge_contract::query_plan::Reducer) -> &'static str {
    use type_bridge_contract::query_plan::Reducer;
    match reducer {
        Reducer::Count => "count",
        Reducer::Sum => "sum",
        Reducer::Max => "max",
        Reducer::Mean => "mean",
        Reducer::Median => "median",
        Reducer::Min => "min",
        Reducer::Std => "std",
    }
}

pub(crate) fn lower_scoped_patterns(
    inputs: &[InputColumn],
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
) -> Result<Vec<tql::Pattern>, Diagnostic> {
    lower_scoped_patterns_with_values(inputs, bindings, patterns, row, None, None, None)
}

pub(crate) fn lower_scoped_patterns_with_values(
    inputs: &[InputColumn],
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    validated: Option<&type_bridge_contract::query_plan::v3::QueryPlanV3>,
    values: Option<&[InputRow]>,
    literal_policy: Option<&crate::query_plan_exec::LiteralComparisonPolicy<'_>>,
) -> Result<Vec<tql::Pattern>, Diagnostic> {
    lower_scoped_patterns_inner(
        inputs,
        bindings,
        patterns,
        row,
        validated,
        values,
        literal_policy,
        &mut PatternScope::default(),
    )
}

#[derive(Default)]
struct PatternScope {
    negative_depth: usize,
    next_iid: usize,
}

#[allow(clippy::too_many_arguments)]
fn lower_scoped_patterns_inner(
    inputs: &[InputColumn],
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    validated: Option<&type_bridge_contract::query_plan::v3::QueryPlanV3>,
    values: Option<&[InputRow]>,
    literal_policy: Option<&crate::query_plan_exec::LiteralComparisonPolicy<'_>>,
    scope: &mut PatternScope,
) -> Result<Vec<tql::Pattern>, Diagnostic> {
    let mut output = Vec::new();
    for (pattern_index, pattern) in patterns.iter().enumerate() {
        match pattern {
            QueryPattern::Iid { binding, iid } => {
                if let QueryOperand::Literal {
                    value: type_bridge_contract::value::CanonicalValue::String(iid),
                } = iid
                {
                    output.push(ast(tql::thing(
                        local_variable(bindings, *binding)?,
                        vec![ast(tql::iid(iid.as_str()))?],
                    ))?);
                } else if scope.negative_depth > 0 {
                    // TypeDB 3.12.3 drops the connection for direct iid() filters in negated disjunctions.
                    let actual = format!("PlanNestedIid{}", scope.next_iid);
                    scope.next_iid += 1;
                    output.push(ast(tql::let_value(
                        &[&actual],
                        ast(tql::function(
                            "iid",
                            vec![ast(tql::var(local_variable(bindings, *binding)?))?],
                        ))?,
                        false,
                    ))?);
                    output.push(ast(tql::compare(
                        ast(tql::var(&actual))?,
                        "==",
                        lower_operand(inputs, bindings, iid, row)?,
                    ))?);
                } else {
                    output.push(ast(tql::compare(
                        ast(tql::function(
                            "iid",
                            vec![ast(tql::var(local_variable(bindings, *binding)?))?],
                        ))?,
                        "==",
                        lower_operand(inputs, bindings, iid, row)?,
                    ))?);
                }
            }
            QueryPattern::Isa {
                binding,
                include_subtypes,
                type_id,
            } => output.push(ast(tql::thing(
                local_variable(bindings, *binding)?,
                vec![ast(tql::isa(type_id.label().as_str(), !*include_subtypes))?],
            ))?),
            QueryPattern::Has {
                attribute,
                attribute_id,
                owner,
            }
            | QueryPattern::HasType {
                attribute,
                attribute_id,
                owner,
                ..
            } => {
                let exact = !matches!(
                    pattern,
                    QueryPattern::HasType {
                        mode: type_bridge_contract::query_plan::TypeMatchMode::Isa,
                        ..
                    }
                );
                let attribute = local_variable(bindings, *attribute)?;
                output.push(ast(tql::thing(
                    local_variable(bindings, *owner)?,
                    vec![ast(tql::has(
                        attribute_id.label().as_str(),
                        ast(tql::var(attribute))?,
                    ))?],
                ))?);
                output.push(ast(tql::thing(
                    attribute,
                    vec![ast(tql::isa(attribute_id.label().as_str(), exact))?],
                ))?);
            }
            QueryPattern::Links {
                players,
                relation,
                relation_id,
            } => {
                let players = players
                    .iter()
                    .map(|player| {
                        Ok((
                            player.role().label().as_str(),
                            local_variable(bindings, player.player())?,
                        ))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                output.push(ast(tql::thing(
                    local_variable(bindings, *relation)?,
                    vec![
                        ast(tql::isa(relation_id.label().as_str(), true))?,
                        ast(tql::links(players))?,
                    ],
                ))?);
            }
            QueryPattern::Value {
                comparator,
                left,
                right,
            } if matches!(
                comparator,
                type_bridge_contract::migration_assertion::ValueComparator::StartsWith
                    | type_bridge_contract::migration_assertion::ValueComparator::EndsWith
                    | type_bridge_contract::migration_assertion::ValueComparator::Like
            ) =>
            {
                output.push(crate::query_plan_exec::string_comparison(
                    inputs,
                    bindings,
                    *comparator,
                    left,
                    right,
                    values.unwrap_or(&[]),
                    literal_policy,
                )?);
            }
            QueryPattern::Value {
                comparator,
                left,
                right,
            } => output.push(ast(tql::compare(
                lower_operand(inputs, bindings, left, row)?,
                render_comparator(*comparator),
                lower_operand(inputs, bindings, right, row)?,
            ))?),
            QueryPattern::Or { branches } => output.push(tql::or(
                branches
                    .iter()
                    .map(|branch| {
                        lower_scoped_patterns_inner(
                            inputs,
                            bindings,
                            branch,
                            row,
                            validated,
                            values,
                            literal_policy,
                            scope,
                        )
                    })
                    .collect::<Result<_, _>>()?,
            )),
            QueryPattern::Not { patterns } => {
                scope.negative_depth += 1;
                let nested = lower_scoped_patterns_inner(
                    inputs,
                    bindings,
                    patterns,
                    row,
                    validated,
                    values,
                    literal_policy,
                    scope,
                );
                scope.negative_depth -= 1;
                output.push(tql::not(nested?));
            }
            QueryPattern::Try { patterns } => {
                output.push(tql::optional(lower_scoped_patterns_inner(
                    inputs,
                    bindings,
                    patterns,
                    row,
                    validated,
                    values,
                    literal_policy,
                    scope,
                )?))
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
                let source_id = *source;
                let source = local_variable(bindings, source_id)?;
                let target = local_variable(bindings, *target)?;
                let mut branches = Vec::new();
                for length in usize::from(*min_depth)..=usize::from(*max_depth) {
                    if length == 0 {
                        // TypeDB requires an exact typed witness in each identity branch.
                        let validated = validated.ok_or_else(|| failure(DiagnosticCategory::Integrity, "query_v2_reachable_scope", "a validated reachable pattern appears outside the root query scope"))?;
                        let domain = validated
                            .binding_types(source_id)
                            .filter(|domain| !domain.is_empty())
                            .ok_or_else(|| {
                                failure(
                                    DiagnosticCategory::Integrity,
                                    "query_v2_reachable_source_domain",
                                    "a validated reachable source has no schema-derived domain",
                                )
                            })?;
                        // Distinct witness names keep mutually exclusive branches independent.
                        for (ordinal, type_id) in domain.iter().enumerate() {
                            let name = format!("R{pattern_index}z{ordinal}");
                            branches.push(vec![
                                ast(tql::thing(
                                    &name,
                                    vec![ast(tql::isa(type_id.label().as_str(), true))?],
                                ))?,
                                ast(tql::identity(source, &name))?,
                                ast(tql::identity(target, &name))?,
                            ]);
                        }
                        continue;
                    }
                    let mut branch = Vec::new();
                    let hop_var = |hop: usize| format!("R{pattern_index}l{length}h{hop}");
                    for hop in 1..=length {
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
                        branch.push(ast(tql::anonymous_relation(
                            vec![
                                (role_from.label().as_str(), &from),
                                (role_to.label().as_str(), &to),
                            ],
                            vec![ast(tql::isa(relation.label().as_str(), true))?],
                        ))?);
                    }
                    branches.push(branch);
                }
                if branches.len() == 1 {
                    output.extend(branches.pop().expect("one branch"));
                } else {
                    output.push(tql::or(branches));
                }
            }
            QueryPattern::FunctionCall {
                arguments,
                assigned,
                function,
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| lower_operand(inputs, bindings, argument, row))
                    .collect::<Result<_, _>>()?;
                output.push(ast(tql::let_value(
                    &[local_variable(bindings, *assigned)?],
                    ast(tql::function(function.label().as_str(), arguments))?,
                    false,
                ))?);
            }
        }
    }
    Ok(output)
}

pub(crate) fn lower_operand(
    inputs: &[InputColumn],
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
    operand: &QueryOperand,
    row: Option<&InputRow>,
) -> Result<tql::Expr, Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => ast(tql::var(local_variable(bindings, *binding)?)),
        QueryOperand::Literal { value } => ast(tql::literal(value)),
        QueryOperand::Input { column } => match row {
            Some(row) => ast(tql::literal(
                row.values()
                    .get(usize::from(column.get()))
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_missing_input_value",
                            "pattern reads an input column absent from the bound row",
                        )
                    })?,
            )),
            None => ast(tql::var(
                inputs
                    .get(usize::from(column.get()))
                    .ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_missing_input_value",
                            "pattern reads an undeclared input column",
                        )
                    })?
                    .public_name()
                    .as_str(),
            )),
        },
    }
}

pub(crate) fn local_variable(
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
