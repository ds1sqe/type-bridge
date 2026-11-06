mod support;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::SemanticProfileBinding;
use type_bridge_contract::migration::MigrationStep;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::reserved::LEGACY_WRITER_CUTOVER_MESSAGE;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId,
    TypeFact,
};
use type_bridge_orm::{
    ConnectOptions, Database, TxType, require_legacy_writer_open_in_transaction,
};
use type_bridge_query::{MigrationAssertionValidationContext, lower_condition_to_plan};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile, derive_safety_conditions,
    diff_managed, inverse_delta, managed_schema_state, resolve,
};
use type_bridge_schema_compat::{
    LEGACY_LEDGER_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL, typeql_to_declared,
};
use type_bridge_schema_migration::{
    LeaseHolderId, MigrationApplyApproval, MigrationApplyTarget, MigrationDriftFinding,
    MigrationExecutionOutcome, MigrationRollbackOutcome, MigrationSafetyPolicy,
    SchemaLoweringBinding, SchemaMigrationDraft, VerifiedSchemaMigrationManifest,
    build_verified_manifest, encode_verified_manifest, schema_lowering_profile_binding,
};
use type_bridge_schema_migration_typedb::{
    MigrationDirectoryApplyError, MigrationDirectoryApplyOutcome,
    MigrationDirectoryRollbackOutcome, TypeDbMigrationRunner, derived_journal_database_name,
    execution_capability_vocabulary,
};

fn connection() -> (String, String, String, String, ConnectOptions) {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let database = env::var("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE")
        .unwrap_or_else(|_| "type_bridge_apply_runner".to_owned());
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
    support::assert_live_server(&managed);
    support::assert_live_server(&journal);
    (managed, journal)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn legacy_writer_fence_survives_a_live_struct_valued_schema_export_on_3_12_3() {
    let (managed, journal) = databases().await;
    let fingerprint = "0000000000000000000000000000000000000000000000000000000000000000";
    let mut schema = managed
        .schema_transaction()
        .await
        .expect("open isolated schema transaction");
    schema
        .query(MANAGED_FENCE_SCHEMA_TYPEQL)
        .await
        .expect("install exact managed fence schema");
    schema
        .query(LEGACY_LEDGER_SCHEMA_TYPEQL)
        .await
        .expect("install exact frozen ledger schema");
    schema
        .query(
            "define\n\
             struct writer-fence-payload: field value string;\n\
             attribute writer-fence-payload-attr, value writer-fence-payload;",
        )
        .await
        .expect("install structured user value");
    schema.commit().await.expect("commit structured schema");

    let mut rows = managed
        .write_transaction()
        .await
        .expect("open isolated writer-authority transaction");
    rows.query(
        "insert $control isa typebridge-internal-migration-control, has typebridge-internal-control-scope \"live-structured-scope\", has typebridge-internal-lease-fence \"1\", has typebridge-internal-lease-state \"free\";",
    )
    .await
    .expect("insert exact managed control");
    rows.query(&format!(
        "insert $anchor isa typebridge-internal-legacy-cutover, has typebridge-internal-legacy-cutover-key \"typebridge-legacy-cutover-anchor/v1\", has typebridge-internal-legacy-cutover-scope \"live-structured-scope\", has typebridge-internal-legacy-cutover-fingerprint \"{fingerprint}\";"
    ))
    .await
    .expect("insert exact cutover anchor");
    rows.query(&format!(
        "insert $sentinel isa type_bridge_migration, has migration_id \"type_bridge_v2_internal:__legacy_writer_cutover__\", has migration_app_label \"type_bridge_v2_internal\", has migration_name \"__legacy_writer_cutover__\", has migration_applied_at 1970-01-01T00:00:00.000000000, has migration_checksum \"{fingerprint}\";"
    ))
    .await
    .expect("insert exact cutover sentinel");
    rows.commit().await.expect("commit writer authority");

    let schema_manager = type_bridge_orm::_schema::SchemaManager::new(managed.as_ref());
    let error = schema_manager
        .sync_schema(true, false)
        .await
        .expect_err("the exported structured value must not reopen the V1 writer");
    assert!(
        error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE),
        "{error}"
    );

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "typebridge-live-runner-{}-{unique:x}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create migration fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entity_id(label: &str) -> TypeId {
    TypeId::new(TypeKind::Entity, label).expect("fixture type id")
}

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(TypeFact::new(entity_id(label)).expect("fixture type fact"))
}

