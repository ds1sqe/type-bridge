//! End-to-end migration assertion execution against TypeDB 3.12.1.

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, AssertionRolePlayer, BindingId,
    MigrationAssertionPlan, QueryVariable, ValueComparator, ValueOperand,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
    RelatesFact, RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact,
    SubFactId, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_orm::migration_assertion::{
    MigrationAssertionExecutionContext, MigrationAssertionExecutionError,
    execute_migration_assertion, lower_migration_assertion,
};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::TxType;
use type_bridge_query::{
    MigrationAssertionValidationContext, ValidatedMigrationAssertionPlan,
    validate_migration_assertion_plan,
};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

struct LiveSchemaFixture {
    employee: TypeId,
    employment: TypeId,
    managed: ManagedSchemaState,
    name: AttributeId,
    person: TypeId,
    resolved: ResolvedSchema,
    worker: RoleId,
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding ID"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn sourced_facts(facts: Vec<SchemaFact>) -> Vec<SourcedSchemaFact> {
    facts
        .into_iter()
        .enumerate()
        .map(|(index, fact)| {
            let byte = u64::try_from(index).expect("fixture byte offset");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("migration-assertion-live").expect("document ID"),
                    byte,
                    byte + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("source span"),
            )
        })
        .collect()
}

fn live_schema_fixture(suffix: &str) -> LiveSchemaFixture {
    let person = TypeId::new(TypeKind::Entity, format!("{suffix}-person")).unwrap();
    let employee = TypeId::new(TypeKind::Entity, format!("{suffix}-employee")).unwrap();
    let name = AttributeId::new(format!("{suffix}-name")).unwrap();
    let name_type = TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap();
    let employment = TypeId::new(TypeKind::Relation, format!("{suffix}-employment")).unwrap();
    let worker = RoleId::new(employment.label().as_str(), format!("{suffix}-worker")).unwrap();
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(employee.clone()).unwrap()),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee.clone(), person.clone()).unwrap(),
        )),
        SchemaFact::Type(TypeFact::new(name_type).unwrap()),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
        SchemaFact::Type(TypeFact::new(employment.clone()).unwrap()),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(employment.clone(), worker.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), worker.clone()).unwrap(),
        )),
    ];
    let declared = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        sourced_facts(facts),
    )
    .unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("migration-assertion-live").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    LiveSchemaFixture {
        employee,
        employment,
        managed,
        name,
        person,
        resolved,
        worker,
    }
}

fn literal(value: &str) -> ValueOperand {
    ValueOperand::literal(CanonicalValue::String(CanonicalString::new(value).unwrap()))
}

fn validated_plan(
    fixture: &LiveSchemaFixture,
    exact_value: &str,
) -> ValidatedMigrationAssertionPlan {
    let person = BindingId::new(0).unwrap();
    let name = BindingId::new(1).unwrap();
    let employment = BindingId::new(2).unwrap();
    let compare = |comparator, value| AssertionPattern::Value {
        comparator,
        left: ValueOperand::binding(name),
        right: literal(value),
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
                include_subtypes: true,
                type_id: fixture.person.clone(),
            },
            AssertionPattern::Isa {
                binding: employment,
                include_subtypes: false,
                type_id: fixture.employment.clone(),
            },
            AssertionPattern::Has {
                attribute: name,
                attribute_id: fixture.name.clone(),
                owner: person,
            },
            AssertionPattern::Links {
                players: vec![AssertionRolePlayer::new(fixture.worker.clone(), person)],
                relation: employment,
                relation_id: fixture.employment.clone(),
            },
            compare(ValueComparator::Equal, exact_value),
            compare(ValueComparator::NotEqual, "blocked"),
            compare(ValueComparator::Less, "Zzzzz"),
            compare(ValueComparator::LessOrEqual, "Ada"),
            compare(ValueComparator::Greater, "A"),
            compare(ValueComparator::GreaterOrEqual, "Ada"),
            AssertionPattern::Not {
                patterns: vec![compare(ValueComparator::Equal, "forbidden")],
            },
        ],
        vec![person, name],
        vec![employment],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .unwrap();
    validate_migration_assertion_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
        StructuralLimits::CANONICAL,
    )
    .unwrap()
}

