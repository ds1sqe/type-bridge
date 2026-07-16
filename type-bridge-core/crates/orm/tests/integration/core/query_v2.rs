//! End-to-end V2 query execution against TypeDB 3.12.1.

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm,
    QueryInvocation, QueryOperand, QueryOperation, QueryOutput, QueryPattern,
    QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2::{
    QueryRowValue, QueryV2Outcome, execute_validated_query,
};
use type_bridge_orm::session::backend::{AnswerCancellation, BoundedAnswerLimits};
use type_bridge_query::{
    MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan,
};
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve,
};

struct LiveQueryFixture {
    managed: type_bridge_contract::schema_delta::ManagedSchemaState,
    name: AttributeId,
    person: TypeId,
    resolved: ResolvedSchema,
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding ID"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding ID")
}

fn live_fixture(suffix: &str) -> LiveQueryFixture {
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let name_type = TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap();
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(name_type).unwrap()),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-live").expect("document"),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    LiveQueryFixture {
        managed,
        name,
        person,
        resolved,
    }
}

fn validated_query(
    fixture: &LiveQueryFixture,
    direction: OrderDirection,
) -> (ValidatedQuery, QueryPlan) {
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("minimum_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: fixture.person.clone(),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: fixture.name.clone(),
                        owner: binding_id(0),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::GreaterOrEqual,
                        left: QueryOperand::Binding { binding: binding_id(1) },
                        right: QueryOperand::Input { column: InputColumnId::new(0) },
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Distinct,
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), direction)],
            },
            ReadStage::Limit { rows: 10 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("live query plan");
    let validated = validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
        StructuralLimits::CANONICAL,
    )
    .expect("validated live query");
    (validated, plan)
}

fn string_row(value: &str) -> InputRow {
    InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new(value).expect("canonical string"),
    ))])
}

fn limits() -> BoundedAnswerLimits {
    BoundedAnswerLimits {
        max_items: 100,
        max_bytes: 1 << 20,
        deadline: None,
        cancellation: AnswerCancellation::default(),
    }
}

