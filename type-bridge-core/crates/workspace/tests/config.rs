use std::cell::Cell;
use std::path::{Path, PathBuf};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::{EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID, ManagedScopeId};
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_workspace::{
    ExtensionRegistryService, ExtensionRequirement, MigrationV2Directory, OutputDirectory,
    SchemaAuthorityOutputPath, SchemaSetPath, SecretReference, SecretReferenceService, SecretSlot,
    TypeBridgeConfig, TypeBridgeConfigBuilder, TypeBridgeConfigServices, WorkspaceConfigErrorCode,
    WorkspaceRoot, WorkspaceServiceError, WorkspaceSourceService,
};

struct CanonicalSource {
    calls: Cell<usize>,
}

impl CanonicalSource {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl WorkspaceSourceService for CanonicalSource {
    fn canonicalize_workspace_root(&self, root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        self.calls.set(self.calls.get() + 1);
        Ok(root.to_path_buf())
    }
}

struct DifferentCanonicalRoot;

impl WorkspaceSourceService for DifferentCanonicalRoot {
    fn canonicalize_workspace_root(&self, _root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        Ok(PathBuf::from("/virtual/canonical"))
    }
}

struct RejectSource;

impl WorkspaceSourceService for RejectSource {
    fn canonicalize_workspace_root(&self, _root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        Err(WorkspaceServiceError::new("root_unavailable"))
    }
}

struct AcceptSecrets {
    calls: Cell<usize>,
}

impl AcceptSecrets {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl SecretReferenceService for AcceptSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
}

struct RejectSecrets;

impl SecretReferenceService for RejectSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        Err(WorkspaceServiceError::new("unknown_secret_reference"))
    }
}

struct AcceptExtensions {
    calls: Cell<usize>,
}

impl AcceptExtensions {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl ExtensionRegistryService for AcceptExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
}

struct RejectExtensions;

impl ExtensionRegistryService for RejectExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        Err(WorkspaceServiceError::new("extension_not_installed"))
    }
}

// An absolute path requires a drive prefix on Windows.
#[cfg(windows)]
const VIRTUAL_ROOT: &str = "C:/virtual/project";
#[cfg(not(windows))]
const VIRTUAL_ROOT: &str = "/virtual/project";

fn root() -> WorkspaceRoot {
    WorkspaceRoot::new(VIRTUAL_ROOT).unwrap()
}

fn base_builder() -> TypeBridgeConfigBuilder {
    TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
}

fn builder_with_paths(schema_set: &str, migration_v2_directory: &str) -> TypeBridgeConfigBuilder {
    TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new(schema_set).unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new(migration_v2_directory).unwrap())
}

fn services<'a>(
    source: &'a dyn WorkspaceSourceService,
    secrets: &'a dyn SecretReferenceService,
    extensions: &'a dyn ExtensionRegistryService,
) -> TypeBridgeConfigServices<'a> {
    TypeBridgeConfigServices::new(source, secrets, extensions)
}

#[test]
fn typed_builder_retains_exact_policy_and_uses_injected_services() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let secret = SecretReference::environment("TYPEDB_CREDENTIAL").unwrap();
    let extension = ExtensionRequirement::new("example.documentation", "v1").unwrap();
    let config = base_builder()
        .require_capability(CapabilityId::new("schema.doc-meta").unwrap())
        .require_capabilities(CapabilitySet::from_iter([
            CapabilityId::new("schema.annotations").unwrap(),
            CapabilityId::new("schema.doc-meta").unwrap(),
        ]))
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("generated/typescript").unwrap(),
        )
        .output(
            BindingTarget::Rust,
            OutputDirectory::new("generated/rust").unwrap(),
        )
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("generated/schema-authority.json").unwrap(),
        )
        .secret(SecretSlot::new("typedb.credential").unwrap(), secret)
        .require_extension(extension)
        .build(&services(&source, &secrets, &extensions))
        .unwrap();

    assert_eq!(source.calls.get(), 1);
    assert_eq!(secrets.calls.get(), 1);
    assert_eq!(extensions.calls.get(), 1);
    assert_eq!(config.workspace_root().as_path(), Path::new(VIRTUAL_ROOT));
    assert_eq!(
        config.schema_set_absolute_path(),
        Path::new(VIRTUAL_ROOT).join("schema/schema.yaml")
    );
    assert_eq!(
        config.migration_v2_absolute_path(),
        Path::new(VIRTUAL_ROOT).join("migrations/v2")
    );
    assert_eq!(config.app_label().as_str(), "example");
    assert_eq!(config.managed_scope().id().as_str(), "example-schema");
    assert_eq!(
        config.managed_scope().profile().id().as_str(),
        EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID
    );
    assert_eq!(config.semantic_profile().as_str(), "typedb-3.12.1/v1");
    assert_eq!(config.outputs().len(), 3);
    assert_eq!(
        config.schema_authority_output().unwrap().as_path(),
        Path::new("generated/schema-authority.json")
    );
    assert_eq!(config.required_capabilities().len(), 2);
    assert_eq!(
        config
            .secret_references()
            .values()
            .next()
            .unwrap()
            .environment_variable(),
        "TYPEDB_CREDENTIAL"
    );
    assert_eq!(config.extensions().len(), 1);
}

