//! Identity shared by the sdk-v4 live producers.
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

/// The committed sdk-v4 workspace, authored on `typedb-3.12.1/v1`.
pub fn v4_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/sdk-v4/workspace")
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

/// Copy the sdk-v4 workspace into `destination` for the requested profile.
///
/// On the committed profile the copy is byte-identical. On a newer profile
/// the copy's workspace moves to that profile and gains the profile bridge
/// `migration upgrade-profile` would author, so the 3.12.1 history replays
/// into the run's fresh database on the newer server. The committed
/// workspace is never written.
pub fn prepare_v4_workspace(destination: &Path) {
    prepare_v4_workspace_for(
        destination,
        &sdk_profile_matrix::requested(4).semantic_profile,
    );
}

/// [`prepare_v4_workspace`] for an explicit semantic profile.
pub fn prepare_v4_workspace_for(destination: &Path, requested: &str) {
    copy_tree(&v4_fixture(), destination);
    let manifest = destination.join("typebridge.yaml");
    let config = fs::read_to_string(&manifest).expect("sdk-v4 workspace manifest reads");
    let committed = "  semantic-profile: typedb-3.12.1/v1\n";
    assert!(
        config.contains(committed),
        "the committed sdk-v4 workspace is on typedb-3.12.1/v1"
    );
    if requested == "typedb-3.12.1/v1" {
        return;
    }
    fs::write(
        &manifest,
        config.replacen(committed, &format!("  semantic-profile: {requested}\n"), 1),
    )
    .expect("sdk-v4 workspace manifest rewrites");
    author_v4_profile_bridge(&destination.join("migrations/v2"), requested);
}

/// Author the zero-operation profile bridge with the library calls
/// `migration upgrade-profile` uses for an unbridged history.
fn author_v4_profile_bridge(directory: &Path, requested: &str) {
    let profile = SemanticProfileId::new(requested).expect("requested profile is valid");
    let target = MigrationProfile::for_semantic_profile(&profile)
        .expect("requested profile is a migration profile");
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("sdk-v4").expect("sdk-v4 scope"),
        profile,
        execution_capability_vocabulary().expect("execution capabilities"),
    );
    let genesis = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), [])
        .expect("empty genesis schema");
    let history = MigrationDirectory::open_ambient(directory).expect("sdk-v4 history opens");
    let MigrationProfileUpgrade::Pending {
        graph,
        from,
        parent,
    } = discover_history_for_profile_upgrade_in(&history, &genesis, &context)
        .expect("sdk-v4 history needs a profile bridge")
    else {
        panic!("the committed sdk-v4 history carries no profile bridge");
    };
    let base = format!(
        "profile_upgrade_typedb_{}",
        target.provider_version().replace('.', "_")
    );
    let id =
        MigrationId::new("sdkv4", allocate_migration_name(&graph, &base)).expect("bridge identity");
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

/// The live server an sdk-v4 report on the requested profile declares.
///
/// Connects to the configured TypeDB and fails unless the detected server is
/// exactly the sdk-v4 matrix entry's live server.
pub async fn requested_v4_server_version() -> String {
    let profile = sdk_profile_matrix::requested(4);
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
    .expect("sdk-v4 server-version probe connects");
    let detected = runtime.server_version().map(|version| version.to_string());
    profile.require_server_version(detected.as_deref());
    profile.live_server_version
}

/// [`requested_v4_server_version`] for a synchronous test.
pub fn requested_v4_server_version_blocking() -> String {
    tokio::runtime::Runtime::new()
        .expect("sdk-v4 server-version probe runtime builds")
        .block_on(requested_v4_server_version())
}
