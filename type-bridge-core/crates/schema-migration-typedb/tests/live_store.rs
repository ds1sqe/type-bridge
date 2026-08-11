use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, diff_managed, inverse_delta};
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionScope, GroupEventRecord, GroupJournalEventKind, LeaseHolderId,
    MigrationApplyTarget, MigrationExecutionJournal, MigrationHistoryGraph, MigrationLeaseStore,
    MigrationSafetyPolicy, PlanRecord, SafetyPolicyDecision, SchemaLoweringBinding,
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    build_verified_migration_apply_plan, typedb_3_12_1_profile,
};
use type_bridge_schema_migration_typedb::{
    JOURNAL_CONTROL_SCHEMA_TYPEQL, TypeDbExecutionBinding, TypeDbMigrationStore,
    VerifiedMigrationCatalog, derived_journal_database_name, partition_typeql_export,
    require_active_managed_fence,
};

fn connection() -> (String, String, String, String, ConnectOptions) {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let database = env::var("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE")
        .unwrap_or_else(|_| "type_bridge_v2_execution_store".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let mut options = ConnectOptions::default();
    if let Ok(port) = env::var("TYPEDB_HTTP_PORT") {
        options.http_port = port.parse().expect("TYPEDB_HTTP_PORT must be a u16");
    }
    (address, database, username, password, options)
}

fn alternate_loopback_address(address: &str) -> String {
    if let Some(port) = address.strip_prefix("localhost:") {
        format!("127.0.0.1:{port}")
    } else if let Some(port) = address.strip_prefix("127.0.0.1:") {
        format!("localhost:{port}")
    } else {
        panic!("isolated provider-authority test requires a localhost loopback address")
    }
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
    assert_eq!(
        journal.database_name(),
        derived_journal_database_name(managed.database_name())
    );
    (managed, journal)
}

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, label).expect("fixture type"))
            .expect("fixture type fact"),
    )
}

fn declared(labels: &[&str]) -> DeclaredSchema {
    let facts = labels.iter().enumerate().map(|(index, label)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            type_fact(label),
            SourceSpan::new(
                DocumentId::new("typedb-store-fixture").expect("document"),
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
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), facts)
        .expect("declared schema")
}

fn context_for_scope(scope: ManagedScopeId) -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        scope,
        SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
        typedb_3_12_1_profile().required_capabilities.clone(),
    )
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("live-store").expect("app"),
        MigrationName::new(name).expect("name"),
    )
}

fn verified_manifest(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    verified_manifest_named("0001_company", Vec::new(), source, target, context)
}

