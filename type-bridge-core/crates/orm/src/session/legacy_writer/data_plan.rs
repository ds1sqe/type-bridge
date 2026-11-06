//! Native reads for the frozen managed fence and legacy ledger.
use crate::session::staged::PlanAnswer;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{Operation, Pattern, PlanSpec, QueryPlan, Stage};
use type_bridge_contract::query_plan::{QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_schema_compat::{LEGACY_LEDGER_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL};

#[derive(Debug)]
pub(crate) struct DataQuery {
    pub(crate) plan: QueryPlan,
    aliases: Vec<String>,
}
impl DataQuery {
    pub(crate) fn documents(
        &self,
        answer: PlanAnswer,
    ) -> Result<Vec<serde_json::Value>, Diagnostic> {
        if self.aliases.is_empty() {
            let PlanAnswer::Rows(rows) = answer else {
                return Err(shape());
            };
            return Ok(rows
                .into_iter()
                .map(|_| serde_json::json!({"exists":true}))
                .collect());
        }
        answer
            .scalar_rows(&self.plan)?
            .into_iter()
            .map(|row| {
                let fields = self
                    .aliases
                    .iter()
                    .zip(row)
                    .map(|(name, value)| {
                        let text = match value {
                            CanonicalValue::String(value) => value.as_str().to_owned(),
                            CanonicalValue::DateTime(value) => {
                                // The frozen sentinel comparison uses provider nanosecond precision.
                                let (hour, minute, second, nanos) = value.time().components();
                                format!(
                                    "{}T{hour:02}:{minute:02}:{second:02}.{nanos:09}",
                                    value.date()
                                )
                            }
                            _ => return Err(shape()),
                        };
                        Ok((name.clone(), serde_json::Value::String(text)))
                    })
                    .collect::<Result<serde_json::Map<_, _>, _>>()?;
                Ok(serde_json::Value::Object(fields))
            })
            .collect()
    }
}
fn shape() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("legacy_writer_data_plan_shape").expect("static code"),
        "legacy writer guard contradicts its frozen schema",
    )
}
fn id(index: usize) -> Result<BindingId, Diagnostic> {
    BindingId::new(u16::try_from(index).map_err(|_| shape())?)
}
fn type_id(kind: &str) -> Result<TypeId, Diagnostic> {
    TypeId::new(TypeKind::Entity, kind)
}
fn literal(value: &str) -> Result<QueryOperand, Diagnostic> {
    Ok(QueryOperand::Literal {
        value: CanonicalValue::String(CanonicalString::new(value)?),
    })
}
static AUTHORITY: std::sync::LazyLock<
    Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
> = std::sync::LazyLock::new(|| {
    let source = format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n{}",
        type_bridge_contract::reserved::IMPORT_LEGACY_MANAGED_FENCE_SCHEMA_TYPEQL
    );
    let schema = type_bridge_schema_compat::typeql_to_declared(
        DocumentId::new("legacy-writer-data-plan.typeql")?,
        &source,
    )
    .map_err(|_| shape())?;
    let semantics = ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1")?,
        source.as_bytes(),
    )?;
    Ok((schema, semantics))
});

struct Builder {
    bindings: Vec<AssertionBinding>,
    stages: Vec<Stage>,
}
impl Builder {
    fn bind(&mut self, name: &str) -> Result<BindingId, Diagnostic> {
        let binding = id(self.bindings.len())?;
        self.bindings.push(AssertionBinding::new(
            binding,
            QueryVariable::new(if name == "predicate" {
                format!("predicate{}", self.bindings.len())
            } else {
                name.to_owned()
            })?,
        ));
        Ok(binding)
    }
    fn matched(
        &mut self,
        kind: &str,
        fixed: &[(&str, &str)],
        bound: &[(&str, &str)],
    ) -> Result<(BindingId, Vec<BindingId>), Diagnostic> {
        let root = self.bind("root")?;
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: root,
                type_id: type_id(kind)?,
                include_subtypes: true,
            }
            .into(),
        ];
        for (field, value) in fixed {
            let attribute = self.bind("predicate")?;
            patterns.extend([
                Pattern::from(QueryPattern::Has {
                    owner: root,
                    attribute,
                    attribute_id: AttributeId::new(*field)?,
                }),
                Pattern::from(QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: attribute },
                    right: literal(value)?,
                }),
            ]);
        }
        let mut columns = Vec::new();
        for (field, alias) in bound {
            let attribute = self.bind(alias)?;
            patterns.push(
                QueryPattern::Has {
                    owner: root,
                    attribute,
                    attribute_id: AttributeId::new(*field)?,
                }
                .into(),
            );
            columns.push(attribute);
        }
        self.stages.push(Stage::Match { patterns });
        Ok((root, columns))
    }
    fn finish(
        self,
        operation: Operation,
        columns: Vec<BindingId>,
    ) -> Result<QueryPlan, Diagnostic> {
        let (schema, semantics) = AUTHORITY.as_ref().map_err(Clone::clone)?;
        QueryPlan::new(
            PlanSpec {
                bindings: self.bindings,
                inputs: vec![],
                given_rows: None,
                operation,
                pipeline: self.stages,
                output: QueryOutput::Rows { columns },
                managed_semantics: semantics.clone(),
                functions: vec![],
            },
            schema,
        )
    }
}
pub(crate) fn read(
    kind: &str,
    fixed: &[(&str, &str)],
    bound: &[(&str, &str)],
) -> Result<DataQuery, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, mut columns) = builder.matched(kind, fixed, bound)?;
    if columns.is_empty() {
        columns.push(root);
    }
    Ok(DataQuery {
        plan: builder.finish(Operation::Rows, columns)?,
        aliases: bound.iter().map(|(_, name)| (*name).to_owned()).collect(),
    })
}

pub(crate) fn read_legacy(
    kind: &str,
    fixed: &[(&str, &str)],
    bound: &[(&str, &str)],
) -> Result<DataQuery, Diagnostic> {
    use type_bridge_contract::reserved::import_legacy_control_label as label;
    let fixed_labels: Vec<_> = fixed.iter().map(|(field, _)| label(field)).collect();
    let bound_labels: Vec<_> = bound.iter().map(|(field, _)| label(field)).collect();
    let fixed: Vec<_> = fixed_labels
        .iter()
        .zip(fixed)
        .map(|(field, (_, value))| (field.as_ref(), *value))
        .collect();
    let bound: Vec<_> = bound_labels
        .iter()
        .zip(bound)
        .map(|(field, (_, alias))| (field.as_ref(), *alias))
        .collect();
    read(&label(kind), &fixed, &bound)
}
