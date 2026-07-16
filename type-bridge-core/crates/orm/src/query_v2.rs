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
use type_bridge_query::{
    DocumentColumnShape, DocumentSchema, OutputSchema, RowSchema, ValidatedQuery,
};

use crate::migration_assertion::{render_comparator, render_literal};

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
    let plan = validated.plan();
    if !invocation.binds(plan)? {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_v2_invocation_plan_mismatch",
            "invocation does not bind the exact validated plan fingerprint",
        ));
    }
    let (row, given) = match invocation.inputs() {
        [] => (None, None),
        [row] => (Some(row), None),
        rows => (None, Some(given_rows_spec(plan, rows)?)),
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
                local_variable(function.bindings(), BindingId::new(
                    u16::try_from(index).expect("dense parameter ordinal"),
                )?)?,
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
            ReadStage::Reduce { assignments, groups } => {
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
                writeln!(typeql, "offset {rows};")
                    .expect("writing to String cannot fail");
            }
            ReadStage::Limit { rows } => {
                writeln!(typeql, "limit {rows};")
                    .expect("writing to String cannot fail");
            }
        }
    }

    if let type_bridge_contract::query_plan::QueryOutput::Documents { fields } =
        plan.output()
    {
        typeql.push_str("fetch {\n");
        for (index, field) in fields.iter().enumerate() {
            write!(typeql, "    \"{}\": ", field.key().as_str())
                .expect("writing to String cannot fail");
            match field.source() {
                type_bridge_contract::query_plan::DocumentSource::Binding {
                    binding,
                } => {
                    write!(typeql, "${}", variable(plan, *binding)?)
                        .expect("writing to String cannot fail");
                }
                type_bridge_contract::query_plan::DocumentSource::AttributeList {
                    attribute,
                    owner,
                } => {
                    write!(
                        typeql,
                        "[ ${}.{} ]",
                        variable(plan, *owner)?,
                        attribute.label(),
                    )
                    .expect("writing to String cannot fail");
                }
            }
            typeql.push_str(if index + 1 == fields.len() { "\n" } else { ",\n" });
        }
        typeql.push_str("};\n");
    }

    Ok(LoweredQuery {
        given,
        operation: invocation.operation(),
        output_schema: validated.output_schema().clone(),
        typeql,
    })
}

/// Build the driver-transported batch for one multi-row invocation.
///
/// The first given vocabulary transports boolean, integer, double, and
/// string values; temporal, decimal, and duration inputs keep the proven
/// single-row inline path until their canonical driver spelling is proven.
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
                .map(|value| {
                    value.as_ref().and_then(given_value).ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_v2_missing_input_value",
                            "given-row lowering requires every input value to be present",
                        )
                    })
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
        _ => None,
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
        CanonicalValue::String(value) => {
            Some(GivenValue::String(value.as_str().to_owned()))
        }
        _ => None,
    }
}

fn render_patterns(
    output: &mut String,
    plan: &QueryPlan,
    patterns: &[QueryPattern],
    row: Option<&InputRow>,
    depth: usize,
) -> Result<(), Diagnostic> {
    render_scoped_patterns(output, plan, plan.bindings(), patterns, row, depth)
}