fn sub_fact(child: &str, parent: &str) -> SchemaFact {
    SchemaFact::Sub(SubFact::new(
        SubFactId::new(entity_id(child), entity_id(parent)).expect("fixture sub identity"),
    ))
}

fn declared_facts(facts: Vec<SchemaFact>) -> DeclaredSchema {
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("live-runner-fixture").expect("document"),
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

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("runner-live-scope")
            .expect("scope"),
        support::live_profile(),
        execution_capability_vocabulary().expect("execution capability vocabulary"),
    )
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("live-runner").expect("app"),
        MigrationName::new(name).expect("name"),
    )
}

fn manifest_with_derived_assertions(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("delta");
    let safety_profile = SafetyDerivationProfile::new(
        SemanticProfileBinding::resolve(context.semantic_profile().clone())
            .expect("semantic profile binding"),
        schema_lowering_profile_binding().expect("lowering profile binding"),
    )
    .expect("safety profile");
    let resolved = resolve(source, context.semantic_profile()).expect("resolved assertion source");
    let source_state = managed_schema_state(source, context).expect("managed assertion source");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &source_state);
    let mut steps = Vec::new();
    for (ordinal, operation) in delta.operations().iter().enumerate() {
        let derived = derive_safety_conditions(ordinal, operation, source, target, &safety_profile)
            .expect("derived safety conditions");
        for (index, condition) in derived.conditions().iter().enumerate() {
            let validated = lower_condition_to_plan(
                condition,
                &validation_context,
                StructuralLimits::CANONICAL,
            )
            .expect("derived assertion plan");
            steps.push(
                MigrationStep::assertion(
                    MigrationStepId::new(format!("assert-{name}-{ordinal}-{index}"))
                        .expect("assertion step id"),
                    validated.plan().clone(),
                    AssertionExpectation::NoRows,
                )
                .expect("assertion step"),
            );
        }
    }
    let reverse = inverse_delta(&delta).expect("inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("step id"),
        delta,
        Some(reverse),
    )
    .expect("schema step");
    steps.push(MigrationStep::from(step));
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, steps).expect("draft");
    build_verified_manifest(draft, (source, context)).expect("verified manifest")
}

