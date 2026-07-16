use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId,
    SchemaDeltaStep,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact,
    SubFact, SubFactId, TypeFact,
};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::SemanticProfileBinding;
use type_bridge_contract::migration::MigrationStep;
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_query::{
    MigrationAssertionValidationContext, lower_condition_to_plan,
};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile,
    derive_safety_conditions, diff_managed, inverse_delta, managed_schema_state,
    resolve,
};
use type_bridge_schema_migration::{
    LeaseHolderId, MigrationApplyApproval, MigrationApplyTarget,
    MigrationExecutionOutcome, MigrationRollbackOutcome, MigrationSafetyPolicy,
    SchemaLoweringBinding, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_verified_manifest,
    encode_verified_manifest, schema_lowering_profile_binding,
};
use type_bridge_schema_migration_typedb::{
    MigrationDirectoryApplyOutcome, MigrationDirectoryRollbackOutcome,
    TypeDbMigrationRunner, derived_journal_database_name,
    execution_capability_vocabulary,
};

fn connection() -> (String, String, String, String, ConnectOptions) {
    let address =
        env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let database = env::var("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE")
        .unwrap_or_else(|_| "type_bridge_v2_apply_runner".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password =
        env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
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
    type_bridge_orm::ensure_database_exists(
        &address,
        &managed_name,
        &username,
        &password,
        options.clone(),
    )
    .await
    .expect("create isolated managed database");
    type_bridge_orm::ensure_database_exists(
        &address,
        &journal_name,
        &username,
        &password,
        options.clone(),
    )
    .await
    .expect("create isolated journal database");
    let managed = Arc::new(
        Database::connect_with_options(
            &address,
            &managed_name,
            &username,
            &password,
            options.clone(),
        )
        .await
        .expect("connect isolated managed database"),
    );
    let journal = Arc::new(
        Database::connect_with_options(
            &address,
            &journal_name,
            &username,
            &password,
            options,
        )
        .await
        .expect("connect isolated journal database"),
    );
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
        SubFactId::new(entity_id(child), entity_id(parent))
            .expect("fixture sub identity"),
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
        type_bridge_contract::managed_scope::ManagedScopeId::new(
            "runner-live-scope",
        )
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
    let resolved = resolve(source, context.semantic_profile())
        .expect("resolved assertion source");
    let source_state =
        managed_schema_state(source, context).expect("managed assertion source");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &source_state);
    let mut steps = Vec::new();
    for (ordinal, operation) in delta.operations().iter().enumerate() {
        let derived = derive_safety_conditions(
            ordinal,
            operation,
            source,
            target,
            &safety_profile,
        )
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
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, steps)
        .expect("draft");
    build_verified_manifest(draft, (source, context)).expect("verified manifest")
}

fn write_manifest(directory: &Path, manifest: &VerifiedSchemaMigrationManifest) {
    fs::write(
        directory.join(format!("{}.tbmigration.json", manifest.id().name().as_str())),
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
    let first =
        manifest_with_derived_assertions("0001_person", Vec::new(), &genesis, &first_target, &context);
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

    let lowering =
        SchemaLoweringBinding::current(context.available_capabilities().clone())
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
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
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
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
        .await
        .expect("repeat apply on an up-to-date ledger");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

    write_manifest(directory.path(), &third);
    let outcome = runner
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
        .await
        .expect("incremental apply from the live applied basis");
    assert!(matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied)
    ));
    let export = managed.schema_text().await.expect("post-increment export");
    assert!(export.contains("company"), "{export}");

    let outcome = runner
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
        .await
        .expect("final apply on a fully applied ledger");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

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
    let second_target =
        declared_facts(vec![type_fact("company"), type_fact("person")]);
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

    let lowering =
        SchemaLoweringBinding::current(context.available_capabilities().clone())
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
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
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

    let approval =
        MigrationApplyApproval::for_rollback(&second, SafetyClass::Destructive)
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
        MigrationDirectoryRollbackOutcome::Executed(
            MigrationRollbackOutcome::RolledBack
        )
    ));
    let export = managed.schema_text().await.expect("post-rollback export");
    assert!(!export.contains("entity company"), "{export}");
    assert!(export.contains("entity person"), "{export}");

    let outcome = runner
        .rollback(
            directory.path(),
            &removals,
            &holder,
            std::slice::from_ref(&approval),
        )
        .await
        .expect("repeat rollback on a retired ledger");
    assert!(matches!(outcome, MigrationDirectoryRollbackOutcome::UpToDate));

    // The retired head is pending again and re-applies from the directory.
    let outcome = runner
        .apply(directory.path(), &MigrationApplyTarget::DefaultHead, &holder, &[])
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
