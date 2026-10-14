//! Live profile upgrade: a history applied on TypeDB 3.12.3 continues on
//! TypeDB 3.13.6 after a profile-upgrade bridge.
//!
//! No production seam writes a 3.12.1-bound ledger on a 3.13.6 server, so the
//! test needs both servers:
//!
//! - `TYPE_BRIDGE_MIGRATION_UPGRADE_FROM`: address of a TypeDB 3.12.3 server.
//! - `TYPE_BRIDGE_MIGRATION_UPGRADE_TO`: address of a TypeDB 3.13.6 server.
//! - `TYPE_BRIDGE_MIGRATION_UPGRADE_CONSOLE` (optional, default `typedb`): a
//!   TypeDB distribution whose `console` subcommand runs `database export`
//!   and `database import` against both servers.
//!
//! `TYPEDB_USERNAME`/`TYPEDB_PASSWORD` apply to both servers. Without the two
//! addresses the test returns immediately.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_orm::{ConnectOptions, Database};
use type_bridge_schema::{ManagedDeltaContext, diff_managed, inverse_delta};
use type_bridge_schema_migration::{
    LeaseHolderId, MigrationApplyTarget, MigrationDirectory, MigrationExecutionOutcome,
    MigrationProfile, MigrationProfileUpgrade, MigrationSafetyPolicy, SchemaLoweringBinding,
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    discover_history_for_profile_upgrade_in, encode_verified_manifest,
};
use type_bridge_schema_migration_typedb::{
    MigrationDirectoryApplyOutcome, TypeDbMigrationRunner, derived_journal_database_name,
    execution_capability_vocabulary, migration_server_for_profile,
};

/// Address of the TypeDB 3.12.3 server the history is first applied on.
const UPGRADE_FROM_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM";
/// Address of the TypeDB 3.13.6 server the history moves to.
const UPGRADE_TO_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_TO";
/// TypeDB distribution used for `database export` and `database import`.
const UPGRADE_CONSOLE_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_CONSOLE";

const OLD: MigrationProfile = MigrationProfile::Typedb3_12_1;
const NEW: MigrationProfile = MigrationProfile::Typedb3_13_6;

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "typebridge-live-upgrade-{label}-{}-{unique:x}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create fixture directory");
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

fn credentials() -> (String, String) {
    (
        env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
    )
}

async fn connect(address: &str, name: &str, profile: MigrationProfile) -> Arc<Database> {
    let (username, password) = credentials();
    let options = ConnectOptions::default();
    type_bridge_orm::ensure_database_exists(address, name, &username, &password, options)
        .await
        .expect("create isolated database");
    let database = Database::connect_with_options(address, name, &username, &password, options)
        .await
        .expect("connect isolated database");
    assert_eq!(
        database.server_version(),
        migration_server_for_profile(&profile.semantic_profile_id()),
        "{address} must run the TypeDB server paired with {}",
        profile.semantic_profile_id().as_str()
    );
    Arc::new(database)
}

/// Run one TypeDB console command against `address`.
fn console(address: &str, command: &str) {
    let (username, password) = credentials();
    let program = env::var(UPGRADE_CONSOLE_ENV).unwrap_or_else(|_| "typedb".to_owned());
    let status = Command::new(&program)
        .args([
            "console",
            "--address",
            address,
            "--username",
            &username,
            "--password",
            &password,
            "--tls-disabled",
            "--command",
            command,
        ])
        .status()
        .unwrap_or_else(|error| panic!("{UPGRADE_CONSOLE_ENV}={program:?} cannot run: {error}"));
    assert!(status.success(), "console `{command}` on {address} failed");
}

/// Move one database between servers with TypeDB export and import.
fn move_database(name: &str, from: &str, to: &str, files: &Path) {
    let schema = files.join(format!("{name}.typeql"));
    let data = files.join(format!("{name}.typedb"));
    let (schema, data) = (schema.display(), data.display());
    console(from, &format!("database export {name} {schema} {data}"));
    console(to, &format!("database import {name} {schema} {data}"));
}

