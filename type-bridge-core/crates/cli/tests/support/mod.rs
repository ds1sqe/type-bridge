//! Identity shared by the administration live producers.
#![allow(dead_code)]

#[path = "../../../../../tests/support/sdk_profile_matrix.rs"]
pub mod sdk_profile_matrix;

use std::fs;
use std::path::{Path, PathBuf};

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_schema::ManagedDeltaContext;
use type_bridge_schema_migration::{
    MigrationDirectory, MigrationProfile, MigrationProfileUpgrade, SchemaMigrationDraft,
    allocate_migration_name, build_verified_manifest, discover_history_for_profile_upgrade_in,
    encode_verified_manifest,
};
use type_bridge_schema_migration_typedb::execution_capability_vocabulary;
use type_bridge_typedb_runtime::{ConnectOptions, TypeDBRuntime};

/// The committed administration workspace, authored on `typedb-3.12.1/v1`.
pub fn administration_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/administration/workspace")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination creates");
    for entry in fs::read_dir(source).expect("source reads") {
        let entry = entry.expect("source entry reads");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("source type reads").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("source file copies");
        }
    }
}

/// Copy the administration workspace into `destination` for the requested profile.
///
/// On the committed profile the copy is byte-identical. On a newer profile
/// the copy's workspace moves to that profile and gains the profile bridge
/// `migration upgrade-profile` would author, so the 3.12.1 history replays
/// into the run's fresh database on the newer server. The committed
/// workspace is never written.
pub fn prepare_administration_workspace(destination: &Path) {
    prepare_administration_native_workspace_for(
        destination,
        &sdk_profile_matrix::requested("administration").semantic_profile,
    );
}

/// Historical bridge-based copy retained for profile-upgrade replay checks.
pub fn prepare_administration_workspace_for(destination: &Path, requested: &str) {
    copy_tree(&administration_fixture(), destination);
    let manifest = destination.join("typebridge.yaml");
    let config = fs::read_to_string(&manifest).expect("administration workspace manifest reads");
    let committed = "  semantic-profile: typedb-3.12.1/v1\n";
    assert!(
        config.contains(committed),
        "the committed administration workspace is on typedb-3.12.1/v1"
    );
    if requested == "typedb-3.12.1/v1" {
        return;
    }
    fs::write(
        &manifest,
        config.replacen(committed, &format!("  semantic-profile: {requested}\n"), 1),
    )
    .expect("administration workspace manifest rewrites");
    author_administration_profile_bridge(&destination.join("migrations/v2"), requested);
}

/// Author the zero-operation profile bridge with the library calls
/// `migration upgrade-profile` uses for an unbridged history.
fn author_administration_profile_bridge(directory: &Path, requested: &str) {
    let profile = SemanticProfileId::new(requested).expect("requested profile is valid");
    let target = MigrationProfile::for_semantic_profile(&profile)
        .expect("requested profile is a migration profile");
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("administration").expect("administration scope"),
        profile,
        execution_capability_vocabulary().expect("execution capabilities"),
    );
    let genesis = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), [])
        .expect("empty genesis schema");
    let history =
        MigrationDirectory::open_ambient(directory).expect("administration history opens");
    let MigrationProfileUpgrade::Pending {
        graph,
        from,
        parent,
    } = discover_history_for_profile_upgrade_in(&history, &genesis, &context)
        .expect("administration history needs a profile bridge")
    else {
        panic!("the committed administration history carries no profile bridge");
    };
    let base = format!(
        "profile_upgrade_typedb_{}",
        target.provider_version().replace('.', "_")
    );
    let id = MigrationId::new("administration", allocate_migration_name(&graph, &base))
        .expect("bridge identity");
    let parent_schema = graph
        .manifest(&parent)
        .expect("bridge parent is in the history")
        .target_schema()
        .clone();
    let draft = SchemaMigrationDraft::profile_bridge(id.clone(), parent, from)
        .expect("profile bridge draft");
    let bridge =
        build_verified_manifest(draft, (&parent_schema, &context)).expect("profile bridge");
    fs::write(
        directory.join(format!("{}.tbmigration.json", id.name().as_str())),
        encode_verified_manifest(&bridge).expect("profile bridge encodes"),
    )
    .expect("profile bridge writes");
}