fn wrong_source_state(source: &ManagedSchemaState) -> ManagedSchemaState {
    ManagedSchemaState::new(
        source.format(),
        source.required_capabilities().clone(),
        source.scope().clone(),
        source.selection().clone(),
        source.declared_identity().clone(),
        source.managed_declared_identity().clone(),
        ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"wrong-live-source",
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn validated_assertions_execute_on_one_borrowed_real_transaction() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "migration-assertion-live");
    let fixture = live_schema_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             relation {}, relates {};\n\
             entity {}, owns {}, plays {}:{};\n\
             entity {} sub {};",
            fixture.name.label(),
            fixture.employment.label(),
            fixture.worker.label(),
            fixture.person.label(),
            fixture.name.label(),
            fixture.employment.label(),
            fixture.worker.label(),
            fixture.employee.label(),
            fixture.person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("live assertion schema");
    db.execute_raw(
        &format!(
            "insert $person isa {}, has {} \"Ada\"; \
             ({}: $person) isa {};",
            fixture.employee.label(),
            fixture.name.label(),
            fixture.worker.label(),
            fixture.employment.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("live assertion data");

    let empty = validated_plan(&fixture, "Missing");
    let violating = validated_plan(&fixture, "Ada");
    let lowered = lower_migration_assertion(&violating).expect("provider lowering");
    for syntax in [" isa ", " isa! ", " has ", "links (", " == ", " != ", " < ", " <= ", " > ", " >= ", "not {"] {
        assert!(lowered.typeql().contains(syntax), "missing lowered syntax {syntax:?}");
    }
    assert!(lowered.typeql().ends_with("limit 1;\n"));

    let available = violating.plan().required_capabilities().clone();
    let missing = CapabilitySet::new();
    let wrong_state = wrong_source_state(&fixture.managed);
    let mut transaction = db.read_transaction().await.expect("borrowed read transaction");
    assert_eq!(transaction.tx_type(), TxType::Read);

    let capability_error = execute_migration_assertion(
        &mut transaction,
        &empty,
        MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &missing,
            StructuralLimits::CANONICAL,
        ),
    )
    .await
    .expect_err("missing capabilities fail before execution");
    assert_eq!(
        capability_error.diagnostic().unwrap().code().as_str(),
        "unsupported_required_capability"
    );

    let state_error = execute_migration_assertion(
        &mut transaction,
        &empty,
        MigrationAssertionExecutionContext::new(
            &wrong_state,
            &available,
            StructuralLimits::CANONICAL,
        ),
    )
    .await
    .expect_err("stale source state fails before execution");
    assert_eq!(
        state_error.diagnostic().unwrap().code().as_str(),
        "migration_assertion_source_managed_semantic_mismatch"
    );

    execute_migration_assertion(
        &mut transaction,
        &empty,
        MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        ),
    )
    .await
    .expect("empty provider result passes NoRows");

    let violation = execute_migration_assertion(
        &mut transaction,
        &violating,
        MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        ),
    )
    .await
    .expect_err("first provider row violates NoRows");
    let failure = match violation {
        MigrationAssertionExecutionError::AssertionFailed(failure) => failure,
        other => panic!("expected typed AssertionFailed, got {other:#?}"),
    };
    let expected_fingerprint = violating.plan().fingerprint().unwrap();
    assert_eq!(failure.plan_fingerprint(), &expected_fingerprint);
    assert_eq!(failure.evidence().len(), 2);
    assert_eq!(failure.evidence()[0].binding(), BindingId::new(0).unwrap());
    assert_eq!(failure.evidence()[0].variable().as_str(), "person");
    assert_eq!(
        failure.evidence()[0].domain(),
        violating
            .binding_domain(&BindingId::new(0).unwrap())
            .expect("person domain")
    );
    assert_eq!(failure.evidence()[1].binding(), BindingId::new(1).unwrap());
    assert_eq!(failure.evidence()[1].variable().as_str(), "name");
    assert_eq!(
        failure.evidence()[1].domain(),
        violating
            .binding_domain(&BindingId::new(1).unwrap())
            .expect("name domain")
    );

    let reuse = transaction
        .query(&format!(
            "match $person isa! {}; select $person; limit 1;",
            fixture.employee.label()
        ))
        .await
        .expect("borrowed transaction remains usable after assertion failure");
    assert!(matches!(reuse, QueryResult::Rows(rows) if rows.len() == 1));
    transaction.close().await.expect("caller closes borrowed transaction");
}

