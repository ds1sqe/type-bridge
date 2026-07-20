//! TypeDB lowering and bounded execution for validated migration assertions.

use std::fmt;
use std::fmt::Write as _;

use serde_json::{Map, Value};
use thiserror::Error;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::{
    AssertionPattern, BindingId, MigrationAssertionPlan, MigrationAssertionPlanFingerprint,
    QueryVariable, ValueComparator, ValueOperand,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_query::{BindingDomain, RowSchema, ValidatedMigrationAssertionPlan};

use crate::error::OrmError;
use crate::session::backend::{
    AnswerCancellation, AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits,
    BoundedAnswerStats, BoxFuture, TransactionOps,
};
use crate::session::transaction::Transaction;

/// Ephemeral provider statement and its exact selected-row contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredMigrationAssertion {
    typeql: String,
    row_schema: RowSchema,
}

impl LoweredMigrationAssertion {
    /// Return deterministic TypeQL. These bytes are not canonical plan identity.
    pub fn typeql(&self) -> &str {
        &self.typeql
    }

    /// Return the row schema used to validate provider evidence.
    pub const fn row_schema(&self) -> &RowSchema {
        &self.row_schema
    }
}

/// Actual provider context checked before the first assertion query call.
#[derive(Clone, Copy, Debug)]
pub struct MigrationAssertionExecutionContext<'a> {
    available_capabilities: &'a CapabilitySet,
    source_state: &'a ManagedSchemaState,
    structural_limits: StructuralLimits,
}

impl<'a> MigrationAssertionExecutionContext<'a> {
    /// Bind live provider capabilities, source identity, and the execution limit policy.
    pub const fn new(
        source_state: &'a ManagedSchemaState,
        available_capabilities: &'a CapabilitySet,
        structural_limits: StructuralLimits,
    ) -> Self {
        Self {
            available_capabilities,
            source_state,
            structural_limits,
        }
    }
}

/// One deterministic output-column shape retained as failure evidence.
///
/// Runtime witness values are deliberately excluded: provider row order is
/// not canonical, so only validated plan shape participates in failure
/// identity and replay comparisons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionEvidenceColumn {
    binding: BindingId,
    domain: BindingDomain,
    variable: QueryVariable,
}

impl AssertionEvidenceColumn {
    /// Return the canonical plan binding.
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the selected provider variable.
    pub const fn variable(&self) -> &QueryVariable {
        &self.variable
    }

    /// Return the validated provider-neutral domain for this column.
    pub const fn domain(&self) -> &BindingDomain {
        &self.domain
    }
}

/// Deterministic identity for an observed `NoRows` assertion failure.
///
/// The selected runtime row is validation-only and is never retained here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionFailed {
    evidence: Vec<AssertionEvidenceColumn>,
    plan_fingerprint: MigrationAssertionPlanFingerprint,
}

impl AssertionFailed {
    /// Return exact plan identity without persisting provider TypeQL.
    pub const fn plan_fingerprint(&self) -> &MigrationAssertionPlanFingerprint {
        &self.plan_fingerprint
    }

    /// Return columns in canonical output-binding order.
    pub fn evidence(&self) -> &[AssertionEvidenceColumn] {
        &self.evidence
    }
}

impl fmt::Display for AssertionFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("migration assertion failed: expected no rows")
    }
}

impl std::error::Error for AssertionFailed {}

/// Stable failure boundary for preflight, lowering, provider, and result validation.
#[derive(Debug, Error)]
pub enum MigrationAssertionExecutionError {
    /// A required capability, identity, or limit policy failed before provider I/O.
    #[error("migration assertion preflight failed: {0}")]
    Preflight(Diagnostic),
    /// A trusted validated plan could not be lowered deterministically.
    #[error("migration assertion lowering failed: {0}")]
    Lowering(Diagnostic),
    /// The provider query call failed.
    #[error("migration assertion provider call failed: {0}")]
    Provider(#[source] OrmError),
    /// Provider output did not conform to the derived row schema.
    #[error("migration assertion result validation failed: {0}")]
    ResultValidation(Diagnostic),
    /// A conforming first row violated the `NoRows` expectation.
    #[error(transparent)]
    AssertionFailed(AssertionFailed),
}

impl MigrationAssertionExecutionError {
    /// Return a structured diagnostic for non-provider contract failures.
    pub const fn diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            Self::Preflight(diagnostic)
            | Self::Lowering(diagnostic)
            | Self::ResultValidation(diagnostic) => Some(diagnostic),
            Self::Provider(_) | Self::AssertionFailed(_) => None,
        }
    }
}

/// Lower every shipped assertion pattern into deterministic TypeQL with `limit 1`.
pub fn lower_migration_assertion(
    validated: &ValidatedMigrationAssertionPlan,
) -> Result<LoweredMigrationAssertion, Diagnostic> {
    let plan = validated.plan();
    if plan.outputs().is_empty() {
        return Err(assertion_diagnostic(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_no_outputs",
            "provider execution requires at least one selected evidence binding",
        ));
    }

    let mut typeql = String::from("match\n");
    render_patterns(&mut typeql, plan, plan.patterns(), 0)?;
    typeql.push_str("select ");
    for (index, output) in plan.outputs().iter().enumerate() {
        if index != 0 {
            typeql.push_str(", ");
        }
        typeql.push('$');
        typeql.push_str(variable(plan, *output)?.as_str());
    }
    typeql.push_str(";\nlimit 1;\n");

    Ok(LoweredMigrationAssertion {
        typeql,
        row_schema: validated.row_schema().clone(),
    })
}