fn declared(labels: &[&str]) -> DeclaredSchema {
    let sourced = labels.iter().enumerate().map(|(index, label)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            SchemaFact::Type(
                TypeFact::new(TypeId::new(TypeKind::Entity, *label).expect("fixture type"))
                    .expect("fixture type fact"),
            ),
            SourceSpan::new(
                DocumentId::new("live-upgrade-fixture").expect("document"),
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

fn context(profile: MigrationProfile) -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("upgrade-live-scope").expect("scope"),
        profile.semantic_profile_id(),
        execution_capability_vocabulary().expect("execution capability vocabulary"),
    )
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("live-upgrade").expect("app"),
        MigrationName::new(name).expect("name"),
    )
}

fn step_manifest(
    name: &str,
    parent: Option<&VerifiedSchemaMigrationManifest>,
    target_labels: &[&str],
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    let genesis = declared(&[]);
    let source = parent.map_or(&genesis, |parent| parent.target_schema());
    let delta = diff_managed(source, &declared(target_labels), context).expect("delta");
    let reverse = inverse_delta(&delta).expect("inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("step id"),
        delta,
        Some(reverse),
    )
    .expect("schema step");
    let parents = parent
        .map(|parent| parent.id().clone())
        .into_iter()
        .collect();
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, vec![step]).expect("draft");
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

fn runner(
    managed: &Arc<Database>,
    journal: &Arc<Database>,
    profile: MigrationProfile,
) -> TypeDbMigrationRunner {
    let context = context(profile);
    let lowering = SchemaLoweringBinding::for_context(&context).expect("lowering binding");
    TypeDbMigrationRunner::new(
        Arc::clone(managed),
        Arc::clone(journal),
        declared(&[]),
        context,
        lowering,
        MigrationSafetyPolicy::default_policy(),
    )
}

