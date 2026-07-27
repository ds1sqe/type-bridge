use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{MigrationAppLabel, MigrationId, MigrationName};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{BUILTIN_SCHEMA_CAPABILITY_IDS, SystemSchemaSourceService};
use type_bridge_schema_compat::{ADOPTED_GENESIS_FILE_NAME, parse_adopted_genesis};
use type_bridge_schema_migration::{
    LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationReference,
    MigrationGenerationOutcome, SafetyClass, SafetyPolicyDecision, build_legacy_frontier_bridge,
    encode_verified_manifest, typedb_3_12_1_profile,
};
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, MigrationV2Directory,
    SchemaSetPath, SecretReference, SecretReferenceService, TypeBridgeConfig,
    TypeBridgeConfigServices, TypeBridgeConfigSpec, TypeBridgeWorkspace,
    TypeBridgeWorkspaceServices, WorkspaceConfigErrorCode, WorkspaceDirectoryAuthority,
    WorkspaceRoot, WorkspaceServiceError,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-workspace-migration-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("migrations/v2")).unwrap();
        Self(path)
    }

    fn root(&self) -> WorkspaceRoot {
        WorkspaceRoot::new(fs::canonicalize(&self.0).unwrap()).unwrap()
    }

    fn write(&self, relative: &str, source: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
    }

    fn schema(&self, fragment: &str) {
        self.write(
            "schema/schema.yaml",
            "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
        );
        self.write("schema/fragments/model.yaml", fragment);
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct AcceptSecrets(AtomicUsize);

impl SecretReferenceService for AcceptSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct AcceptExtensions(AtomicUsize);

impl ExtensionRegistryService for AcceptExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn capabilities() -> CapabilitySet {
    let mut capabilities = typedb_3_12_1_profile().required_capabilities.clone();
    for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
        capabilities.insert(CapabilityId::new(*capability).unwrap());
    }
    for capability in migration_assertion_capability_vocabulary().iter().cloned() {
        capabilities.insert(capability);
    }
    for capability in [
        "migration.conditional-resolution",
        "schema.annotations",
        "schema.doc-meta",
        "schema.roles",
    ] {
        capabilities.insert(CapabilityId::new(capability).unwrap());
    }
    capabilities
}

fn load_workspace(
    directory: &TempDirectory,
    secrets: &AcceptSecrets,
    extensions: &AcceptExtensions,
    available: &CapabilitySet,
) -> TypeBridgeWorkspace {
    let authority = WorkspaceDirectoryAuthority::open(directory.root()).unwrap();
    let config = TypeBridgeConfig::builder(directory.root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .build(&TypeBridgeConfigServices::new(
            &authority, secrets, extensions,
        ))
        .unwrap();
    let services = TypeBridgeWorkspaceServices::new(&authority, secrets, extensions, available);
    TypeBridgeWorkspace::from_config(config, &services).unwrap()
}

const ADOPTED_GENESIS: &str = "define\nentity person;\n";

fn write_adoption_authority_piece(
    directory: &TempDirectory,
    workspace: &TypeBridgeWorkspace,
    genesis: bool,
    bridge: bool,
) {
    if genesis {
        directory.write(
            &format!("migrations/v2/{ADOPTED_GENESIS_FILE_NAME}"),
            ADOPTED_GENESIS,
        );
    }
    if bridge {
        let source = parse_adopted_genesis(
            DocumentId::new("workspace-adoption-test.typeql").unwrap(),
            ADOPTED_GENESIS,
        )
        .unwrap();
        let legacy_id = MigrationId::from_components(
            MigrationAppLabel::new("example").unwrap(),
            MigrationName::new("0001_initial").unwrap(),
        );
        let reference = LegacyMigrationReference::new(
            legacy_id,
            LegacyMigrationChecksum::new("0123456789abcdef").unwrap(),
        );
        let applied_set = LegacyAppliedSetDigest::compute(vec![reference.clone()]).unwrap();
        let bridge_id = MigrationId::from_components(
            MigrationAppLabel::new("example").unwrap(),
            MigrationName::new("0000_legacy_frontier").unwrap(),
        );
        let manifest = build_legacy_frontier_bridge(
            bridge_id,
            vec![reference],
            applied_set,
            &source,
            workspace.delta_context(),
        )
        .unwrap();
        let bytes = encode_verified_manifest(&manifest).unwrap();
        fs::write(
            directory
                .0
                .join("migrations/v2/0000_legacy_frontier.tbmigration.json"),
            bytes,
        )
        .unwrap();
    }
}

fn assert_incomplete_adoption(error: &type_bridge_workspace::TypeBridgeWorkspaceError) {
    assert_eq!(
        error
            .contract()
            .expect("contract diagnostic")
            .code()
            .as_str(),
        "migration_adoption_authority_incomplete"
    );
}

#[test]
fn ordinary_workspace_paths_reject_genesis_without_bridge() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    write_adoption_authority_piece(&directory, &workspace, true, false);

    assert_incomplete_adoption(&workspace.discover_migrations().unwrap_err());
    assert_incomplete_adoption(&workspace.migration_make("must_not_author").unwrap_err());
    assert_incomplete_adoption(&workspace.migration_plan(&BTreeSet::new()).unwrap_err());
}