#[tokio::test]
async fn assertion_and_multiple_schema_statements_commit_in_one_schema_transaction() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "migration-assertion-schema-tx");
    let fixture = live_schema_fixture(&suffix);

    db.execute_raw(
        &format!(
            "define\n\
             attribute {}, value string;\n\
             relation {}, relates {};\n\
             entity {}, owns {}, plays {}:{};\n\
             entity {} sub {};",
            fixture.name.label(),
            fixture.employment.label(),
            fixture.worker.label(),
            fixture.person.label(),
            fixture.name.label(),
            fixture.employment.label(),
            fixture.worker.label(),
            fixture.employee.label(),
            fixture.person.label(),
        ),
        TxType::Schema,
    )
    .await
    .expect("mixed-transaction source schema");
    db.execute_raw(
        &format!(
            "insert $person isa {}, has {} \"Ada\"; \
             ({}: $person) isa {};",
            fixture.employee.label(),
            fixture.name.label(),
            fixture.worker.label(),
            fixture.employment.label(),
        ),
        TxType::Write,
    )
    .await
    .expect("mixed-transaction source data");

    let assertion = validated_plan(&fixture, "Missing");
    let available = assertion.plan().required_capabilities().clone();
    let added = format!("{suffix}-added");
    let added_subtype = format!("{suffix}-added-subtype");
    let mut transaction = db
        .schema_transaction()
        .await
        .expect("owned schema transaction");
    assert_eq!(transaction.tx_type(), TxType::Schema);

    execute_migration_assertion(
        &mut transaction,
        &assertion,
        MigrationAssertionExecutionContext::new(
            &fixture.managed,
            &available,
            StructuralLimits::CANONICAL,
        ),
    )
    .await
    .expect("bounded assertion in schema transaction");

    let first = transaction
        .query(&format!("define entity {added};"))
        .await
        .expect("first schema statement");
    assert!(matches!(first, QueryResult::Ok));
    let second = transaction
        .query(&format!("define entity {added_subtype} sub {added};"))
        .await
        .expect("dependent schema statement");
    assert!(matches!(second, QueryResult::Ok));
    transaction.commit().await.expect("single schema commit");

    let exported = db.schema_text().await.expect("committed schema export");
    assert!(exported.contains(&added));
    assert!(exported.contains(&added_subtype));
}

#[tokio::test]
async fn schema_transaction_excludes_fence_takeover_until_commit() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "migration-fence-race");
    let lease_type = format!("{suffix}-lease");
    let scope_attribute = format!("{suffix}-scope");
    let holder_attribute = format!("{suffix}-holder");
    let fence_attribute = format!("{suffix}-fence");
    let staged_type = format!("{suffix}-stale-ddl");
    let scope = format!("{suffix}-managed-scope");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {scope_attribute}, value string;\n\
             attribute {holder_attribute}, value string;\n\
             attribute {fence_attribute}, value integer;\n\
             entity {lease_type},\n\
               owns {scope_attribute} @key,\n\
               owns {holder_attribute} @card(1..1),\n\
               owns {fence_attribute} @card(1..1);"
        ),
        TxType::Schema,
    )
    .await
    .expect("fence control schema");
    db.execute_raw(
        &format!(
            "insert $lease isa {lease_type},\n\
               has {scope_attribute} \"{scope}\",\n\
               has {holder_attribute} \"owner-a\",\n\
               has {fence_attribute} 1;"
        ),
        TxType::Write,
    )
    .await
    .expect("initial fenced lease");

    let mut stale = db.schema_transaction().await.expect("stale schema transaction");
    let fence_read = stale
        .query(&format!(
            "match $lease isa {lease_type},\n\
               has {scope_attribute} \"{scope}\",\n\
               has {holder_attribute} \"owner-a\",\n\
               has {fence_attribute} 1;\n\
             select $lease; limit 1;"
        ))
        .await
        .expect("schema transaction reads exact fence");
    assert!(matches!(fence_read, QueryResult::Rows(rows) if rows.len() == 1));
    let staged = stale
        .query(&format!("define entity {staged_type};"))
        .await
        .expect("stage schema change behind fence read");
    assert!(matches!(staged, QueryResult::Ok));

    let blocked = match db.write_transaction().await {
        Ok(_) => panic!("write takeover must not open while a schema transaction is active"),
        Err(error) => error,
    };
    assert!(
        !blocked.to_string().is_empty(),
        "blocked takeover returned an empty transaction diagnostic"
    );
    stale
        .commit()
        .await
        .expect("schema commit linearizes before lease takeover");

    let exported = db.schema_text().await.expect("schema after fenced commit");
    assert!(
        exported.contains(&staged_type),
        "schema DDL was lost even though no takeover could open"
    );

    let mut takeover = db
        .write_transaction()
        .await
        .expect("takeover opens after schema commit");
    let replaced = takeover
        .query(&format!(
            "match $lease isa {lease_type},\n\
               has {scope_attribute} \"{scope}\",\n\
               has {holder_attribute} \"owner-a\",\n\
               has {fence_attribute} 1;\n\
             delete $lease;\n\
             insert $next isa {lease_type},\n\
               has {scope_attribute} \"{scope}\",\n\
               has {holder_attribute} \"owner-b\",\n\
               has {fence_attribute} 2;"
        ))
        .await
        .expect("replace lease with higher fence");
    assert!(matches!(
        replaced,
        QueryResult::Ok | QueryResult::Rows(_)
    ));
    takeover.commit().await.expect("commit lease takeover");

    let mut verify = db.read_transaction().await.expect("verify takeover");
    let current = verify
        .query(&format!(
            "match $lease isa {lease_type},\n\
               has {scope_attribute} \"{scope}\",\n\
               has {holder_attribute} \"owner-b\",\n\
               has {fence_attribute} 2;\n\
             select $lease; limit 1;"
        ))
        .await
        .expect("read advanced fence");
    assert!(matches!(current, QueryResult::Rows(rows) if rows.len() == 1));
    verify.close().await.expect("close takeover verification");
}