fn render_scoped_patterns(
    output: &mut String,
    plan: &QueryPlan,
    bindings: &[type_bridge_contract::migration_assertion::AssertionBinding],
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
            QueryPattern::Not { patterns } => {
                writeln!(output, "{indent}not {{")
                    .expect("writing to String cannot fail");
                render_scoped_patterns(output, plan, bindings, patterns, row, depth + 1)?;
                writeln!(output, "{indent}}};")
                    .expect("writing to String cannot fail");
            }
            QueryPattern::Try { patterns } => {
                writeln!(output, "{indent}try {{")
                    .expect("writing to String cannot fail");
                render_scoped_patterns(output, plan, bindings, patterns, row, depth + 1)?;
                writeln!(output, "{indent}}};")
                    .expect("writing to String cannot fail");
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
        QueryOperand::Literal { value } => Ok(render_literal(value)),
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
                Ok(render_literal(value))
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
    limits: crate::session::backend::BoundedAnswerLimits,
) -> Result<QueryV2Outcome, QueryV2ExecutionError> {
    let transaction = transaction
        .provider_mut()
        .map_err(QueryV2ExecutionError::Provider)?;
    let mut provider =
        crate::migration_assertion::TransactionAssertionProvider { transaction };
    execute_with_provider(&mut provider, validated, invocation, limits).await
}

pub(crate) async fn execute_with_provider<
    P: crate::migration_assertion::AssertionProviderCall + ?Sized,
>(
    provider: &mut P,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: crate::session::backend::BoundedAnswerLimits,
) -> Result<QueryV2Outcome, QueryV2ExecutionError> {
    let lowered = lower_validated_query(validated, invocation)
        .map_err(QueryV2ExecutionError::Validation)?;

    let exists_probe = matches!(lowered.operation(), QueryOperation::Exists);
    let mut limits = limits;
    if exists_probe {
        limits.max_items = 1;
    }

    let mut rows: Vec<QueryResultRow> = Vec::new();
    let mut documents: Vec<QueryResultDocument> = Vec::new();
    let mut validation: Option<Diagnostic> = None;
    let mut consumer = |item| {
        let validated_item = match (validated.output_schema(), item) {
            (
                OutputSchema::Rows(schema),
                crate::session::backend::AnswerItem::Row(row),
            ) => validate_result_row(&row, validated, schema)
                .map(|values| rows.push(QueryResultRow { values })),
            (
                OutputSchema::Documents(schema),
                crate::session::backend::AnswerItem::Document(document),
            ) => validate_result_document(&document, schema)
                .map(|values| documents.push(QueryResultDocument { values })),
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
            Ok(()) => Ok(if exists_probe {
                crate::session::backend::AnswerControl::Stop
            } else {
                crate::session::backend::AnswerControl::Continue
            }),
            Err(diagnostic) => {
                validation = Some(diagnostic);
                Ok(crate::session::backend::AnswerControl::Stop)
            }
        }
    };
    match lowered.given_rows() {
        Some(spec) => {
            if !provider.supports_given_rows() {
                return Err(QueryV2ExecutionError::Validation(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_v2_multi_row_given_unsupported",
                    "explicit multi-row input requires the native given transport capability",
                )));
            }
            provider
                .query_with_rows_bounded(
                    lowered.typeql(),
                    spec.clone(),
                    limits,
                    &mut consumer,
                )
                .await
                .map_err(QueryV2ExecutionError::Provider)?;
        }
        None => {
            provider
                .query_bounded(lowered.typeql(), limits, &mut consumer)
                .await
                .map_err(QueryV2ExecutionError::Provider)?;
        }
    }
    drop(consumer);
    if let Some(diagnostic) = validation {
        return Err(QueryV2ExecutionError::Validation(diagnostic));
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
                    DocumentFieldValue::Scalar(
                        crate::migration_assertion::parse_provider_value(
                            value,
                            *value_type,
                        )?,
                    )
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
                DocumentFieldValue::List(
                    elements
                        .iter()
                        .map(|element| {
                            crate::migration_assertion::parse_provider_value(
                                element,
                                *element_type,
                            )
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
) -> Result<Vec<QueryRowValue>, Diagnostic> {
    use type_bridge_contract::id::{TypeId, TypeKind};

    let object = row.as_object().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_result_row_malformed",
            "provider row must be a JSON object keyed by selected variables",
        )
    })?;
    let visible = visible_variables(validated.plan());
    // Given lowerings echo the driver-bound input variables in every row;
    // input names are contract-unique against binding names, so tolerating
    // exactly them stays closed.
    let inputs: std::collections::BTreeSet<&str> = validated
        .plan()
        .inputs()
        .iter()
        .map(|column| column.public_name().as_str())
        .collect();
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
        // value concept: no type label, only the typed scalar itself.
        if domain.type_ids().is_empty() {
            let Some(expected) = domain.value_type() else {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "output column has neither a thing domain nor a scalar domain",
                ));
            };
            let category = crate::migration_assertion::string_field(
                concept,
                "category",
                column.variable(),
            )?;
            if !matches!(category, "value" | "Value") {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "query_v2_result_type_mismatch",
                    "value column evidence is not a provider value concept",
                ));
            }
            let actual = crate::migration_assertion::string_field(
                concept,
                "value_type",
                column.variable(),
            )?;
            if crate::migration_assertion::provider_value_type(actual) != Some(expected)
            {
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
        let category = crate::migration_assertion::string_field(
            concept,
            "category",
            column.variable(),
        )?;
        let label = crate::migration_assertion::string_field(
            concept,
            "label",
            column.variable(),
        )?;
        let kind = crate::migration_assertion::provider_concept_kind(category)
            .ok_or_else(|| {
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
                let iid = crate::migration_assertion::string_field(
                    concept,
                    "iid",
                    column.variable(),
                )?;
                if iid.is_empty() {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_v2_result_concept_malformed",
                        "thing concept carries an empty instance identity",
                    ));
                }
                QueryRowValue::Thing {
                    type_id,
                    iid: iid.to_owned(),
                }
            }
            (TypeKind::Attribute, Some(expected)) => {
                let actual = crate::migration_assertion::string_field(
                    concept,
                    "value_type",
                    column.variable(),
                )?;
                if crate::migration_assertion::provider_value_type(actual)
                    != Some(expected)
                {
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

/// Return the visible variable names after select and reduce stages.
fn visible_variables(plan: &QueryPlan) -> Vec<&str> {
    // A reduce stage replaces the whole row environment with its group
    // keys and assigned results, superseding any earlier select.
    for stage in plan.pipeline() {
        if let ReadStage::Reduce { assignments, groups } = stage {
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
    plan.bindings()
        .iter()
        .map(|binding| binding.variable().as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, QueryVariable, ValueComparator,
    };
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, QueryOutput as PlanOutput,
    };
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
        SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::value::{
        CanonicalString, CanonicalValue, ValueTypeTag,
    };
    use type_bridge_query::{
        MigrationAssertionValidationContext, validate_query_plan,
    };
    use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

    use super::*;
    use crate::migration_assertion::AssertionProviderCall;
    use crate::session::backend::{
        AnswerCancellation, AnswerConsumer, AnswerItem, BoundedAnswerLimits,
        BoundedAnswerStats, BoxFuture,
    };

    struct ScriptedProvider {
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
                    if consumer.accept(AnswerItem::Row(row.clone()))?
                        == crate::session::backend::AnswerControl::Stop
                    {
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

    fn binding_id(id: u16) -> BindingId {
        BindingId::new(id).expect("binding id")
    }

    fn fixture() -> (ValidatedQuery, QueryPlan) {
        let person = TypeId::new(TypeKind::Entity, "person").expect("type");
        let name = AttributeId::new("name").expect("attribute");
        let facts = vec![
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SchemaFact::Type(
                TypeFact::new(
                    TypeId::new(TypeKind::Attribute, "name").expect("type"),
                )
                .expect("type fact"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(name.clone()),
                ValueTypeTag::String,
            )),
            SchemaFact::Owns(OwnsFact::new(
                OwnsFactId::new(person.clone(), name).expect("owns id"),
            )),
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
        let declared = DeclaredSchema::from_facts(
            FormatVersion::V1,
            CapabilitySet::new(),
            sourced,
        )
        .expect("declared schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-executor-scope").expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        );
        let managed =
            managed_schema_state(&declared, &context).expect("managed state");
        let resolved = resolve(&declared, &profile).expect("resolved schema");

        let plan = QueryPlan::new(
            vec![
                AssertionBinding::new(
                    binding_id(0),
                    QueryVariable::new("person").expect("variable"),
                ),
                AssertionBinding::new(
                    binding_id(1),
                    QueryVariable::new("name").expect("variable"),
                ),
            ],
            vec![InputColumn::new(
                InputColumnId::new(0),
                QueryVariable::new("minimum_name").expect("input name"),
                ValueTypeTag::String,
                false,
            )],
            vec![ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: false,
                        type_id: TypeId::new(TypeKind::Entity, "person")
                            .expect("type"),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::GreaterOrEqual,
                        left: QueryOperand::Binding { binding: binding_id(1) },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
                    },
                ],
            }],
            PlanOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            managed.managed_semantic_schema().clone(),
        )
        .expect("query plan");
        let validation_context =
            MigrationAssertionValidationContext::new(&resolved, &managed);
        let validated =
            validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
                .expect("validated query");
        (validated, plan)
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

    fn limits() -> BoundedAnswerLimits {
        BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 1 << 20,
            deadline: None,
            cancellation: AnswerCancellation::default(),
        }
    }

    #[tokio::test]
    async fn rows_count_and_exists_share_one_validated_stream() {
        let (validated, plan) = fixture();
        let mut provider = ScriptedProvider {
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
            &CanonicalValue::String(
                CanonicalString::new("grace").expect("canonical string")
            ),
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

        let mut empty = ScriptedProvider { rows: Vec::new() };
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
    async fn forged_and_malformed_provider_rows_fail_closed() {
        let (validated, plan) = fixture();
        let mut forged = ScriptedProvider {
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
}