#[test]
fn root_level_schema_set_can_emit_a_non_schema_json_authority() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let config = builder_with_paths("schema.yaml", "migrations/v2")
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("generated/schema-authority.json").unwrap(),
        )
        .build(&services(&source, &secrets, &extensions))
        .expect("a lowercase JSON authority cannot enter schema YAML discovery");

    assert_eq!(config.schema_set().as_path(), Path::new("schema.yaml"));
    assert_eq!(
        config.schema_authority_output().unwrap().as_path(),
        Path::new("generated/schema-authority.json")
    );
}

#[test]
fn schema_authority_requires_a_lowercase_json_path() {
    for path in [
        "generated/authority.yaml",
        "generated/authority.JSON",
        "authority",
    ] {
        let error = SchemaAuthorityOutputPath::new(path).unwrap_err();
        assert_eq!(
            error.code(),
            WorkspaceConfigErrorCode::InvalidSchemaAuthorityOutputPath
        );
        assert_eq!(error.detail(), Some("schema_authority_output"));
    }
}

#[test]
fn workspace_owned_paths_reject_portable_case_aliases() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let service_set = services(&source, &secrets, &extensions);

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        )
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("GENERATED/PYTHON/schema-authority.json").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(
        error.detail(),
        Some("output.python,artifact.schema_authority")
    );

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/models").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("GENERATED/MODELS").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("output.python,output.typescript"));

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/caf\u{e9}").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("generated/cafe\u{301}").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("output.python,output.typescript"));
}