/// Execute one validated `NoRows` assertion through the existing transaction seam.
pub async fn execute_migration_assertion(
    transaction: &mut Transaction,
    validated: &ValidatedMigrationAssertionPlan,
    context: MigrationAssertionExecutionContext<'_>,
) -> Result<(), MigrationAssertionExecutionError> {
    let transaction = transaction
        .provider_mut()
        .map_err(MigrationAssertionExecutionError::Provider)?;
    let mut provider = TransactionAssertionProvider { transaction };
    execute_with_provider(&mut provider, validated, context).await
}

pub(crate) trait AssertionProviderCall: Send {
    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>>;

    /// Whether this provider transports `given`-stage input rows.
    fn supports_given_rows(&self) -> bool {
        false
    }

    /// Execute one `given`-lowered query with driver-transported rows.
    ///
    /// Callers gate on [`Self::supports_given_rows`]; the default fails
    /// closed for providers without the transport.
    fn query_with_rows_bounded<'a>(
        &'a mut self,
        _typeql: &'a str,
        _rows: crate::session::backend::GivenRowsSpec,
        _limits: BoundedAnswerLimits,
        _consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async {
            Err(OrmError::QueryExecution(
                "given-stage parameterized queries are not supported by this provider".into(),
            ))
        })
    }
}

pub(crate) struct TransactionAssertionProvider<'a, T: ?Sized> {
    pub(crate) transaction: &'a mut T,
}

impl<T: TransactionOps + ?Sized> AssertionProviderCall for TransactionAssertionProvider<'_, T> {
    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.transaction.query_bounded(typeql, limits, consumer)
    }

    fn supports_given_rows(&self) -> bool {
        self.transaction.supports_given_rows()
    }

    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: crate::session::backend::GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.transaction
            .query_with_rows_bounded(typeql, rows, limits, consumer)
    }
}

