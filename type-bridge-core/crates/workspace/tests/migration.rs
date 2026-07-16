use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_schema::{BUILTIN_SCHEMA_CAPABILITY_IDS, SystemSchemaSourceService};
use type_bridge_schema_migration::{
    MigrationGenerationOutcome, SafetyClass, SafetyPolicyDecision,
    typedb_3_12_1_profile,
};
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement,
    MigrationV2Directory, SchemaSetPath, SecretReference, SecretReferenceService,
    TypeBridgeConfig, TypeBridgeConfigServices, TypeBridgeConfigSpec,
    TypeBridgeWorkspace, TypeBridgeWorkspaceServices, WorkspaceConfigErrorCode,
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
    source: &SystemSchemaSourceService,
    secrets: &AcceptSecrets,
    extensions: &AcceptExtensions,
    available: &CapabilitySet,
) -> TypeBridgeWorkspace {
    let config = TypeBridgeConfig::builder(directory.root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .build(&TypeBridgeConfigServices::new(source, secrets, extensions))
        .unwrap();
    let services =
        TypeBridgeWorkspaceServices::new(source, secrets, extensions, available);
    TypeBridgeWorkspace::from_config(config, &services).unwrap()
}

#[test]
fn workspace_makes_writes_and_plans_migrations_offline() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let source = SystemSchemaSourceService;
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = AcceptExtensions(AtomicUsize::new(0));
    let available = capabilities();

    let workspace =
        load_workspace(&directory, &source, &secrets, &extensions, &available);
    let MigrationGenerationOutcome::Generated(first) =
        workspace.migration_make("init").unwrap()
    else {
        panic!("an empty history with a non-empty desired schema generates");
    };
    assert_eq!(first.manifest().id().name().as_str(), "0001_init");
    let manifest_path = workspace.write_generated_migration(&first).unwrap();
    assert!(manifest_path.ends_with("migrations/v2/0001_init.tbmigration.json"));
    let preview = fs::read_to_string(
        directory.0.join("migrations/v2/0001_init.typeql"),
    )
    .unwrap();
    assert!(preview.contains("entity person"), "{preview}");

    // The committed head now equals the desired schema.
    assert!(matches!(
        workspace.migration_make("noop").unwrap(),
        MigrationGenerationOutcome::UpToDate,
    ));

    // Evolving the schema sources chains the next generated migration.
    directory.schema(
        "format: typebridge.schema/v2\nentities: {company: {}, person: {}}\n",
    );
    let workspace =
        load_workspace(&directory, &source, &secrets, &extensions, &available);
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
    assert!(plan.iter().all(|entry| {
        entry.safety() == SafetyClass::Additive && entry.reversible()
    }));
}

#[test]
fn manifest_destructive_policy_tightens_but_never_forces() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let origin = || {
        ConfigOrigin::new(directory.root(), "typebridge.yaml", "policy fixture")
            .unwrap()
    };
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
        rejecting.migration_policy().decision(SafetyClass::Destructive),
        SafetyPolicyDecision::Reject,
    );

    let default = TypeBridgeConfigSpec::parse_yaml(
        manifest("require-approval"),
        origin(),
    )
    .unwrap()
    .resolve(&services)
    .unwrap();
    assert_eq!(
        default.migration_policy().decision(SafetyClass::Destructive),
        SafetyPolicyDecision::RequireApproval,
    );

    // The invalid permanent force shape is rejected by name.
    let forced = TypeBridgeConfigSpec::parse_yaml(manifest("force"), origin())
        .unwrap()
        .resolve(&services)
        .expect_err("a standing destructive allowance is invalid");
    assert_eq!(forced.code(), WorkspaceConfigErrorCode::InvalidWorkspaceValue);
}

#[test]
fn environments_parse_with_symbolic_credentials_and_optin_migrate() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let origin = || {
        ConfigOrigin::new(directory.root(), "typebridge.yaml", "environment fixture")
            .unwrap()
    };
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
    assert_eq!(environment.username().environment_variable(), "TYPEDB_USERNAME");
    assert_eq!(environment.password().environment_variable(), "TYPEDB_PASSWORD");
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
    assert_eq!(literal.code(), WorkspaceConfigErrorCode::SecretLiteralRejected);

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
    assert_eq!(vague.code(), WorkspaceConfigErrorCode::InvalidWorkspaceValue);
}