#[test]
fn workspace_root_is_explicit_and_matches_injected_canonical_spelling() {
    assert_eq!(
        WorkspaceRoot::new("relative/project").unwrap_err().code(),
        WorkspaceConfigErrorCode::WorkspaceRootNotAbsolute
    );
    assert_eq!(
        WorkspaceRoot::new(format!("{VIRTUAL_ROOT}/../other"))
            .unwrap_err()
            .code(),
        WorkspaceConfigErrorCode::WorkspaceRootNotCanonical
    );

    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let error = base_builder()
        .build(&services(&DifferentCanonicalRoot, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::WorkspaceRootNotCanonical
    );

    let error = base_builder()
        .build(&services(&RejectSource, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed
    );
    assert_eq!(error.detail(), Some("root_unavailable"));
}

#[test]
fn schema_migration_and_output_paths_are_portable_and_confined() {
    for path in [
        "../schema/schema.yaml",
        "/schema/schema.yaml",
        "schema\\schema.yaml",
    ] {
        assert_eq!(
            SchemaSetPath::new(path).unwrap_err().code(),
            WorkspaceConfigErrorCode::PathNotConfined
        );
    }
    assert_eq!(
        SchemaSetPath::new("schema/schema.yml").unwrap_err().code(),
        WorkspaceConfigErrorCode::InvalidSchemaSetPath
    );
    assert_eq!(
        MigrationV2Directory::new("migrations").unwrap_err().code(),
        WorkspaceConfigErrorCode::InvalidMigrationV2Directory
    );
    assert_eq!(
        MigrationV2Directory::new("../v2").unwrap_err().code(),
        WorkspaceConfigErrorCode::PathNotConfined
    );
    assert_eq!(
        OutputDirectory::new("/generated/python")
            .unwrap_err()
            .code(),
        WorkspaceConfigErrorCode::PathNotConfined
    );
    assert_eq!(
        MigrationV2Directory::new("migrations/v2")
            .unwrap()
            .as_path(),
        Path::new("migrations/v2")
    );

    for path in [
        "../generated/schema-authority.json",
        "/generated/schema-authority.json",
        "generated\\schema-authority.json",
        "generated/con.json",
        "generated/schema?.json",
        "generated/schema-authority.json.",
    ] {
        assert_eq!(
            SchemaAuthorityOutputPath::new(path).unwrap_err().code(),
            WorkspaceConfigErrorCode::PathNotConfined,
            "nonportable schema-authority output {path:?} was accepted"
        );
    }
    assert_eq!(
        SchemaAuthorityOutputPath::new("generated/schema-authority.json")
            .unwrap()
            .as_path(),
        Path::new("generated/schema-authority.json")
    );
}

#[test]
fn secret_literals_are_rejected_and_symbolic_references_are_never_resolved() {
    assert_eq!(
        SecretReference::parse_symbolic("plain-text-secret")
            .unwrap_err()
            .code(),
        WorkspaceConfigErrorCode::SecretLiteralRejected
    );
    assert_eq!(
        SecretReference::parse_symbolic("env:INVALID-NAME")
            .unwrap_err()
            .code(),
        WorkspaceConfigErrorCode::InvalidSecretReference
    );

    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let config = base_builder()
        .secret(
            SecretSlot::new("typedb.uri").unwrap(),
            SecretReference::parse_symbolic("env:TYPEDB_URI").unwrap(),
        )
        .build(&services(&source, &secrets, &extensions))
        .unwrap();
    assert_eq!(secrets.calls.get(), 1);
    assert_eq!(
        config
            .secret_references()
            .values()
            .next()
            .unwrap()
            .environment_variable(),
        "TYPEDB_URI"
    );
}

#[test]
fn capability_requirements_are_additive_and_deterministic() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let config = base_builder()
        .require_capability(CapabilityId::new("schema.roles").unwrap())
        .require_capabilities([
            CapabilityId::new("schema.annotations").unwrap(),
            CapabilityId::new("schema.roles").unwrap(),
        ])
        .build(&services(&source, &secrets, &extensions))
        .unwrap();
    assert_eq!(
        config
            .required_capabilities()
            .iter()
            .map(CapabilityId::as_str)
            .collect::<Vec<_>>(),
        vec!["schema.annotations", "schema.roles"]
    );
}

#[test]
fn frozen_semantic_profiles_and_all_required_fields_fail_closed() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let error = TypeBridgeConfig::builder(root())
        .build(&services(&source, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::MissingRequiredField);
    assert_eq!(error.detail(), Some("schema_set"));

    let band8 = TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.11.5/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .build(&services(&source, &secrets, &extensions))
        .expect("the retained TypeDB 3.11 semantic profile is supported for generation");
    assert_eq!(band8.semantic_profile().as_str(), "typedb-3.11.5/v1");

    let error = TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.10.0/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .build(&services(&source, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::UnsupportedSemanticProfile
    );
}

#[test]
fn duplicate_targets_secrets_extensions_and_singletons_are_rejected() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let service_set = services(&source, &secrets, &extensions);

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/one").unwrap(),
        )
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/two").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateOutputTarget
    );

    let slot = SecretSlot::new("typedb.credential").unwrap();
    let error = base_builder()
        .secret(
            slot.clone(),
            SecretReference::environment("FIRST_SECRET").unwrap(),
        )
        .secret(slot, SecretReference::environment("SECOND_SECRET").unwrap())
        .build(&service_set)
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::DuplicateSecretSlot);

    let error = base_builder()
        .require_extension(ExtensionRequirement::new("example.docs", "v1").unwrap())
        .require_extension(ExtensionRequirement::new("example.docs", "v2").unwrap())
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateExtensionHandler
    );

    let error = base_builder()
        .schema_set(SchemaSetPath::new("other/schema.yaml").unwrap())
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateRequiredField
    );

    let error = base_builder()
        .schema_authority_output(SchemaAuthorityOutputPath::new("generated/one.json").unwrap())
        .schema_authority_output(SchemaAuthorityOutputPath::new("generated/two.json").unwrap())
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateRequiredField
    );
    assert_eq!(error.detail(), Some("schema_authority_output"));
}

#[test]
fn injected_services_reject_locally_without_provider_or_secret_access() {
    let source = CanonicalSource::new();
    let extensions = AcceptExtensions::new();
    let error = base_builder()
        .secret(
            SecretSlot::new("typedb.database").unwrap(),
            SecretReference::environment("TYPEDB_DATABASE").unwrap(),
        )
        .build(&services(&source, &RejectSecrets, &extensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::SecretReferenceRejected
    );
    assert_eq!(error.detail(), Some("unknown_secret_reference"));

    let secrets = AcceptSecrets::new();
    let error = base_builder()
        .require_extension(ExtensionRequirement::new("example.docs", "v1").unwrap())
        .build(&services(&source, &secrets, &RejectExtensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::ExtensionRequirementRejected
    );
    assert_eq!(error.detail(), Some("extension_not_installed"));
}

#[test]
fn workspace_owned_paths_reject_equality_and_name_both_fields() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let service_set = services(&source, &secrets, &extensions);

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("schema/schema.yaml").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("schema_set,output.python"));

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("migrations/v2").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("migration_v2_directory,output.python"));

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/models").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("generated/models").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("output.python,output.typescript"));

    let config = base_builder()
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("schema/schema-authority.json").unwrap(),
        )
        .build(&service_set)
        .expect("JSON authority beside schema YAML cannot enter schema discovery");
    assert_eq!(
        config.schema_authority_output().unwrap().as_path(),
        Path::new("schema/schema-authority.json")
    );

    let error = base_builder()
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("migrations/v2/schema-authority.json").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(
        error.detail(),
        Some("migration_v2_directory,artifact.schema_authority")
    );

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        )
        .schema_authority_output(
            SchemaAuthorityOutputPath::new("generated/python/schema-authority.json").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(
        error.detail(),
        Some("output.python,artifact.schema_authority")
    );
}

