//! Native data plans over the canonical legacy migration-state schema.
use super::legacy_ledger::{LegacyReadError as MigrationError, Result};
use super::legacy_schema::migration_state_schema;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    Operation, Pattern, PlanSpec, QueryPlan, Stage, ThingDocument,
};
use type_bridge_contract::query_plan::{QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

fn diagnostic(error: Diagnostic) -> MigrationError {
    MigrationError::State {
        message: error.to_string(),
    }
}
pub(super) fn text(value: &str) -> Result<CanonicalValue> {
    Ok(CanonicalValue::String(
        CanonicalString::new(value).map_err(diagnostic)?,
    ))
}
fn binding(index: u16) -> BindingId {
    BindingId::new(index).expect("bounded migration bindings")
}
fn operand(value: CanonicalValue) -> QueryOperand {
    QueryOperand::Literal { value }
}
static AUTHORITY: std::sync::LazyLock<
    std::result::Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), String>,
> = std::sync::LazyLock::new(|| {
    let schema_text = migration_state_schema()
        .to_typeql()
        .map_err(|error| error.to_string())?;
    let schema = type_bridge_schema_compat::typeql_to_declared(
        DocumentId::new("migration-state-plan.typeql").map_err(|error| error.to_string())?,
        &schema_text,
    )
    .map_err(|error| error.to_string())?;
    let semantics = ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").map_err(|error| error.to_string())?,
        schema_text.as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    Ok((schema, semantics))
});

fn build(
    bindings: Vec<AssertionBinding>,
    pipeline: Vec<Stage>,
    operation: Operation,
) -> Result<QueryPlan> {
    let (schema, semantics) = AUTHORITY
        .as_ref()
        .map_err(|message| MigrationError::State {
            message: message.clone(),
        })?;
    QueryPlan::new(
        PlanSpec {
            bindings,
            inputs: vec![],
            given_rows: None,
            operation,
            pipeline,
            output: QueryOutput::Rows { columns: vec![] },
            managed_semantics: semantics.clone(),
            functions: vec![],
        },
        schema,
    )
    .map_err(diagnostic)
}
fn root() -> AssertionBinding {
    AssertionBinding::new(
        binding(0),
        QueryVariable::new("migration").expect("static variable"),
    )
}
fn matches(kind: &str, fixed: &[(&str, &str)]) -> Result<(Vec<AssertionBinding>, Vec<Pattern>)> {
    let mut bindings = vec![root()];
    let mut patterns = vec![
        QueryPattern::Isa {
            binding: binding(0),
            type_id: TypeId::new(TypeKind::Entity, kind).map_err(diagnostic)?,
            include_subtypes: false,
        }
        .into(),
    ];
    for (index, (field, value)) in fixed.iter().enumerate() {
        let slot = u16::try_from(index + 1).map_err(|_| MigrationError::State {
            message: "migration predicate limit".into(),
        })?;
        bindings.push(AssertionBinding::new(
            binding(slot),
            QueryVariable::new(format!("predicate{slot}")).map_err(diagnostic)?,
        ));
        patterns.extend([
            QueryPattern::Has {
                attribute: binding(slot),
                owner: binding(0),
                attribute_id: AttributeId::new(*field).map_err(diagnostic)?,
            }
            .into(),
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding {
                    binding: binding(slot),
                },
                right: operand(text(value)?),
            }
            .into(),
        ]);
    }
    Ok((bindings, patterns))
}
pub(super) fn read(kind: &str, fixed: &[(&str, &str)]) -> Result<QueryPlan> {
    let (bindings, patterns) = matches(kind, fixed)?;
    build(
        bindings,
        vec![
            Stage::Match { patterns },
            Stage::Fetch(ThingDocument::new(binding(0), vec![])),
        ],
        Operation::Rows,
    )
}
