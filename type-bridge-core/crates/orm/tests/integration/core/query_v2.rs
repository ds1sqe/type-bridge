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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reduce_stages_group_and_total_live() {
    use type_bridge_contract::query_plan::{
        QueryOperation, ReduceAssignment, Reducer,
    };
    use type_bridge_contract::value::CanonicalValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-reduce-live");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {age}, value integer;\n\
             entity {person}, owns {age} @card(0..);",
            age = age.label(),
            person = person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live reduce schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {age} 30, has {age} 40; \
             $b isa {person}, has {age} 25;",
            person = person.label(),
            age = age.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live reduce data");

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
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-reduce-live").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-reduce-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    let match_stage = ReadStage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            },
            QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: age.clone(),
                owner: binding_id(0),
            },
        ],
    };

    // Grouped: each person joins its age sum and age count, sorted by sum.
    let grouped_plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "age"),
            binding(2, "age_sum"),
            binding(3, "age_count"),
        ],
        Vec::new(),
        vec![
            match_stage.clone(),
            ReadStage::Reduce {
                assignments: vec![
                    ReduceAssignment::new(
                        binding_id(2),
                        Reducer::Sum,
                        Some(binding_id(1)),
                    ),
                    ReduceAssignment::new(
                        binding_id(3),
                        Reducer::Count,
                        Some(binding_id(1)),
                    ),
                ],
                groups: vec![binding_id(0)],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(2), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(2), binding_id(3)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &grouped_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation =
        QueryInvocation::new(&grouped_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("grouped reduce execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let reduced = rows
        .iter()
        .map(|row| {
            let QueryRowValue::Thing { .. } = &row.values()[0] else {
                panic!("group key is the person entity: {row:?}");
            };
            let long = |value: &QueryRowValue| match value {
                QueryRowValue::Value { value: CanonicalValue::Long(value) } => *value,
                other => panic!("expected long value: {other:?}"),
            };
            (long(&row.values()[1]), long(&row.values()[2]))
        })
        .collect::<Vec<_>>();
    assert_eq!(reduced, vec![(25, 1), (70, 2)]);

    // Global: one bare count row totals every match row.
    let global_plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "age"),
            binding(2, "total"),
        ],
        Vec::new(),
        vec![
            match_stage,
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(2),
                    Reducer::Count,
                    None,
                )],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows { columns: vec![binding_id(2)] },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &global_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation =
        QueryInvocation::new(&global_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("global count execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value { value: CanonicalValue::Long(3) },
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_blocks_carry_optional_columns_live() {
    use type_bridge_contract::query_plan::{
        QueryOperation, ReduceAssignment, Reducer,
    };
    use type_bridge_contract::value::CanonicalValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-try-live");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             attribute {age}, value integer;\n\
             entity {person}, owns {name}, owns {age} @card(0..1);",
            name = name.label(),
            age = age.label(),
            person = person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live try schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\", has {age} 30; \
             $b isa {person}, has {name} \"bob\";",
            person = person.label(),
            name = name.label(),
            age = age.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live try data");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-try-live").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-try-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    let match_stage = ReadStage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            },
            QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: name.clone(),
                owner: binding_id(0),
            },
            QueryPattern::Try {
                patterns: vec![QueryPattern::Has {
                    attribute: binding_id(2),
                    attribute_id: age.clone(),
                    owner: binding_id(0),
                }],
            },
        ],
    };

    // Projection: rows carry the age where present and absence where not.
    let plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "age"),
        ],
        Vec::new(),
        vec![
            match_stage.clone(),
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1), binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    assert!(
        validated.output_schema().rows().expect("row plan").columns()[1].optional()
    );
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("optional projection execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 2);
    let QueryRowValue::Attribute { value, .. } = &rows[0].values()[1] else {
        panic!("ada carries her age: {rows:?}");
    };
    assert_eq!(value, &CanonicalValue::Long(30));
    assert_eq!(rows[1].values()[1], QueryRowValue::Absent);

    // A total reducer over the optional binding skips absence.
    let count_plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "age"),
            binding(3, "age_count"),
        ],
        Vec::new(),
        vec![
            match_stage,
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(3),
                    Reducer::Count,
                    Some(binding_id(2)),
                )],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows { columns: vec![binding_id(3)] },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &count_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation =
        QueryInvocation::new(&count_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("optional count execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value { value: CanonicalValue::Long(1) },
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_row_given_invocations_correlate_inputs_live() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-given-live");
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
    .expect("live given schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\"; \
             $b isa {person}, has {name} \"bob\"; \
             $c isa {person}, has {name} \"eve\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live given data");

    // One prepared plan: exact name equality against a driver-bound input.
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
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
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: binding_id(1) },
                        right: QueryOperand::Input { column: InputColumnId::new(0) },
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("given plan");
    let validated = validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
        StructuralLimits::CANONICAL,
    )
    .expect("validated given plan");

    // Two input rows through one prepared plan, one provider call.
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![string_row("eve"), string_row("ada")],
    )
    .expect("multi-row invocation");
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("multi-row given execution");
    let names = row_names(&outcome);
    assert_eq!(names, vec!["ada".to_owned(), "eve".to_owned()]);

    // The same batch decides count in Rust over the validated stream.
    let count = QueryInvocation::new(
        &plan,
        QueryOperation::Count,
        vec![string_row("eve"), string_row("ada"), string_row("nobody")],
    )
    .expect("count invocation");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &count,
        limits(),
    )
    .await
    .expect("multi-row count execution");
    assert_eq!(outcome, QueryV2Outcome::Count(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_fetch_returns_typed_documents_live() {
    use type_bridge_contract::query_plan::{
        DocumentField, DocumentSource, QueryOperation,
    };
    use type_bridge_orm::query_v2::DocumentFieldValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-fetch-live");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             attribute {age}, value integer;\n\
             entity {person}, owns {name}, owns {age} @card(0..);",
            name = name.label(),
            age = age.label(),
            person = person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live fetch schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\", has {age} 30, has {age} 40; \
             $b isa {person}, has {name} \"bob\";",
            person = person.label(),
            name = name.label(),
            age = age.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live fetch data");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-fetch-live").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-fetch-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: person.clone(),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: name.clone(),
                        owner: binding_id(0),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Documents {
            fields: vec![
                DocumentField::new(
                    QueryVariable::new("name").unwrap(),
                    DocumentSource::Binding { binding: binding_id(1) },
                ),
                DocumentField::new(
                    QueryVariable::new("ages").unwrap(),
                    DocumentSource::AttributeList {
                        attribute: age.clone(),
                        owner: binding_id(0),
                    },
                ),
            ],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("document fetch execution");
    let QueryV2Outcome::Documents(documents) = &outcome else {
        panic!("documents outcome: {outcome:?}");
    };
    assert_eq!(documents.len(), 2);
    let scalar = |value: &DocumentFieldValue| match value {
        DocumentFieldValue::Scalar(CanonicalValue::String(value)) => {
            value.as_str().to_owned()
        }
        other => panic!("expected string scalar: {other:?}"),
    };
    let longs = |value: &DocumentFieldValue| match value {
        DocumentFieldValue::List(values) => values
            .iter()
            .map(|value| match value {
                CanonicalValue::Long(value) => *value,
                other => panic!("expected long element: {other:?}"),
            })
            .collect::<Vec<_>>(),
        other => panic!("expected list: {other:?}"),
    };
    assert_eq!(scalar(&documents[0].values()[0]), "ada");
    let mut ada_ages = longs(&documents[0].values()[1]);
    ada_ages.sort_unstable();
    assert_eq!(ada_ages, vec![30, 40]);
    assert_eq!(scalar(&documents[1].values()[0]), "bob");
    assert_eq!(longs(&documents[1].values()[1]), Vec::<i64>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_functions_execute_per_row_live() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{
        LocalFunction, LocalReturn, QueryOperation, Reducer,
    };
    use type_bridge_contract::value::CanonicalValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-local-fn-live");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             attribute {age}, value integer;\n\
             entity {person}, owns {name}, owns {age} @card(0..);",
            name = name.label(),
            age = age.label(),
            person = person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live local-fn schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\", has {age} 30, has {age} 40; \
             $b isa {person}, has {name} \"bob\";",
            person = person.label(),
            name = name.label(),
            age = age.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live local-fn data");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-local-fn-live").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-local-fn-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    let person_label = Label::new(person.label().as_str()).unwrap();
    let local = |fun_name: &str, reducer, value_type| {
        LocalFunction::new(
            FunctionId::new(fun_name).unwrap(),
            vec![binding(0, "subject"), binding(1, "measure")],
            vec![person_label.clone()],
            vec![QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: age.clone(),
                owner: binding_id(0),
            }],
            LocalReturn::new(reducer, binding_id(1), value_type),
        )
    };
    let call = |fun_name: &str, assigned: u16| QueryPattern::FunctionCall {
        arguments: vec![QueryOperand::Binding { binding: binding_id(0) }],
        assigned: binding_id(assigned),
        function: FunctionId::new(fun_name).unwrap(),
    };

    // Two locals per row: age count and age sum, sorted by count.
    let plan = QueryPlan::new_with_functions(
        vec![
            binding(0, "person"),
            binding(1, "age_count"),
            binding(2, "age_sum"),
        ],
        vec![
            local(
                &format!("{}_count", suffix.replace('-', "_")),
                Reducer::Count,
                ValueTypeTag::Long,
            ),
            local(
                &format!("{}_sum", suffix.replace('-', "_")),
                Reducer::Sum,
                ValueTypeTag::Long,
            ),
        ],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: person.clone(),
                    },
                    call(&format!("{}_count", suffix.replace('-', "_")), 1),
                    call(&format!("{}_sum", suffix.replace('-', "_")), 2),
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1), binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("local function execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let long = |value: &QueryRowValue| match value {
        QueryRowValue::Value { value: CanonicalValue::Long(value) } => *value,
        other => panic!("expected long value: {other:?}"),
    };
    let reduced = rows
        .iter()
        .map(|row| (long(&row.values()[0]), long(&row.values()[1])))
        .collect::<Vec<_>>();
    assert_eq!(reduced, vec![(0, 0), (2, 70)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_reachability_executes_live() {
    use type_bridge_contract::id::RoleId;
    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_contract::schema::{
        PlaysFact, PlaysFactId, RelatesFact, RelatesFactId,
    };
    use type_bridge_contract::value::CanonicalValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-reach-live");
    let node = TypeId::new(TypeKind::Entity, format!("{suffix}-node")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let edge = TypeId::new(TypeKind::Relation, format!("{suffix}-edge")).unwrap();
    let from = RoleId::new(edge.label().as_str(), "origin").unwrap();
    let to = RoleId::new(edge.label().as_str(), "destination").unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             relation {edge}, relates origin, relates destination;\n\
             entity {node}, owns {name}, plays {edge}:origin, plays {edge}:destination;",
            name = name.label(),
            edge = edge.label(),
            node = node.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live reach schema");
    db.execute_raw(
        &format!(
            "insert $a isa {node}, has {name} \"na\"; \
             $b isa {node}, has {name} \"nb\"; \
             $c isa {node}, has {name} \"nc\"; \
             $d isa {node}, has {name} \"nd\"; \
             (origin: $a, destination: $b) isa {edge}; \
             (origin: $b, destination: $c) isa {edge}; \
             (origin: $c, destination: $d) isa {edge};",
            node = node.label(),
            name = name.label(),
            edge = edge.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live reach data");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(node.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(TypeFact::new(edge.clone()).unwrap()),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(node.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), from.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), to.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), from.clone()).unwrap(),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), to.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-reach-live").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-reach-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    // Every node within two hops of "na", in one provider query.
    let plan = QueryPlan::new(
        vec![
            binding(0, "start"),
            binding(1, "start_name"),
            binding(2, "finish"),
            binding(3, "finish_name"),
        ],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: name.clone(),
                        owner: binding_id(0),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: binding_id(1) },
                        right: QueryOperand::Literal {
                            value: CanonicalValue::String(
                                CanonicalString::new("na").unwrap(),
                            ),
                        },
                    },
                    QueryPattern::Reachable {
                        max_depth: 2,
                        relation: edge.clone(),
                        role_from: from.clone(),
                        role_to: to.clone(),
                        source: binding_id(0),
                        target: binding_id(2),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(3),
                        attribute_id: name.clone(),
                        owner: binding_id(2),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(3), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows { columns: vec![binding_id(3)] },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .unwrap();
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("reachability execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let names = rows
        .iter()
        .map(|row| match &row.values()[0] {
            QueryRowValue::Attribute {
                value: CanonicalValue::String(value),
                ..
            } => value.as_str().to_owned(),
            other => panic!("expected string names: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["nb".to_owned(), "nc".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_envelope_round_trip_matches_local_execution_live() {
    use type_bridge_contract::capability::CapabilitySet as Caps;
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::query_remote::{RemoteLimits, RemoteQueryFailure};
    use type_bridge_orm::query_v2_remote::{
        decode_remote_outcome, encode_remote_request, execute_remote_envelope,
    };

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-remote-live");
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
    .expect("live remote schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\"; \
             $b isa {person}, has {name} \"bob\"; \
             $c isa {person}, has {name} \"eve\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live remote data");

    let (validated, plan) = validated_query(&fixture, OrderDirection::Ascending);
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("b")])
            .expect("invocation");

    // Local execution is the semantic reference.
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let local = execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        limits(),
    )
    .await
    .expect("local execution");
    assert_eq!(row_names(&local), vec!["bob".to_owned(), "eve".to_owned()]);

    // The same invocation travels the envelope and returns equal results.
    let nonce = "parity-nonce-0123456789abcdef";
    let caller_limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
    };
    let request = encode_remote_request(&validated, &invocation, caller_limits, nonce)
        .expect("request envelope");
    let context = MigrationAssertionValidationContext::new(
        &fixture.resolved,
        &fixture.managed,
    );
    let mut server_transaction =
        db.read_transaction().await.expect("server transaction");
    let response = execute_remote_envelope(
        &request,
        &context,
        &query_plan_capability_vocabulary(),
        &mut server_transaction,
        limits(),
    )
    .await;
    let remote = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        nonce,
        caller_limits,
    )
    .expect("remote outcome");
    assert_eq!(remote, local);

    // Replayed evidence: a foreign nonce never constructs host objects.
    let error = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        "some-other-nonce-9876543210",
        caller_limits,
    )
    .expect_err("foreign nonce");
    assert_eq!(error.code().as_str(), "query_remote_nonce_mismatch");

    // Forged owner: evidence for a different plan is rejected.
    let (other_validated, _) = validated_query(&fixture, OrderDirection::Descending);
    let error = decode_remote_outcome(
        &response,
        &other_validated,
        QueryOperation::Rows,
        nonce,
        caller_limits,
    )
    .expect_err("foreign plan");
    assert_eq!(error.code().as_str(), "query_remote_plan_mismatch");

    // Oversized evidence rejects before decoding.
    let error = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        nonce,
        RemoteLimits {
            deadline_ms: None,
            max_bytes: 16,
            max_items: 100,
        },
    )
    .expect_err("oversized response");
    assert_eq!(error.code().as_str(), "query_remote_response_oversized");

    // Unknown capability: an executor advertising nothing rejects the
    // plan before data I/O with a structured failure envelope.
    let response = execute_remote_envelope(
        &request,
        &context,
        &Caps::new(),
        &mut server_transaction,
        limits(),
    )
    .await;
    let failure = RemoteQueryFailure::decode(&response).expect("failure envelope");
    assert_eq!(
        failure.diagnostic().expect("diagnostic").code().as_str(),
        "query_remote_capability_unsupported",
    );
    assert_eq!(failure.nonce(), Some(nonce));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_envelope_parity_corpus_live() {
    use type_bridge_contract::id::{FunctionId, Label, RoleId};
    use type_bridge_contract::query_plan::{
        DocumentField, DocumentSource, LocalFunction, LocalReturn, QueryOperation,
        ReduceAssignment, Reducer,
    };
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::query_remote::RemoteLimits;
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
        FunctionReturnMode, FunctionSignature, PlaysFact, PlaysFactId, RelatesFact,
        RelatesFactId, TypeReference,
    };
    use type_bridge_orm::query_v2_remote::{
        decode_remote_outcome, encode_remote_request, execute_remote_envelope,
    };

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-parity-corpus");
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();
    let edge = TypeId::new(TypeKind::Relation, format!("{suffix}-edge")).unwrap();
    let origin = RoleId::new(edge.label().as_str(), "origin").unwrap();
    let destination = RoleId::new(edge.label().as_str(), "destination").unwrap();
    let age_sum = FunctionId::new(format!("{}_age_sum", suffix.replace('-', "_")))
        .unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             attribute {age}, value integer;\n\
             relation {edge}, relates origin, relates destination;\n\
             entity {person}, owns {name}, owns {age} @card(0..), \
             plays {edge}:origin, plays {edge}:destination;\n\
             fun {age_sum}($subject: {person}) -> integer:\n\
             match $subject has {age} $a;\n\
             return sum($a);",
            name = name.label(),
            age = age.label(),
            edge = edge.label(),
            person = person.label(),
            age_sum = age_sum.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("corpus schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\", has {age} 30, has {age} 40; \
             $b isa {person}, has {name} \"bob\", has {age} 25; \
             $c isa {person}, has {name} \"eve\"; \
             (origin: $a, destination: $b) isa {edge}; \
             (origin: $b, destination: $c) isa {edge};",
            person = person.label(),
            name = name.label(),
            age = age.label(),
            edge = edge.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("corpus data");

    let person_label = Label::new(person.label().as_str()).unwrap();
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap(),
            )
            .unwrap(),
        ),
        SchemaFact::Type(TypeFact::new(edge.clone()).unwrap()),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).unwrap(),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), origin.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), destination.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), origin.clone()).unwrap(),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), destination.clone()).unwrap(),
        )),
        SchemaFact::Function(FunctionFact::new(
            age_sum.clone(),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("subject").unwrap(),
                    TypeReference::Schema(person_label.clone()),
                )],
                FunctionReturnMode::scalar(FunctionReturnElement::new(
                    TypeReference::Value(ValueTypeTag::Long),
                    false,
                )),
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
                DocumentId::new("query-v2-parity-corpus").unwrap(),
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
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-v2-parity-corpus").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let semantics = managed.managed_semantic_schema().clone();

    let match_person_name = |extra: Vec<QueryPattern>| {
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            },
            QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: name.clone(),
                owner: binding_id(0),
            },
        ];
        patterns.extend(extra);
        ReadStage::Match { patterns }
    };
    let sort_by = |binding: u16| ReadStage::Sort {
        terms: vec![OrderTerm::new(binding_id(binding), OrderDirection::Ascending)],
    };

    // The corpus: one plan per Phase 6 capability family.
    let corpus: Vec<(&str, QueryPlan, Vec<InputRow>)> = vec![
        (
            "optional-projection",
            QueryPlan::new(
                vec![
                    binding(0, "person"),
                    binding(1, "name"),
                    binding(2, "age"),
                ],
                Vec::new(),
                vec![
                    match_person_name(vec![QueryPattern::Try {
                        patterns: vec![QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: age.clone(),
                            owner: binding_id(0),
                        }],
                    }]),
                    sort_by(1),
                ],
                QueryOutput::Rows {
                    columns: vec![binding_id(1), binding_id(2)],
                },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
        (
            "grouped-reduce",
            QueryPlan::new(
                vec![
                    binding(0, "person"),
                    binding(1, "name"),
                    binding(2, "age"),
                    binding(3, "age_total"),
                ],
                Vec::new(),
                vec![
                    match_person_name(vec![QueryPattern::Has {
                        attribute: binding_id(2),
                        attribute_id: age.clone(),
                        owner: binding_id(0),
                    }]),
                    ReadStage::Reduce {
                        assignments: vec![ReduceAssignment::new(
                            binding_id(3),
                            Reducer::Sum,
                            Some(binding_id(2)),
                        )],
                        groups: vec![binding_id(0)],
                    },
                    sort_by(3),
                ],
                QueryOutput::Rows {
                    columns: vec![binding_id(0), binding_id(3)],
                },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
        (
            "document-fetch",
            QueryPlan::new(
                vec![binding(0, "person"), binding(1, "name")],
                Vec::new(),
                vec![match_person_name(Vec::new()), sort_by(1)],
                QueryOutput::Documents {
                    fields: vec![
                        DocumentField::new(
                            QueryVariable::new("name").unwrap(),
                            DocumentSource::Binding { binding: binding_id(1) },
                        ),
                        DocumentField::new(
                            QueryVariable::new("ages").unwrap(),
                            DocumentSource::AttributeList {
                                attribute: age.clone(),
                                owner: binding_id(0),
                            },
                        ),
                    ],
                },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
        (
            "multi-row-given",
            QueryPlan::new(
                vec![binding(0, "person"), binding(1, "name")],
                vec![InputColumn::new(
                    InputColumnId::new(0),
                    QueryVariable::new("wanted_name").unwrap(),
                    ValueTypeTag::String,
                    false,
                )],
                vec![
                    match_person_name(vec![QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: binding_id(1) },
                        right: QueryOperand::Input { column: InputColumnId::new(0) },
                    }]),
                    sort_by(1),
                ],
                QueryOutput::Rows {
                    columns: vec![binding_id(0), binding_id(1)],
                },
                semantics.clone(),
            )
            .unwrap(),
            vec![string_row("eve"), string_row("ada")],
        ),
        (
            "schema-function-call",
            QueryPlan::new(
                vec![
                    binding(0, "person"),
                    binding(1, "name"),
                    binding(2, "age"),
                    binding(3, "age_total"),
                ],
                Vec::new(),
                vec![
                    match_person_name(vec![
                        QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: age.clone(),
                            owner: binding_id(0),
                        },
                        QueryPattern::FunctionCall {
                            arguments: vec![QueryOperand::Binding {
                                binding: binding_id(0),
                            }],
                            assigned: binding_id(3),
                            function: age_sum.clone(),
                        },
                    ]),
                    sort_by(3),
                ],
                QueryOutput::Rows { columns: vec![binding_id(3)] },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
        (
            "local-function-call",
            QueryPlan::new_with_functions(
                vec![
                    binding(0, "person"),
                    binding(1, "name"),
                    binding(2, "age_count"),
                ],
                vec![LocalFunction::new(
                    FunctionId::new("corpus_age_count").unwrap(),
                    vec![binding(0, "subject"), binding(1, "measure")],
                    vec![person_label.clone()],
                    vec![QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: age.clone(),
                        owner: binding_id(0),
                    }],
                    LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
                )],
                Vec::new(),
                vec![
                    match_person_name(vec![QueryPattern::FunctionCall {
                        arguments: vec![QueryOperand::Binding {
                            binding: binding_id(0),
                        }],
                        assigned: binding_id(2),
                        function: FunctionId::new("corpus_age_count").unwrap(),
                    }]),
                    sort_by(1),
                ],
                QueryOutput::Rows {
                    columns: vec![binding_id(1), binding_id(2)],
                },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
        (
            "bounded-reachability",
            QueryPlan::new(
                vec![
                    binding(0, "person"),
                    binding(1, "name"),
                    binding(2, "other"),
                    binding(3, "other_name"),
                ],
                Vec::new(),
                vec![
                    match_person_name(vec![
                        QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding { binding: binding_id(1) },
                            right: QueryOperand::Literal {
                                value: CanonicalValue::String(
                                    CanonicalString::new("ada").unwrap(),
                                ),
                            },
                        },
                        QueryPattern::Reachable {
                            max_depth: 2,
                            relation: edge.clone(),
                            role_from: origin.clone(),
                            role_to: destination.clone(),
                            source: binding_id(0),
                            target: binding_id(2),
                        },
                        QueryPattern::Has {
                            attribute: binding_id(3),
                            attribute_id: name.clone(),
                            owner: binding_id(2),
                        },
                    ]),
                    sort_by(3),
                ],
                QueryOutput::Rows { columns: vec![binding_id(3)] },
                semantics.clone(),
            )
            .unwrap(),
            Vec::new(),
        ),
    ];

    let caller_limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 1000,
    };
    let mut transaction = db.read_transaction().await.expect("local transaction");
    let mut server_transaction =
        db.read_transaction().await.expect("server transaction");
    for (index, (label, plan, rows)) in corpus.iter().enumerate() {
        let validated =
            validate_query_plan(plan, &context, StructuralLimits::CANONICAL)
                .unwrap_or_else(|error| panic!("{label}: validation: {error}"));
        let invocation =
            QueryInvocation::new(plan, QueryOperation::Rows, rows.clone())
                .unwrap_or_else(|error| panic!("{label}: invocation: {error}"));
        let local = execute_validated_query(
            &mut transaction,
            &validated,
            &invocation,
            limits(),
        )
        .await
        .unwrap_or_else(|error| panic!("{label}: local execution: {error}"));

        let nonce = format!("corpus-parity-nonce-{index:04}");
        let request =
            encode_remote_request(&validated, &invocation, caller_limits, &nonce)
                .unwrap_or_else(|error| panic!("{label}: request: {error}"));
        let response = execute_remote_envelope(
            &request,
            &context,
            &query_plan_capability_vocabulary(),
            &mut server_transaction,
            limits(),
        )
        .await;
        let remote = decode_remote_outcome(
            &response,
            &validated,
            QueryOperation::Rows,
            &nonce,
            caller_limits,
        )
        .unwrap_or_else(|error| panic!("{label}: remote outcome: {error}"));
        assert_eq!(remote, local, "{label}: remote and local outcomes differ");
    }
}
