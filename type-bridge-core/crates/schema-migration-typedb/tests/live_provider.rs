use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::{ManagedScopeId, SemanticProfileBinding};
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationStep, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::migration_backfill::{
    AttributeBackfillPlan, BackfillPartition, BackfillReverseProgram,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_query::{MigrationAssertionValidationContext, lower_condition_to_plan};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyDerivationProfile, derive_safety_conditions, diff_managed,
    inverse_delta, managed_schema_state, resolve,
};
use type_bridge_schema_migration::{
    BackfillExecutionDirection, ExecutionScope, GroupCommitCertainty, LeaseHolderId,
    MigrationApplyTarget, MigrationExecutionControl, MigrationExecutionJournal,
    MigrationExecutionOutcome, MigrationExecutionProvider, MigrationHistoryGraph,
    MigrationLeaseStore, MigrationSafetyPolicy, SchemaLoweringBinding, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_verified_manifest, build_verified_migration_apply_plan,
    execute_verified_migration_apply_plan, schema_lowering_profile_binding,
};
use type_bridge_schema_migration_typedb::{
    TypeDbExecutionBinding, TypeDbMigrationProvider, TypeDbMigrationStore,
    VerifiedMigrationCatalog, derived_journal_database_name, execution_capability_vocabulary,
};

fn connection() -> (String, String, String, String, ConnectOptions) {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let database = env::var("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE")
        .unwrap_or_else(|_| "type_bridge_v2_execution_provider".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let mut options = ConnectOptions::default();
    if let Ok(port) = env::var("TYPEDB_HTTP_PORT") {
        options.http_port = port.parse().expect("TYPEDB_HTTP_PORT must be a u16");
    }
    (address, database, username, password, options)
}

async fn databases() -> (Arc<Database>, Arc<Database>) {
    let (address, database, username, password, options) = connection();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let managed_name = format!("{database}_{}_{unique:x}", std::process::id());
    let journal_name = derived_journal_database_name(&managed_name);
    type_bridge_orm::ensure_database_exists(&address, &managed_name, &username, &password, options)
        .await
        .expect("create isolated managed database");
    type_bridge_orm::ensure_database_exists(&address, &journal_name, &username, &password, options)
        .await
        .expect("create isolated journal database");
    let managed = Arc::new(
        Database::connect_with_options(&address, &managed_name, &username, &password, options)
            .await
            .expect("connect isolated managed database"),
    );
    let journal = Arc::new(
        Database::connect_with_options(&address, &journal_name, &username, &password, options)
            .await
            .expect("connect isolated journal database"),
    );
    (managed, journal)
}

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, label).expect("fixture type"))
            .expect("fixture type fact"),
    )
}

fn abstract_fact(label: &str) -> SchemaFact {
    let id = TypeId::new(TypeKind::Entity, label).expect("fixture type");
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(AnnotationSubjectId::Type(id), AnnotationKindId::Abstract),
            SchemaAnnotationValue::Presence,
        )
        .expect("abstract annotation"),
    )
}

fn declared_facts(facts: Vec<SchemaFact>) -> DeclaredSchema {
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("live-provider-fixture").expect("document"),
                offset,
                offset + 1,
                line,
                1,
                line,
                2,
            )
            .expect("source span"),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, Default::default(), sourced)
        .expect("declared schema")
}

fn workforce_backfill_schema() -> DeclaredSchema {
    let owner = TypeId::new(TypeKind::Entity, "person").expect("person type");
    let person_id = AttributeId::new("person-id").expect("person ID attribute");
    let legacy_name = AttributeId::new("legacy-name").expect("legacy name attribute");
    let display_name = AttributeId::new("display-name").expect("display name attribute");
    let mut facts = vec![type_fact("person")];
    for attribute in [&person_id, &legacy_name, &display_name] {
        facts.push(SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, attribute.label().as_str())
                    .expect("attribute type"),
            )
            .expect("attribute type fact"),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute.clone()),
            ValueTypeTag::String,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(owner.clone(), attribute.clone()).expect("ownership"),
        )));
    }
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(
                    OwnsFactId::new(owner, person_id).expect("key ownership"),
                ),
                AnnotationKindId::Key,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("key annotation"),
    ));
    declared_facts(facts)
}

async fn query_document_count(database: &Database, query: &str) -> usize {
    let mut transaction = database
        .read_transaction()
        .await
        .expect("open fixture read transaction");
    let count = match transaction.query(query).await.expect("fixture query") {
        type_bridge_orm::session::backend::QueryResult::Documents(documents) => documents.len(),
        result => panic!("fixture query must return documents, got {result:?}"),
    };
    transaction.close().await.expect("close fixture read");
    count
}

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("provider-live-scope").expect("scope"),
        type_bridge_contract::fingerprint::SemanticProfileId::new("typedb-3.12.1/v1")
            .expect("profile"),
        execution_capability_vocabulary().expect("execution capability vocabulary"),
    )
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("live-provider").expect("app"),
        type_bridge_contract::migration::MigrationName::new(name).expect("name"),
    )
}

