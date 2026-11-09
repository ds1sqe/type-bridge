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
use type_bridge_schema_compat::{MANAGED_FENCE_SCHEMA_TYPEQL, typeql_to_declared};
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
    // A record edit is detected even when the old sequence counter is unchanged.
    old.execute_raw(
        "match $r isa typebridge-internal-v2-migration-journal; delete $r;",
        TxType::Write,
    )
    .await
    .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn atomic_typeql_data_step_applies_once_reverses_and_reapplies() {
    atomic_data_round_trip(0, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn atomic_plan_data_step_applies_once_reverses_and_reapplies() {
    atomic_data_round_trip(1, false).await;
}

fn entity_data_programs(
    schema: &DeclaredSchema,
    context: &ManagedDeltaContext,
    label: &str,
) -> (
    type_bridge_contract::migration_data::DataProgram,
    type_bridge_contract::migration_data::DataProgram,
) {
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::migration_data::DataProgram;
    use type_bridge_contract::query_plan::{
        DeleteItem, InsertThing, Operation, PlanSpec, QueryOutput, QueryPattern, QueryPlan, Stage,
        WriteReturns,
    };
    let binding = BindingId::new(0).unwrap();
    let semantics = managed_schema_state(schema, context)
        .unwrap()
        .managed_semantic_schema()
        .clone();
    let build = |pipeline| {
        QueryPlan::new(
            PlanSpec {
                bindings: vec![AssertionBinding::new(
                    binding,
                    QueryVariable::new("person").unwrap(),
                )],
                inputs: vec![],
                given_rows: None,
                operation: Operation::Write {
                    returns: WriteReturns::Matched,
                },
                pipeline,
                output: QueryOutput::Rows { columns: vec![] },
                managed_semantics: semantics.clone(),
                functions: vec![],
            },
            schema,
        )
        .unwrap()
    };
    let forward = build(vec![Stage::Insert {
        things: vec![InsertThing {
            binding,
            type_id: entity_id(label),
            fields: vec![],
            players: vec![],
        }],
    }]);
    let reverse = build(vec![
        Stage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding,
                    type_id: entity_id(label),
                    include_subtypes: false,
                }
                .into(),
            ],
        },
        Stage::Delete {
            items: vec![DeleteItem::Thing { binding }],
        },
    ]);
    (
        DataProgram::Plan {
            plan: Box::new(forward),
        },
        DataProgram::Plan {
            plan: Box::new(reverse),
        },
    )
}