async fn execute_with_provider<P: AssertionProviderCall + ?Sized>(
    provider: &mut P,
    validated: &ValidatedMigrationAssertionPlan,
    context: MigrationAssertionExecutionContext<'_>,
) -> Result<(), MigrationAssertionExecutionError> {
    preflight(validated, context).map_err(MigrationAssertionExecutionError::Preflight)?;
    let lowered =
        lower_migration_assertion(validated).map_err(MigrationAssertionExecutionError::Lowering)?;
    let plan_fingerprint = validated
        .plan()
        .fingerprint()
        .map_err(MigrationAssertionExecutionError::Lowering)?;

    let mut evidence = None;
    let mut consumer = |item| {
        evidence = Some(match item {
            AnswerItem::Row(row) => validate_evidence_row(&row, validated),
            AnswerItem::Document(_) => Err(assertion_diagnostic(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_result_not_row",
                "provider returned a document for a selected-row assertion",
            )),
        });
        Ok(AnswerControl::Stop)
    };
    provider
        .query_bounded(
            lowered.typeql(),
            BoundedAnswerLimits {
                max_items: 1,
                max_bytes: u64::try_from(context.structural_limits.diagnostic_bytes)
                    .unwrap_or(u64::MAX),
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
            &mut consumer,
        )
        .await
        .map_err(MigrationAssertionExecutionError::Provider)?;

    match evidence {
        None => Ok(()),
        Some(Err(diagnostic)) => Err(MigrationAssertionExecutionError::ResultValidation(
            diagnostic,
        )),
        Some(Ok(())) => {
            let evidence = assertion_evidence_shape(validated)
                .map_err(MigrationAssertionExecutionError::ResultValidation)?;
            Err(MigrationAssertionExecutionError::AssertionFailed(
                AssertionFailed {
                    evidence,
                    plan_fingerprint,
                },
            ))
        }
    }
}

fn preflight(
    validated: &ValidatedMigrationAssertionPlan,
    context: MigrationAssertionExecutionContext<'_>,
) -> Result<(), Diagnostic> {
    if validated.structural_limits() != context.structural_limits {
        return Err(assertion_diagnostic(
            DiagnosticCategory::ResourceLimit,
            "migration_assertion_structural_limits_mismatch",
            "execution must use the exact structural policy used during validation",
        ));
    }
    validated
        .plan()
        .required_capabilities()
        .ensure_supported_by(context.available_capabilities)?;
    if validated.source_state().managed_semantic_schema()
        != context.source_state.managed_semantic_schema()
        || validated.plan().managed_semantics() != context.source_state.managed_semantic_schema()
    {
        return Err(assertion_diagnostic(
            DiagnosticCategory::Integrity,
            "migration_assertion_source_managed_semantic_mismatch",
            "provider source state does not match the validated managed semantic schema",
        ));
    }
    if validated.source_state().declared_identity() != context.source_state.declared_identity() {
        return Err(assertion_diagnostic(
            DiagnosticCategory::Integrity,
            "migration_assertion_source_declared_identity_mismatch",
            "provider source state does not match the full declaration used by resolution",
        ));
    }
    Ok(())
}

fn render_patterns(
    output: &mut String,
    plan: &MigrationAssertionPlan,
    patterns: &[AssertionPattern],
    depth: usize,
) -> Result<(), Diagnostic> {
    let indent = "    ".repeat(depth);
    for pattern in patterns {
        match pattern {
            AssertionPattern::Isa {
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
            AssertionPattern::Has {
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
            AssertionPattern::Links {
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
            AssertionPattern::Value {
                comparator,
                left,
                right,
            } => {
                writeln!(
                    output,
                    "{indent}{} {} {};",
                    render_operand(plan, left)?,
                    render_comparator(*comparator),
                    render_operand(plan, right)?,
                )
                .expect("writing to String cannot fail");
            }
            AssertionPattern::Not { patterns } => {
                writeln!(output, "{indent}not {{").expect("writing to String cannot fail");
                render_patterns(output, plan, patterns, depth + 1)?;
                writeln!(output, "{indent}}};").expect("writing to String cannot fail");
            }
        }
    }
    Ok(())
}

fn variable(
    plan: &MigrationAssertionPlan,
    binding: BindingId,
) -> Result<&QueryVariable, Diagnostic> {
    plan.binding(binding)
        .map(|binding| binding.variable())
        .ok_or_else(|| {
            assertion_diagnostic(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_unknown_binding",
                "validated assertion references an unknown binding",
            )
        })
}

fn render_operand(
    plan: &MigrationAssertionPlan,
    operand: &ValueOperand,
) -> Result<String, Diagnostic> {
    match operand {
        ValueOperand::Binding { binding } => Ok(format!("${}", variable(plan, *binding)?)),
        ValueOperand::Literal { value } => Ok(render_literal(value)),
    }
}

pub(crate) const fn render_comparator(comparator: ValueComparator) -> &'static str {
    match comparator {
        ValueComparator::Equal => "==",
        ValueComparator::NotEqual => "!=",
        ValueComparator::Less => "<",
        ValueComparator::LessOrEqual => "<=",
        ValueComparator::Greater => ">",
        ValueComparator::GreaterOrEqual => ">=",
    }
}

pub(crate) fn render_literal(value: &CanonicalValue) -> String {
    match value {
        CanonicalValue::String(value) => {
            serde_json::to_string(value.as_str()).expect("serializing a string cannot fail")
        }
        CanonicalValue::Long(value) => value.to_string(),
        CanonicalValue::Double(value) => render_double(*value),
        CanonicalValue::Boolean(value) => value.to_string(),
        CanonicalValue::Date(value) => value.to_string(),
        CanonicalValue::DateTime(value) => value.to_string(),
        CanonicalValue::DateTimeTz(value) => value.to_string(),
        CanonicalValue::Decimal(value) => format!("{}dec", value.as_str()),
        CanonicalValue::Duration(value) => value.to_string(),
    }
}

fn render_double(value: CanonicalDouble) -> String {
    let value = value.get();
    let absolute = value.abs();
    if absolute >= 1e21 || absolute != 0.0 && absolute < 1e-6 {
        return format!("{value:e}");
    }
    let mut rendered = value.to_string();
    if !rendered.contains('.') && !rendered.contains('e') && !rendered.contains('E') {
        rendered.push_str(".0");
    }
    rendered
}

fn validate_evidence_row(
    row: &Value,
    validated: &ValidatedMigrationAssertionPlan,
) -> Result<(), Diagnostic> {
    let object = row.as_object().ok_or_else(|| {
        assertion_diagnostic(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_result_row_malformed",
            "provider row must be a JSON object keyed by selected variables",
        )
    })?;
    let columns = validated.row_schema().columns();
    if object.len() != columns.len()
        || columns
            .iter()
            .any(|column| !object.contains_key(column.variable().as_str()))
    {
        return Err(assertion_diagnostic(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_result_column_mismatch",
            "provider row columns do not exactly match the derived row schema",
        ));
    }

    for (column, binding) in columns.iter().zip(validated.plan().outputs()) {
        let domain = validated.binding_domain(binding).ok_or_else(|| {
            assertion_diagnostic(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_result_domain_missing",
                "derived output binding has no validated type domain",
            )
        })?;
        let concept = object
            .get(column.variable().as_str())
            .and_then(Value::as_object)
            .ok_or_else(|| malformed_concept(column.variable(), &["category", "label"]))?;
        let category = string_field(concept, "category", column.variable())?;
        let label = string_field(concept, "label", column.variable())?;
        let kind = provider_concept_kind(category).ok_or_else(|| {
            type_mismatch(
                column.variable(),
                category,
                label,
                concept.get("value_type").and_then(Value::as_str),
                domain,
            )
        })?;
        reject_unexpected_concept_fields(concept, column.variable(), kind)?;
        let type_id =
            TypeId::new(kind, label).map_err(|_| invalid_concept(column.variable(), &["label"]))?;
        if !domain.type_ids().contains(&type_id) {
            return Err(type_mismatch(
                column.variable(),
                category,
                label,
                concept.get("value_type").and_then(Value::as_str),
                domain,
            ));
        }
        match kind {
            TypeKind::Entity | TypeKind::Relation => {
                let iid = string_field(concept, "iid", column.variable())?;
                if iid.is_empty() {
                    return Err(invalid_concept(column.variable(), &["iid"]));
                }
            }
            TypeKind::Attribute => {
                if let Some(iid) = concept.get("iid") {
                    match iid.as_str() {
                        Some(iid) if !iid.is_empty() => {}
                        _ => return Err(invalid_concept(column.variable(), &["iid"])),
                    }
                }
            }
            TypeKind::Struct => {
                return Err(type_mismatch(
                    column.variable(),
                    category,
                    label,
                    concept.get("value_type").and_then(Value::as_str),
                    domain,
                ));
            }
        }
        match domain.value_type() {
            None if kind != TypeKind::Attribute => {}
            Some(expected) if kind == TypeKind::Attribute => {
                let actual = string_field(concept, "value_type", column.variable())?;
                if provider_value_type(actual) != Some(expected) {
                    return Err(type_mismatch(
                        column.variable(),
                        category,
                        label,
                        Some(actual),
                        domain,
                    ));
                }
                parse_provider_value(
                    concept
                        .get("value")
                        .ok_or_else(|| malformed_concept(column.variable(), &["value"]))?,
                    expected,
                )?;
            }
            _ => {
                return Err(type_mismatch(
                    column.variable(),
                    category,
                    label,
                    concept.get("value_type").and_then(Value::as_str),
                    domain,
                ));
            }
        }
    }
    Ok(())
}

fn assertion_evidence_shape(
    validated: &ValidatedMigrationAssertionPlan,
) -> Result<Vec<AssertionEvidenceColumn>, Diagnostic> {
    let columns = validated.row_schema().columns();
    if columns.len() != validated.plan().outputs().len() {
        return Err(assertion_diagnostic(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_result_column_mismatch",
            "derived row schema does not match the canonical output bindings",
        ));
    }
    columns
        .iter()
        .zip(validated.plan().outputs())
        .map(|(column, binding)| {
            let domain = validated.binding_domain(binding).ok_or_else(|| {
                assertion_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "migration_assertion_result_domain_missing",
                    "derived output binding has no validated type domain",
                )
            })?;
            Ok(AssertionEvidenceColumn {
                binding: *binding,
                domain: domain.clone(),
                variable: column.variable().clone(),
            })
        })
        .collect()
}

pub(crate) fn string_field<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
    binding: &QueryVariable,
) -> Result<&'a str, Diagnostic> {
    match object.get(field) {
        None => Err(malformed_concept(binding, &[field])),
        Some(value) => value
            .as_str()
            .ok_or_else(|| invalid_concept(binding, &[field])),
    }
}