fn row_names(outcome: &QueryV2Outcome) -> Vec<String> {
    let QueryV2Outcome::Rows(rows) = outcome else {
        panic!("rows operation returns rows: {outcome:?}");
    };
    rows.iter()
        .map(|row| {
            let QueryRowValue::Attribute { value, .. } = &row.values()[1] else {
                panic!("second output column is the name attribute");
            };
            let CanonicalValue::String(value) = value else {
                panic!("name is a string attribute");
            };
            value.as_str().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn validated_queries_execute_rows_count_and_exists_live() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-live");
    let fixture = live_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             entity {}, owns {};",
            fixture.name.label(),
            fixture.person.label(),
            fixture.name.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live query schema");
    db.execute_raw(
        &format!(
            "insert $ada isa {person}, has {name} \"Ada\"; \
             $grace isa {person}, has {name} \"Grace\"; \
             $alan isa {person}, has {name} \"Alan\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live query data");

    let (ascending, plan) = validated_query(&fixture, OrderDirection::Ascending);
    let mut transaction =
        db.read_transaction().await.expect("borrowed read transaction");

    let rows = execute_validated_query(
        &mut transaction,
        &ascending,
        &QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("Al")])
            .expect("rows invocation"),
        limits(),
    )
    .await
    .expect("ascending rows");
    assert_eq!(row_names(&rows), vec!["Alan", "Grace"]);

    let (descending, descending_plan) =
        validated_query(&fixture, OrderDirection::Descending);
    let rows = execute_validated_query(
        &mut transaction,
        &descending,
        &QueryInvocation::new(
            &descending_plan,
            QueryOperation::Rows,
            vec![string_row("A")],
        )
        .expect("descending invocation"),
        limits(),
    )
    .await
    .expect("descending rows");
    assert_eq!(row_names(&rows), vec!["Grace", "Alan", "Ada"]);

    let count = execute_validated_query(
        &mut transaction,
        &ascending,
        &QueryInvocation::new(&plan, QueryOperation::Count, vec![string_row("A")])
            .expect("count invocation"),
        limits(),
    )
    .await
    .expect("count outcome");
    assert_eq!(count, QueryV2Outcome::Count(3));

    let exists = execute_validated_query(
        &mut transaction,
        &ascending,
        &QueryInvocation::new(&plan, QueryOperation::Exists, vec![string_row("Z")])
            .expect("exists invocation"),
        limits(),
    )
    .await
    .expect("exists outcome");
    assert_eq!(exists, QueryV2Outcome::Exists(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scalar_schema_function_calls_execute_live() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
        FunctionReturnMode, FunctionSignature, TypeReference,
    };
    use type_bridge_orm::query_v2::lower_validated_query;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-fn-live");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();
    let count_fn = FunctionId::new(format!("{suffix}-person-count")).unwrap();
    let sum_fn = FunctionId::new(format!("{suffix}-age-sum")).unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {age}, value integer;\n\
             entity {person}, owns {age};\n\
             fun {count_fn}() -> integer:\n\
             match $p isa {person};\n\
             return count($p);\n\
             fun {sum_fn}($subject: {person}) -> integer:\n\
             match $subject has {age} $a;\n\
             return sum($a);",
            age = age.label(),
            person = person.label(),
            count_fn = count_fn.label(),
            sum_fn = sum_fn.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live function schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {age} 30; \
             $b isa {person}, has {age} 40; \
             $c isa {person}, has {age} 25;",
            person = person.label(),
            age = age.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live function data");

    // The declared authority carries typed signatures for both functions.
    let person_label = Label::new(person.label().as_str()).unwrap();
    let scalar_long = FunctionReturnMode::scalar(FunctionReturnElement::new(
        TypeReference::Value(ValueTypeTag::Long),
        false,
    ));
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).unwrap(),
        )),
        SchemaFact::Function(FunctionFact::new(
            count_fn.clone(),
            FunctionSignature::new(Vec::new(), scalar_long.clone()).unwrap(),
            FunctionBody::new("match $p isa person; return count($p);").unwrap(),
        )),
        SchemaFact::Function(FunctionFact::new(
            sum_fn.clone(),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("subject").unwrap(),
                    TypeReference::Schema(person_label),
                )],
                scalar_long,
            )
            .unwrap(),
            FunctionBody::new("match $subject has age $a; return sum($a);").unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-fn-live").unwrap(),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .unwrap(),
        )
    });
    let declared = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        sourced,
    )
    .unwrap();
    let profile =
        type_bridge_contract::fingerprint::SemanticProfileId::new("typedb-3.12.1/v1")
            .unwrap();
    let resolved = type_bridge_schema::resolve(&declared, &profile).unwrap();
    let managed = type_bridge_schema::managed_schema_state(
        &declared,
        &type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("query-v2-fn-live")
                .unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    // Zero-argument call: one row carrying the counted value.
    let count_plan = QueryPlan::new(
        vec![AssertionBinding::new(
            binding_id(0),
            QueryVariable::new("person_count").unwrap(),
        )],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::FunctionCall {
                arguments: Vec::new(),
                assigned: binding_id(0),
                function: count_fn,
            }],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&count_plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    let invocation =
        QueryInvocation::new(&count_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let lowered = lower_validated_query(&validated, &invocation).unwrap();
    assert!(
        lowered.typeql().contains("let $person_count = "),
        "{}",
        lowered.typeql(),
    );
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("count function execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value {
            value: type_bridge_contract::value::CanonicalValue::Long(3)
        },
    );

    // Per-row call: each person joins its summed (single) age, sorted.
    let sum_plan = QueryPlan::new(
        vec![
            AssertionBinding::new(binding_id(0), QueryVariable::new("person").unwrap()),
            AssertionBinding::new(
                binding_id(1),
                QueryVariable::new("age_sum").unwrap(),
            ),
        ],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: person,
                    },
                    QueryPattern::FunctionCall {
                        arguments: vec![
                            type_bridge_contract::query_plan::QueryOperand::Binding {
                                binding: binding_id(0),
                            },
                        ],
                        assigned: binding_id(1),
                        function: sum_fn,
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows { columns: vec![binding_id(1)] },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&sum_plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    let invocation =
        QueryInvocation::new(&sum_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("sum function execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let sums = rows
        .iter()
        .map(|row| match &row.values()[0] {
            QueryRowValue::Value {
                value: type_bridge_contract::value::CanonicalValue::Long(value),
            } => *value,
            other => panic!("expected long values: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(sums, vec![25, 30, 40]);
}