#[test]
fn workspace_owned_paths_reject_ancestor_descendant_nesting_deterministically() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let service_set = services(&source, &secrets, &extensions);

    let error = builder_with_paths("schema/schema.yaml", "schema/schema.yaml/v2")
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("schema_set,migration_v2_directory"));

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("schema").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("schema_set,output.python"));

    let error = base_builder()
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("generated").unwrap(),
        )
        .build(&service_set)
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("output.python,output.typescript"));
}

#[test]
fn environment_uris_admit_only_plain_host_addresses() {
    use type_bridge_workspace::{SecretReference, WorkspaceEnvironment};

    let environment = || {
        (
            SecretReference::environment("TYPEDB_USERNAME").unwrap(),
            SecretReference::environment("TYPEDB_PASSWORD").unwrap(),
        )
    };
    for accepted in [
        "localhost:1729",
        "127.0.0.1:65535",
        "typedb.example.internal.:1",
        "[::1]:1729",
        "[2001:db8::1]:1729",
        "db-1.internal:1729,db-2.internal:1729,[2001:db8::2]:1730",
    ] {
        let (username, password) = environment();
        WorkspaceEnvironment::new(accepted, "app", username, password)
            .unwrap_or_else(|error| panic!("{accepted}: {error:?}"));
    }
    // Empty members, missing/invalid ports, malformed hosts/brackets,
    // userinfo, schemes, paths, whitespace, and control bytes must all fail
    // before an address can reach driver errors or tracing.
    for rejected in [
        "",
        ",",
        "host:1729,",
        ",host:1729",
        "host:1729,,other:1729",
        "host",
        "host:",
        ":1729",
        "host:0",
        "host:65536",
        "host:not-a-port",
        "host:+1729",
        "host::1729",
        "-host:1729",
        "host-:1729",
        "host..internal:1729",
        "host_name:1729",
        "[::1]",
        "[::1]:",
        "[::1]:0",
        "[::1]:65536",
        "[::1]1729",
        "[::1:1729",
        "::1:1729",
        "[127.0.0.1]:1729",
        "admin:secret@host:1729",
        "typedb://host:1729",
        "host:1729?tls=false",
        "host:1729/path",
        "host 1729",
        "host:1729\n",
    ] {
        let (username, password) = environment();
        let error =
            WorkspaceEnvironment::new(rejected, "app", username, password).expect_err(rejected);
        assert_eq!(
            error.code(),
            WorkspaceConfigErrorCode::InvalidWorkspaceValue,
            "{rejected}"
        );
    }
}

#[test]
fn environment_database_contract_remains_nonempty_scalar_only() {
    use type_bridge_workspace::{SecretReference, WorkspaceEnvironment};

    let references = || {
        (
            SecretReference::environment("TYPEDB_USERNAME").unwrap(),
            SecretReference::environment("TYPEDB_PASSWORD").unwrap(),
        )
    };
    let (username, password) = references();
    WorkspaceEnvironment::new("localhost:1729", "app-prod_01", username, password)
        .expect("a nonempty database scalar remains accepted");

    let (username, password) = references();
    let error = WorkspaceEnvironment::new("localhost:1729", "", username, password)
        .expect_err("an empty database is rejected");
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.to_string(), "environment database must be non-empty");
}

#[test]
fn normalized_endpoint_sets_reject_managed_and_reserved_journal_aliases() {
    use type_bridge_workspace::{SecretReference, WorkspaceEnvironment};

    let environment = |uri: &str, database: &str| {
        WorkspaceEnvironment::new(
            uri,
            database,
            SecretReference::environment("TYPEDB_USERNAME").unwrap(),
            SecretReference::environment("TYPEDB_PASSWORD").unwrap(),
        )
        .unwrap()
    };
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let error = base_builder()
        .environment(
            "primary",
            environment("DB-1.EXAMPLE.:1729,[2001:0db8::1]:01730", "production"),
        )
        .environment(
            "journal-alias",
            environment("[2001:db8::1]:1730", "production__tbv2_journal"),
        )
        .build(&services(&source, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::EnvironmentDatabaseCollision
    );
    assert_eq!(error.detail(), Some("journal-alias,primary"));

    base_builder()
        .environment("primary", environment("db-1.example:1729", "production"))
        .environment(
            "separate-cluster",
            environment("db-2.example:1729", "production__tbv2_journal"),
        )
        .build(&services(&source, &secrets, &extensions))
        .expect("database namespaces on distinct endpoint sets do not alias");
}