fn reject_unexpected_concept_fields(
    object: &Map<String, Value>,
    binding: &QueryVariable,
    kind: TypeKind,
) -> Result<(), Diagnostic> {
    let allowed: &[&str] = match kind {
        TypeKind::Entity | TypeKind::Relation => &["category", "iid", "label"],
        TypeKind::Attribute => &["category", "iid", "label", "value", "value_type"],
        TypeKind::Struct => &[],
    };
    let mut unexpected = object
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unexpected.sort();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(assertion_diagnostic(
        DiagnosticCategory::InvalidContract,
        "migration_assertion_result_concept_malformed",
        "provider concept evidence contains unexpected typed fields",
    )
    .with_detail("binding", binding.as_str())
    .with_detail("unexpected_fields", unexpected))
}

pub(crate) fn provider_value_type(value: &str) -> Option<ValueTypeTag> {
    match value {
        "string" => Some(ValueTypeTag::String),
        "integer" => Some(ValueTypeTag::Long),
        "double" => Some(ValueTypeTag::Double),
        "boolean" => Some(ValueTypeTag::Boolean),
        "date" => Some(ValueTypeTag::Date),
        "datetime" => Some(ValueTypeTag::DateTime),
        "datetime-tz" => Some(ValueTypeTag::DateTimeTz),
        "decimal" => Some(ValueTypeTag::Decimal),
        "duration" => Some(ValueTypeTag::Duration),
        _ => None,
    }
}

pub(crate) fn provider_concept_kind(value: &str) -> Option<TypeKind> {
    match value {
        "entity" | "Entity" => Some(TypeKind::Entity),
        "relation" | "Relation" => Some(TypeKind::Relation),
        "attribute" | "Attribute" => Some(TypeKind::Attribute),
        _ => None,
    }
}

pub(crate) fn parse_provider_value(
    value: &Value,
    value_type: ValueTypeTag,
) -> Result<CanonicalValue, Diagnostic> {
    match value_type {
        ValueTypeTag::String => value
            .as_str()
            .ok_or_else(malformed_value)
            .and_then(|value| CanonicalString::new(value).map_err(|_| malformed_value()))
            .map(CanonicalValue::String),
        ValueTypeTag::Long => value
            .as_i64()
            .ok_or_else(malformed_value)
            .map(CanonicalValue::Long),
        ValueTypeTag::Double => value
            .as_f64()
            .ok_or_else(malformed_value)
            .and_then(|value| CanonicalDouble::new(value).map_err(|_| malformed_value()))
            .map(CanonicalValue::Double),
        ValueTypeTag::Boolean => value
            .as_bool()
            .ok_or_else(malformed_value)
            .map(CanonicalValue::Boolean),
        ValueTypeTag::Date => parse_text::<CanonicalDate>(value, CanonicalValue::Date),
        ValueTypeTag::DateTime => parse_text::<CanonicalDateTime>(value, CanonicalValue::DateTime),
        ValueTypeTag::DateTimeTz => {
            parse_text::<CanonicalDateTimeTz>(value, CanonicalValue::DateTimeTz)
        }
        ValueTypeTag::Decimal => value
            .as_str()
            .ok_or_else(malformed_value)
            .and_then(|value| DecimalValue::new(value).map_err(|_| malformed_value()))
            .map(CanonicalValue::Decimal),
        ValueTypeTag::Duration => parse_text::<CanonicalDuration>(value, CanonicalValue::Duration),
    }
}

fn parse_text<T>(
    value: &Value,
    wrap: impl FnOnce(T) -> CanonicalValue,
) -> Result<CanonicalValue, Diagnostic>
where
    T: std::str::FromStr,
{
    value
        .as_str()
        .and_then(|value| value.parse::<T>().ok())
        .map(wrap)
        .ok_or_else(malformed_value)
}

