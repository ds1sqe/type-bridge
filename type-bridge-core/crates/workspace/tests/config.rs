use std::cell::Cell;
use std::path::{Path, PathBuf};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::{EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID, ManagedScopeId};
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_workspace::{
    ExtensionRegistryService, ExtensionRequirement, MigrationV2Directory, OutputDirectory,
    SchemaSetPath, SecretReference, SecretReferenceService, SecretSlot, TypeBridgeConfig,
    TypeBridgeConfigBuilder, TypeBridgeConfigServices, WorkspaceConfigErrorCode, WorkspaceRoot,
    WorkspaceServiceError, WorkspaceSourceService,
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

fn root() -> WorkspaceRoot {
    WorkspaceRoot::new("/virtual/project").unwrap()
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
        .secret(SecretSlot::new("typedb.credential").unwrap(), secret)
        .require_extension(extension)
        .build(&services(&source, &secrets, &extensions))
        .unwrap();

    assert_eq!(source.calls.get(), 1);
    assert_eq!(secrets.calls.get(), 1);
    assert_eq!(extensions.calls.get(), 1);
    assert_eq!(
        config.workspace_root().as_path(),
        Path::new("/virtual/project")
    );
    assert_eq!(
        config.schema_set_absolute_path(),
        PathBuf::from("/virtual/project/schema/schema.yaml")
    );
    assert_eq!(
        config.migration_v2_absolute_path(),
        PathBuf::from("/virtual/project/migrations/v2")
    );
    assert_eq!(config.app_label().as_str(), "example");
    assert_eq!(config.managed_scope().id().as_str(), "example-schema");
    assert_eq!(
        config.managed_scope().profile().id().as_str(),
        EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID
    );
    assert_eq!(config.semantic_profile().as_str(), "typedb-3.12.1/v1");
    assert_eq!(config.outputs().len(), 3);
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
fn workspace_root_is_explicit_and_matches_injected_canonical_spelling() {
    assert_eq!(
        WorkspaceRoot::new("relative/project").unwrap_err().code(),
        WorkspaceConfigErrorCode::WorkspaceRootNotAbsolute
    );
    assert_eq!(
        WorkspaceRoot::new("/virtual/../project")
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
fn exact_semantic_profile_and_all_required_fields_fail_closed() {
    let source = CanonicalSource::new();
    let secrets = AcceptSecrets::new();
    let extensions = AcceptExtensions::new();
    let error = TypeBridgeConfig::builder(root())
        .build(&services(&source, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::MissingRequiredField);
    assert_eq!(error.detail(), Some("schema_set"));

    let error = TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.11.5/v1").unwrap())
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