/// The live server an administration report on the requested profile declares.
///
/// Connects to the configured TypeDB and fails unless the detected server is
/// exactly the administration matrix entry's live server.
pub async fn requested_administration_server_version() -> String {
    let profile = sdk_profile_matrix::requested("administration");
    let runtime = TypeDBRuntime::connect(
        &std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".to_owned())
                .parse()
                .expect("HTTP port is valid"),
            ..ConnectOptions::default()
        },
    )
    .await
    .expect("administration server-version probe connects");
    let detected = runtime.server_version().map(|version| version.to_string());
    profile.require_server_version(detected.as_deref());
    profile.live_server_version
}

/// [`requested_administration_server_version`] for a synchronous test.
pub fn requested_administration_server_version_blocking() -> String {
    tokio::runtime::Runtime::new()
        .expect("administration server-version probe runtime builds")
        .block_on(requested_administration_server_version())
}

/// Cargo output directory shared by the parent and generated consumers.
pub fn cargo_target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

/// Rebuild the same four migration operations natively on the requested profile.
pub fn prepare_administration_native_workspace_for(destination: &Path, requested: &str) {
    use type_bridge_contract::migration_backfill::AttributeBackfillPlan;
    use type_bridge_schema::managed_schema_state;
    use type_bridge_schema_migration::{
        BackfillMigrationGenerationRequest, MigrationGenerationOutcome, MigrationGenerationRequest,
        MigrationHistoryGraph, decode_verified_manifest, generate_backfill_migration,
        generate_next_migration,
    };

    copy_tree(&administration_fixture(), destination);
    if requested == "typedb-3.12.1/v1" {
        return;
    }
    let config_path = destination.join("typebridge.yaml");
    let config = fs::read_to_string(&config_path).expect("workspace config reads");
    fs::write(&config_path, config.replace("typedb-3.12.1/v1", requested))
        .expect("workspace profile rewrites");
    let context = |profile: &str| {
        ManagedDeltaContext::new(
            ManagedScopeId::new("administration").expect("scope"),
            SemanticProfileId::new(profile).expect("profile"),
            execution_capability_vocabulary().expect("execution capabilities"),
        )
    };
    let committed_context = context("typedb-3.12.1/v1");
    let requested_context = context(requested);
    let genesis =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), []).expect("genesis");
    let mut source = genesis.clone();
    let mut rebuilt = Vec::new();
    let directory = destination.join("migrations/v2");
    let mut paths = fs::read_dir(&directory)
        .expect("history directory reads")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".tbmigration.json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let committed = decode_verified_manifest(
            &fs::read(&path).expect("manifest reads"),
            (&source, &committed_context),
        )
        .expect("committed manifest verifies");
        let graph = MigrationHistoryGraph::from_verified(rebuilt.clone()).expect("history graph");
        let base = committed
            .id()
            .name()
            .as_str()
            .split_once('_')
            .expect("ordinal name")
            .1;
        let generated = if let Some((_, plan)) =
            committed.steps().iter().find_map(|step| step.as_backfill())
        {
            let state = managed_schema_state(&source, &requested_context).expect("native state");
            let plan = AttributeBackfillPlan::new(
                plan.owner().clone(),
                plan.source().clone(),
                plan.destination().clone(),
                plan.partition().clone(),
                state.managed_semantic_schema().clone(),
                plan.reverse(),
            )
            .expect("native backfill plan");
            generate_backfill_migration(
                &graph,
                &BackfillMigrationGenerationRequest {
                    app_label: "administration",
                    base_name: base,
                    genesis_source: &genesis,
                    desired: committed.target_schema(),
                    plan: &plan,
                    context: &requested_context,
                },
            )
            .expect("native backfill migration")
        } else {
            let MigrationGenerationOutcome::Generated(generated) = generate_next_migration(
                &graph,
                &MigrationGenerationRequest {
                    app_label: "administration",
                    base_name: base,
                    genesis_source: &genesis,
                    desired: committed.target_schema(),
                    context: &requested_context,
                },
            )
            .expect("native schema migration") else {
                panic!("expected a migration")
            };
            *generated
        };
        assert_eq!(generated.manifest().id(), committed.id());
        fs::write(path, generated.canonical_bytes()).expect("native manifest writes");
        rebuilt.push(generated.manifest().clone());
        source = committed.target_schema().clone();
    }
}