async fn atomic_data_round_trip(program_kind: u8, checkpoint_gap: bool) {
    use type_bridge_contract::migration_data::{DataProgram, DataStep};
    let (managed, journal) = databases().await;
    let context = context();
    let empty = declared_facts(vec![]);
    let schema = declared_facts(vec![type_fact("data-person")]);
    let first = manifest_with_derived_assertions("0001_schema", vec![], &empty, &schema, &context);
    let (forward, reverse) = if program_kind == 1 {
        entity_data_programs(&schema, &context, "data-person")
    } else if (2..=4).contains(&program_kind) {
        let user = DataProgram::User {
            step_id: MigrationStepId::new("seed-person").unwrap(),
        };
        (user.clone(), user)
    } else {
        (
            DataProgram::Typeql {
                text: "insert $p isa data-person;".into(),
            },
            DataProgram::Typeql {
                text: "match $p isa data-person; delete $p;".into(),
            },
        )
    };
    let step = DataStep::new(
        MigrationStepId::new("seed-person").unwrap(),
        forward,
        (program_kind != 5).then_some(reverse),
    )
    .unwrap();
    let data = build_verified_manifest(
        SchemaMigrationDraft::new(
            migration_id("0002_data"),
            vec![first.id().clone()],
            vec![MigrationStep::Data(step)],
        )
        .unwrap(),
        (&schema, &context),
    )
    .unwrap();
    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &data);
    let lowering = SchemaLoweringBinding::for_context(&context).unwrap();
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        empty,
        context.clone(),
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let mut users = type_bridge_schema_migration_typedb::MigrationUserSteps::default();
    let escaped = Arc::new(std::sync::Mutex::new(None));
    let attempted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    if (2..=4).contains(&program_kind) {
        let inject_failure = program_kind == 2 || program_kind == 4;
        let escaped = Arc::clone(&escaped);
        let attempted = Arc::clone(&attempted);
        users.register_with_reverse(MigrationStepId::new("seed-person").unwrap(),
            move |tx| {
                let escaped = Arc::clone(&escaped);
                let attempted = Arc::clone(&attempted);
                async move {
                    assert!(tx.commit().await.is_err());
                    assert!(tx.rollback().await.is_err());
                    assert!(tx.close().await.is_err());
                    *escaped.lock().unwrap() = Some(tx.clone());
                    tx.execute_query("insert $p isa data-person;").await.unwrap();
                    if attempted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 && inject_failure {
                        assert_ne!(program_kind, 4, "injected callback panic");
                        return Err(type_bridge_contract::diagnostic::Diagnostic::new(
                            type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
                            type_bridge_contract::diagnostic::DiagnosticCode::new("injected_callback_error").unwrap(),
                            "failure after data write"));
                    }
                    Ok(())
                }
            },
            |tx| async move {
                tx.execute_query("match $p isa data-person; delete $p;").await.unwrap();
                Ok(())
            }).unwrap();
    }
    let runner = runner.with_user_steps(users);
    let holder = LeaseHolderId::new("data-runner").unwrap();
    let approval = MigrationApplyApproval::for_manifest(&data).unwrap();
    if checkpoint_gap {
        use type_bridge_schema_migration::{
            MigrationExecutionJournal, MigrationExecutionProvider, MigrationLeaseStore,
        };
        runner
            .apply(
                directory.path(),
                &MigrationApplyTarget::Explicit(BTreeSet::from([first.id().clone()])),
                &holder,
                &[],
            )
            .await
            .unwrap();
        let graph = type_bridge_schema_migration::MigrationHistoryGraph::from_verified([
            first.clone(),
            data.clone(),
        ])
        .unwrap();
        let plan = type_bridge_schema_migration::build_verified_migration_apply_plan(
            &graph,
            &BTreeSet::from([first.id().clone()]),
            &MigrationApplyTarget::DefaultHead,
            &context,
            &SchemaLoweringBinding::for_context(&context).unwrap(),
            &MigrationSafetyPolicy::default_policy(),
            std::slice::from_ref(&approval),
        )
        .unwrap();
        let binding = type_bridge_schema_migration_typedb::TypeDbExecutionBinding::new(
            Arc::clone(&managed),
            Arc::clone(&journal),
            context.clone(),
        )
        .unwrap();
        let catalog = type_bridge_schema_migration_typedb::VerifiedMigrationCatalog::new(
            graph.manifests().map(|(_, manifest)| manifest),
        )
        .unwrap();
        let store =
            type_bridge_schema_migration_typedb::TypeDbMigrationStore::new(&binding, catalog)
                .unwrap()
                .bind_plan(&plan)
                .unwrap();
        let provider = type_bridge_schema_migration_typedb::TypeDbMigrationProvider::new(
            &binding,
            plan.schema_authorities(),
        )
        .unwrap();
        let lease = store
            .acquire(
                &type_bridge_schema_migration::ExecutionScope::new(context.scope_id().clone()),
                &holder,
            )
            .await
            .unwrap();
        let source = plan.source_state().unwrap();
        let observed = provider
            .observe_managed_state(&lease, source, source)
            .await
            .unwrap();
        let record = type_bridge_schema_migration::PlanRecord::from_verified_plan(
            &lease,
            &plan,
            &[first.id().clone()],
            &observed,
        )
        .unwrap();
        store.begin_plan(&lease, record).await.unwrap();
        let step = data.steps()[0].as_data().unwrap();
        let event = type_bridge_schema_migration::DataStepEventRecord::new(
            &lease,
            &data,
            step,
            type_bridge_schema_migration::DataStepDirection::Forward,
            type_bridge_schema_migration::GroupJournalEventKind::BeforeCommit,
        )
        .unwrap();
        store.record_data_event(&lease, event).await.unwrap();
        if program_kind == 3 {
            let cancellation = type_bridge_schema_migration::MigrationCancellation::default();
            let signal = cancellation.clone();
            let mut callbacks = type_bridge_schema_migration_typedb::MigrationUserSteps::default();
            let escaped = Arc::new(std::sync::Mutex::new(None));
            let retained = Arc::clone(&escaped);
            callbacks
                .register(MigrationStepId::new("seed-person").unwrap(), move |tx| {
                    let signal = signal.clone();
                    let retained = Arc::clone(&retained);
                    async move {
                        *retained.lock().unwrap() = Some(tx.clone());
                        tx.execute_query("insert $p isa data-person;")
                            .await
                            .unwrap();
                        signal.cancel();
                        std::future::pending().await
                    }
                })
                .unwrap();
            let provider = provider.with_user_steps(callbacks);
            let control = type_bridge_schema_migration::MigrationExecutionControl::new(
                cancellation,
                None,
                Default::default(),
            );
            let failure = provider
                .execute_data_step(
                    &lease,
                    &data,
                    step,
                    type_bridge_schema_migration::DataStepDirection::Forward,
                    &control,
                )
                .await
                .unwrap_err();
            assert_eq!(
                failure.certainty(),
                type_bridge_schema_migration::GroupCommitCertainty::DefinitelyAborted
            );
            assert_eq!(
                failure.diagnostic().code().as_str(),
                "migration_execution_cancelled"
            );
            let saved = escaped.lock().unwrap().take().unwrap();
            assert!(saved.query("insert $p isa data-person;").await.is_err());
            let event = type_bridge_schema_migration::DataStepEventRecord::new(
                &lease,
                &data,
                step,
                type_bridge_schema_migration::DataStepDirection::Forward,
                type_bridge_schema_migration::GroupJournalEventKind::DefinitelyAborted,
            )
            .unwrap();
            store.record_data_event(&lease, event).await.unwrap();
        } else {
            provider
                .execute_data_step(
                    &lease,
                    &data,
                    step,
                    type_bridge_schema_migration::DataStepDirection::Forward,
                    &type_bridge_schema_migration::MigrationExecutionControl::default(),
                )
                .await
                .unwrap();
        }
        // Stop at the real durability boundary: data and marker committed,
        // but no Committed event or applied record was ever written.
        store.release(&lease).await.unwrap();
    }
    if program_kind == 2 || program_kind == 4 {
        let error = runner
            .apply(
                directory.path(),
                &MigrationApplyTarget::DefaultHead,
                &holder,
                std::slice::from_ref(&approval),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(if program_kind == 4 {
                "migration_user_step_panicked"
            } else {
                "injected_callback_error"
            }),
            "{error}"
        );
        let saved = escaped.lock().unwrap().take().unwrap();
        assert!(saved.query("insert $p isa data-person;").await.is_err());
        let rows = managed
            .execute_raw(
                "match $p isa data-person; fetch { \"iid\": iid($p) };",
                TxType::Read,
            )
            .await
            .unwrap();
        assert!(
            matches!(rows, type_bridge_orm::session::backend::QueryResult::Documents(rows) if rows.is_empty())
        );
    }
    runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            std::slice::from_ref(&approval),
        )
        .await
        .unwrap();
    assert!(matches!(
        runner
            .apply(
                directory.path(),
                &MigrationApplyTarget::DefaultHead,
                &holder,
                std::slice::from_ref(&approval)
            )
            .await
            .unwrap(),
        MigrationDirectoryApplyOutcome::UpToDate
    ));
    let count = |rows: type_bridge_orm::session::backend::QueryResult| match rows {
        type_bridge_orm::session::backend::QueryResult::Documents(rows)
        | type_bridge_orm::session::backend::QueryResult::Rows(rows) => rows.len(),
        other => panic!("unexpected answer {other:?}"),
    };
    let query = "match $p isa data-person; fetch { \"iid\": iid($p) };";
    assert_eq!(
        count(managed.execute_raw(query, TxType::Read).await.unwrap()),
        1
    );
    if program_kind == 5 {
        let control_query = "match $c isa typebridge-internal-migration-control, has typebridge-internal-lease-fence $f, has typebridge-internal-next-sequence $n; fetch { \"fence\": $f, \"next\": $n };";
        let before = journal
            .execute_raw(control_query, TxType::Read)
            .await
            .unwrap();
        let error = runner
            .rollback(
                directory.path(),
                &BTreeSet::from([data.id().clone()]),
                &holder,
                &[],
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("migration_rollback_irreversible_data"),
            "{error}"
        );
        let after = journal
            .execute_raw(control_query, TxType::Read)
            .await
            .unwrap();
        assert_eq!(format!("{before:?}"), format!("{after:?}"));
        assert_eq!(
            count(managed.execute_raw(query, TxType::Read).await.unwrap()),
            1
        );
        managed.delete_database().await.unwrap();
        journal.delete_database().await.unwrap();
        return;
    }
    let reverse = MigrationApplyApproval::for_rollback(&data, SafetyClass::Opaque).unwrap();
    runner
        .rollback(
            directory.path(),
            &BTreeSet::from([data.id().clone()]),
            &holder,
            &[reverse],
        )
        .await
        .unwrap();
    assert_eq!(
        count(managed.execute_raw(query, TxType::Read).await.unwrap()),
        0
    );
    runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[approval],
        )
        .await
        .unwrap();
    assert_eq!(
        count(managed.execute_raw(query, TxType::Read).await.unwrap()),
        1
    );
    managed.delete_database().await.unwrap();
    journal.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn unregistered_user_step_refuses_before_any_control_write() {
    use type_bridge_contract::migration_data::{DataProgram, DataStep};
    let (managed, journal) = databases().await;
    let context = context();
    let empty = declared_facts(vec![]);
    let schema = declared_facts(vec![type_fact("data-person")]);
    let first = manifest_with_derived_assertions("0001_schema", vec![], &empty, &schema, &context);
    let data = build_verified_manifest(
        SchemaMigrationDraft::new(
            migration_id("0002_user"),
            vec![first.id().clone()],
            vec![MigrationStep::Data(
                DataStep::new(
                    MigrationStepId::new("derive").unwrap(),
                    DataProgram::User {
                        step_id: MigrationStepId::new("derive-person").unwrap(),
                    },
                    None,
                )
                .unwrap(),
            )],
        )
        .unwrap(),
        (&schema, &context),
    )
    .unwrap();
    let directory = TempDirectory::new();
    write_manifest(directory.path(), &first);
    write_manifest(directory.path(), &data);
    let lowering = SchemaLoweringBinding::for_context(&context).unwrap();
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        empty,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let before_managed = managed.schema_text().await.unwrap();
    let before_journal = journal.schema_text().await.unwrap();
    let holder = LeaseHolderId::new("unregistered-user").unwrap();
    let error = runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[MigrationApplyApproval::for_manifest(&data).unwrap()],
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("migration_user_step_unregistered"),
        "{error}"
    );
    assert_eq!(managed.schema_text().await.unwrap(), before_managed);
    assert_eq!(journal.schema_text().await.unwrap(), before_journal);
    // A user program outside the selected closure does not block schema apply.
    runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::Explicit(BTreeSet::from([first.id().clone()])),
            &holder,
            &[],
        )
        .await
        .unwrap();
    managed.delete_database().await.unwrap();
    journal.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn atomic_user_callback_rolls_back_errors_and_reverses() {
    atomic_data_round_trip(2, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn committed_data_marker_repairs_missing_journal_checkpoint_live() {
    atomic_data_round_trip(0, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn cancelled_user_callback_rolls_back_and_invalidates_handles_live() {
    atomic_data_round_trip(3, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn panicking_user_callback_rolls_back_before_retry_live() {
    atomic_data_round_trip(4, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn irreversible_data_refuses_rollback_before_lease_write_live() {
    atomic_data_round_trip(5, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB server paired with TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
async fn mixed_data_history_recovers_after_a_retired_rollback_prefix() {
    use type_bridge_contract::migration_data::{DataProgram, DataStep};
    let (managed, journal) = databases().await;
    let context = context();
    let empty = declared_facts(vec![]);
    let schema = declared_facts(vec![
        type_fact("data-a"),
        type_fact("data-b"),
        type_fact("data-c"),
    ]);
    let first = manifest_with_derived_assertions("0001_schema", vec![], &empty, &schema, &context);
    let user = DataProgram::User {
        step_id: MigrationStepId::new("derive-a").unwrap(),
    };
    let plan = entity_data_programs(&schema, &context, "data-c");
    let programs = [
        ("0002_user", user.clone(), user),
        (
            "0003_typeql",
            DataProgram::Typeql {
                text: "insert $p isa data-b;".into(),
            },
            DataProgram::Typeql {
                text: "match $p isa data-b; delete $p;".into(),
            },
        ),
        ("0004_plan", plan.0, plan.1),
    ];
    let mut manifests = vec![first];
    for (name, forward, reverse) in programs {
        let step = DataStep::new(
            MigrationStepId::new("derive").unwrap(),
            forward,
            Some(reverse),
        )
        .unwrap();
        manifests.push(
            build_verified_manifest(
                SchemaMigrationDraft::new(
                    migration_id(name),
                    vec![manifests.last().unwrap().id().clone()],
                    vec![MigrationStep::Data(step)],
                )
                .unwrap(),
                (&schema, &context),
            )
            .unwrap(),
        );
    }
    let directory = TempDirectory::new();
    for manifest in &manifests {
        write_manifest(directory.path(), manifest);
    }
    let forward = manifests[1..]
        .iter()
        .map(|manifest| MigrationApplyApproval::for_manifest(manifest).unwrap())
        .collect::<Vec<_>>();
    let reverse = manifests[1..]
        .iter()
        .map(|manifest| {
            MigrationApplyApproval::for_rollback(manifest, SafetyClass::Opaque).unwrap()
        })
        .collect::<Vec<_>>();
    let removals = manifests[1..]
        .iter()
        .map(|manifest| manifest.id().clone())
        .collect();
    let reverse_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reverse_attempt = Arc::clone(&reverse_calls);
    let mut users = type_bridge_schema_migration_typedb::MigrationUserSteps::default();
    users
        .register_with_reverse(
            MigrationStepId::new("derive-a").unwrap(),
            |tx| async move {
                tx.execute_query("insert $p isa data-a;").await.unwrap();
                Ok(())
            },
            move |tx| {
                let attempts = Arc::clone(&reverse_attempt);
                async move {
                    tx.execute_query("match $p isa data-a; delete $p;")
                        .await
                        .unwrap();
                    if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        return Err(type_bridge_contract::diagnostic::Diagnostic::new(
                            type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
                            type_bridge_contract::diagnostic::DiagnosticCode::new(
                                "injected_reverse_error",
                            )
                            .unwrap(),
                            "failure after reverse write",
                        ));
                    }
                    Ok(())
                }
            },
        )
        .unwrap();
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        empty,
        context.clone(),
        SchemaLoweringBinding::for_context(&context).unwrap(),
        MigrationSafetyPolicy::default_policy(),
    )
    .with_user_steps(users);
    let holder = LeaseHolderId::new("mixed-data").unwrap();
    runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &forward,
        )
        .await
        .unwrap();
    let error = runner
        .rollback(directory.path(), &removals, &holder, &reverse)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("injected_reverse_error"),
        "{error}"
    );
    for (label, expected) in [("data-a", 1), ("data-b", 0), ("data-c", 0)] {
        let result = managed
            .execute_raw(
                &format!("match $p isa {label}; fetch {{ \"iid\": iid($p) }};"),
                TxType::Read,
            )
            .await
            .unwrap();
        assert!(
            matches!(result, type_bridge_orm::session::backend::QueryResult::Documents(rows) if rows.len() == expected)
        );
    }
    runner
        .rollback(directory.path(), &removals, &holder, &reverse)
        .await
        .unwrap();
    assert_eq!(reverse_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    runner
        .apply(
            directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &forward,
        )
        .await
        .unwrap();
    for label in ["data-a", "data-b", "data-c"] {
        let result = managed
            .execute_raw(
                &format!("match $p isa {label}; fetch {{ \"iid\": iid($p) }};"),
                TxType::Read,
            )
            .await
            .unwrap();
        assert!(
            matches!(result, type_bridge_orm::session::backend::QueryResult::Documents(rows) if rows.len() == 1)
        );
    }
    managed.delete_database().await.unwrap();
    journal.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB server"]
async fn catalog_import_checks_adoption_evidence_before_any_new_journal_write() {
    use type_bridge_schema_migration::{
        MigrationCatalog, VerifiedMigrationHistoryBundle, discover_verified_migration_chain,
        encode_verified_migration_history_bundle,
    };
    use type_bridge_schema_migration_typedb::{
        ManagedDatabasePairAdministrator, ManagedDatabasePairCreateOutcome,
        ManagedDatabasePairState, execute_catalog_apply_plan_controlled,
    };
    for mismatch in [
        None,
        Some("anchor"),
        Some("sentinel"),
        Some("applied-set"),
        Some("bridge"),
    ] {
        let (managed, journal) = databases().await;
        journal.delete_database().await.unwrap();
        let context = context();
        let source = "define entity person;\n";
        let head = type_bridge_schema_compat::parse_adopted_genesis(
            DocumentId::new("catalog-adopted.typeql").unwrap(),
            source,
        )
        .unwrap();
        let target = type_bridge_schema_compat::parse_adopted_genesis(
            DocumentId::new("catalog-target.typeql").unwrap(),
            "define entity person; entity company;\n",
        )
        .unwrap();
        let bridge = adopted_history::bridge(&context, source);
        adopted_history::seed(&managed, &journal, &context, &bridge, source).await;
        let old = managed.derived_legacy_journal_database();
        let administrator = ManagedDatabasePairAdministrator::new(
            Arc::clone(&managed),
            Arc::clone(&journal),
            context.scope_id().clone(),
        )
        .unwrap();
        assert_eq!(
            administrator.inspect().await.unwrap(),
            ManagedDatabasePairState::ImportPending
        );
        assert_eq!(
            administrator.create_database_outcome().await.unwrap(),
            ManagedDatabasePairCreateOutcome::AlreadyExists
        );
        let mut tx = managed.read_transaction().await.unwrap();
        type_bridge_orm::plan_authority::verify_import_pending_query_control_scope(
            &mut tx,
            context.scope_id(),
            false,
            None,
            &type_bridge_orm::session::backend::AnswerCancellation::default(),
        )
        .await
        .unwrap();
        tx.close().await.unwrap();
        let tamper = match mismatch {
            Some("anchor") => Some((
                managed.as_ref(),
                "match $a isa typebridge-internal-v2-legacy-cutover; delete $a;",
            )),
            Some("sentinel") => Some((
                managed.as_ref(),
                "match $a isa type_bridge_migration, has migration_name \"__legacy_writer_cutover__\"; delete $a;",
            )),
            Some("applied-set") => Some((
                managed.as_ref(),
                "match $a isa type_bridge_migration, has migration_name \"0001_initial\", has migration_checksum $checksum; delete has $checksum of $a; insert $a has migration_checksum \"2222222222222222\";",
            )),
            Some("bridge") => Some((
                &old,
                "match $a isa typebridge-internal-v2-migration-journal; delete $a;",
            )),
            None => None,
            _ => unreachable!(),
        };
        if let Some((database, query)) = tamper {
            database.execute_raw(query, TxType::Write).await.unwrap();
        }
        let directory = TempDirectory::new();
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
        let graph = discover_verified_migration_chain(directory.path(), &head, &context).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog = MigrationCatalog::open(
            &encode_verified_migration_history_bundle(&bundle).unwrap(),
            &context,
        )
        .unwrap();
        let plan = catalog
            .authorize_apply(
                &BTreeSet::from([bridge.id().clone()]),
                &MigrationApplyTarget::DefaultHead,
                &MigrationSafetyPolicy::default_policy(),
                &[],
            )
            .unwrap();
        let result = execute_catalog_apply_plan_controlled(
            Arc::clone(&managed),
            &catalog,
            &LeaseHolderId::new("sdk-first-import").unwrap(),
            &plan,
            &type_bridge_schema_migration::MigrationExecutionControl::default(),
        )
        .await;
        if let Some(mismatch) = mismatch {
            let error = result.expect_err(mismatch);
            assert_eq!(
                error.code().as_str(),
                "migration_journal_import_invalid",
                "{mismatch}: {error}"
            );
            assert!(
                !journal.database_exists().await.unwrap(),
                "{mismatch} wrote a new journal"
            );
        } else {
            assert!(matches!(
                result.unwrap(),
                MigrationExecutionOutcome::Applied { .. }
            ));
            assert_eq!(
                administrator.inspect().await.unwrap(),
                ManagedDatabasePairState::OwnedPair
            );
            assert!(
                managed
                    .schema_text()
                    .await
                    .unwrap()
                    .contains("entity company")
            );
            managed
                .execute_raw(
                    "match $row isa type_bridge_migration; delete $row;",
                    TxType::Write,
                )
                .await
                .unwrap();
            let next_target = type_bridge_schema_compat::parse_adopted_genesis(
                DocumentId::new("catalog-next.typeql").unwrap(),
                "define entity person; entity company; entity project;\n",
            )
            .unwrap();
            write_manifest(
                directory.path(),
                &manifest_with_derived_assertions(
                    "0002_project",
                    vec![migration_id("0001_company")],
                    &target,
                    &next_target,
                    &context,
                ),
            );
            let graph =
                discover_verified_migration_chain(directory.path(), &head, &context).unwrap();
            let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
            let catalog = MigrationCatalog::open(
                &encode_verified_migration_history_bundle(&bundle).unwrap(),
                &context,
            )
            .unwrap();
            let applied = type_bridge_schema_migration_typedb::load_catalog_applied_migrations(
                Arc::clone(&managed),
                &catalog,
            )
            .await
            .unwrap()
            .into_iter()
            .collect();
            let repeat = catalog
                .authorize_apply(
                    &applied,
                    &MigrationApplyTarget::DefaultHead,
                    &MigrationSafetyPolicy::default_policy(),
                    &[],
                )
                .unwrap();
            let repeated = execute_catalog_apply_plan_controlled(
                Arc::clone(&managed),
                &catalog,
                &LeaseHolderId::new("sdk-after-import").unwrap(),
                &repeat,
                &type_bridge_schema_migration::MigrationExecutionControl::default(),
            )
            .await
            .unwrap();
            assert!(matches!(
                repeated,
                MigrationExecutionOutcome::Applied { .. }
            ));
        }
        administrator
            .plan_delete()
            .await
            .unwrap()
            .execute()
            .await
            .unwrap();
        assert!(!old.database_exists().await.unwrap());
        assert!(!managed.database_exists().await.unwrap());
        assert!(!journal.database_exists().await.unwrap());
        administrator.create_database_outcome().await.unwrap();
        assert_eq!(
            administrator.inspect().await.unwrap(),
            ManagedDatabasePairState::StandaloneManaged
        );
        managed.delete_database().await.unwrap();
    }
}
