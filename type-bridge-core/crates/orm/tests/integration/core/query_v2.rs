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