fn write_manifest(directory: &Path, manifest: &VerifiedSchemaMigrationManifest) {
    fs::write(
        directory.join(format!(
            "{}.tbmigration.json",
            manifest.id().name().as_str()
        )),
        encode_verified_manifest(manifest).expect("manifest encoding"),
    )
    .expect("write manifest file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn runner_applies_discovered_chain_incrementally_on_3_12_3() {
    let (managed, journal) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let first_target = declared_facts(vec![type_fact("person")]);
    // The second migration reproduces the #190 shape end to end: a subtype is
    // added under a parent that exists only in the already-applied source.
    let second_target = declared_facts(vec![
        type_fact("employee"),
        type_fact("person"),
        sub_fact("employee", "person"),
    ]);
    let third_target = declared_facts(vec![
        type_fact("company"),
        type_fact("employee"),
        type_fact("person"),
        sub_fact("employee", "person"),
    ]);
    let first = manifest_with_derived_assertions(
        "0001_person",
        Vec::new(),
        &genesis,
        &first_target,
        &context,
    );
    let second = manifest_with_derived_assertions(
        "0002_employee_sub_person",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let third = manifest_with_derived_assertions(
        "0003_company",
        vec![second.id().clone()],
        &second_target,
        &third_target,
        &context,
    );

    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &second);

    let lowering = SchemaLoweringBinding::for_context(&context).expect("lowering binding");
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        genesis,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("live-runner").expect("holder");

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("first directory apply");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    let export = managed.schema_text().await.expect("post-apply export");
    assert!(export.contains("entity person"), "{export}");
    assert!(export.contains("entity employee"), "{export}");
    assert!(export.contains("sub person"), "{export}");

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("repeat apply on an up-to-date ledger");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

    write_manifest(directory.path(), &third);
    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("incremental apply from the live applied basis");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    let export = managed.schema_text().await.expect("post-increment export");
    assert!(export.contains("company"), "{export}");

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("final apply on a fully applied ledger");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

    let mut drift = managed
        .schema_transaction()
        .await
        .expect("open out-of-band schema transaction");
    drift
        .query("define entity intruder;")
        .await
        .expect("add out-of-band type");
    drift.commit().await.expect("commit out-of-band type");
    let error = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect_err("an up-to-date ledger cannot hide live schema drift");
    assert!(
        error
            .to_string()
            .contains("migration_typedb_observation_no_candidate_match"),
        "{error}"
    );

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn runner_rejects_fresh_list_projection_drift_before_target_or_checkpoint_on_3_12_3() {
    let (managed, journal) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let first_target = typeql_to_declared(
        DocumentId::new("live-list-first.typeql").expect("document id"),
        "define\nattribute tag, value string;\nentity person, owns tag;\n",
    )
    .expect("portable scalar schema");
    let second_target = typeql_to_declared(
        DocumentId::new("live-list-second.typeql").expect("document id"),
        "define\nattribute tag, value string;\n\
         entity person, owns tag;\nentity company;\n",
    )
    .expect("portable target schema");
    let first = manifest_with_derived_assertions(
        "0001_scalar_ownership",
        Vec::new(),
        &genesis,
        &first_target,
        &context,
    );
    let second = manifest_with_derived_assertions(
        "0002_company",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &second);

    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        genesis,
        context.clone(),
        SchemaLoweringBinding::for_context(&context).expect("lowering binding"),
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("live-list-drift").expect("holder");
    let first_only = MigrationApplyTarget::Explicit(BTreeSet::from([first.id().clone()]));
    let outcome = runner
        .apply(directory.path(), &first_only, &holder, &[])
        .await
        .expect("apply scalar source schema");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));

    let mut drift = managed
        .schema_transaction()
        .await
        .expect("open out-of-band schema transaction");
    drift
        .query("undefine owns tag from person;")
        .await
        .expect("remove scalar ownership");
    drift
        .query("define person owns tag[] @distinct;")
        .await
        .expect("install released list semantics");
    drift.commit().await.expect("commit list drift");

    let error = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect_err("fresh authority must reject list projection drift");
    assert!(
        error
            .to_string()
            .contains("migration_typedb_observation_no_candidate_match"),
        "{error}"
    );
    let rejected_export = managed.schema_text().await.expect("rejected export");
    assert!(
        !rejected_export.contains("entity company"),
        "{rejected_export}"
    );
    assert!(rejected_export.contains("owns tag[]"), "{rejected_export}");

    let mut restore = managed
        .schema_transaction()
        .await
        .expect("open restore transaction");
    restore
        .query("undefine owns tag[] from person;")
        .await
        .expect("remove list ownership");
    restore
        .query("define person owns tag;")
        .await
        .expect("restore scalar ownership");
    restore.commit().await.expect("commit restored schema");

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("rejected migration was not checkpointed");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    let final_export = managed.schema_text().await.expect("final export");
    assert!(final_export.contains("entity company"), "{final_export}");

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn runner_rolls_back_the_applied_head_and_reapplies_on_3_12_3() {
    let (managed, journal) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let first_target = declared_facts(vec![type_fact("person")]);
    let second_target = declared_facts(vec![type_fact("company"), type_fact("person")]);
    let first = manifest_with_derived_assertions(
        "0001_person",
        Vec::new(),
        &genesis,
        &first_target,
        &context,
    );
    let second = manifest_with_derived_assertions(
        "0002_company",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );

    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &second);

    let lowering = SchemaLoweringBinding::for_context(&context).expect("lowering binding");
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        genesis,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("live-rollback").expect("holder");

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("apply the two-migration chain");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    let export = managed.schema_text().await.expect("post-apply export");
    assert!(export.contains("entity company"), "{export}");

    // Rolling back the additive head destroys the company type, so the
    // default policy demands an approval bound to the reverse transition.
    let removals = BTreeSet::from([second.id().clone()]);
    let unapproved = runner
        .rollback(directory.path(), &removals, &holder, &[])
        .await
        .expect_err("destructive reverse work requires approval");
    assert!(
        unapproved
            .to_string()
            .contains("migration_rollback_approval_required"),
        "{unapproved}"
    );

    let approval = MigrationApplyApproval::for_rollback(&second, SafetyClass::Destructive)
        .expect("rollback approval");
    let outcome = runner
        .rollback(
            directory.path(),
            &removals,
            &holder,
            std::slice::from_ref(&approval),
        )
        .await
        .expect("approved head rollback");
    assert!(matches!(
        outcome,
        MigrationDirectoryRollbackOutcome::Executed(MigrationRollbackOutcome::RolledBack { .. })
    ));
    let export = managed.schema_text().await.expect("post-rollback export");
    assert!(!export.contains("entity company"), "{export}");
    assert!(export.contains("entity person"), "{export}");

    let unknown = BTreeSet::from([migration_id("9999_unknown")]);
    let error = runner
        .rollback(directory.path(), &unknown, &holder, &[])
        .await
        .expect_err("an unknown rollback identity is not up to date");
    assert!(
        error
            .to_string()
            .contains("migration_history_unknown_rollback_target"),
        "{error}"
    );

    let outcome = runner
        .rollback(
            directory.path(),
            &removals,
            &holder,
            std::slice::from_ref(&approval),
        )
        .await
        .expect("repeat rollback on a retired ledger");
    assert!(matches!(
        outcome,
        MigrationDirectoryRollbackOutcome::UpToDate
    ));

    // The retired head is pending again and re-applies from the directory.
    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("re-apply the rolled-back head");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    let export = managed.schema_text().await.expect("post-reapply export");
    assert!(export.contains("entity company"), "{export}");

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn legacy_writer_allows_reserved_label_collisions_without_frozen_capabilities_on_3_12_3() {
    let (managed, journal) = databases().await;
    let collision_schema = r#"define
attribute typebridge-internal-control-scope, value string;
attribute typebridge-internal-lease-holder, value string;
attribute typebridge-internal-lease-fence, value string;
attribute typebridge-internal-lease-state, value string;
attribute typebridge-internal-legacy-cutover-key, value string;
attribute typebridge-internal-legacy-cutover-scope, value string;
attribute typebridge-internal-legacy-cutover-fingerprint, value string;
entity typebridge-internal-migration-control;
entity typebridge-internal-legacy-cutover;
"#;
    let mut setup = managed
        .schema_transaction()
        .await
        .expect("open collision schema transaction");
    setup
        .query(collision_schema)
        .await
        .expect("install every reserved label without frozen ownership capabilities");
    setup.commit().await.expect("commit collision schema");

    let writer = managed
        .transaction_context(TxType::Schema)
        .await
        .expect("open released writer transaction");
    tokio::time::timeout(
        Duration::from_secs(10),
        require_legacy_writer_open_in_transaction(&writer),
    )
    .await
    .expect("schema-fenced export must not deadlock an open schema transaction")
    .expect("labels without exact capabilities must not activate the V2 fence");
    writer
        .query("define entity compatibility-proof;")
        .await
        .expect("execute legitimate released schema write");
    writer.commit().await.expect("commit released schema write");

    let export = managed.schema_text().await.expect("read collision export");
    assert!(export.contains("entity compatibility-proof"), "{export}");

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn runner_verifies_the_migration_state_triad_on_3_12_3() {
    let (managed, journal) = databases().await;
    let context = context();
    let genesis = declared_facts(Vec::new());
    let first_target = declared_facts(vec![type_fact("person")]);
    let second_target = declared_facts(vec![type_fact("company"), type_fact("person")]);
    let first = manifest_with_derived_assertions(
        "0001_person",
        Vec::new(),
        &genesis,
        &first_target,
        &context,
    );
    let second = manifest_with_derived_assertions(
        "0002_company",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &second);

    let lowering = SchemaLoweringBinding::for_context(&context).expect("lowering binding");
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        genesis,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("live-verify").expect("holder");

    // Before any apply, the untouched journal database has no control
    // schema; the read-only load refuses to bootstrap one and cannot
    // distinguish a never-migrated pair from a wiped journal, so verify
    // fails closed instead of reporting everything as merely pending.
    let error = runner
        .verify(directory.path(), Some(&second_target))
        .await
        .expect_err("pre-apply verification fails closed");
    let MigrationDirectoryApplyError::Diagnostic(diagnostic) = &error else {
        panic!("expected a diagnostic failure: {error}");
    };
    assert_eq!(
        diagnostic.code().as_str(),
        "migration_journal_import_pending",
    );

    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("apply the chain");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));

    // A coherent triad verifies clean.
    let report = runner
        .verify(directory.path(), Some(&second_target))
        .await
        .expect("clean verification");
    assert!(report.is_clean(), "findings: {:?}", report.findings());
    assert_eq!(report.applied_frontier(), &[second.id().clone()]);
    assert_eq!(
        report.observed_semantics(),
        Some(second.target_state().managed_semantic_schema()),
    );

    // A desired schema ahead of the committed head is divergence.
    let desired = declared_facts(vec![
        type_fact("company"),
        type_fact("person"),
        type_fact("team"),
    ]);
    let report = runner
        .verify(directory.path(), Some(&desired))
        .await
        .expect("desired divergence verification");
    assert!(matches!(
        report.findings(),
        [MigrationDriftFinding::DesiredDivergence { .. }]
    ));

    // Out-of-band schema mutation is live drift, never generation input.
    let mut transaction = managed
        .schema_transaction()
        .await
        .expect("open out-of-band transaction");
    transaction
        .query("define entity intruder;")
        .await
        .expect("mutate the managed schema out of band");
    transaction
        .commit()
        .await
        .expect("commit out-of-band change");
    let report = runner
        .verify(directory.path(), Some(&second_target))
        .await
        .expect("live drift verification");
    let [MigrationDriftFinding::LiveSemantics { recorded, observed }] = report.findings() else {
        panic!("expected exactly one live-semantics finding: {report:?}");
    };
    assert_eq!(recorded, second.target_state().managed_semantic_schema());
    assert_ne!(observed, recorded);

    managed
        .delete_database()
        .await
        .expect("delete isolated managed database");
    journal
        .delete_database()
        .await
        .expect("delete isolated journal database");
}

