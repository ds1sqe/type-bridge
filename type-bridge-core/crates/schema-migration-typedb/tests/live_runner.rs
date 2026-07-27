use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};
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
use type_bridge_migration::{
    AppliedMigrationRecord, LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_NAME, LEGACY_WRITER_CUTOVER_MESSAGE, LegacyAdoptionMetadata,
    LegacySchemaEffect, MigrationDependencySpec, MigrationRunRecord, MigrationStateStore,
    TypeDbStateStore, migration_file_checksum,
};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    ConnectOptions, Database, TxType, require_legacy_writer_open_in_transaction,
};
use type_bridge_query::{MigrationAssertionValidationContext, lower_condition_to_plan};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile, derive_safety_conditions,
    diff_managed, inverse_delta, managed_schema_state, resolve,
};
use type_bridge_schema_compat::{
    ADOPTED_GENESIS_FILE_NAME, LEGACY_LEDGER_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL,
    released_typeql_to_declared_projection, typeql_to_declared,
};
use type_bridge_schema_migration::{
    LeaseHolderId, LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationReference,
    MigrationApplyApproval, MigrationApplyTarget, MigrationDriftFinding, MigrationExecutionOutcome,
    MigrationRollbackOutcome, MigrationSafetyPolicy, SchemaLoweringBinding, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_legacy_frontier_bridge, build_verified_manifest,
    encode_verified_manifest, schema_lowering_profile_binding,
};
use type_bridge_schema_migration_typedb::{
    MigrationDirectoryApplyError, MigrationDirectoryApplyOutcome,
    MigrationDirectoryRollbackOutcome, TypeDbMigrationRunner, derived_journal_database_name,
    execution_capability_vocabulary,
};

fn write_legacy_migration(
    directory: &Path,
    app_label: &str,
    name: &str,
    python_source: &str,
    dependencies: Vec<(&str, &str)>,
    schema_typeql: &str,
) -> String {
    let checksum = migration_file_checksum(python_source);
    fs::write(directory.join(format!("{name}.py")), python_source)
        .expect("write legacy python source");
    let dependencies = dependencies
        .into_iter()
        .map(|(app_label, migration_name)| MigrationDependencySpec {
            app_label: app_label.to_owned(),
            migration_name: migration_name.to_owned(),
        })
        .collect::<Vec<_>>();
    let schema_hash = Sha256::digest(schema_typeql.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let source_sha256 = Sha256::digest(python_source.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let archive = LegacyAdoptionMetadata::new(
        app_label,
        name,
        dependencies,
        checksum.clone(),
        source_sha256,
        LegacySchemaEffect::Snapshot,
        MigrationDependencySpec {
            app_label: app_label.to_owned(),
            migration_name: name.to_owned(),
        },
        schema_hash.clone(),
    )
    .expect("legacy adoption metadata");
    fs::write(
        directory.join(format!("{name}.adoption.json")),
        serde_json::to_vec(&archive).expect("adoption metadata encoding"),
    )
    .expect("write adoption metadata");
    let snapshot = directory.join("snapshots/v0001");
    fs::create_dir_all(&snapshot).expect("create snapshot directory");
    fs::write(snapshot.join("schema.tql"), schema_typeql).expect("write snapshot schema");
    fs::write(
        snapshot.join("snapshot.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": "v0001",
            "source_migration": name,
            "schema_hash": schema_hash,
            "file_hashes": {"schema.tql": schema_hash},
        }))
        .expect("snapshot manifest encoding"),
    )
    .expect("write snapshot manifest");
    checksum
}