#[test]
fn ordinary_workspace_paths_reject_bridge_without_genesis() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    write_adoption_authority_piece(&directory, &workspace, false, true);

    assert_incomplete_adoption(&workspace.discover_migrations().unwrap_err());
    assert_incomplete_adoption(&workspace.migration_make("must_not_author").unwrap_err());
    assert_incomplete_adoption(&workspace.migration_plan(&BTreeSet::new()).unwrap_err());
}

#[test]
fn workspace_makes_writes_and_plans_migrations_offline() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();

    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let MigrationGenerationOutcome::Generated(first) = workspace.migration_make("init").unwrap()
    else {
        panic!("an empty history with a non-empty desired schema generates");
    };
    assert_eq!(first.manifest().id().name().as_str(), "0001_init");
    let manifest_path = workspace.write_generated_migration(&first).unwrap();
    assert!(manifest_path.ends_with("migrations/v2/0001_init.tbmigration.json"));
    let preview = fs::read_to_string(directory.0.join("migrations/v2/0001_init.typeql")).unwrap();
    assert!(preview.contains("entity person"), "{preview}");

    // The committed head now equals the desired schema.
    assert!(matches!(
        workspace.migration_make("noop").unwrap(),
        MigrationGenerationOutcome::UpToDate,
    ));

    // Evolving the schema sources chains the next generated migration.
    directory.schema("format: typebridge.schema/v2\nentities: {company: {}, person: {}}\n");
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let MigrationGenerationOutcome::Generated(second) =
        workspace.migration_make("company").unwrap()
    else {
        panic!("a diverged desired schema generates the next migration");
    };
    assert_eq!(second.manifest().id().name().as_str(), "0002_company");
    workspace.write_generated_migration(&second).unwrap();

    let plan = workspace.migration_plan(&Default::default()).unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].id().name().as_str(), "0001_init");
    assert_eq!(plan[1].id().name().as_str(), "0002_company");
    assert!(
        plan.iter()
            .all(|entry| { entry.safety() == SafetyClass::Additive && entry.reversible() })
    );
}

#[test]
fn differently_named_concurrent_candidates_cannot_publish_from_one_stale_head() {
    use std::sync::{Arc, Barrier};

    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let first_workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let second_workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let MigrationGenerationOutcome::Generated(first) = first_workspace
        .migration_make("first")
        .expect("first candidate")
    else {
        panic!("empty history must generate the first candidate");
    };
    let MigrationGenerationOutcome::Generated(second) = second_workspace
        .migration_make("second")
        .expect("second candidate")
    else {
        panic!("empty history must generate the second candidate");
    };
    assert_ne!(first.file_name(), second.file_name());

    let barrier = Arc::new(Barrier::new(2));
    let handles = [
        (first_workspace, first, Arc::clone(&barrier)),
        (second_workspace, second, Arc::clone(&barrier)),
    ]
    .into_iter()
    .map(|(workspace, generated, barrier)| {
        std::thread::spawn(move || {
            barrier.wait();
            workspace.write_generated_migration(&generated)
        })
    })
    .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread does not panic"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let final_workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let graph = final_workspace
        .discover_migrations()
        .expect("one unambiguous history remains");
    assert_eq!(graph.manifests().count(), 1);
    assert!(matches!(
        final_workspace
            .migration_make("noop")
            .expect("head replays"),
        MigrationGenerationOutcome::UpToDate
    ));
}