fn derived_assertion_step(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
    delta: &type_bridge_contract::schema::SchemaDelta,
) -> MigrationStep {
    let safety_profile = SafetyDerivationProfile::new(
        SemanticProfileBinding::resolve(context.semantic_profile().clone())
            .expect("semantic profile binding"),
        schema_lowering_profile_binding().expect("lowering profile binding"),
    )
    .expect("safety profile");
    let derived =
        derive_safety_conditions(0, &delta.operations()[0], source, target, &safety_profile)
            .expect("conditional safety condition");
    assert_eq!(derived.conditions().len(), 1);
    let resolved = resolve(source, context.semantic_profile()).expect("resolved assertion source");
    let source_state = managed_schema_state(source, context).expect("managed assertion source");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &source_state);
    let validated = lower_condition_to_plan(
        &derived.conditions()[0],
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("derived assertion plan");
    MigrationStep::assertion(
        MigrationStepId::new("assert-person-empty").expect("assertion step ID"),
        validated.plan().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("assertion step")
}

fn additive_manifest(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("delta");
    let reverse = inverse_delta(&delta).expect("inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("step id"),
        delta,
        Some(reverse),
    )
    .expect("schema step");
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, vec![step]).expect("draft");
    build_verified_manifest(draft, (source, context)).expect("verified manifest")
}