fn malformed_concept(binding: &QueryVariable, missing_fields: &[&str]) -> Diagnostic {
    assertion_diagnostic(
        DiagnosticCategory::InvalidContract,
        "migration_assertion_result_concept_malformed",
        "provider concept evidence is missing a required typed field",
    )
    .with_detail("binding", binding.as_str())
    .with_detail(
        "missing_fields",
        missing_fields
            .iter()
            .map(|field| (*field).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn invalid_concept(binding: &QueryVariable, invalid_fields: &[&str]) -> Diagnostic {
    assertion_diagnostic(
        DiagnosticCategory::InvalidContract,
        "migration_assertion_result_concept_malformed",
        "provider concept evidence contains an invalid typed field",
    )
    .with_detail("binding", binding.as_str())
    .with_detail(
        "invalid_fields",
        invalid_fields
            .iter()
            .map(|field| (*field).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn malformed_value() -> Diagnostic {
    assertion_diagnostic(
        DiagnosticCategory::InvalidContract,
        "migration_assertion_result_value_malformed",
        "provider attribute value is outside its declared canonical scalar domain",
    )
}

fn type_mismatch(
    binding: &QueryVariable,
    actual_category: &str,
    actual_type_label: &str,
    actual_value_type: Option<&str>,
    allowed: &BindingDomain,
) -> Diagnostic {
    let allowed_types = allowed
        .type_ids()
        .iter()
        .map(|type_id| {
            format!(
                "{}:{}",
                match type_id.kind() {
                    TypeKind::Entity => "entity",
                    TypeKind::Relation => "relation",
                    TypeKind::Attribute => "attribute",
                    TypeKind::Struct => "struct",
                },
                type_id.label(),
            )
        })
        .collect::<Vec<_>>();
    assertion_diagnostic(
        DiagnosticCategory::InvalidContract,
        "migration_assertion_result_type_mismatch",
        "provider concept type lies outside the derived row-schema domain",
    )
    .with_detail("binding", binding.as_str())
    .with_detail("actual_category", actual_category)
    .with_detail("actual_type_label", actual_type_label)
    .with_detail("actual_value_type", actual_value_type.unwrap_or("<absent>"))
    .with_detail("allowed_type_domains", allowed_types)
    .with_detail(
        "expected_value_type",
        allowed
            .value_type()
            .map(ValueTypeTag::as_str)
            .unwrap_or("<none>"),
    )
}

fn assertion_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId};
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, AssertionExpectation, AssertionRolePlayer,
    };
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact,
        RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
    use type_bridge_query::{
        MigrationAssertionValidationContext, validate_migration_assertion_plan,
    };
    use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

    use crate::session::backend::{BoundedAnswerReader, QueryResult};

    fn type_id(kind: TypeKind, label: &str) -> TypeId {
        TypeId::new(kind, label).expect("fixture type")
    }

    fn binding(id: u16, variable: &str) -> AssertionBinding {
        AssertionBinding::new(
            BindingId::new(id).expect("binding id"),
            QueryVariable::new(variable).expect("variable"),
        )
    }

    struct SchemaFixture {
        managed: ManagedSchemaState,
        resolved: type_bridge_schema::ResolvedSchema,
    }

    fn schema_fixture_with_extra_type(extra_type: bool) -> SchemaFixture {
        let person = type_id(TypeKind::Entity, "person");
        let company = type_id(TypeKind::Entity, "company");
        let name = AttributeId::new("name").expect("attribute");
        let employment = type_id(TypeKind::Relation, "employment");
        let employee = RoleId::new("employment", "employee").expect("role");
        let mut facts = vec![
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SchemaFact::Type(TypeFact::new(company).expect("type fact")),
            SchemaFact::Type(
                TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(name.clone()),
                ValueTypeTag::String,
            )),
            SchemaFact::Owns(OwnsFact::new(
                OwnsFactId::new(person.clone(), name).expect("owns id"),
            )),
            SchemaFact::Type(TypeFact::new(employment.clone()).expect("type fact")),
            SchemaFact::Relates(
                RelatesFact::new(
                    RelatesFactId::new(employment, employee.clone()).expect("relates id"),
                    None,
                )
                .expect("relates fact"),
            ),
            SchemaFact::Plays(PlaysFact::new(
                PlaysFactId::new(person, employee).expect("plays id"),
            )),
        ];
        if extra_type {
            facts.push(SchemaFact::Type(
                TypeFact::new(type_id(TypeKind::Entity, "department")).expect("extra type fact"),
            ));
        }
        let sourced = facts
            .into_iter()
            .enumerate()
            .map(|(index, fact)| {
                let byte = u64::try_from(index).expect("byte");
                let line = u32::try_from(index + 1).expect("line");
                SourcedSchemaFact::new(
                    fact,
                    SourceSpan::new(
                        DocumentId::new("assertion-execution-fixture").expect("document"),
                        byte,
                        byte + 1,
                        line,
                        1,
                        line,
                        2,
                    )
                    .expect("span"),
                )
            })
            .collect::<Vec<_>>();
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let resolved = resolve(&declared, &profile).expect("resolved schema");
        let managed = managed_schema_state(
            &declared,
            &ManagedDeltaContext::new(
                ManagedScopeId::new("assertion-execution-fixture").expect("scope"),
                profile,
                CapabilitySet::new(),
            ),
        )
        .expect("managed schema state");
        SchemaFixture { managed, resolved }
    }

    fn schema_fixture() -> SchemaFixture {
        schema_fixture_with_extra_type(false)
    }

    fn validated(fixture: &SchemaFixture) -> ValidatedMigrationAssertionPlan {
        validated_with_output_mode(fixture, false)
    }

    fn validated_all_outputs(fixture: &SchemaFixture) -> ValidatedMigrationAssertionPlan {
        validated_with_output_mode(fixture, true)
    }

    fn validated_with_output_mode(
        fixture: &SchemaFixture,
        all_outputs: bool,
    ) -> ValidatedMigrationAssertionPlan {
        let person = BindingId::new(0).expect("binding");
        let name = BindingId::new(1).expect("binding");
        let employment = BindingId::new(2).expect("binding");
        let (outputs, witnesses) = if all_outputs {
            (vec![person, name, employment], Vec::new())
        } else {
            (vec![person], vec![name, employment])
        };
        let plan = MigrationAssertionPlan::new(
            vec![
                binding(0, "person"),
                binding(1, "name"),
                binding(2, "employment"),
            ],
            vec![
                AssertionPattern::Isa {
                    binding: person,
                    include_subtypes: false,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                AssertionPattern::Isa {
                    binding: person,
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                AssertionPattern::Has {
                    attribute: name,
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: person,
                },
                AssertionPattern::Links {
                    players: vec![AssertionRolePlayer::new(
                        RoleId::new("employment", "employee").expect("role"),
                        person,
                    )],
                    relation: employment,
                    relation_id: type_id(TypeKind::Relation, "employment"),
                },
                AssertionPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: ValueOperand::binding(name),
                    right: ValueOperand::literal(CanonicalValue::String(
                        CanonicalString::new("Ada\n\"Lovelace\"").expect("literal"),
                    )),
                },
                AssertionPattern::Not {
                    patterns: vec![AssertionPattern::Value {
                        comparator: ValueComparator::NotEqual,
                        left: ValueOperand::binding(name),
                        right: ValueOperand::literal(CanonicalValue::String(
                            CanonicalString::new("blocked").expect("literal"),
                        )),
                    }],
                },
            ],
            outputs,
            witnesses,
            fixture.managed.managed_semantic_schema().clone(),
            AssertionExpectation::NoRows,
        )
        .expect("assertion plan");
        validate_migration_assertion_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
            StructuralLimits::CANONICAL,
        )
        .expect("validated assertion")
    }

    struct FakeProvider {
        calls: usize,
        result: Option<QueryResult>,
        statements: Vec<String>,
    }

    impl FakeProvider {
        fn returning(result: QueryResult) -> Self {
            Self {
                calls: 0,
                result: Some(result),
                statements: Vec::new(),
            }
        }
    }

    impl AssertionProviderCall for FakeProvider {
        fn query_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.calls += 1;
            self.statements.push(typeql.to_owned());
            let result = self.result.take().unwrap_or(QueryResult::Rows(Vec::new()));
            Box::pin(async move {
                let mut reader = BoundedAnswerReader::new(limits);
                reader.check_before_read()?;
                let items = match result {
                    QueryResult::Ok => Vec::new(),
                    QueryResult::Rows(rows) => {
                        rows.into_iter().map(AnswerItem::Row).collect::<Vec<_>>()
                    }
                    QueryResult::Documents(documents) => documents
                        .into_iter()
                        .map(AnswerItem::Document)
                        .collect::<Vec<_>>(),
                };
                for item in items {
                    if reader.accept(item, consumer)? == AnswerControl::Stop {
                        break;
                    }
                }
                Ok(reader.stats())
            })
        }
    }

    fn capabilities(validated: &ValidatedMigrationAssertionPlan) -> CapabilitySet {
        validated.plan().required_capabilities().clone()
    }

    fn valid_row() -> Value {
        valid_entity_row("0x01")
    }

    fn valid_entity_row(iid: &str) -> Value {
        serde_json::json!({
            "person": {"category": "entity", "iid": iid, "label": "person"}
        })
    }

    fn all_category_row(attribute_iid: Option<&str>) -> Value {
        let mut row = serde_json::json!({
            "person": {"category": "entity", "iid": "0x01", "label": "person"},
            "name": {
                "category": "attribute",
                "label": "name",
                "value": "Ada",
                "value_type": "string"
            },
            "employment": {
                "category": "relation",
                "iid": "0x02",
                "label": "employment"
            }
        });
        if let Some(iid) = attribute_iid {
            row["name"]
                .as_object_mut()
                .expect("attribute concept")
                .insert("iid".into(), Value::String(iid.to_owned()));
        }
        row
    }

    #[test]
    fn lowering_is_deterministic_and_covers_the_closed_pattern_algebra() {
        let fixture = schema_fixture();
        let validated = validated(&fixture);
        let lowered = lower_migration_assertion(&validated).expect("lowering");
        assert_eq!(
            lowered.typeql(),
            "match\n\
$person isa! person;\n\
$person isa person;\n\
$person has name $name;\n\
$name isa! name;\n\
$employment isa! employment, links (employee: $person);\n\
$name == \"Ada\\n\\\"Lovelace\\\"\";\n\
not {\n\
\x20\x20\x20\x20$name != \"blocked\";\n\
};\n\
select $person;\n\
limit 1;\n"
        );
        assert_eq!(lowered, lower_migration_assertion(&validated).unwrap());
        assert_eq!(lowered.row_schema(), validated.row_schema());
    }

    #[test]
    fn canonical_scalar_literals_have_provider_stable_spelling() {
        assert_eq!(
            render_literal(&CanonicalValue::String(
                CanonicalString::new("quote \" slash \\ line\n").unwrap(),
            )),
            "\"quote \\\" slash \\\\ line\\n\""
        );
        assert_eq!(
            render_literal(&CanonicalValue::Long(i64::MIN)),
            i64::MIN.to_string()
        );
        assert_eq!(
            render_literal(&CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap())),
            "-0.0"
        );
        assert_eq!(
            render_literal(&CanonicalValue::Double(
                CanonicalDouble::new(f64::MAX).unwrap()
            )),
            "1.7976931348623157e308"
        );
        assert_eq!(
            render_literal(&CanonicalValue::Double(
                CanonicalDouble::new(f64::MIN_POSITIVE).unwrap()
            )),
            "2.2250738585072014e-308"
        );
        assert_eq!(
            render_literal(&CanonicalValue::Decimal(
                DecimalValue::new("12.30dec").unwrap()
            )),
            "12.3dec"
        );
        assert_eq!(
            render_literal(&CanonicalValue::String(
                CanonicalString::new("\0\u{0008}\u{000c}\n\r\t").unwrap()
            )),
            "\"\\u0000\\b\\f\\n\\r\\t\""
        );
        let date: CanonicalDate = "2024-02-29".parse().expect("date");
        assert_eq!(render_literal(&CanonicalValue::Date(date)), "2024-02-29");
    }

    #[tokio::test]
    async fn empty_passes_and_first_conforming_row_returns_typed_stable_evidence() {
        let fixture = schema_fixture();
        let validated = validated(&fixture);
        let available = capabilities(&validated);
        let context = MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        );

        let mut empty = FakeProvider::returning(QueryResult::Rows(Vec::new()));
        execute_with_provider(&mut empty, &validated, context)
            .await
            .expect("empty result passes");
        assert_eq!(empty.calls, 1);

        let mut first = FakeProvider::returning(QueryResult::Rows(vec![valid_row()]));
        let error = execute_with_provider(&mut first, &validated, context)
            .await
            .expect_err("one row violates NoRows");
        let MigrationAssertionExecutionError::AssertionFailed(failure) = error else {
            panic!("expected typed assertion failure");
        };
        assert_eq!(failure.evidence().len(), 1);
        assert_eq!(failure.evidence()[0].variable().as_str(), "person");
        assert_eq!(failure.evidence()[0].binding(), BindingId::new(0).unwrap());
        assert_eq!(
            failure.evidence()[0].domain(),
            validated
                .binding_domain(&BindingId::new(0).unwrap())
                .expect("person domain")
        );

        let mut replay = FakeProvider::returning(QueryResult::Rows(vec![valid_row()]));
        let replay_error = execute_with_provider(&mut replay, &validated, context)
            .await
            .expect_err("replay also violates");
        let MigrationAssertionExecutionError::AssertionFailed(replayed) = replay_error else {
            panic!("expected typed replay failure");
        };
        assert_eq!(failure, replayed);
        assert_eq!(first.statements, replay.statements);
    }

    #[tokio::test]
    async fn failure_identity_ignores_unordered_runtime_witnesses() {
        let fixture = schema_fixture();
        let validated = validated(&fixture);
        let available = capabilities(&validated);
        let context = MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        );

        let mut forward = FakeProvider::returning(QueryResult::Rows(vec![
            valid_entity_row("0x01"),
            valid_entity_row("0x02"),
        ]));
        let MigrationAssertionExecutionError::AssertionFailed(forward) =
            execute_with_provider(&mut forward, &validated, context)
                .await
                .expect_err("a conforming row fails the assertion")
        else {
            panic!("expected typed assertion failure")
        };

        let mut reverse = FakeProvider::returning(QueryResult::Rows(vec![
            valid_entity_row("0x02"),
            valid_entity_row("0x01"),
        ]));
        let MigrationAssertionExecutionError::AssertionFailed(reverse) =
            execute_with_provider(&mut reverse, &validated, context)
                .await
                .expect_err("a conforming row fails the assertion")
        else {
            panic!("expected typed assertion failure")
        };

        assert_eq!(forward, reverse);
    }

    #[tokio::test]
    async fn all_concept_categories_validate_and_attribute_iid_is_noncanonical() {
        let fixture = schema_fixture();
        let validated = validated_all_outputs(&fixture);
        let available = capabilities(&validated);
        let context = MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        );

        let mut absent = FakeProvider::returning(QueryResult::Rows(vec![all_category_row(None)]));
        let MigrationAssertionExecutionError::AssertionFailed(absent) =
            execute_with_provider(&mut absent, &validated, context)
                .await
                .expect_err("valid entity, attribute, and relation row fails the assertion")
        else {
            panic!("expected typed assertion failure")
        };
        let mut present = FakeProvider::returning(QueryResult::Rows(vec![all_category_row(Some(
            "0x-internal",
        ))]));
        let MigrationAssertionExecutionError::AssertionFailed(present) =
            execute_with_provider(&mut present, &validated, context)
                .await
                .expect_err("attribute IID remains optional validation metadata")
        else {
            panic!("expected typed assertion failure")
        };

        assert_eq!(absent, present);
        assert_eq!(absent.evidence().len(), 3);
        for column in absent.evidence() {
            assert_eq!(
                column.domain(),
                validated
                    .binding_domain(&column.binding())
                    .expect("output domain")
            );
        }
    }

    #[tokio::test]
    async fn category_specific_missing_invalid_and_extra_fields_fail_closed() {
        let fixture = schema_fixture();
        let validated = validated_all_outputs(&fixture);
        let available = capabilities(&validated);
        let context = MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        );

        let mut cases = Vec::new();
        let mut entity_missing = all_category_row(None);
        entity_missing["person"]
            .as_object_mut()
            .expect("entity")
            .remove("iid");
        cases.push((entity_missing, "missing_fields"));

        let mut entity_extra = all_category_row(None);
        entity_extra["person"]
            .as_object_mut()
            .expect("entity")
            .insert("value".into(), Value::String("redacted".into()));
        cases.push((entity_extra, "unexpected_fields"));

        let mut relation_missing = all_category_row(None);
        relation_missing["employment"]
            .as_object_mut()
            .expect("relation")
            .remove("iid");
        cases.push((relation_missing, "missing_fields"));

        let mut relation_extra = all_category_row(None);
        relation_extra["employment"]
            .as_object_mut()
            .expect("relation")
            .insert("value_type".into(), Value::String("string".into()));
        cases.push((relation_extra, "unexpected_fields"));

        let mut attribute_missing = all_category_row(None);
        attribute_missing["name"]
            .as_object_mut()
            .expect("attribute")
            .remove("value");
        cases.push((attribute_missing, "missing_fields"));

        let mut attribute_invalid = all_category_row(None);
        attribute_invalid["name"]
            .as_object_mut()
            .expect("attribute")
            .insert("iid".into(), Value::from(7));
        cases.push((attribute_invalid, "invalid_fields"));

        let mut attribute_extra = all_category_row(None);
        attribute_extra["name"]
            .as_object_mut()
            .expect("attribute")
            .insert("scalar".into(), Value::String("redacted".into()));
        cases.push((attribute_extra, "unexpected_fields"));

        for (row, detail_key) in cases {
            let mut provider = FakeProvider::returning(QueryResult::Rows(vec![row]));
            let error = execute_with_provider(&mut provider, &validated, context)
                .await
                .expect_err("malformed category row must fail closed");
            let MigrationAssertionExecutionError::ResultValidation(diagnostic) = error else {
                panic!("expected result validation failure")
            };
            assert_eq!(
                diagnostic.code().as_str(),
                "migration_assertion_result_concept_malformed"
            );
            assert!(diagnostic.details().contains_key("binding"));
            assert!(diagnostic.details().contains_key(detail_key));
        }
    }

    #[tokio::test]
    async fn preflight_failures_make_zero_provider_calls() {
        let fixture = schema_fixture();
        let validated = validated(&fixture);
        let available = capabilities(&validated);

        let mut no_capabilities = FakeProvider::returning(QueryResult::Rows(Vec::new()));
        let missing = CapabilitySet::new();
        let error = execute_with_provider(
            &mut no_capabilities,
            &validated,
            MigrationAssertionExecutionContext::new(
                &fixture.managed,
                &missing,
                StructuralLimits::CANONICAL,
            ),
        )
        .await
        .expect_err("capability preflight");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "unsupported_required_capability"
        );
        assert_eq!(no_capabilities.calls, 0);

        let wrong_semantic = ManagedSchemaState::new(
            fixture.managed.format(),
            fixture.managed.required_capabilities().clone(),
            fixture.managed.scope().clone(),
            fixture.managed.selection().clone(),
            fixture.managed.declared_identity().clone(),
            fixture.managed.managed_declared_identity().clone(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
                b"wrong-source-semantic",
            )
            .unwrap(),
        )
        .unwrap();
        let mut semantic = FakeProvider::returning(QueryResult::Rows(Vec::new()));
        let error = execute_with_provider(
            &mut semantic,
            &validated,
            MigrationAssertionExecutionContext::new(
                &wrong_semantic,
                &available,
                StructuralLimits::CANONICAL,
            ),
        )
        .await
        .expect_err("semantic identity preflight");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_source_managed_semantic_mismatch"
        );
        assert_eq!(semantic.calls, 0);

        let different = schema_fixture_with_extra_type(true);
        let wrong_declared = ManagedSchemaState::new(
            fixture.managed.format(),
            fixture.managed.required_capabilities().clone(),
            fixture.managed.scope().clone(),
            fixture.managed.selection().clone(),
            different.managed.declared_identity().clone(),
            fixture.managed.managed_declared_identity().clone(),
            fixture.managed.managed_semantic_schema().clone(),
        )
        .unwrap();
        let mut declared = FakeProvider::returning(QueryResult::Rows(Vec::new()));
        let error = execute_with_provider(
            &mut declared,
            &validated,
            MigrationAssertionExecutionContext::new(
                &wrong_declared,
                &available,
                StructuralLimits::CANONICAL,
            ),
        )
        .await
        .expect_err("declared identity preflight");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_source_declared_identity_mismatch"
        );
        assert_eq!(declared.calls, 0);

        let tight = StructuralLimits {
            bindings: StructuralLimits::CANONICAL.bindings - 1,
            ..StructuralLimits::CANONICAL
        };
        let mut limits = FakeProvider::returning(QueryResult::Rows(Vec::new()));
        let error = execute_with_provider(
            &mut limits,
            &validated,
            MigrationAssertionExecutionContext::new(&fixture.managed, &available, tight),
        )
        .await
        .expect_err("limit policy preflight");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_structural_limits_mismatch"
        );
        assert_eq!(limits.calls, 0);
    }

    #[tokio::test]
    async fn malformed_provider_rows_fail_result_validation() {
        let fixture = schema_fixture();
        let validated = validated(&fixture);
        let available = capabilities(&validated);
        let context = MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        );

        let mut document = FakeProvider::returning(QueryResult::Documents(vec![Value::Null]));
        let error = execute_with_provider(&mut document, &validated, context)
            .await
            .expect_err("documents are invalid");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_result_not_row"
        );

        let mut row = valid_row();
        row.as_object_mut()
            .unwrap()
            .insert("unexpected".into(), Value::Null);
        let mut columns = FakeProvider::returning(QueryResult::Rows(vec![row]));
        let error = execute_with_provider(&mut columns, &validated, context)
            .await
            .expect_err("extra columns are invalid");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_result_column_mismatch"
        );

        let mut row = valid_row();
        row["person"]["category"] = Value::String("relation".into());
        let mut types = FakeProvider::returning(QueryResult::Rows(vec![row]));
        let error = execute_with_provider(&mut types, &validated, context)
            .await
            .expect_err("wrong provider types are invalid");
        assert_eq!(
            error.diagnostic().unwrap().code().as_str(),
            "migration_assertion_result_type_mismatch"
        );
        let details = error.diagnostic().unwrap().details();
        for key in [
            "actual_category",
            "actual_type_label",
            "actual_value_type",
            "allowed_type_domains",
            "binding",
            "expected_value_type",
        ] {
            assert!(details.contains_key(key), "missing mismatch detail {key}");
        }
    }
}