#[test]
fn manifest_destructive_policy_tightens_but_never_forces() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let origin =
        || ConfigOrigin::new(directory.root(), "typebridge.yaml", "policy fixture").unwrap();
    let source = SystemSchemaSourceService;
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let services = TypeBridgeConfigServices::new(&source, &secrets, &extensions);
    let manifest = |destructive: &str| {
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: example-schema\ncompatibility:\n  \
             semantic-profile: typedb-3.12.1/v1\nmigrations:\n  directory: \
             migrations/v2\n  app-label: example\n  destructive: {destructive}\n"
        )
    };

    let rejecting = TypeBridgeConfigSpec::parse_yaml(manifest("reject"), origin())
        .unwrap()
        .resolve(&services)
        .unwrap();
    assert_eq!(
        rejecting
            .migration_policy()
            .decision(SafetyClass::Destructive),
        SafetyPolicyDecision::Reject,
    );

    let default = TypeBridgeConfigSpec::parse_yaml(manifest("require-approval"), origin())
        .unwrap()
        .resolve(&services)
        .unwrap();
    assert_eq!(
        default
            .migration_policy()
            .decision(SafetyClass::Destructive),
        SafetyPolicyDecision::RequireApproval,
    );

    // The invalid permanent force shape is rejected by name.
    let forced = TypeBridgeConfigSpec::parse_yaml(manifest("force"), origin())
        .unwrap()
        .resolve(&services)
        .expect_err("a standing destructive allowance is invalid");
    assert_eq!(
        forced.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
}

#[test]
fn environments_parse_with_symbolic_credentials_and_optin_migrate() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let origin =
        || ConfigOrigin::new(directory.root(), "typebridge.yaml", "environment fixture").unwrap();
    let source = SystemSchemaSourceService;
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let services = TypeBridgeConfigServices::new(&source, &secrets, &extensions);
    let manifest = |environments: &str| {
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: example-schema\ncompatibility:\n  \
             semantic-profile: typedb-3.12.1/v1\nmigrations:\n  directory: \
             migrations/v2\n  app-label: example\nenvironments:\n{environments}"
        )
    };

    let config = TypeBridgeConfigSpec::parse_yaml(
        manifest(
            "  development:\n    database: myapp_dev\n    uri: localhost:32786\n    \
             http-port: '32787'\n    migrate: 'true'\n    credential:\n      \
             username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n",
        ),
        origin(),
    )
    .unwrap()
    .resolve(&services)
    .unwrap();
    let environment = config.environment("development").expect("environment");
    assert_eq!(environment.uri(), "localhost:32786");
    assert_eq!(environment.database(), "myapp_dev");
    assert_eq!(environment.http_port(), Some(32787));
    assert!(environment.migrate());
    assert_eq!(
        environment.username().environment_variable(),
        "TYPEDB_USERNAME"
    );
    assert_eq!(
        environment.password().environment_variable(),
        "TYPEDB_PASSWORD"
    );
    // Credential validation went through the injected secret service.
    assert!(secrets.0.load(Ordering::Relaxed) >= 2);

    // Committed credential literals are rejected by name.
    let literal = TypeBridgeConfigSpec::parse_yaml(
        manifest(
            "  development:\n    database: myapp_dev\n    uri: localhost:32786\n    \
             credential:\n      username: admin\n      password: env:TYPEDB_PASSWORD\n",
        ),
        origin(),
    )
    .unwrap()
    .resolve(&services)
    .expect_err("a committed credential literal is forbidden");
    assert_eq!(
        literal.code(),
        WorkspaceConfigErrorCode::SecretLiteralRejected
    );

    let vague = TypeBridgeConfigSpec::parse_yaml(
        manifest(
            "  development:\n    database: myapp_dev\n    uri: localhost:32786\n    \
             migrate: 'yes'\n    credential:\n      username: env:U\n      \
             password: env:P\n",
        ),
        origin(),
    )
    .unwrap()
    .resolve(&services)
    .expect_err("migrate admits only true or false");
    assert_eq!(
        vague.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
}

#[cfg(unix)]
#[test]
fn migration_directories_reject_symbolic_link_escapes() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let outside = TempDirectory::new();

    // Swap the confined history directory for a link pointing outside the
    // workspace: lexical config validation cannot see it, so resolution
    // must check the filesystem and fail closed.
    fs::remove_dir_all(directory.0.join("migrations/v2")).unwrap();
    std::os::unix::fs::symlink(&outside.0, directory.0.join("migrations/v2")).unwrap();

    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);

    let error = workspace
        .open_migration_directory()
        .expect_err("a symlinked history directory is not migration authority");
    assert_eq!(
        error.config().expect("config-level rejection").code(),
        WorkspaceConfigErrorCode::PathNotConfined,
    );
    assert!(workspace.discover_migrations().is_err());
    assert!(workspace.migration_make("escape").is_err());
}

