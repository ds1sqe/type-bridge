use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, MAX_WORKSPACE_LOCK_BYTES,
    MigrationV2Directory, OutputDirectory, SchemaSetPath, SecretReference, SecretReferenceService,
    TypeBridgeConfig, TypeBridgeConfigServices, TypeBridgeConfigSpec, TypeBridgeWorkspace,
    TypeBridgeWorkspaceServices, WorkspaceDirectoryAuthority, WorkspaceLockErrorCode,
    WorkspaceRoot, WorkspaceServiceError, generate_workspace_lock, verify_workspace_lock,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-workspace-lock-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        let value = Self(path);
        value.write(
            "schema/schema.yaml",
            "# manifest source\nformat: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
        );
        value.write(
            "schema/fragments/model.yaml",
            "# model source\nformat: typebridge.schema/v2\nentities: {person: {}}\n",
        );
        value
    }

    fn root(&self) -> WorkspaceRoot {
        WorkspaceRoot::new(fs::canonicalize(&self.0).unwrap()).unwrap()
    }

    fn write(&self, relative: &str, source: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Secrets;

impl SecretReferenceService for Secrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

struct Extensions;

impl ExtensionRegistryService for Extensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

fn capabilities() -> CapabilitySet {
    ["schema.annotations", "schema.doc-meta", "schema.roles"]
        .into_iter()
        .map(|value| CapabilityId::new(value).unwrap())
        .collect()
}

fn programmatic_workspace(
    directory: &TempDirectory,
    app_label: &str,
    with_output: bool,
) -> TypeBridgeWorkspace {
    let source = WorkspaceDirectoryAuthority::open(directory.root()).unwrap();
    let secrets = Secrets;
    let extensions = Extensions;
    let available = capabilities();
    let mut builder = TypeBridgeConfig::builder(directory.root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new(app_label).unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .require_capability(CapabilityId::new("schema.doc-meta").unwrap());
    if with_output {
        builder = builder.output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        );
    }
    let config = builder
        .build(&TypeBridgeConfigServices::new(
            &source,
            &secrets,
            &extensions,
        ))
        .unwrap();
    TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .unwrap()
}

fn workspace_yaml(comment: &str) -> String {
    format!(
        "# {comment}\nformat: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: example-schema\ncompatibility:\n  semantic-profile: typedb-3.12.1/v1\n  require: [schema.doc-meta]\nmigrations:\n  directory: migrations/v2\n  app-label: example\n"
    )
}

fn located_workspace(
    directory: &TempDirectory,
    source_text: &str,
    bytes: bool,
) -> TypeBridgeWorkspace {
    let source = WorkspaceDirectoryAuthority::open(directory.root()).unwrap();
    let secrets = Secrets;
    let extensions = Extensions;
    let available = capabilities();
    let origin = ConfigOrigin::new(directory.root(), "typebridge.yaml", "lock fixture").unwrap();
    let located = if bytes {
        TypeBridgeConfigSpec::from_yaml_bytes(source_text.as_bytes(), origin).unwrap()
    } else {
        TypeBridgeConfigSpec::parse_yaml(source_text, origin).unwrap()
    };
    TypeBridgeWorkspace::from_located_config(
        located,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .unwrap()
}

fn replace_first(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .replacen(from, to, 1)
        .into_bytes()
}

#[test]
fn generate_twice_is_byte_stable_and_roundtrips_only_through_verification() {
    let directory = TempDirectory::new();
    let workspace = programmatic_workspace(&directory, "example", false);
    let first = generate_workspace_lock(&workspace).unwrap();
    let second = generate_workspace_lock(&workspace).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let verified = verify_workspace_lock(first.bytes(), &workspace).unwrap();
    assert_eq!(verified.bytes(), first.bytes());
    assert_eq!(verified.lock(), &first);
}

#[test]
fn malformed_noncanonical_unknown_oversize_and_tampered_bytes_fail_closed() {
    let directory = TempDirectory::new();
    let workspace = programmatic_workspace(&directory, "example", false);
    let lock = generate_workspace_lock(&workspace).unwrap();

    for bytes in [b"{".as_slice(), b"{ }".as_slice()] {
        let error = verify_workspace_lock(bytes, &workspace).unwrap_err();
        assert_eq!(error.code(), WorkspaceLockErrorCode::Contract);
    }

    let mut unknown = String::from_utf8(lock.bytes().to_vec()).unwrap();
    unknown.insert_str(unknown.rfind('}').unwrap(), ",\"zz_unknown\":true");
    let error = verify_workspace_lock(unknown.as_bytes(), &workspace).unwrap_err();
    assert_eq!(error.code(), WorkspaceLockErrorCode::Contract);

    let oversized = vec![b' '; MAX_WORKSPACE_LOCK_BYTES + 1];
    let error = verify_workspace_lock(&oversized, &workspace).unwrap_err();
    assert_eq!(error.code(), WorkspaceLockErrorCode::Contract);

    let text = String::from_utf8(lock.bytes().to_vec()).unwrap();
    let marker = "\"digest\":\"";
    let digest = text.find(marker).unwrap() + marker.len();
    let mut tampered = text.into_bytes();
    tampered[digest] = if tampered[digest] == b'0' { b'1' } else { b'0' };
    let error = verify_workspace_lock(&tampered, &workspace).unwrap_err();
    assert_eq!(error.code(), WorkspaceLockErrorCode::Stale);

    let unsupported = replace_first(
        lock.bytes(),
        "typebridge.workspace-lock/v1",
        "typebridge.workspace-lock/v2",
    );
    let error = verify_workspace_lock(&unsupported, &workspace).unwrap_err();
    assert_eq!(error.code(), WorkspaceLockErrorCode::UnsupportedVersion);
}

#[test]
fn source_byte_drift_and_stale_glob_expansion_are_rejected() {
    let directory = TempDirectory::new();
    let first = programmatic_workspace(&directory, "example", false);
    let first_lock = generate_workspace_lock(&first).unwrap();

    directory.write(
        "schema/fragments/model.yaml",
        "# changed bytes\nformat: typebridge.schema/v2\nentities: {person: {}}\n",
    );
    let changed = programmatic_workspace(&directory, "example", false);
    assert_eq!(
        verify_workspace_lock(first_lock.bytes(), &changed)
            .unwrap_err()
            .code(),
        WorkspaceLockErrorCode::Stale
    );
    let changed_lock = generate_workspace_lock(&changed).unwrap();

    directory.write(
        "schema/fragments/added.yaml",
        "format: typebridge.schema/v2\nentities: {company: {}}\n",
    );
    let expanded = programmatic_workspace(&directory, "example", false);
    assert_eq!(
        verify_workspace_lock(changed_lock.bytes(), &expanded)
            .unwrap_err()
            .code(),
        WorkspaceLockErrorCode::Stale
    );
}

#[test]
fn config_profile_scope_and_capability_claim_tampering_is_rejected() {
    let directory = TempDirectory::new();
    let workspace = programmatic_workspace(&directory, "example", false);
    let lock = generate_workspace_lock(&workspace).unwrap();

    for tampered in [
        replace_first(lock.bytes(), "example-schema", "changed-schema"),
        replace_first(lock.bytes(), "typedb-3.12.1/v1", "typedb-3.11.5/v1"),
        replace_first(lock.bytes(), "schema.doc-meta", "schema.annotations"),
    ] {
        assert_eq!(
            verify_workspace_lock(&tampered, &workspace)
                .unwrap_err()
                .code(),
            WorkspaceLockErrorCode::Stale
        );
    }

    let changed_config = programmatic_workspace(&directory, "different", false);
    assert_eq!(
        verify_workspace_lock(lock.bytes(), &changed_config)
            .unwrap_err()
            .code(),
        WorkspaceLockErrorCode::Stale
    );
}

#[test]
fn authoring_provenance_differs_honestly_while_semantics_and_text_bytes_converge() {
    let directory = TempDirectory::new();
    let source_text = workspace_yaml("authored source");
    let from_text = located_workspace(&directory, &source_text, false);
    let from_bytes = located_workspace(&directory, &source_text, true);
    let programmatic = programmatic_workspace(&directory, "example", false);

    assert_eq!(
        from_text.resolved_schema().semantic_fingerprint(),
        programmatic.resolved_schema().semantic_fingerprint()
    );
    let text_lock = generate_workspace_lock(&from_text).unwrap();
    let bytes_lock = generate_workspace_lock(&from_bytes).unwrap();
    let programmatic_lock = generate_workspace_lock(&programmatic).unwrap();
    assert_eq!(text_lock.bytes(), bytes_lock.bytes());
    assert_ne!(text_lock.bytes(), programmatic_lock.bytes());
    assert_eq!(
        verify_workspace_lock(text_lock.bytes(), &programmatic)
            .unwrap_err()
            .code(),
        WorkspaceLockErrorCode::Stale
    );

    let reformatted = located_workspace(&directory, &workspace_yaml("different source"), false);
    assert_eq!(
        reformatted.resolved_schema().semantic_fingerprint(),
        from_text.resolved_schema().semantic_fingerprint()
    );
    assert_ne!(
        generate_workspace_lock(&reformatted).unwrap().bytes(),
        text_lock.bytes()
    );
}

#[test]
fn unconsumed_output_configuration_does_not_create_placeholder_lock_claims() {
    let directory = TempDirectory::new();
    let without_output = programmatic_workspace(&directory, "example", false);
    let with_output = programmatic_workspace(&directory, "example", true);
    assert_eq!(
        generate_workspace_lock(&without_output).unwrap().bytes(),
        generate_workspace_lock(&with_output).unwrap().bytes()
    );
}
