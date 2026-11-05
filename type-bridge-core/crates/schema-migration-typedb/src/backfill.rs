//! Native programs for the closed canonical attribute-backfill operation.
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::migration_backfill::AttributeBackfillPlan;
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, Operation, Pattern, PlanSpec, QueryPlanV3, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOperand, QueryOutput, QueryPattern, TypeMatchMode,
};
use type_bridge_contract::schema::{DeclaredSchema, SchemaFactId};
use type_bridge_schema_migration::BackfillExecutionDirection;

pub(crate) struct NativeBackfill {
    pub(crate) conflict: Option<QueryPlanV3>,
    pub(crate) incomplete: QueryPlanV3,
    pub(crate) mutation: QueryPlanV3,
}
fn id(value: u16) -> BindingId {
    BindingId::new(value).expect("bounded backfill binding")
}
fn invalid(message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_typedb_backfill_native_authority").expect("static code"),
        message,
    )
}
impl NativeBackfill {
    pub(crate) fn new(
        plan: &AttributeBackfillPlan,
        direction: BackfillExecutionDirection,
        authority: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        let profile = plan
            .managed_semantics()
            .as_fingerprint()
            .semantic_profile()
            .ok_or_else(|| invalid("backfill requires a semantic profile"))?;
        let resolved = type_bridge_schema::resolve(authority, profile)
            .map_err(|_| invalid("backfill planning authority cannot be resolved"))?;
        let ownership = resolved
            .types()
            .get(plan.owner())
            .and_then(|owner| owner.owns().get(plan.destination()))
            .ok_or_else(|| invalid("backfill destination has no effective ownership"))?;
        let SchemaFactId::Owns(field) = ownership.origin().declared() else {
            return Err(invalid(
                "backfill destination has no declared ownership origin",
            ));
        };
        let bindings = ["owner", "source", "destination", "stable"]
            .into_iter()
            .enumerate()
            .map(|(n, name)| {
                Ok(AssertionBinding::new(
                    id(n as u16),
                    QueryVariable::new(name)?,
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let has = |attribute, label, mode| QueryPattern::HasType {
            owner: id(0),
            attribute: id(attribute),
            attribute_id: label,
            mode,
        };
        let compare = |comparator| QueryPattern::Value {
            comparator,
            left: QueryOperand::Binding { binding: id(2) },
            right: QueryOperand::Binding { binding: id(1) },
        };
        let source = vec![
            QueryPattern::Isa {
                binding: id(0),
                type_id: plan.owner().clone(),
                include_subtypes: true,
            },
            has(1, plan.source().clone(), plan.mode()),
        ];
        let equal = vec![
            has(2, plan.destination().clone(), plan.mode()),
            compare(ValueComparator::Equal),
        ];
        let build = |patterns: Vec<QueryPattern>, mut tail: Vec<Stage>, operation, columns| {
            let mut pipeline = vec![Stage::Match {
                patterns: patterns.into_iter().map(Pattern::from).collect(),
            }];
            pipeline.append(&mut tail);
            QueryPlanV3::new(
                PlanSpec {
                    bindings: bindings.clone(),
                    inputs: vec![],
                    given_rows: None,
                    operation,
                    pipeline,
                    output: QueryOutput::Rows { columns },
                    managed_semantics: plan.managed_semantics().clone(),
                    functions: vec![],
                },
                authority,
            )
        };
        let witness = |patterns, column| {
            build(
                patterns,
                vec![
                    Stage::Select {
                        bindings: vec![id(column)],
                    },
                    Stage::Limit { rows: 1 },
                ],
                Operation::Rows,
                vec![id(column)],
            )
        };
        let reverse = direction == BackfillExecutionDirection::Reverse;
        let conflict = if reverse {
            None
        } else {
            let mut patterns = source.clone();
            patterns.extend([
                has(2, plan.destination().clone(), plan.mode()),
                compare(ValueComparator::NotEqual),
            ]);
            Some(witness(patterns, 2)?)
        };
        let mut unfinished = source;
        if reverse {
            unfinished.extend(equal);
        } else {
            unfinished.push(QueryPattern::Not { patterns: equal });
        }
        let incomplete = witness(unfinished.clone(), if reverse { 2 } else { 1 })?;
        // Partition-key matching keeps the historical TypeQL `has` semantics.
        unfinished.push(has(
            3,
            plan.partition().stable_attribute().clone(),
            TypeMatchMode::Isa,
        ));
        let mutation = if reverse {
            Stage::Delete {
                items: vec![DeleteItem::HasValue {
                    owner: id(0),
                    field: field.clone(),
                    value: id(2),
                }],
            }
        } else {
            Stage::Attach {
                items: vec![AttachItem::Has {
                    optional: false,
                    owner: id(0),
                    field: field.clone(),
                    operand: QueryOperand::Binding { binding: id(1) },
                }],
            }
        };
        let mutation = build(
            unfinished,
            vec![
                Stage::Sort {
                    terms: vec![OrderTerm::new(id(3), OrderDirection::Ascending)],
                },
                Stage::Limit {
                    rows: u64::from(plan.partition().batch_rows()),
                },
                mutation,
            ],
            Operation::Write {
                returns: WriteReturns::Rows,
            },
            vec![id(3)],
        )?;
        Ok(Self {
            conflict,
            incomplete,
            mutation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::{
        fingerprint::SemanticProfileId,
        id::{AttributeId, TypeId, TypeKind},
        migration_backfill::{BackfillPartition, BackfillReverseProgram},
        schema::DocumentId,
        schema_fingerprint::ManagedSemanticSchemaFingerprint,
    };
    fn fixture() -> (DeclaredSchema, AttributeBackfillPlan) {
        let schema = type_bridge_schema_compat::typeql_to_declared(DocumentId::new("backfill-test.typeql").unwrap(), "define attribute old-name, value string; attribute display-name, value string; attribute person-id, value string; entity person, owns old-name, owns display-name, owns person-id @key;").unwrap();
        let plan = AttributeBackfillPlan::new(
            TypeId::new(TypeKind::Entity, "person").unwrap(),
            AttributeId::new("old-name").unwrap(),
            AttributeId::new("display-name").unwrap(),
            TypeMatchMode::Isa,
            BackfillPartition::new(37, AttributeId::new("person-id").unwrap()).unwrap(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
                b"schema",
            )
            .unwrap(),
            Some(BackfillReverseProgram::RemoveEqualCopiedDestination),
        )
        .unwrap();
        (schema, plan)
    }
    #[test]
    fn forward_is_bounded_and_reverse_deletes_only_the_equal_bound_value() {
        let (schema, plan) = fixture();
        for direction in [
            BackfillExecutionDirection::Forward,
            BackfillExecutionDirection::Reverse,
        ] {
            let native = NativeBackfill::new(&plan, direction, &schema).unwrap();
            assert_eq!(native.incomplete.statement_counts(), (1, 0));
            assert_eq!(native.mutation.statement_counts(), (0, 1));
            assert!(matches!(
                native.mutation.pipeline(),
                [
                    Stage::Match { .. },
                    Stage::Sort { .. },
                    Stage::Limit { rows: 37 },
                    _
                ]
            ));
            if direction == BackfillExecutionDirection::Reverse {
                assert!(native.conflict.is_none());
                assert!(
                    matches!(native.mutation.pipeline().last(), Some(Stage::Delete { items }) if matches!(items.as_slice(), [DeleteItem::HasValue { value, .. }] if *value == id(2)))
                );
                let Stage::Match { patterns } = &native.mutation.pipeline()[0] else {
                    panic!("match");
                };
                assert!(patterns.iter().any(|pattern| matches!(
                    pattern,
                    Pattern::Read(QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        ..
                    })
                )));
            } else {
                assert_eq!(native.conflict.unwrap().statement_counts(), (1, 0));
            }
        }
    }
}