#[cfg(unix)]
#[test]
fn retained_directory_authority_survives_component_swap_without_redirecting() {
    use std::os::unix::fs::symlink;

    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let outside = TempDirectory::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let authority = workspace
        .open_migration_directory()
        .expect("real migration directory opens");

    let held_path = directory.0.join("migrations/v2-held");
    fs::rename(directory.0.join("migrations/v2"), &held_path).expect("directory swaps out");
    symlink(&outside.0, directory.0.join("migrations/v2")).expect("replacement symlink");
    fs::write(
        outside.0.join("0001_foreign.tbmigration.json"),
        b"foreign authority",
    )
    .expect("foreign candidate writes");

    let MigrationGenerationOutcome::Generated(generated) = workspace
        .migration_make_in(&authority, "anchored")
        .expect("discovery remains anchored to the opened directory")
    else {
        panic!("the empty retained history must generate");
    };
    workspace
        .write_generated_migration_in(&authority, &generated)
        .expect("publication remains anchored to the opened directory");

    assert!(held_path.join(generated.file_name()).is_file());
    assert!(!outside.0.join(generated.file_name()).exists());
    assert_eq!(
        fs::read(outside.0.join("0001_foreign.tbmigration.json")).expect("foreign bytes read"),
        b"foreign authority",
    );
}

#[test]
fn migration_directory_authority_cannot_cross_workspace_instances() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let first = load_workspace(&directory, &secrets, &extensions, &available);
    let second = load_workspace(&directory, &secrets, &extensions, &available);
    let authority = first
        .open_migration_directory()
        .expect("first workspace opens its migration authority");

    let error = second
        .discover_migrations_in(&authority)
        .expect_err("another workspace instance cannot borrow the authority");
    assert_eq!(
        error.config().expect("authority rejection").code(),
        WorkspaceConfigErrorCode::PathNotConfined,
    );
}

#[cfg(unix)]
#[test]
fn convenience_write_rejects_a_swapped_migration_history() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);
    let MigrationGenerationOutcome::Generated(generated) =
        workspace.migration_make("anchored").expect("generation")
    else {
        panic!("the empty history must generate");
    };

    let held = directory.0.join("migrations/v2-held");
    fs::rename(directory.0.join("migrations/v2"), &held).expect("original history moves");
    fs::create_dir(directory.0.join("migrations/v2")).expect("replacement history creates");
    fs::write(
        directory.0.join("migrations/v2/adopted-genesis.typeql"),
        "define\nentity company;\n",
    )
    .expect("replacement history receives different genesis");

    let error = workspace
        .write_generated_migration(&generated)
        .expect_err("a draft derived from the old history must not publish into the replacement");
    assert_eq!(
        error
            .contract()
            .expect("integrity rejection")
            .code()
            .as_str(),
        "migration_adoption_authority_incomplete",
    );
    assert!(!held.join(generated.file_name()).exists());
    assert!(
        !directory
            .0
            .join("migrations/v2")
            .join(generated.file_name())
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn workspace_root_swap_cannot_redirect_reopened_migration_authority() {
    use std::os::unix::fs::symlink;

    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let outside = TempDirectory::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);

    let held_root = directory.0.with_extension("retained-root");
    fs::rename(&directory.0, &held_root).expect("workspace root moves after validation");
    symlink(&outside.0, &directory.0).expect("workspace name redirects outside");

    let MigrationGenerationOutcome::Generated(generated) = workspace
        .migration_make("anchored-root")
        .expect("migration discovery uses the retained workspace root")
    else {
        panic!("the retained empty history must generate");
    };
    workspace
        .write_generated_migration(&generated)
        .expect("migration publication uses the retained workspace root");

    assert!(
        held_root
            .join("migrations/v2")
            .join(generated.file_name())
            .is_file()
    );
    assert!(
        !outside
            .0
            .join("migrations/v2")
            .join(generated.file_name())
            .exists(),
        "root replacement redirected migration publication"
    );

    fs::remove_file(&directory.0).expect("replacement symlink removes");
    fs::rename(&held_root, &directory.0).expect("workspace root restores for cleanup");
}

#[test]
fn adopted_genesis_read_is_bounded_before_parsing() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    fs::write(
        directory.0.join("migrations/v2/adopted-genesis.typeql"),
        vec![b' '; type_bridge_schema_compat::MAX_TYPEQL_SCHEMA_BYTES + 1],
    )
    .expect("oversized artifact writes");
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();
    let workspace = load_workspace(&directory, &secrets, &extensions, &available);

    let error = workspace
        .migration_genesis()
        .expect_err("oversized adopted authority must fail before parsing");
    assert_eq!(
        error
            .contract()
            .expect("resource diagnostic")
            .code()
            .as_str(),
        "workspace_adopted_genesis_oversized",
    );
}
