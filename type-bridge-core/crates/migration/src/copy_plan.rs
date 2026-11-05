//! Native copy mutations and their separately planned guarded/total counts.
use crate::{NativeDataStep, OperationSpec};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, Operation, Pattern, PlanSpec, QueryPlanV3, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{
    QueryOperand, QueryOutput, QueryPattern, ReduceAssignment, Reducer, TypeMatchMode,
};
use type_bridge_contract::schema::{DeclaredSchema, SchemaFact, SchemaFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::ValueTypeTag;

fn invalid(message: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_copy_attribute_native_input").expect("static code"),
        message,
    )
}
fn id(value: u16) -> BindingId {
    BindingId::new(value).expect("bounded copy bindings")
}

pub(crate) fn build(
    op: &OperationSpec,
    schema: &DeclaredSchema,
    reversible: bool,
) -> crate::Result<NativeDataStep> {
    let OperationSpec::CopyAttribute {
        owner: Some(owner),
        source: Some(source),
        dest: Some(dest),
        mode,
        filter: None,
        forward: None,
        reverse: None,
    } = op
    else {
        return Err(invalid("CopyAttribute requires structured owner/source/dest without text or filters; use RunPlan for native programs or RunTypeql for explicit raw TypeQL").into());
    };
    let mode = TypeMatchMode::from_typeql(mode.as_deref().unwrap_or(""))?;
    let owner = schema
        .facts()
        .find_map(|fact| match fact {
            SchemaFact::Type(fact)
                if fact.id().label().as_str() == owner
                    && matches!(fact.id().kind(), TypeKind::Entity | TypeKind::Relation) =>
            {
                Some(fact.id().clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            invalid("CopyAttribute owner is absent from the explicit schema authority")
        })?;
    let source = AttributeId::new(source)?;
    let dest = AttributeId::new(dest)?;
    let profile = SemanticProfileId::new("typedb-3.12.1/v1")?;
    let resolved = type_bridge_schema::resolve(schema, &profile)
        .map_err(|error| invalid(&error.to_string()))?;
    let ownership = resolved
        .types()
        .get(&owner)
        .and_then(|owner| owner.owns().get(&dest))
        .ok_or_else(|| invalid("CopyAttribute destination is not effectively owned"))?;
    let SchemaFactId::Owns(field) = ownership.origin().declared() else {
        return Err(invalid("CopyAttribute destination has no declared ownership origin").into());
    };
    let field = field.clone();
    let canonical = type_bridge_schema::canonical_semantic_schema_bytes(schema, &profile)
        .map_err(|error| invalid(&error.to_string()))?;
    let semantics = ManagedSemanticSchemaFingerprint::compute(profile, &canonical)?;
    let bindings = ["x", "v", "d", "c"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            Ok(AssertionBinding::new(
                id(index as u16),
                QueryVariable::new(name)?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let isa = QueryPattern::Isa {
        binding: id(0),
        type_id: owner,
        include_subtypes: true,
    };
    let total = vec![
        Pattern::from(isa.clone()),
        QueryPattern::HasType {
            mode,
            owner: id(0),
            attribute: id(1),
            attribute_id: source,
        }
        .into(),
    ];
    let mut guarded = total.clone();
    guarded.push(
        QueryPattern::Not {
            patterns: vec![QueryPattern::HasType {
                mode,
                owner: id(0),
                attribute: id(2),
                attribute_id: dest.clone(),
            }],
        }
        .into(),
    );
    let build = |pipeline, operation, columns| {
        QueryPlanV3::new(
            PlanSpec {
                bindings: bindings.clone(),
                inputs: vec![],
                given_rows: None,
                operation,
                pipeline,
                output: QueryOutput::Rows { columns },
                managed_semantics: semantics.clone(),
                functions: vec![],
            },
            schema,
        )
    };
    let count = |patterns| {
        build(
            vec![
                Stage::Match { patterns },
                Stage::Reduce {
                    assignments: vec![ReduceAssignment::new(id(3), Reducer::Count, None)],
                    groups: vec![],
                },
            ],
            Operation::Rows,
            vec![id(3)],
        )
    };
    let counts = [count(guarded.clone())?, count(total)?];
    let forward = build(
        vec![
            Stage::Match { patterns: guarded },
            Stage::Attach {
                items: vec![AttachItem::Has {
                    optional: false,
                    owner: id(0),
                    field: field.clone(),
                    operand: QueryOperand::Binding { binding: id(1) },
                }],
            },
        ],
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )?;
    let reverse = reversible
        .then(|| {
            build(
                vec![
                    Stage::Match {
                        patterns: vec![
                            isa.into(),
                            QueryPattern::HasType {
                                mode,
                                owner: id(0),
                                attribute: id(1),
                                attribute_id: dest,
                            }
                            .into(),
                        ],
                    },
                    Stage::Delete {
                        items: vec![DeleteItem::HasValue {
                            value: id(1),
                            owner: id(0),
                            field,
                        }],
                    },
                ],
                Operation::Write {
                    returns: WriteReturns::Matched,
                },
                vec![],
            )
        })
        .transpose()?;
    Ok(NativeDataStep::from_copy_plans(
        schema, forward, reverse, counts,
    ))
}

pub(crate) fn validate_count(plan: &QueryPlanV3) -> Result<(), Diagnostic> {
    if plan.operation() == Operation::Rows
        && matches!(plan.output(), QueryOutput::Rows { columns } if columns.len() == 1
            && plan.scalar_value_type(columns[0]) == Some(ValueTypeTag::Long))
        && matches!(plan.pipeline().last(), Some(Stage::Reduce { groups, assignments }) if groups.is_empty()
            && assignments.len() == 1 && assignments[0].reducer() == Reducer::Count
            && assignments[0].input().is_none()
            && matches!(plan.output(), QueryOutput::Rows { columns } if columns == &[assignments[0].assigned()]))
    {
        Ok(())
    } else {
        Err(invalid(
            "migration count plans must be read-only ungrouped integer reductions",
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture() -> (DeclaredSchema, OperationSpec) {
        let schema = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("copy-test.typeql").unwrap(),
            "define attribute old-name, value string; attribute new-name, value string; entity person, owns old-name, owns new-name;",
        ).unwrap();
        (
            schema,
            OperationSpec::CopyAttribute {
                mode: Some("isa".into()),
                owner: Some("person".into()),
                source: Some("old-name".into()),
                dest: Some("new-name".into()),
                filter: None,
                forward: None,
                reverse: None,
            },
        )
    }
    pub(crate) fn step() -> crate::ExecutionStep {
        let (schema, op) = fixture();
        crate::ExecutionStep {
            data: Some(build(&op, &schema, true).unwrap()),
            tx_type: type_bridge_orm::TxType::Write,
            kind: crate::StepKind::Backfill,
            operation_kind: crate::OperationKind::CopyAttribute,
            forward: String::new(),
            reverse: None,
        }
    }
    #[test]
    fn copy_requires_mode_and_accepts_abstract_source_owned_by_a_subtype() {
        let (_, op) = fixture();
        let schema = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("abstract-copy.typeql").unwrap(),
            "define attribute old-name @abstract, value string; attribute specialised, sub old-name; attribute new-name, value string; entity person, owns new-name; entity employee, sub person, owns specialised;",
        ).unwrap();
        assert!(build(&op, &schema, true).is_ok());
        let mut wire = serde_json::to_value(&op).unwrap();
        wire["mode"] = serde_json::json!("isa!");
        let exact = serde_json::from_value(wire.clone()).unwrap();
        assert!(
            build(&exact, &schema, true)
                .unwrap_err()
                .to_string()
                .contains("query_plan_abstract_exact_ownership")
        );
        wire.as_object_mut().unwrap().remove("mode");
        let missing = serde_json::from_value(wire).unwrap();
        let error = build(&missing, &schema, true).unwrap_err().to_string();
        assert!(
            error.contains("query_plan_type_match_mode") && error.contains("isa or isa!"),
            "{error}"
        );
    }

    #[test]
    fn legacy_reverse_keeps_its_destination_only_predicate() {
        let step = step();
        let reverse = step
            .data
            .as_ref()
            .unwrap()
            .program(crate::MigrationAction::Rollback)
            .unwrap();
        assert!(
            matches!(reverse.pipeline(), [Stage::Match { patterns }, Stage::Delete { items }]
            if matches!(patterns.as_slice(), [Pattern::Read(QueryPattern::Isa { .. }), Pattern::Read(QueryPattern::HasType { attribute_id, mode: TypeMatchMode::Isa, .. })] if attribute_id.label().as_str() == "new-name")
            && matches!(items.as_slice(), [DeleteItem::HasValue { .. }]))
        );
    }

    #[test]
    fn inherited_destination_uses_its_actual_declaration_origin() {
        let (_, op) = fixture();
        let schema = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("inherited-copy.typeql").unwrap(),
            "define attribute old-name, value string; attribute new-name, value string; entity base, owns old-name, owns new-name; entity person, sub base;",
        ).unwrap();
        let data = build(&op, &schema, true).unwrap();
        let forward = data.program(crate::MigrationAction::Apply).unwrap();
        let Some(Stage::Attach { items }) = forward.pipeline().last() else {
            panic!("copy attachment")
        };
        assert!(
            matches!(&items[0], AttachItem::Has { field, .. } if field.owner().label().as_str() == "base")
        );
        assert!(data.program(crate::MigrationAction::Rollback).is_some());
    }

    #[test]
    fn copy_roundtrips_native_counts_and_refuses_tampered_write_count() {
        let step = step();
        let mut wire = serde_json::to_value(&step).unwrap();
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<crate::ExecutionStep>(wire.clone()).unwrap()
            )
            .unwrap(),
            wire
        );
        wire["data"]["counts"][0] = wire["data"]["forward"].clone();
        assert!(serde_json::from_value::<crate::ExecutionStep>(wire).is_err());
    }
    #[test]
    fn copy_refuses_text_and_filters_and_irreversible_copy_omits_reverse() {
        let (schema, op) = fixture();
        for field in ["forward", "reverse", "filter"] {
            let mut wire = serde_json::to_value(&op).unwrap();
            wire[field] = serde_json::json!("arbitrary text");
            let changed = serde_json::from_value(wire).unwrap();
            let error = build(&changed, &schema, true).unwrap_err().to_string();
            assert!(
                error.contains("RunPlan") && error.contains("RunTypeql"),
                "{error}"
            );
        }
        assert!(
            build(&op, &schema, false)
                .unwrap()
                .program(crate::MigrationAction::Rollback)
                .is_none()
        );
    }
}
