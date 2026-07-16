//! Deterministic TypeQL lowering for validated V2 query plans.
//!
//! One validated plan plus one bound invocation lowers to exact TypeQL text.
//! The first revision ships the proven single-row inline transport: input
//! operands substitute their canonical row literal directly into the pattern
//! text. Explicit multi-row batches reject before any data I/O until the
//! native `given` transport capability is proven end to end — rejection,
//! never silent row-by-row emulation.

use std::fmt::Write as _;

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    InputRow, QueryInvocation, QueryOperand, QueryOperation, QueryOutput,
    QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_query::{RowSchema, ValidatedQuery};

use crate::migration_assertion::{render_comparator, render_literal};

/// Exact provider text and typed shape for one lowered invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoweredQuery {
    operation: QueryOperation,
    row_schema: RowSchema,
    typeql: String,
}

impl LoweredQuery {
    /// Return the deterministic provider query text.
    #[must_use]
    pub fn typeql(&self) -> &str {
        &self.typeql
    }

    /// Return the validated output row shape.
    #[must_use]
    pub const fn row_schema(&self) -> &RowSchema {
        &self.row_schema
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
    let plan = validated.plan();
    if !invocation.binds(plan)? {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_invocation_plan_mismatch",
            "invocation does not bind the exact validated plan fingerprint",
        ));
    }
    let row = match invocation.inputs() {
        [] => None,
        [row] => Some(row),
        _ => {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_v2_multi_row_given_unsupported",
                "explicit multi-row input requires the native given transport capability",
            ));
        }
    };
    if let Some(row) = row
        && row.values().iter().any(Option::is_none)
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_missing_input_value",
            "single-row inline lowering requires every input value to be present",
        ));
    }

    let mut typeql = String::from("match\n");
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_match_not_first",
            "a validated plan always opens with its match stage",
        ));
    };
    render_patterns(&mut typeql, plan, patterns, row, 0)?;

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
                writeln!(typeql, "offset {rows};")
                    .expect("writing to String cannot fail");
            }
            ReadStage::Limit { rows } => {
                writeln!(typeql, "limit {rows};")
                    .expect("writing to String cannot fail");
            }
        }
    }

    Ok(LoweredQuery {
        operation: invocation.operation(),
        row_schema: validated.row_schema().clone(),
        typeql,
    })
}

fn render_patterns(
    output: &mut String,
    plan: &QueryPlan,
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    depth: usize,
) -> Result<(), Diagnostic> {
    let indent = "    ".repeat(depth);
    for pattern in patterns {
        match pattern {
            QueryPattern::Isa {
                binding,
                include_subtypes,
                type_id,
            } => {
                writeln!(
                    output,
                    "{indent}${} isa{} {};",
                    variable(plan, *binding)?,
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
                    variable(plan, *owner)?,
                    attribute_id.label(),
                    variable(plan, *attribute)?,
                )
                .expect("writing to String cannot fail");
                writeln!(
                    output,
                    "{indent}${} isa! {};",
                    variable(plan, *attribute)?,
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
                    variable(plan, *relation)?,
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
                        variable(plan, player.player())?,
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
                    render_operand(plan, left, row)?,
                    render_comparator(*comparator),
                    render_operand(plan, right, row)?,
                )
                .expect("writing to String cannot fail");
            }
            QueryPattern::Not { patterns } => {
                writeln!(output, "{indent}not {{")
                    .expect("writing to String cannot fail");
                render_patterns(output, plan, patterns, row, depth + 1)?;
                writeln!(output, "{indent}}};")
                    .expect("writing to String cannot fail");
            }
        }
    }
    Ok(())
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
    operand: &QueryOperand,
    row: Option<&InputRow>,
) -> Result<String, Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => {
            Ok(format!("${}", variable(plan, *binding)?))
        }
        QueryOperand::Literal { value } => Ok(render_literal(value)),
        QueryOperand::Input { column } => {
            let value = row
                .and_then(|row| row.values().get(usize::from(column.get())))
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "query_v2_missing_input_value",
                        "pattern reads an input column absent from the bound row",
                    )
                })?;
            Ok(render_literal(value))
        }
    }
}

fn variable(plan: &QueryPlan, binding: BindingId) -> Result<&str, Diagnostic> {
    plan.bindings()
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

fn failure(
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

/// Return the output projection columns of one validated plan.
///
/// The provider `select` keeps the plan's visible environment; the exact
/// projection to output columns is enforced in Rust during result
/// validation, mirroring the released V1 executor discipline.
#[must_use]
pub fn output_columns(plan: &QueryPlan) -> &[BindingId] {
    let QueryOutput::Rows { columns } = plan.output();
    columns
}