fn connection() -> (String, String, String, String, ConnectOptions) {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let database = env::var("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE")
        .unwrap_or_else(|_| "type_bridge_v2_apply_runner".to_owned());
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn legacy_writer_fence_survives_a_live_struct_valued_schema_export_on_3_12_1() {
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
        "insert $control isa typebridge-internal-v2-migration-control, has typebridge-internal-v2-control-scope \"live-structured-scope\", has typebridge-internal-v2-lease-fence \"1\", has typebridge-internal-v2-lease-state \"free\";",
    )
    .await
    .expect("insert exact managed control");
    rows.query(&format!(
        "insert $anchor isa typebridge-internal-v2-legacy-cutover, has typebridge-internal-v2-legacy-cutover-key \"typebridge-legacy-cutover-anchor/v1\", has typebridge-internal-v2-legacy-cutover-scope \"live-structured-scope\", has typebridge-internal-v2-legacy-cutover-fingerprint \"{fingerprint}\";"
    ))
    .await
    .expect("insert exact cutover anchor");
    rows.query(&format!(
        "insert $sentinel isa type_bridge_migration, has migration_id \"type_bridge_v2_internal:__legacy_writer_cutover__\", has migration_app_label \"type_bridge_v2_internal\", has migration_name \"__legacy_writer_cutover__\", has migration_applied_at 1970-01-01T00:00:00.000000000, has migration_checksum \"{fingerprint}\";"
    ))
    .await
    .expect("insert exact cutover sentinel");
    rows.commit().await.expect("commit writer authority");

    let schema_manager = type_bridge_orm::SchemaManager::new(managed.as_ref());
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
        type_bridge_contract::fingerprint::SemanticProfileId::new("typedb-3.12.1/v1")
            .expect("profile"),
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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn runner_applies_discovered_chain_incrementally_on_3_12_1() {
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

    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn runner_rejects_fresh_list_projection_drift_before_target_or_checkpoint_on_3_12_1() {
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
        SchemaLoweringBinding::current(context.available_capabilities().clone())
            .expect("lowering binding"),
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
            .contains("migration_typedb_export_invalid"),
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn runner_rolls_back_the_applied_head_and_reapplies_on_3_12_1() {
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

    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
        MigrationDirectoryRollbackOutcome::Executed(MigrationRollbackOutcome::RolledBack)
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn legacy_writer_allows_reserved_label_collisions_without_frozen_capabilities_on_3_12_1() {
    let (managed, journal) = databases().await;
    let collision_schema = r#"define
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-legacy-cutover-key, value string;
attribute typebridge-internal-v2-legacy-cutover-scope, value string;
attribute typebridge-internal-v2-legacy-cutover-fingerprint, value string;
entity typebridge-internal-v2-migration-control;
entity typebridge-internal-v2-legacy-cutover;
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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn runner_imports_a_completed_legacy_frontier_on_3_12_1() {
    let (managed, journal) = databases().await;
    let context = context();

    // The completed legacy history includes released list semantics absent
    // from the portable V2 fact graph. Seed its on-disk pair, applied-ledger
    // record, and exact migrated user schema as a real v1 database carries them.
    let legacy_root = TempDirectory::new();
    let legacy_directory = legacy_root.path().join("example");
    fs::create_dir(&legacy_directory).expect("create app-labelled legacy directory");
    let adopted_genesis = "define\nattribute tag, value string;\n\
                           entity person, owns tag[] @distinct;\n";
    let checksum = write_legacy_migration(
        &legacy_directory,
        "example",
        "0001_initial",
        "class Migration:\n    operations = []\n",
        Vec::new(),
        adopted_genesis,
    );
    let state_store = TypeDbStateStore::new(Arc::clone(&managed));
    state_store
        .ensure_schema()
        .await
        .expect("install the legacy ledger schema");
    state_store
        .record_applied(AppliedMigrationRecord {
            app_label: "example".to_owned(),
            name: "0001_initial".to_owned(),
            checksum: checksum.clone(),
            applied_at: None,
        })
        .await
        .expect("seed the legacy applied ledger");
    let mut transaction = managed
        .schema_transaction()
        .await
        .expect("open legacy schema transaction");
    transaction
        .query(adopted_genesis)
        .await
        .expect("replay the legacy migration effect");
    transaction.commit().await.expect("commit legacy schema");

    // Canonical side: the bridge records the loaded frontier and verifies
    // against the reconstructed legacy head; ordinary work chains onto it.
    let head = released_typeql_to_declared_projection(
        DocumentId::new("live-adopted-list-head.typeql").expect("document id"),
        adopted_genesis,
    )
    .expect("portable adopted head");
    let with_company = released_typeql_to_declared_projection(
        DocumentId::new("live-adopted-list-company.typeql").expect("document id"),
        "define\nattribute tag, value string;\n\
         entity person, owns tag[] @distinct;\nentity company;\n",
    )
    .expect("portable post-adoption target");
    let legacy_frontier = vec![LegacyMigrationReference::new(
        type_bridge_contract::migration::MigrationId::from_components(
            MigrationAppLabel::new("example").expect("legacy app label"),
            MigrationName::new("0001_initial").expect("legacy name"),
        ),
        LegacyMigrationChecksum::new(checksum.clone()).expect("legacy checksum"),
    )];
    let legacy_applied_set = LegacyAppliedSetDigest::compute(legacy_frontier.clone())
        .expect("legacy applied set digest");
    let bridge = build_legacy_frontier_bridge(
        migration_id("0000_legacy_frontier"),
        legacy_frontier,
        legacy_applied_set,
        &head,
        &context,
    )
    .expect("legacy frontier bridge");
    let follow = manifest_with_derived_assertions(
        "0001_company",
        vec![bridge.id().clone()],
        &head,
        &with_company,
        &context,
    );
    let canonical_directory = TempDirectory::new();
    fs::write(
        canonical_directory.path().join(ADOPTED_GENESIS_FILE_NAME),
        adopted_genesis,
    )
    .expect("write adopted genesis authority");
    write_manifest(canonical_directory.path(), &bridge);
    write_manifest(canonical_directory.path(), &follow);

    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let runner = TypeDbMigrationRunner::new(
        Arc::clone(&managed),
        Arc::clone(&journal),
        head,
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    );
    let holder = LeaseHolderId::new("legacy-import").expect("holder");

    let outcome = runner
        .import_legacy_frontier(&legacy_directory, canonical_directory.path(), &holder)
        .await
        .expect("import the completed legacy frontier");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
    ));

    // The cutover pair is durable and exactly fingerprint-bound. Archival
    // readers continue to expose the frozen ledger, including the V2-only row,
    // while every legacy writer surface rejects before mutation.
    let applied_after_cutover = state_store
        .load_applied()
        .await
        .expect("read the adopted legacy ledger");
    let sentinel = applied_after_cutover
        .iter()
        .find(|record| {
            record.app_label == LEGACY_CUTOVER_SENTINEL_APP_LABEL
                && record.name == LEGACY_CUTOVER_SENTINEL_NAME
        })
        .expect("exact V2 cutover sentinel");
    assert_eq!(
        sentinel.applied_at.as_deref(),
        Some(LEGACY_CUTOVER_SENTINEL_APPLIED_AT),
        "TypeDB must return the fixed timestamp byte-for-byte"
    );
    assert_eq!(sentinel.checksum.len(), 64);
    assert!(
        sentinel
            .checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );

    let mut cutover_read = managed
        .read_transaction()
        .await
        .expect("open cutover-pair read snapshot");
    let anchor_result = cutover_read
        .query(
            "match $anchor isa typebridge-internal-v2-legacy-cutover, has typebridge-internal-v2-legacy-cutover-fingerprint $fingerprint; fetch { \"fingerprint\": $fingerprint };",
        )
        .await
        .expect("read managed cutover anchor");
    cutover_read.close().await.expect("close cutover read");
    let (QueryResult::Documents(anchor_docs) | QueryResult::Rows(anchor_docs)) = anchor_result
    else {
        panic!("cutover anchor fetch must return documents");
    };
    let anchor_fingerprint = anchor_docs
        .first()
        .and_then(|document| document.get("fingerprint"))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .expect("anchor fingerprint scalar");
    assert_eq!(sentinel.checksum, anchor_fingerprint);
    assert!(
        state_store
            .load_runs()
            .await
            .expect("read adopted run log")
            .is_empty()
    );

    for rejected in [
        state_store.ensure_schema().await,
        state_store
            .record_applied(AppliedMigrationRecord {
                app_label: "example".to_owned(),
                name: "9999_must_not_write".to_owned(),
                checksum: "blocked".to_owned(),
                applied_at: None,
            })
            .await,
        state_store
            .record_unapplied("example", "0001_initial")
            .await,
        state_store
            .record_run(MigrationRunRecord {
                run_id: "blocked-run".to_owned(),
                app_label: "example".to_owned(),
                name: "0001_initial".to_owned(),
                checksum: checksum.clone(),
                direction: "apply".to_owned(),
                status: "started".to_owned(),
                started_at: "1970-01-01T00:00:00.000000".to_owned(),
                finished_at: None,
                error: None,
                executor_ip: None,
                executor_mac: None,
            })
            .await,
    ] {
        let error = rejected.expect_err("legacy writer must remain closed");
        assert!(
            error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE),
            "{error}"
        );
    }
    let released_schema_manager = type_bridge_orm::SchemaManager::new(managed.as_ref());
    let schema_error = released_schema_manager
        .sync_schema(true, false)
        .await
        .expect_err("the released Rust SchemaManager must share the permanent cutover fence");
    assert!(
        schema_error
            .to_string()
            .contains(LEGACY_WRITER_CUTOVER_MESSAGE),
        "{schema_error}"
    );
    assert_eq!(
        state_store
            .load_applied()
            .await
            .expect("ledger remains readable after rejected writes"),
        applied_after_cutover
    );

    let outcome = runner
        .import_legacy_frontier(&legacy_directory, canonical_directory.path(), &holder)
        .await
        .expect("repeat import on a bridged ledger");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

    // Ordinary canonical work proceeds from the imported basis.
    let outcome = runner
        .apply(
            canonical_directory.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("apply the post-bridge migration");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
    ));
    let export = managed.schema_text().await.expect("post-import export");
    assert!(export.contains("entity company"), "{export}");
    assert!(export.contains("owns tag[]"), "{export}");
    assert!(export.contains("@distinct"), "{export}");

    // Mutating the legacy source breaks byte-for-byte continuity: the
    // checked v1 loader rejects the directory before any provider write.
    fs::write(
        legacy_directory.join("0001_initial.py"),
        "class Migration:\n    operations = [\"changed\"]\n",
    )
    .expect("mutate legacy python source");
    let drifted = runner
        .import_legacy_frontier(&legacy_directory, canonical_directory.path(), &holder)
        .await
        .expect_err("a drifted legacy file must not import");
    assert!(
        drifted
            .to_string()
            .contains("legacy migration directory failed the checked adoption loader"),
        "{drifted}"
    );

    // A valid post-cutover user function may mention the managed partition in
    // its body. Presence-only writer fencing must still inspect the canonical
    // rows; the stricter query/adoption authority remains unchanged.
    let mut extension = managed
        .schema_transaction()
        .await
        .expect("open post-cutover function transaction");
    extension
        .query(
            r#"define
fun compatibility-writer-probe($candidate: person) -> { person }:
  match
    $candidate isa person;
    $control isa typebridge-internal-v2-migration-control;
  return { $candidate };
"#,
        )
        .await
        .expect("install valid function referencing managed control");
    extension
        .commit()
        .await
        .expect("commit post-cutover function");
    let writer = managed
        .transaction_context(TxType::Schema)
        .await
        .expect("open released writer after function extension");
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        require_legacy_writer_open_in_transaction(&writer),
    )
    .await
    .expect("function-bearing schema export must not deadlock")
    .expect_err("the function body must not reopen the released writer");
    assert!(
        error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE),
        "{error}"
    );
    writer
        .rollback()
        .await
        .expect("close rejected released writer transaction");

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
#[ignore = "requires an isolated TypeDB 3.12.1 server"]
async fn runner_verifies_the_migration_state_triad_on_3_12_1() {
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

    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
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
        "migration_typedb_journal_control_schema_absent",
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
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
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