#[path = "../../../../tests/support/adopted_history.rs"]
mod adopted_history;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn runner_applies_a_completed_legacy_frontier() {
    let (managed, journal) = databases().await;
    let context = context();
    let source = "define\nattribute tag, value string;\nentity person, owns tag[] @distinct;\n";
    let head = type_bridge_schema_compat::parse_adopted_genesis(
        DocumentId::new("head.typeql").unwrap(),
        source,
    )
    .unwrap();
    let target = type_bridge_schema_compat::parse_adopted_genesis(
        DocumentId::new("target.typeql").unwrap(),
        &format!("{source}entity company;\n"),
    )
    .unwrap();
    let bridge = adopted_history::bridge(&context, source);
    adopted_history::seed(&managed, &journal, &context, &bridge, source).await;
    // An interrupted importer may have installed the new schema without
    // publishing its mirror; that must not reopen schema sync.
    managed
        .execute_raw(MANAGED_FENCE_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    adopted_history::assert_writer_closed(&managed).await;
    let old = managed.derived_legacy_journal_database();
    let copied_query = "match $r isa typebridge-internal-migration-journal, has typebridge-internal-record-sequence \"1\", has typebridge-internal-record-payload $p, has typebridge-internal-record-payload-digest $d; fetch { \"payload\": $p, \"digest\": $d };";
    let old_query = copied_query.replace("typebridge-internal-", "typebridge-internal-v2-");
    let before = old.execute_raw(&old_query, TxType::Read).await.unwrap();

    adopted_history::assert_writer_closed(&managed).await;
    let directory = TempDirectory::new();
    fs::write(directory.path().join("adopted-genesis.typeql"), source).unwrap();
    write_manifest(directory.path(), &bridge);
    write_manifest(
        directory.path(),
        &manifest_with_derived_assertions(
            "0001_company",
            vec![bridge.id().clone()],
            &head,
            &target,
            &context,
        ),
    );
    let lowering = SchemaLoweringBinding::for_context(&context).unwrap();
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        head,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("completed-adoption").unwrap();
    let outcome = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    ));
    assert!(matches!(
        runner
            .apply(
                directory.path(),
                &MigrationApplyTarget::DefaultHead,
                &holder,
                &[]
            )
            .await
            .unwrap(),
        MigrationDirectoryApplyOutcome::UpToDate
    ));
    let export = managed.schema_text().await.unwrap();
    assert!(export.contains("entity company"), "{export}");
    assert!(export.contains("owns tag[]"), "{export}");
    assert!(export.contains("@distinct"), "{export}");
    adopted_history::assert_writer_closed(&managed).await;
    let copied = journal
        .execute_raw(copied_query, TxType::Read)
        .await
        .unwrap();
    assert_eq!(
        format!("{before:?}"),
        format!("{copied:?}"),
        "copied payload and digest bytes must be unchanged"
    );
    let untouched = old.execute_raw(&old_query, TxType::Read).await.unwrap();
    assert_eq!(format!("{before:?}"), format!("{untouched:?}"));
    // Once imported, the old applied ledger is no longer an execution dependency.
    managed
        .execute_raw(
            "match $r isa type_bridge_migration; delete $r;",
            TxType::Write,
        )
        .await
        .unwrap();
    runner.verify(directory.path(), None).await.unwrap();
    // A restarted old runner is detected without reading its applied ledger.
    old.execute_raw("match $c isa typebridge-internal-v2-migration-control, has typebridge-internal-v2-next-sequence $n; delete has $n of $c; insert $c has typebridge-internal-v2-next-sequence \"2\";", TxType::Write).await.unwrap();
    let error = runner.verify(directory.path(), None).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("migration_old_writer_after_import"),
        "{error}"
    );
    old.delete_database().await.unwrap();
    managed.delete_database().await.unwrap();
    journal.delete_database().await.unwrap();
}