fn applied(outcome: &MigrationDirectoryApplyOutcome) -> bool {
    matches!(
        outcome,
        MigrationDirectoryApplyOutcome::Executed(MigrationExecutionOutcome::Applied { .. })
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TYPE_BRIDGE_MIGRATION_UPGRADE_FROM (TypeDB 3.12.3) and \
            TYPE_BRIDGE_MIGRATION_UPGRADE_TO (TypeDB 3.13.6)"]
async fn runner_upgrades_a_3_12_history_to_3_13_at_a_profile_bridge() {
    let (Ok(from), Ok(to)) = (env::var(UPGRADE_FROM_ENV), env::var(UPGRADE_TO_ENV)) else {
        eprintln!("{UPGRADE_FROM_ENV} and {UPGRADE_TO_ENV} are unset; skipping");
        return;
    };
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let managed_name = format!("type_bridge_v2_upgrade_{}_{unique:x}", std::process::id());
    let journal_name = derived_journal_database_name(&managed_name);
    let history = TempDirectory::new("history");
    let holder = LeaseHolderId::new("live-upgrade").expect("holder");

    // 1. Apply a small history on the 3.12.3 server.
    let old_context = context(OLD);
    let root = step_manifest("0001_person", None, &["person"], &old_context);
    let more = step_manifest(
        "0002_company",
        Some(&root),
        &["company", "person"],
        &old_context,
    );
    write_manifest(history.path(), &root);
    write_manifest(history.path(), &more);
    let from_managed = connect(&from, &managed_name, OLD).await;
    let from_journal = connect(&from, &journal_name, OLD).await;
    let outcome = runner(&from_managed, &from_journal, OLD)
        .apply(
            history.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("apply the 3.12.1 history");
    assert!(applied(&outcome), "{outcome:?}");

    // 2. Move both databases to the 3.13.6 server.
    let files = TempDirectory::new("export");
    move_database(&managed_name, &from, &to, files.path());
    move_database(&journal_name, &from, &to, files.path());
    let to_managed = connect(&to, &managed_name, NEW).await;
    let to_journal = connect(&to, &journal_name, NEW).await;

    // 3. The workspace moves to 3.13.6; ordinary commands require the bridge.
    let new_runner = runner(&to_managed, &to_journal, NEW);
    let error = new_runner
        .apply(
            history.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect_err("an unbridged 3.12.1 history needs an upgrade");
    assert!(
        error
            .to_string()
            .contains("migration_history_profile_upgrade_required"),
        "{error}"
    );

    // 4. Author and publish the bridge from the discovered head.
    let new_context = context(NEW);
    let directory = MigrationDirectory::open_ambient(history.path()).expect("history directory");
    let MigrationProfileUpgrade::Pending {
        graph,
        from: from_profile,
        parent,
    } = discover_history_for_profile_upgrade_in(&directory, &declared(&[]), &new_context)
        .expect("upgrade discovery")
    else {
        panic!("the history has no bridge yet");
    };
    assert_eq!(from_profile, OLD);
    assert_eq!(&parent, more.id());
    let draft = SchemaMigrationDraft::profile_bridge(
        migration_id("0003_profile_upgrade"),
        parent.clone(),
        from_profile,
    )
    .expect("bridge draft");
    let parent_schema = graph.manifest(&parent).expect("parent").target_schema();
    let bridge =
        build_verified_manifest(draft, (parent_schema, &new_context)).expect("bridge manifest");
    write_manifest(history.path(), &bridge);

    // The 3.12.3 server is not the upgrade target.
    let error = runner(&from_managed, &from_journal, NEW)
        .upgrade_profile_in(&directory, &holder)
        .await
        .expect_err("the old server cannot record the bridge");
    assert!(
        error
            .to_string()
            .contains("migration_typedb_server_version_unsupported"),
        "{error}"
    );

    // 5. Live drift refuses the upgrade before the ledger changes.
    let mut drift = to_managed.schema_transaction().await.expect("drift tx");
    drift
        .query("define entity intruder;")
        .await
        .expect("add out-of-band type");
    drift.commit().await.expect("commit out-of-band type");
    let error = new_runner
        .upgrade_profile_in(&directory, &holder)
        .await
        .expect_err("live drift refuses the upgrade");
    assert!(
        error
            .to_string()
            .contains("migration_upgrade_profile_live_drift"),
        "{error}"
    );
    let mut repair = to_managed.schema_transaction().await.expect("repair tx");
    repair
        .query("undefine intruder;")
        .await
        .expect("remove out-of-band type");
    repair.commit().await.expect("commit repair");

    // 6. Record the bridge, then again as a no-op.
    let outcome = new_runner
        .upgrade_profile_in(&directory, &holder)
        .await
        .expect("record the bridge");
    assert!(applied(&outcome), "{outcome:?}");
    let outcome = new_runner
        .upgrade_profile_in(&directory, &holder)
        .await
        .expect("repeat the upgrade");
    assert!(matches!(outcome, MigrationDirectoryApplyOutcome::UpToDate));

    // 7. The history continues on 3.13.6 and verifies clean.
    let after = step_manifest(
        "0004_project",
        Some(&bridge),
        &["company", "person", "project"],
        &new_context,
    );
    write_manifest(history.path(), &after);
    let outcome = new_runner
        .apply(
            history.path(),
            &MigrationApplyTarget::DefaultHead,
            &holder,
            &[],
        )
        .await
        .expect("apply past the bridge");
    assert!(applied(&outcome), "{outcome:?}");
    let report = new_runner
        .verify(history.path(), Some(after.target_schema()))
        .await
        .expect("verify the bridged history");
    assert!(report.is_clean(), "findings: {:?}", report.findings());
    assert_eq!(report.applied_frontier(), &[after.id().clone()]);

    for database in [to_managed, to_journal, from_managed, from_journal] {
        database
            .delete_database()
            .await
            .expect("delete isolated database");
    }
}
