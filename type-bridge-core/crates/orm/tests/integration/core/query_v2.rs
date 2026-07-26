//! End-to-end V2 query execution against TypeDB 3.12.1.

use std::env;
use std::time::Duration;

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use crate::common::typedb::connect_options_from_env;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_REMOTE_ENVELOPE_BYTES, StructuralLimits};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm, QueryInvocation, QueryOperand,
    QueryOperation, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteExecutorBinding, RemoteQueryFailure, decode_signed_remote_failure,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SourceSpan,
    SourcedSchemaFact, SubFact, SubFactId, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_orm::query_v2::{QueryRowValue, QueryV2Outcome, execute_validated_query};
use type_bridge_orm::query_v2_remote::{Ed25519RemoteReplyVerifier, RemoteReplySigningKey};
use type_bridge_orm::session::backend::{
    AnswerCancellation, BoundedAnswerLimits, QueryV2AnswerLimits,
};
use type_bridge_orm::{Database, TxType};
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

struct LiveQueryFixture {
    managed: type_bridge_contract::schema_delta::ManagedSchemaState,
    name: AttributeId,
    person: TypeId,
    resolved: ResolvedSchema,
}

fn remote_advertisement(capabilities: CapabilitySet) -> RemoteCapabilities {
    RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("orm-live-query-executor", "orm-live-query-epoch-000001")
            .expect("remote executor binding"),
        remote_signer().public_key(),
    )
}

fn remote_signer() -> RemoteReplySigningKey {
    RemoteReplySigningKey::from_secret_bytes([0x2b; 32])
}

fn decode_authenticated_failure(
    bytes: &[u8],
    advertisement: &RemoteCapabilities,
) -> RemoteQueryFailure {
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    decode_signed_remote_failure(
        bytes,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("remote envelope ceiling fits u64"),
        &Ed25519RemoteReplyVerifier,
    )
    .expect("authenticated failure envelope")
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
        // Windowed live plans sort by the name attribute; the unique
        // ownership proves the sort tuple total for the person column and
        // matches the live schema definition text.
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(
                        OwnsFactId::new(person.clone(), name.clone()).unwrap(),
                    ),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .unwrap(),
        ),
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
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
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

fn limits() -> QueryV2AnswerLimits {
    QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 1 << 20,
            deadline: None,
            cancellation: AnswerCancellation::default(),
        },
        max_collection_members: 1 << 16,
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
             entity {}, owns {} @unique;",
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
    let mut transaction = db
        .read_transaction()
        .await
        .expect("borrowed read transaction");

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

    let (descending, descending_plan) = validated_query(&fixture, OrderDirection::Descending);
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