fn verified_manifest_named(
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

fn additive_policy() -> MigrationSafetyPolicy {
    MigrationSafetyPolicy::default_policy()
        .with_decision(SafetyClass::Conditional, SafetyPolicyDecision::Reject)
        .expect("additive-only policy")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn foreign_journal_schema_rejects_without_mutating_either_database() {
    let (managed, journal) = databases().await;
    let mut transaction = journal
        .schema_transaction()
        .await
        .expect("schema transaction");
    transaction
        .query("define entity journal-collision-probe;")
        .await
        .expect("install unrelated user schema");
    transaction.commit().await.expect("commit unrelated schema");
    let journal_before = journal.schema_text().await.expect("journal export before");
    let managed_before = managed.schema_text().await.expect("managed export before");
    let scope = ManagedScopeId::new("foreign-journal-scope").unwrap();
    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        context_for_scope(scope),
    )
    .expect("bind exact managed/journal pair and context");
    let catalog =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let store = TypeDbMigrationStore::new(&binding, catalog).unwrap();

    let error = store
        .ensure_control_schema()
        .await
        .expect_err("foreign schema must not be claimed");
    assert_eq!(
        error.code().as_str(),
        "migration_typedb_journal_database_not_exclusive"
    );
    assert_eq!(journal.schema_text().await.unwrap(), journal_before);
    assert_eq!(managed.schema_text().await.unwrap(), managed_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn different_provider_authority_rejects_before_mutating_either_database() {
    let (managed, journal) = databases().await;
    let (address, _, username, password, options) = connection();
    let alternate = alternate_loopback_address(&address);
    let journal_through_other_authority = Arc::new(
        Database::connect_with_options(
            &alternate,
            journal.database_name(),
            &username,
            &password,
            options,
        )
        .await
        .expect("connect the same live journal through a distinct endpoint authority"),
    );
    let managed_before = managed.schema_text().await.unwrap();
    let journal_before = journal.schema_text().await.unwrap();
    let scope = ManagedScopeId::new("different-provider-authority-scope").unwrap();

    let result = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        journal_through_other_authority,
        context_for_scope(scope),
    );
    let Err(error) = result else {
        panic!("different endpoint authorities must reject at construction");
    };
    assert_eq!(
        error.code().as_str(),
        "migration_typedb_database_authority_mismatch"
    );
    assert_eq!(managed.schema_text().await.unwrap(), managed_before);
    assert_eq!(journal.schema_text().await.unwrap(), journal_before);
    let rendered = format!("{error}\n{error:?}");
    assert!(!rendered.contains(&address));
    assert!(!rendered.contains(&alternate));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn missing_and_wrong_journal_owner_reject_unchanged_before_managed_fence() {
    let (managed, journal) = databases().await;
    let mut transaction = journal
        .schema_transaction()
        .await
        .expect("schema transaction");
    transaction
        .query(JOURNAL_CONTROL_SCHEMA_TYPEQL)
        .await
        .expect("install exact owner-aware schema without its owner row");
    transaction.commit().await.expect("commit exact schema");
    let journal_schema = journal.schema_text().await.unwrap();
    let managed_before = managed.schema_text().await.unwrap();
    let scope = ManagedScopeId::new("journal-owner-scope").unwrap();
    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        context_for_scope(scope),
    )
    .expect("bind exact managed/journal pair and context");
    let catalog =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let missing = TypeDbMigrationStore::new(&binding, catalog)
        .unwrap()
        .ensure_control_schema()
        .await
        .expect_err("an exact schema without the immutable owner must reject");
    assert_eq!(
        missing.code().as_str(),
        "migration_typedb_journal_owner_mismatch"
    );
    assert_eq!(journal.schema_text().await.unwrap(), journal_schema);
    assert_eq!(managed.schema_text().await.unwrap(), managed_before);

    let mut transaction = journal
        .write_transaction()
        .await
        .expect("write transaction");
    transaction
        .query(
            "insert $owner isa typebridge-internal-v2-journal-owner, \
             has typebridge-internal-v2-journal-owner-key \"typebridge-journal-owner/v1\", \
             has typebridge-internal-v2-journal-owner-managed-database \"another-database\", \
             has typebridge-internal-v2-journal-owner-managed-scope \"another-scope\";",
        )
        .await
        .expect("insert a foreign owner");
    transaction.commit().await.expect("commit foreign owner");
    let catalog =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let wrong = TypeDbMigrationStore::new(&binding, catalog)
        .unwrap()
        .ensure_control_schema()
        .await
        .expect_err("a foreign immutable owner must reject");
    assert_eq!(
        wrong.code().as_str(),
        "migration_typedb_journal_owner_mismatch"
    );
    assert_eq!(journal.schema_text().await.unwrap(), journal_schema);
    assert_eq!(managed.schema_text().await.unwrap(), managed_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn concurrent_bootstrap_is_singleton_and_read_only_verify_never_bootstraps() {
    let (managed, journal) = databases().await;
    let scope_id = ManagedScopeId::new("concurrent-journal-scope").unwrap();
    let scope = ExecutionScope::new(scope_id.clone());
    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        context_for_scope(scope_id),
    )
    .expect("bind exact managed/journal pair and context");
    let catalog_a =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let store_a = TypeDbMigrationStore::new(&binding, catalog_a).unwrap();
    let journal_before = journal.schema_text().await.unwrap();
    let managed_before = managed.schema_text().await.unwrap();
    let verify_error = store_a
        .load_applied_read_only(&scope)
        .await
        .expect_err("read-only verification cannot bootstrap an empty journal");
    assert_eq!(
        verify_error.code().as_str(),
        "migration_typedb_journal_control_schema_absent"
    );
    assert_eq!(journal.schema_text().await.unwrap(), journal_before);
    assert_eq!(managed.schema_text().await.unwrap(), managed_before);
    assert!(!journal_before.contains("typebridge-internal-v2-"));
    assert!(!managed_before.contains("typebridge-internal-v2-"));

    let catalog_b =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let store_b = TypeDbMigrationStore::new(&binding, catalog_b).unwrap();
    let (first, second) = tokio::join!(
        store_a.ensure_control_schema(),
        store_b.ensure_control_schema()
    );
    first.expect("first serialized bootstrap succeeds");
    second.expect("concurrent serialized verification succeeds");

    let mut transaction = journal.read_transaction().await.unwrap();
    let owners = transaction
        .query(
            "match $owner isa typebridge-internal-v2-journal-owner, \
             has typebridge-internal-v2-journal-owner-key $key, \
             has typebridge-internal-v2-journal-owner-managed-database $database, \
             has typebridge-internal-v2-journal-owner-managed-scope $scope; \
             fetch { \"key\": $key, \"database\": $database, \"scope\": $scope };",
        )
        .await
        .unwrap();
    let (QueryResult::Documents(owners) | QueryResult::Rows(owners)) = owners else {
        panic!("owner query must return documents");
    };
    assert_eq!(owners.len(), 1);
    transaction.close().await.unwrap();

    let catalog_resume =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .unwrap();
    let resumed = TypeDbMigrationStore::new(&binding, catalog_resume).unwrap();
    resumed
        .ensure_control_schema()
        .await
        .expect("a restarted apply/adopt process verifies the existing owner");
    assert!(
        resumed
            .load_applied_read_only(&scope)
            .await
            .expect("read-only verification resumes against the owned journal")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn control_schema_and_fenced_lease_round_trip_on_3_12_1() {
    let (managed_database, journal_database) = databases().await;
    let scope_id = ManagedScopeId::new("journal-live-scope").expect("managed scope id");
    let context = context_for_scope(scope_id.clone());
    let binding = TypeDbExecutionBinding::new(
        Arc::clone(&managed_database),
        Arc::clone(&journal_database),
        context.clone(),
    )
    .expect("bind exact managed/journal pair and context");
    let catalog =
        VerifiedMigrationCatalog::new(std::iter::empty::<&VerifiedSchemaMigrationManifest>())
            .expect("empty verified catalog");
    let store =
        TypeDbMigrationStore::new(&binding, catalog).expect("bind exact managed/journal pair");
    store
        .ensure_control_schema()
        .await
        .expect("install and verify frozen control schema");

    let scope = ExecutionScope::new(scope_id);
    let owner_a = LeaseHolderId::new("live-owner-a").expect("lease owner A");
    let owner_b = LeaseHolderId::new("live-owner-b").expect("lease owner B");

    let lease_a = store.acquire(&scope, &owner_a).await.expect("first lease");
    assert_eq!(lease_a.fence().get(), 1);
    let lease_b = store
        .acquire(&scope, &owner_b)
        .await
        .expect("immediate fenced takeover");
    assert_eq!(lease_b.fence().get(), 2);

    let stale_read = store
        .load_applied(&lease_a)
        .await
        .expect_err("old fence must not read the journal");
    assert_eq!(
        stale_read.code().as_str(),
        "migration_execution_stale_fence"
    );
    let stale_release = store
        .release(&lease_a)
        .await
        .expect_err("old fence must not release the new lease");
    assert_eq!(
        stale_release.code().as_str(),
        "migration_execution_stale_fence"
    );
    assert!(
        store
            .load_applied(&lease_b)
            .await
            .expect("current fence reads the empty ledger")
            .is_empty()
    );

    store
        .release(&lease_b)
        .await
        .expect("release current lease");
    let lease_c = store
        .acquire(&scope, &owner_a)
        .await
        .expect("reacquire after release");
    assert_eq!(lease_c.fence().get(), 3);
    store.release(&lease_c).await.expect("release final lease");

    let export = managed_database
        .schema_text()
        .await
        .expect("export provider schema");
    let partition = partition_typeql_export(
        DocumentId::new("live-provider-export.typeql").expect("document id"),
        &export,
    )
    .expect("partition provider export");
    assert_eq!(partition.user().facts().len(), 0);
    assert!(partition.internal().facts().len() > 0);

    let base = declared(&["person"]);
    let target = declared(&["person", "company"]);
    let migration = verified_manifest(&base, &target, &context);
    let graph =
        MigrationHistoryGraph::from_verified([migration.clone()]).expect("verified history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let plan = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .expect("verified apply plan");
    assert_eq!(plan.migrations().len(), 1);

    let catalog = VerifiedMigrationCatalog::new([&migration]).expect("catalog");
    let journal = TypeDbMigrationStore::new(&binding, catalog)
        .expect("bind exact managed/journal pair")
        .bind_plan(&plan)
        .expect("bind exact plan");
    let journal_scope =
        ExecutionScope::new(ManagedScopeId::new("journal-live-scope").expect("journal scope"));
    let executor_a = LeaseHolderId::new("journal-executor-a").expect("executor A");
    let executor_b = LeaseHolderId::new("journal-executor-b").expect("executor B");
    let lease_one = journal
        .acquire(&journal_scope, &executor_a)
        .await
        .expect("journal fence one");
    assert_eq!(lease_one.fence().get(), 4);

    let plan_record = PlanRecord::from_verified_plan(
        &lease_one,
        &plan,
        plan.applied_migrations(),
        plan.source_state().expect("nonempty plan source"),
    )
    .expect("plan record");
    let stored_plan = journal
        .begin_plan(&lease_one, plan_record)
        .await
        .expect("persist open plan");
    assert_eq!(stored_plan.sequence().get(), 1);

    let migration_apply = &plan.migrations()[0];
    let group = &migration_apply.transaction_groups()[0];
    let before = GroupEventRecord::new(
        &lease_one,
        migration_apply,
        group,
        GroupJournalEventKind::BeforeCommit,
        None,
    )
    .expect("before-commit record");
    let committed_export = managed_database
        .schema_text()
        .await
        .expect("export committed schema before the prepared transaction");
    let mut managed_schema_transaction = managed_database
        .schema_transaction()
        .await
        .expect("open prepared managed schema transaction");
    require_active_managed_fence(&mut managed_schema_transaction, &lease_one)
        .await
        .expect("prepared transaction reads exact managed fence");
    managed_schema_transaction
        .query("define entity live-observer-uncommitted-probe;")
        .await
        .expect("stage uncommitted DDL inside the prepared transaction");
    let export_during_retained_transaction = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        managed_database.schema_text(),
    )
    .await
    .expect(
        "schema_text must not block behind a retained schema transaction; \
         if this hangs, live observation needs a transaction-native export seam",
    )
    .expect("export committed schema while the prepared transaction is retained");
    assert_eq!(
        export_during_retained_transaction, committed_export,
        "schema export must be the byte-stable committed schema, \
         invisible to in-flight uncommitted DDL",
    );
    assert!(
        !export_during_retained_transaction.contains("live-observer-uncommitted-probe"),
        "schema export must not leak uncommitted DDL from the retained transaction",
    );
    let stored_before = journal
        .record_group_event(&lease_one, before)
        .await
        .expect("persist before-commit");
    assert_eq!(stored_before.sequence().get(), 2);
    require_active_managed_fence(&mut managed_schema_transaction, &lease_one)
        .await
        .expect("journal append cannot disturb managed fence");
    managed_schema_transaction
        .rollback()
        .await
        .expect("close prepared transaction after conformance probe");

    let lease_two = journal
        .acquire(&journal_scope, &executor_b)
        .await
        .expect("recovery takeover");
    assert_eq!(lease_two.fence().get(), 5);
    let recovered = journal
        .load_open_plan(&lease_two)
        .await
        .expect("rehydrate open plan")
        .expect("open plan survives takeover");
    assert_eq!(recovered.plan().sequence().get(), 1);
    assert_eq!(recovered.events().len(), 1);
    assert_eq!(recovered.events()[0].sequence().get(), 2);

    let committed = GroupEventRecord::new(
        &lease_two,
        migration_apply,
        group,
        GroupJournalEventKind::Committed,
        Some(
            plan.target_state()
                .expect("nonempty plan target")
                .managed_semantic_schema()
                .clone(),
        ),
    )
    .expect("committed record");
    let stored_committed = journal
        .record_group_event(&lease_two, committed)
        .await
        .expect("persist committed");
    assert_eq!(stored_committed.sequence().get(), 3);

    let applied = AppliedRecord::from_verified_manifest_contract(&lease_two, &migration)
        .expect("applied record");
    let stored_applied = journal
        .record_applied(&lease_two, applied)
        .await
        .expect("persist applied checkpoint");
    assert_eq!(stored_applied.sequence().get(), 4);
    assert!(
        journal
            .load_open_plan(&lease_two)
            .await
            .expect("load completed plan")
            .is_none()
    );
    let applied = journal
        .load_applied(&lease_two)
        .await
        .expect("load applied ledger");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].sequence().get(), 4);
    journal
        .release(&lease_two)
        .await
        .expect("release recovery lease");

    let final_target = declared(&["person", "company", "office"]);
    let second = verified_manifest_named(
        "0002_office",
        vec![migration.id().clone()],
        &target,
        &final_target,
        &context,
    );
    let follow_on_graph = MigrationHistoryGraph::from_verified([migration.clone(), second.clone()])
        .expect("follow-on history");
    let applied_basis = BTreeSet::from([migration.id().clone()]);
    let follow_on_plan = build_verified_migration_apply_plan(
        &follow_on_graph,
        &applied_basis,
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .expect("follow-on plan");
    assert_eq!(
        follow_on_plan.applied_migrations(),
        &[migration.id().clone()]
    );
    assert_eq!(follow_on_plan.migrations().len(), 1);
    assert_eq!(follow_on_plan.migrations()[0].manifest().id(), second.id());

    let reopened_catalog =
        VerifiedMigrationCatalog::new([&migration, &second]).expect("reopened catalog");
    let reopened = TypeDbMigrationStore::new(&binding, reopened_catalog)
        .expect("rebind exact managed/journal pair")
        .bind_plan(&follow_on_plan)
        .expect("bind follow-on plan after store restart");
    let executor_c = LeaseHolderId::new("journal-executor-c").expect("executor C");
    let lease_three = reopened
        .acquire(&journal_scope, &executor_c)
        .await
        .expect("fence three after restart");
    assert_eq!(lease_three.fence().get(), 6);
    let rehydrated = reopened
        .load_applied(&lease_three)
        .await
        .expect("rehydrate applied ledger from verified catalog");
    assert_eq!(rehydrated.len(), 1);
    assert_eq!(rehydrated[0].sequence().get(), 4);
    assert!(
        reopened
            .load_open_plan(&lease_three)
            .await
            .expect("completed plan remains cleared")
            .is_none()
    );

    let follow_on_record = PlanRecord::from_verified_plan(
        &lease_three,
        &follow_on_plan,
        follow_on_plan.applied_migrations(),
        follow_on_plan
            .source_state()
            .expect("follow-on source state"),
    )
    .expect("follow-on plan record");
    let stored_follow_on = reopened
        .begin_plan(&lease_three, follow_on_record)
        .await
        .expect("applied basis gates follow-on begin");
    assert_eq!(stored_follow_on.sequence().get(), 5);

    let follow_on_migration = &follow_on_plan.migrations()[0];
    let follow_on_group = &follow_on_migration.transaction_groups()[0];
    let follow_on_before = GroupEventRecord::new(
        &lease_three,
        follow_on_migration,
        follow_on_group,
        GroupJournalEventKind::BeforeCommit,
        None,
    )
    .expect("follow-on before-commit record");
    assert_eq!(
        reopened
            .record_group_event(&lease_three, follow_on_before)
            .await
            .expect("persist follow-on before-commit")
            .sequence()
            .get(),
        6
    );
    let follow_on_committed = GroupEventRecord::new(
        &lease_three,
        follow_on_migration,
        follow_on_group,
        GroupJournalEventKind::Committed,
        Some(
            follow_on_plan
                .target_state()
                .expect("follow-on target state")
                .managed_semantic_schema()
                .clone(),
        ),
    )
    .expect("follow-on committed record");
    assert_eq!(
        reopened
            .record_group_event(&lease_three, follow_on_committed)
            .await
            .expect("persist follow-on committed")
            .sequence()
            .get(),
        7
    );
    let follow_on_applied = AppliedRecord::from_verified_manifest_contract(&lease_three, &second)
        .expect("follow-on applied record");
    assert_eq!(
        reopened
            .record_applied(&lease_three, follow_on_applied)
            .await
            .expect("persist follow-on applied")
            .sequence()
            .get(),
        8
    );
    let completed_basis = reopened
        .load_applied(&lease_three)
        .await
        .expect("load completed two-manifest ledger");
    assert_eq!(completed_basis.len(), 2);
    assert_eq!(completed_basis[0].record().migration_id(), migration.id());
    assert_eq!(completed_basis[1].record().migration_id(), second.id());
    assert!(
        reopened
            .load_open_plan(&lease_three)
            .await
            .expect("follow-on completion clears open plan")
            .is_none()
    );
    reopened
        .release(&lease_three)
        .await
        .expect("release restarted store lease");

    managed_database
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal_database
        .delete_database()
        .await
        .expect("delete isolated journal database");
}