fn conditional_manifest(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("conditional delta");
    let assertion = derived_assertion_step(source, target, context, &delta);
    let reverse = inverse_delta(&delta).expect("conditional inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("step id"),
        delta,
        Some(reverse),
    )
    .expect("conditional schema step");
    let draft = SchemaMigrationDraft::new(
        migration_id(name),
        parents,
        vec![assertion, MigrationStep::from(step)],
    )
    .expect("conditional draft");
    build_verified_manifest(draft, (source, context)).expect("verified manifest")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.3 server"]
async fn coordinator_applies_verified_plan_through_live_provider_on_3_12_3() {
    let (managed, journal_database) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let first_target = declared_facts(vec![type_fact("person"), type_fact("company")]);
    let second_target = declared_facts(vec![
        type_fact("person"),
        type_fact("company"),
        abstract_fact("person"),
    ]);
    let first = additive_manifest(
        "0001_company",
        Vec::new(),
        &genesis,
        &first_target,
        &context,
    );
    let second = conditional_manifest(
        "0002_person_abstract",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([first.clone(), second.clone()])
        .expect("verified history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let plan = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect("verified apply plan");
    assert_eq!(plan.migrations().len(), 2);

    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        Arc::clone(&journal_database),
        context.clone(),
    )
    .expect("bind exact managed/journal pair and context");
    let catalog = VerifiedMigrationCatalog::new([&first, &second]).expect("catalog");
    let store = TypeDbMigrationStore::new(&binding, catalog)
        .expect("paired store")
        .bind_plan(&plan)
        .expect("bind exact plan");
    let provider =
        TypeDbMigrationProvider::new(&binding).expect("version-gated provider from shared binding");
    let holder = LeaseHolderId::new("live-provider-coordinator").expect("holder");

    let outcome = execute_verified_migration_apply_plan(&store, &provider, &holder, &plan)
        .await
        .expect("coordinator execution against live TypeDB");
    assert!(matches!(outcome, MigrationExecutionOutcome::Applied { .. }));

    let export = managed
        .schema_text()
        .await
        .expect("post-apply schema export");
    assert!(export.contains("person"));
    assert!(export.contains("company"));
    assert!(export.contains("@abstract"));

    let scope = ExecutionScope::new(
        plan.source_state()
            .expect("plan source state")
            .scope()
            .id()
            .clone(),
    );
    let inspector = LeaseHolderId::new("live-provider-inspector").expect("holder");
    let lease = store
        .acquire(&scope, &inspector)
        .await
        .expect("post-apply inspection lease");
    let applied = store
        .load_applied(&lease)
        .await
        .expect("load applied ledger");
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0].record().migration_id(), first.id());
    assert_eq!(applied[1].record().migration_id(), second.id());
    assert!(
        store
            .load_open_plan(&lease)
            .await
            .expect("completed plan is cleared")
            .is_none()
    );
    let target = plan.target_state().expect("plan target state");
    let observed = provider
        .observe_managed_state(&lease, target, target)
        .await
        .expect("full-stack live observation of the applied target");
    assert_eq!(&observed, target);
    store
        .release(&lease)
        .await
        .expect("release inspection lease");

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal_database
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.3 server"]
async fn closed_backfill_conflict_retry_and_reverse_round_trip_on_3_12_3() {
    let (managed, journal_database) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let expanded = workforce_backfill_schema();
    let expand = additive_manifest(
        "0001_expand_names",
        Vec::new(),
        &genesis,
        &expanded,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([expand.clone()]).expect("expand history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let expand_plan = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect("expand plan");
    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        Arc::clone(&journal_database),
        context.clone(),
    )
    .expect("exact execution binding");
    let catalog = VerifiedMigrationCatalog::new([&expand]).expect("expand catalog");
    let store = TypeDbMigrationStore::new(&binding, catalog)
        .expect("paired store")
        .bind_plan(&expand_plan)
        .expect("bind expand plan");
    let provider = TypeDbMigrationProvider::new(&binding).expect("backfill provider");
    execute_verified_migration_apply_plan(
        &store,
        &provider,
        &LeaseHolderId::new("live-backfill-expand").expect("holder"),
        &expand_plan,
    )
    .await
    .expect("apply expand schema");

    let mut seed = managed
        .write_transaction()
        .await
        .expect("open seed transaction");
    seed.query(
        "insert\n  $first isa person, has person-id \"p1\", has legacy-name \"Ada\";\n  $second isa person, has person-id \"p2\", has legacy-name \"Bob\", has display-name \"conflict\";",
    )
    .await
    .expect("seed source and conflicting destination rows");
    seed.commit().await.expect("commit fixture rows");

    let semantics = managed_schema_state(&expanded, &context)
        .expect("expanded managed state")
        .managed_semantic_schema()
        .clone();
    let backfill = AttributeBackfillPlan::new(
        TypeId::new(TypeKind::Entity, "person").expect("owner"),
        AttributeId::new("legacy-name").expect("source"),
        AttributeId::new("display-name").expect("destination"),
        BackfillPartition::new(1, AttributeId::new("person-id").expect("partition key"))
            .expect("single-row deterministic groups"),
        semantics,
        Some(BackfillReverseProgram::RemoveEqualCopiedDestination),
    )
    .expect("closed backfill plan");
    let scope = ExecutionScope::new(context.scope_id().clone());
    let lease = store
        .acquire(
            &scope,
            &LeaseHolderId::new("live-backfill-executor").expect("holder"),
        )
        .await
        .expect("backfill lease");
    let control = MigrationExecutionControl::default();

    let conflict = provider
        .execute_backfill(
            &lease,
            &backfill,
            BackfillExecutionDirection::Forward,
            &control,
        )
        .await
        .expect_err("unequal destination rejects before mutation");
    assert_eq!(
        conflict.certainty(),
        GroupCommitCertainty::DefinitelyAborted
    );
    assert_eq!(
        conflict.diagnostic().code().as_str(),
        "migration_typedb_backfill_destination_conflict"
    );
    assert_eq!(
        query_document_count(
            &managed,
            "match $person isa person, has display-name $name; fetch { \"name\": $name };",
        )
        .await,
        1,
        "the conflict precheck publishes no partial copy",
    );

    let mut repair = managed
        .write_transaction()
        .await
        .expect("open fixture repair transaction");
    repair
        .query(
            "match $person isa person, has person-id \"p2\", has display-name $name; delete has $name of $person;",
        )
        .await
        .expect("remove deliberate conflict");
    repair.commit().await.expect("commit conflict repair");

    let applied = provider
        .execute_backfill(
            &lease,
            &backfill,
            BackfillExecutionDirection::Forward,
            &control,
        )
        .await
        .expect("execute exact copy backfill");
    assert_eq!(applied.counts().changed(), 2);
    assert_eq!(applied.counts().transaction_groups(), 2);
    assert_eq!(
        query_document_count(
            &managed,
            "match $person isa person, has legacy-name $source, has display-name $destination; $source == $destination; fetch { \"name\": $destination };",
        )
        .await,
        2,
    );

    let retried = provider
        .execute_backfill(
            &lease,
            &backfill,
            BackfillExecutionDirection::Forward,
            &control,
        )
        .await
        .expect("idempotent retry");
    assert_eq!(retried.counts().changed(), 0);

    let reversed = provider
        .execute_backfill(
            &lease,
            &backfill,
            BackfillExecutionDirection::Reverse,
            &control,
        )
        .await
        .expect("checked reverse");
    assert_eq!(reversed.counts().changed(), 2);
    assert_eq!(
        query_document_count(
            &managed,
            "match $person isa person, has display-name $name; fetch { \"name\": $name };",
        )
        .await,
        0,
    );
    assert_eq!(
        query_document_count(
            &managed,
            "match $person isa person, has legacy-name $name; fetch { \"name\": $name };",
        )
        .await,
        2,
        "reverse preserves the historical source meaning",
    );

    store.release(&lease).await.expect("release backfill lease");
    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal_database
        .delete_database()
        .await
        .expect("delete isolated journal database");
}