#[tokio::test]
async fn v2_exists_semantic_limit_releases_database_immediately_live() {
    let _guard = crate::common::integration_test_guard().await;
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let database_name = unique_schema_suffix("rust", "query-v2-exists-terminal");
    let db = Database::connect_with_options(
        &address,
        &database_name,
        &username,
        &password,
        connect_options_from_env(),
    )
    .await
    .expect("V2 terminal exists fixture should connect");
    db.create_database()
        .await
        .expect("V2 terminal exists database should be created");
    let suffix = unique_schema_suffix("rust", "query-v2-exists-terminal-type");
    let fixture = live_fixture(&suffix);
    db.execute_raw(
        &format!(
            "define attribute {}, value string; entity {}, owns {} @unique;",
            fixture.name.label(),
            fixture.person.label(),
            fixture.name.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("V2 terminal exists schema should be defined");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"Ada\"; \
             $b isa {person}, has {name} \"Alan\"; \
             $c isa {person}, has {name} \"Grace\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("V2 terminal exists rows should commit");

    let (validated, plan) = validated_query(&fixture, OrderDirection::Ascending);
    let invocation = QueryInvocation::new(&plan, QueryOperation::Exists, vec![string_row("A")])
        .expect("V2 terminal exists invocation");
    let mut transaction = db
        .read_transaction()
        .await
        .expect("V2 terminal exists transaction should open");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("V2 terminal exists should execute");
    assert_eq!(outcome, QueryV2Outcome::Exists(true));
    transaction
        .close()
        .await
        .expect("V2 terminal exists transaction should close");
    drop(transaction);

    tokio::time::timeout(Duration::from_secs(10), db.delete_database())
        .await
        .expect("V2 exists must not leave database deletion blocked")
        .expect("V2 exists must release the database before delete returns");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scalar_schema_function_calls_execute_live() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, TypeReference,
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
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
    let profile =
        type_bridge_contract::fingerprint::SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = type_bridge_schema::resolve(&declared, &profile).unwrap();
    let managed = type_bridge_schema::managed_schema_state(
        &declared,
        &type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("query-v2-fn-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &count_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation = QueryInvocation::new(&count_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let lowered = lower_validated_query(&validated, &invocation).unwrap();
    assert!(
        lowered.typeql().contains("let $person_count = "),
        "{}",
        lowered.typeql(),
    );
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
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
            AssertionBinding::new(binding_id(1), QueryVariable::new("age_sum").unwrap()),
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
                        arguments: vec![type_bridge_contract::query_plan::QueryOperand::Binding {
                            binding: binding_id(0),
                        }],
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
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&sum_plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&sum_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
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
    use type_bridge_contract::query_plan::{QueryOperation, ReduceAssignment, Reducer};
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
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
                    ReduceAssignment::new(binding_id(2), Reducer::Sum, Some(binding_id(1))),
                    ReduceAssignment::new(binding_id(3), Reducer::Count, Some(binding_id(1))),
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
    let invocation = QueryInvocation::new(&grouped_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
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
                QueryRowValue::Value {
                    value: CanonicalValue::Long(value),
                } => *value,
                other => panic!("expected long value: {other:?}"),
            };
            (long(&row.values()[1]), long(&row.values()[2]))
        })
        .collect::<Vec<_>>();
    assert_eq!(reduced, vec![(25, 1), (70, 2)]);

    // Global: one bare count row totals every match row.
    let global_plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age"), binding(2, "total")],
        Vec::new(),
        vec![
            match_stage,
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(binding_id(2), Reducer::Count, None)],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &global_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation = QueryInvocation::new(&global_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("global count execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value {
            value: CanonicalValue::Long(3)
        },
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_blocks_carry_optional_columns_live() {
    use type_bridge_contract::query_plan::{QueryOperation, ReduceAssignment, Reducer};
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
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
        vec![binding(0, "person"), binding(1, "name"), binding(2, "age")],
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
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    assert!(
        validated
            .output_schema()
            .rows()
            .expect("row plan")
            .columns()[1]
            .optional()
    );
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
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
        QueryOutput::Rows {
            columns: vec![binding_id(3)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &count_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation = QueryInvocation::new(&count_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("optional count execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value {
            value: CanonicalValue::Long(1)
        },
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
             entity {}, owns {} @unique;",
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
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
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
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
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
    let outcome = execute_validated_query(&mut transaction, &validated, &count, limits())
        .await
        .expect("multi-row count execution");
    assert_eq!(outcome, QueryV2Outcome::Count(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_fetch_returns_typed_documents_live() {
    use type_bridge_contract::query_plan::{DocumentField, DocumentSource, QueryOperation};
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
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
                    DocumentSource::Binding {
                        binding: binding_id(1),
                    },
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
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("document fetch execution");
    let QueryV2Outcome::Documents(documents) = &outcome else {
        panic!("documents outcome: {outcome:?}");
    };
    assert_eq!(documents.len(), 2);
    let scalar = |value: &DocumentFieldValue| match value {
        DocumentFieldValue::Scalar(CanonicalValue::String(value)) => value.as_str().to_owned(),
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

    // Executable lowering uses a list-valued read subquery with an N+1
    // sentinel. Ada's two ages therefore prove both that the TypeQL syntax
    // preserves scalar-list output and that a one-member budget detects the
    // over-limit ownership without fetching the unbounded list form.
    let mut one_member = limits();
    one_member.max_collection_members = 1;
    let error = execute_validated_query(&mut transaction, &validated, &invocation, one_member)
        .await
        .expect_err("the second age is the over-limit sentinel");
    let type_bridge_orm::query_v2::QueryV2ExecutionError::Provider(
        type_bridge_orm::OrmError::Match(error),
    ) = error
    else {
        panic!("document member limit must surface from the bounded provider: {error}");
    };
    assert_eq!(
        error.category(),
        type_bridge_orm::match_request::MatchErrorCategory::ResourceLimit
    );
    assert_eq!(error.code().as_str(), "query_v2_document_member_limit");
    assert_eq!(
        error.message(),
        "document lists exceed the aggregate member ceiling"
    );
    assert_eq!(
        error.path().segments(),
        &[type_bridge_orm::match_request::MatchErrorPathSegment::ProviderEvidence]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_functions_execute_per_row_live() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, QueryOperation, Reducer};
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
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
        arguments: vec![QueryOperand::Binding {
            binding: binding_id(0),
        }],
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
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("local function execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let long = |value: &QueryRowValue| match value {
        QueryRowValue::Value {
            value: CanonicalValue::Long(value),
        } => *value,
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
    use type_bridge_contract::query_plan::{QueryOperation, ReduceAssignment, Reducer};
    use type_bridge_contract::schema::{PlaysFact, PlaysFactId, RelatesFact, RelatesFactId};
    use type_bridge_contract::value::CanonicalValue;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-reach-live");
    let node = TypeId::new(TypeKind::Entity, format!("{suffix}-node")).unwrap();
    let node_child = TypeId::new(TypeKind::Entity, format!("{suffix}-node-child")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let age = AttributeId::new(format!("{suffix}-age")).unwrap();
    let edge = TypeId::new(TypeKind::Relation, format!("{suffix}-edge")).unwrap();
    let from = RoleId::new(edge.label().as_str(), "origin").unwrap();
    let to = RoleId::new(edge.label().as_str(), "destination").unwrap();
    let express_edge = TypeId::new(TypeKind::Relation, format!("{suffix}-express-edge")).unwrap();
    let express_from = RoleId::new(express_edge.label().as_str(), "express-origin").unwrap();
    let express_to = RoleId::new(express_edge.label().as_str(), "express-destination").unwrap();

    db.execute_raw(
        &format!(
            "define\n\
             attribute {name}, value string;\n\
             attribute {age}, value integer;\n\
             relation {edge}, relates origin, relates destination;\n\
             relation {express_edge} sub {edge}, \
               relates express-origin as origin, \
               relates express-destination as destination;\n\
             entity {node}, owns {name}, owns {age} @card(0..1), \
               plays {edge}:origin, plays {edge}:destination, \
               plays {express_edge}:express-origin, \
               plays {express_edge}:express-destination;\n\
             entity {node_child} sub {node};",
            name = name.label(),
            age = age.label(),
            edge = edge.label(),
            express_edge = express_edge.label(),
            node = node.label(),
            node_child = node_child.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live reach schema");
    db.execute_raw(
        &format!(
            "insert $a isa {node_child}, has {name} \"na\"; \
             $b isa {node}, has {name} \"nb\", has {age} 10; \
             $c isa {node}, has {name} \"nc\"; \
             $d isa {node}, has {name} \"nd\"; \
             $e isa {node}, has {name} \"ne\"; \
             (origin: $a, destination: $b) isa {edge}; \
             (origin: $b, destination: $c) isa {edge}; \
             (origin: $a, destination: $c) isa {edge}; \
             (origin: $a, destination: $c) isa {edge}; \
             (origin: $c, destination: $d) isa {edge}; \
             (express-origin: $a, express-destination: $e) isa {express_edge};",
            node = node.label(),
            node_child = node_child.label(),
            name = name.label(),
            age = age.label(),
            edge = edge.label(),
            express_edge = express_edge.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live reach data");
    let widened_control = db
        .execute_raw(
            &format!(
                "match \
                 $a has {name} \"na\"; \
                 $e has {name} \"ne\"; \
                 (origin: $a, destination: $e) isa {edge}; \
                 select $e;",
                name = name.label(),
                edge = edge.label(),
            ),
            TxType::Read,
        )
        .await
        .expect("non-strict parent relation control query");
    assert!(
        matches!(
            widened_control,
            type_bridge_orm::session::backend::QueryResult::Rows(rows) if rows.len() == 1
        ),
        "fixture must expose the subtype edge to a non-strict parent relation match",
    );

    let facts = vec![
        SchemaFact::Type(TypeFact::new(node.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(node_child.clone()).unwrap()),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(node_child, node.clone()).unwrap(),
        )),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
        ),
        SchemaFact::Type(TypeFact::new(edge.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(express_edge.clone()).unwrap()),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(express_edge.clone(), edge.clone()).unwrap(),
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(node.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(node.clone(), age.clone()).unwrap(),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), from.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Relates(
            RelatesFact::new(RelatesFactId::new(edge.clone(), to.clone()).unwrap(), None).unwrap(),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(express_edge.clone(), express_from.clone()).unwrap(),
                Some(from.clone()),
            )
            .unwrap(),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(express_edge.clone(), express_to.clone()).unwrap(),
                Some(to.clone()),
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), from.clone()).unwrap(),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), to.clone()).unwrap(),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), express_from.clone()).unwrap(),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), express_to.clone()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

    // Every node within zero through three exact base-edge hops of subtype
    // instance "na", in one provider query. This exercises the mixed
    // identity/relation planner workaround over a validator-derived source
    // domain containing both the base type and its concrete subtype. `nc` has
    // one indirect and two parallel direct proofs, but Reachable is
    // existential and must expose the endpoint exactly once. The direct
    // express-edge subtype hop to `ne` must not widen this exact relation.
    let plan = QueryPlan::new_v2(
        vec![
            binding(0, "start"),
            binding(1, "start_name"),
            binding(2, "finish"),
            binding(3, "finish_name"),
            binding(4, "finish_age"),
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
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Literal {
                            value: CanonicalValue::String(CanonicalString::new("na").unwrap()),
                        },
                    },
                    QueryPattern::Reachable {
                        min_depth: 0,
                        max_depth: 3,
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
                    QueryPattern::Try {
                        patterns: vec![QueryPattern::Has {
                            attribute: binding_id(4),
                            attribute_id: age.clone(),
                            owner: binding_id(2),
                        }],
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(3), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(3), binding_id(4)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("reachability execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let names_and_ages = rows
        .iter()
        .map(|row| {
            let name = match &row.values()[0] {
                QueryRowValue::Attribute {
                    value: CanonicalValue::String(value),
                    ..
                } => value.as_str().to_owned(),
                other => panic!("expected string names: {other:?}"),
            };
            let age = match &row.values()[1] {
                QueryRowValue::Attribute {
                    value: CanonicalValue::Long(value),
                    ..
                } => Some(*value),
                QueryRowValue::Absent => None,
                other => panic!("expected optional integer age: {other:?}"),
            };
            (name, age)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names_and_ages,
        vec![
            ("na".to_owned(), None),
            ("nb".to_owned(), Some(10)),
            ("nc".to_owned(), None),
            ("nd".to_owned(), None),
        ],
    );

    let express_plan = QueryPlan::new_v2(
        vec![
            binding(0, "express_start"),
            binding(1, "express_start_name"),
            binding(2, "express_finish"),
            binding(3, "express_finish_name"),
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
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Literal {
                            value: CanonicalValue::String(CanonicalString::new("na").unwrap()),
                        },
                    },
                    QueryPattern::Reachable {
                        min_depth: 1,
                        max_depth: 1,
                        relation: express_edge,
                        role_from: express_from,
                        role_to: express_to,
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
        QueryOutput::Rows {
            columns: vec![binding_id(3)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &express_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation = QueryInvocation::new(&express_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("exact subtype reachability execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(
        rows.iter()
            .map(|row| match &row.values()[0] {
                QueryRowValue::Attribute {
                    value: CanonicalValue::String(value),
                    ..
                } => value.as_str(),
                other => panic!("expected express-edge target name: {other:?}"),
            })
            .collect::<Vec<_>>(),
        vec!["ne"],
    );

    // A user reducer observes endpoint rows, not the multiplicity of path
    // proofs that the internal reachability aggregate discarded.
    let count_plan = QueryPlan::new_v2(
        vec![
            binding(0, "start"),
            binding(1, "start_name"),
            binding(2, "finish"),
            binding(3, "finish_name"),
            binding(4, "finish_age"),
            binding(5, "finish_count"),
        ],
        Vec::new(),
        vec![
            plan.pipeline()[0].clone(),
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(binding_id(5), Reducer::Count, None)],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(5)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validated = validate_query_plan(
        &count_plan,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .unwrap();
    let invocation = QueryInvocation::new(&count_plan, QueryOperation::Rows, Vec::new()).unwrap();
    let outcome = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("reachability count execution");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    assert_eq!(
        rows[0].values()[0],
        QueryRowValue::Value {
            value: CanonicalValue::Long(4),
        },
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_envelope_round_trip_matches_local_execution_live() {
    use type_bridge_contract::capability::CapabilitySet as Caps;
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::query_remote::RemoteLimits;
    use type_bridge_orm::query_v2_remote::{
        decode_remote_outcome, encode_remote_request, execute_admitted_remote_request,
        preflight_remote_request,
    };

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-remote-live");
    let fixture = live_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             entity {}, owns {} @unique;",
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
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("b")])
        .expect("invocation");

    // Local execution is the semantic reference.
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let local = execute_validated_query(&mut transaction, &validated, &invocation, limits())
        .await
        .expect("local execution");
    assert_eq!(row_names(&local), vec!["bob".to_owned(), "eve".to_owned()]);

    // The same invocation travels the envelope and returns equal results.
    let nonce = "parity-nonce-0123456789abcdef";
    let caller_limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    };
    let advertisement = remote_advertisement(query_plan_capability_vocabulary());
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    let signer = remote_signer();
    let request = encode_remote_request(
        &validated,
        &invocation,
        &advertisement,
        caller_limits,
        nonce,
    )
    .expect("request envelope");
    let expected_request =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
            .expect("request fingerprint");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let admitted = preflight_remote_request(&request, &context, &advertisement, limits())
        .unwrap_or_else(|rejection| {
            panic!("remote request preflight: {}", rejection.diagnostic_code())
        });
    let mut server_transaction = db.read_transaction().await.expect("server transaction");
    let response =
        execute_admitted_remote_request(admitted, &mut server_transaction, &signer).await;
    let remote = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        nonce,
        &expected_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        caller_limits,
    )
    .expect("remote outcome");
    assert_eq!(remote, local);

    // Same-nonce replay with different rows: the response is bound to the
    // whole request envelope, so evidence for invocation A can never be
    // accepted as the answer to invocation B even under a reused nonce.
    let other_invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("e")])
        .expect("other invocation");
    let other_request = encode_remote_request(
        &validated,
        &other_invocation,
        &advertisement,
        caller_limits,
        nonce,
    )
    .expect("other request envelope");
    let other_fingerprint =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&other_request)
            .expect("other request fingerprint");
    let error = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        nonce,
        &other_fingerprint,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        caller_limits,
    )
    .expect_err("same-nonce different-rows replay");
    assert_eq!(error.code().as_str(), "query_remote_request_mismatch");

    // Replayed evidence: a foreign nonce never constructs host objects.
    let error = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        "some-other-nonce-9876543210",
        &expected_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
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
        &expected_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
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
        &expected_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        RemoteLimits {
            deadline_ms: None,
            max_bytes: 16,
            max_items: 100,
            max_collection_members: 1 << 16,
        },
    )
    .expect_err("oversized response");
    assert_eq!(error.code().as_str(), "query_remote_response_oversized");

    // Unknown capability: an executor advertising nothing rejects the
    // plan before data I/O with a structured failure envelope.
    let starved = remote_advertisement(Caps::new());
    let starved_request =
        encode_remote_request(&validated, &invocation, &starved, caller_limits, nonce)
            .expect("starved request");
    let starved_fingerprint =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&starved_request)
            .expect("starved request fingerprint");
    let rejection = match preflight_remote_request(&starved_request, &context, &starved, limits()) {
        Ok(_) => panic!("capability-starved request must reject in preflight"),
        Err(rejection) => rejection,
    };
    let starved_advertisement_fingerprint = starved
        .fingerprint()
        .expect("starved advertisement fingerprint");
    let response = rejection.into_failure_envelope(&starved_advertisement_fingerprint, &signer);
    let failure = decode_authenticated_failure(&response, &starved);
    assert_eq!(
        failure.diagnostic().expect("diagnostic").code().as_str(),
        "query_remote_capability_unsupported",
    );
    assert_eq!(failure.nonce(), Some(nonce));
    failure
        .verify_binding(nonce, &starved_fingerprint)
        .expect("post-decode failures bind the exact request envelope");
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
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId, TypeReference,
    };
    use type_bridge_orm::query_v2_remote::{
        decode_remote_outcome, encode_remote_request, execute_admitted_remote_request,
        preflight_remote_request,
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
    let age_sum = FunctionId::new(format!("{}_age_sum", suffix.replace('-', "_"))).unwrap();

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
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, age.label().as_str()).unwrap()).unwrap(),
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
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
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
        terms: vec![OrderTerm::new(
            binding_id(binding),
            OrderDirection::Ascending,
        )],
    };

    // The corpus: one plan per Phase 6 capability family.
    let corpus: Vec<(&str, QueryPlan, Vec<InputRow>)> = vec![
        (
            "optional-projection",
            QueryPlan::new(
                vec![binding(0, "person"), binding(1, "name"), binding(2, "age")],
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
                            DocumentSource::Binding {
                                binding: binding_id(1),
                            },
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
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
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
                QueryOutput::Rows {
                    columns: vec![binding_id(3)],
                },
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
                            left: QueryOperand::Binding {
                                binding: binding_id(1),
                            },
                            right: QueryOperand::Literal {
                                value: CanonicalValue::String(CanonicalString::new("ada").unwrap()),
                            },
                        },
                        QueryPattern::Reachable {
                            min_depth: 1,
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
                QueryOutput::Rows {
                    columns: vec![binding_id(3)],
                },
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
        max_collection_members: 1 << 16,
    };
    let mut transaction = db.read_transaction().await.expect("local transaction");
    // The live provider transports multi-row given batches, so the executor
    // truthfully advertises the transport capability under one stable epoch.
    let mut advertised = query_plan_capability_vocabulary();
    advertised.insert(type_bridge_contract::query_given_rows_capability());
    let advertisement = remote_advertisement(advertised);
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    let signer = remote_signer();
    for (index, (label, plan, rows)) in corpus.iter().enumerate() {
        let validated = validate_query_plan(plan, &context, StructuralLimits::CANONICAL)
            .unwrap_or_else(|error| panic!("{label}: validation: {error}"));
        let invocation = QueryInvocation::new(plan, QueryOperation::Rows, rows.clone())
            .unwrap_or_else(|error| panic!("{label}: invocation: {error}"));
        let local = execute_validated_query(&mut transaction, &validated, &invocation, limits())
            .await
            .unwrap_or_else(|error| panic!("{label}: local execution: {error}"));

        let nonce = format!("corpus-parity-nonce-{index:04}");
        let request = encode_remote_request(
            &validated,
            &invocation,
            &advertisement,
            caller_limits,
            &nonce,
        )
        .unwrap_or_else(|error| panic!("{label}: request: {error}"));
        let expected_request =
            type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
                .unwrap_or_else(|error| panic!("{label}: request fingerprint: {error}"));
        let admitted = preflight_remote_request(&request, &context, &advertisement, limits())
            .unwrap_or_else(|rejection| {
                panic!("{label}: remote preflight: {}", rejection.diagnostic_code())
            });
        let mut server_transaction = db
            .read_transaction()
            .await
            .unwrap_or_else(|error| panic!("{label}: server transaction: {error}"));
        let response =
            execute_admitted_remote_request(admitted, &mut server_transaction, &signer).await;
        let remote = decode_remote_outcome(
            &response,
            &validated,
            QueryOperation::Rows,
            &nonce,
            &expected_request,
            &advertisement_fingerprint,
            advertisement.reply_key(),
            caller_limits,
        )
        .unwrap_or_else(|error| panic!("{label}: remote outcome: {error}"));
        assert_eq!(remote, local, "{label}: remote and local outcomes differ");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadlines_and_cancellation_bound_both_executors_live() {
    use std::time::{Duration, Instant};

    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::query_remote::RemoteLimits;
    use type_bridge_orm::query_v2_remote::{encode_remote_request, preflight_remote_request};
    use type_bridge_orm::session::backend::AnswerCancellation;

    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-deadline-live");
    let fixture = live_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             entity {}, owns {} @unique;",
            fixture.name.label(),
            fixture.person.label(),
            fixture.name.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live deadline schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live deadline data");

    let (validated, plan) = validated_query(&fixture, OrderDirection::Ascending);
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("a")])
        .expect("invocation");

    // Local: an already-expired deadline rejects before streaming.
    let mut transaction = db.read_transaction().await.expect("read transaction");
    let expired = QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 1 << 20,
            deadline: Some(Instant::now() - Duration::from_secs(1)),
            cancellation: AnswerCancellation::default(),
        },
        max_collection_members: 1 << 16,
    };
    let error = execute_validated_query(&mut transaction, &validated, &invocation, expired)
        .await
        .expect_err("expired local deadline");
    assert!(
        error.to_string().contains("deadline"),
        "local deadline error: {error}",
    );

    // Remote: a zero caller deadline becomes an immediately elapsed absolute
    // expiry and rejects during preflight, before provider execution.
    let nonce = "deadline-nonce-0123456789abc";
    let caller_limits = RemoteLimits {
        deadline_ms: Some(0),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    };
    let advertisement = remote_advertisement(query_plan_capability_vocabulary());
    let signer = remote_signer();
    let request = encode_remote_request(
        &validated,
        &invocation,
        &advertisement,
        caller_limits,
        nonce,
    )
    .expect("request envelope");
    let expected_request =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
            .expect("request fingerprint");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let rejection = match preflight_remote_request(&request, &context, &advertisement, limits()) {
        Ok(_) => panic!("expired request must reject in preflight"),
        Err(rejection) => rejection,
    };
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    let response = rejection.into_failure_envelope(&advertisement_fingerprint, &signer);
    let failure = decode_authenticated_failure(&response, &advertisement);
    assert_eq!(failure.nonce(), Some(nonce));
    failure
        .verify_binding(nonce, &expected_request)
        .expect("admitted failure binds the exact request envelope");
    assert_eq!(
        failure.diagnostic().expect("diagnostic").code().as_str(),
        "query_remote_request_expired",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_fenced_transaction_blocks_schema_admission_until_close_live() {
    let _guard = crate::common::integration_test_guard().await;
    let fence_db = setup_db().await;
    let schema_db = setup_db().await;

    let mut warmup = tokio::time::timeout(Duration::from_secs(10), schema_db.schema_transaction())
        .await
        .expect("schema admission warmup must not stall")
        .expect("schema admission warmup");
    tokio::time::timeout(Duration::from_secs(10), warmup.close())
        .await
        .expect("schema warmup close must not stall")
        .expect("schema warmup close");

    let (mut fence, fenced_schema) = tokio::time::timeout(
        Duration::from_secs(10),
        fence_db.schema_fenced_read_transaction(Duration::from_secs(10)),
    )
    .await
    .expect("schema-fenced admission must not stall")
    .expect("schema-fenced admission");
    assert!(
        !fenced_schema.is_empty(),
        "fenced schema export is required"
    );

    let pending_schema = schema_db.schema_transaction();
    tokio::pin!(pending_schema);
    match tokio::time::timeout(Duration::from_millis(500), &mut pending_schema).await {
        Err(_) => {}
        Ok(Ok(mut unexpectedly_open)) => {
            let _ = unexpectedly_open.close().await;
            let _ = fence.close().await;
            panic!("SCHEMA transaction opened while the V2 schema fence was retained");
        }
        Ok(Err(error)) => {
            let _ = fence.close().await;
            panic!("SCHEMA transaction failed instead of waiting for the V2 fence: {error}");
        }
    }

    tokio::time::timeout(Duration::from_secs(10), fence.close())
        .await
        .expect("schema-fenced close must not stall")
        .expect("schema-fenced close");
    let mut schema = tokio::time::timeout(Duration::from_secs(10), &mut pending_schema)
        .await
        .expect("SCHEMA transaction must open after the V2 fence closes")
        .expect("SCHEMA transaction admission after V2 fence close");
    tokio::time::timeout(Duration::from_secs(10), schema.close())
        .await
        .expect("post-fence SCHEMA close must not stall")
        .expect("post-fence SCHEMA close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_facade_executes_locally_and_remotely_live() {
    use std::sync::Arc;

    use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
    use type_bridge_contract::query_remote::RemoteLimits;
    use type_bridge_contract::schema::encode_declared_schema;
    use type_bridge_orm::query_v2_builder::QueryPlanBuilder;
    use type_bridge_orm::query_v2_prepared::{
        QueryAuthority, execute_prepared_local, prepare_remote_query,
    };
    use type_bridge_orm::query_v2_remote::{
        execute_admitted_remote_request, preflight_remote_request,
    };

    let _guard = crate::common::integration_test_guard().await;
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let database_name = unique_schema_suffix("rust", "query-v2-prepared-live-database");
    let db = Database::connect_with_options(
        &address,
        &database_name,
        &username,
        &password,
        connect_options_from_env(),
    )
    .await
    .expect("prepared V2 fixture should connect");
    db.create_database()
        .await
        .expect("prepared V2 fixture database should be created");
    let suffix = unique_schema_suffix("rust", "query-v2-prepared-live");
    let fixture = live_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             entity {}, owns {} @unique;",
            fixture.name.label(),
            fixture.person.label(),
            fixture.name.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live prepared schema");
    db.execute_raw(
        &format!(
            "insert $a isa {person}, has {name} \"ada\"; \
             $b isa {person}, has {name} \"bob\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live prepared data");

    // The binding boundary: declared bytes, plan bytes, JSON payloads.
    let declared_bytes = {
        // live_fixture derives its state from these facts; rebuild the
        // declared document the same way to encode it.
        let name_type = TypeId::new(TypeKind::Attribute, fixture.name.label().as_str()).unwrap();
        let facts = vec![
            SchemaFact::Type(TypeFact::new(fixture.person.clone()).unwrap()),
            SchemaFact::Type(TypeFact::new(name_type).unwrap()),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(fixture.name.clone()),
                ValueTypeTag::String,
            )),
            SchemaFact::Owns(OwnsFact::new(
                OwnsFactId::new(fixture.person.clone(), fixture.name.clone()).unwrap(),
            )),
            SchemaFact::Annotation(
                AnnotationFact::new(
                    AnnotationFactId::new(
                        AnnotationSubjectId::Owns(
                            OwnsFactId::new(fixture.person.clone(), fixture.name.clone()).unwrap(),
                        ),
                        AnnotationKindId::Unique,
                    ),
                    SchemaAnnotationValue::Presence,
                )
                .unwrap(),
            ),
        ];
        let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
            let byte = u64::try_from(index).unwrap();
            let line = u32::try_from(index + 1).unwrap();
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("query-v2-live").unwrap(),
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
        let declared =
            DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
        encode_declared_schema(&declared).unwrap()
    };
    let authority = Arc::new(
        QueryAuthority::from_declared_bytes(&declared_bytes, "query-v2-live", "typedb-3.12.1/v1")
            .expect("authority from declared bytes"),
    );
    let local_authority = Arc::new(
        QueryAuthority::from_declared_bytes_query_only(
            &declared_bytes,
            "query-v2-live",
            "typedb-3.12.1/v1",
            &db,
        )
        .expect("query-only authority from declared bytes"),
    );

    // Author the advanced plan and its fingerprint-bound invocation through
    // the same incremental builder projected by Python and Node.
    let mut builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = builder.binding("person").expect("person binding");
    let name = builder.binding("name").expect("name binding");
    let minimum_name = builder
        .input("minimum_name", ValueTypeTag::String, false)
        .expect("typed input");
    let person_isa = builder
        .isa(&person, fixture.person.clone(), true)
        .expect("person isa");
    let name_has = builder
        .has(&person, &name, fixture.name.clone())
        .expect("name has");
    let name_operand = builder.binding_operand(&name).expect("name operand");
    let minimum_operand = builder.input_operand(&minimum_name).expect("input operand");
    let comparison = builder
        .value(
            ValueComparator::GreaterOrEqual,
            &name_operand,
            &minimum_operand,
        )
        .expect("typed comparison");
    builder
        .r#match(vec![person_isa, name_has, comparison])
        .expect("match");
    builder
        .select(vec![person.clone(), name.clone()])
        .expect("select");
    builder.require(vec![name.clone()]).expect("require");
    builder.distinct().expect("distinct");
    let order = builder
        .order(&name, OrderDirection::Ascending)
        .expect("name order");
    builder.sort(vec![order]).expect("sort");
    builder.limit(10).expect("total ordered limit");
    let plan = builder
        .finalize_rows(vec![person, name])
        .expect("builder-authored plan");
    let invocation = plan
        .rows(vec![string_row("a").values().to_vec()])
        .expect("builder-authored invocation");
    let plan_bytes = plan.canonical_bytes();
    let invocation_json =
        String::from_utf8(invocation.canonical_bytes()).expect("canonical invocation UTF-8");

    // Local execution through the facade.
    let local_json = execute_prepared_local(
        &db,
        &local_authority,
        &plan_bytes,
        &invocation_json,
        limits(),
    )
    .await
    .expect("prepared local outcome");
    assert!(local_json.contains("\"ada\""), "{local_json}");
    assert!(local_json.contains("\"bob\""), "{local_json}");

    // Remote execution through the same facade and envelope.
    let caller_limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    };
    let advertisement = remote_advertisement(query_plan_v2_capability_vocabulary());
    let advertisement_bytes = advertisement.encode().expect("advertisement bytes");
    let pending = prepare_remote_query(
        &authority,
        &plan_bytes,
        &invocation_json,
        &advertisement_bytes,
        caller_limits,
    )
    .expect("prepared request");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let admitted =
        preflight_remote_request(pending.request_bytes(), &context, &advertisement, limits())
            .unwrap_or_else(|rejection| {
                panic!("prepared preflight: {}", rejection.diagnostic_code())
            });
    let signer = remote_signer();
    let mut server_transaction = db.read_transaction().await.expect("server transaction");
    let response =
        execute_admitted_remote_request(admitted, &mut server_transaction, &signer).await;
    let remote_json = pending
        .decode_reply(&response)
        .expect("prepared remote outcome");
    assert_eq!(remote_json, local_json);
    assert_eq!(
        pending
            .decode_reply(&response)
            .expect_err("pending reply decoder is one-shot")
            .code()
            .as_str(),
        "query_remote_reply_replayed",
    );
    server_transaction
        .close()
        .await
        .expect("prepared V2 server transaction should close");
    drop(server_transaction);

    tokio::time::timeout(Duration::from_secs(10), db.delete_database())
        .await
        .expect("prepared V2 fixture database deletion must not stall")
        .expect("prepared V2 fixture database should be deleted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_negation_witnesses_execute_without_an_explicit_select_live() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "query-v2-witness");
    let fixture = live_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             entity {}, owns {} @unique;",
            fixture.name.label(),
            fixture.person.label(),
            fixture.name.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("witness schema");
    db.execute_raw(
        &format!(
            "insert $ada isa {person}, has {name} \"Ada\"; \
             $grace isa {person}, has {name} \"Grace\"; \
             $zed isa {person}, has {name} \"Zed\";",
            person = fixture.person.label(),
            name = fixture.name.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("witness data");

    // The witness binding exists only inside the negation and is never
    // projected; the plan carries no explicit Select stage, so implicit
    // projection must come from the validator-derived root visibility.
    let plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "hidden"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
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
                QueryPattern::Not {
                    patterns: vec![
                        QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: fixture.name.clone(),
                            owner: binding_id(0),
                        },
                        QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding {
                                binding: binding_id(2),
                            },
                            right: QueryOperand::Literal {
                                value: CanonicalValue::String(
                                    CanonicalString::new("Zed").expect("literal"),
                                ),
                            },
                        },
                    ],
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("witness plan");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let validated = validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
        .expect("validated witness query");

    let mut transaction = db
        .read_transaction()
        .await
        .expect("borrowed read transaction");
    let rows = execute_validated_query(
        &mut transaction,
        &validated,
        &QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation"),
        limits(),
    )
    .await
    .expect("witness rows execute without a provider column mismatch");
    let mut names = row_names(&rows);
    names.sort();
    assert_eq!(names, vec!["Ada", "Grace"]);
}
