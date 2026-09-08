use std::cell::Cell;
use std::path::{Path, PathBuf};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, MigrationV2Directory,
    OutputDirectory, SchemaAuthorityOutputPath, SchemaSetPath, SecretReference,
    SecretReferenceService, SecretSlot, TypeBridgeConfig, TypeBridgeConfigServices,
    TypeBridgeConfigSpec, WorkspaceConfigErrorCode, WorkspaceRoot, WorkspaceServiceError,
    WorkspaceSourceService,
};

const WORKSPACE_YAML: &str = r#"# retained workspace comment
format: typebridge.workspace/v1
schema:
  root: ../schema/schema.yaml
  ownership: exclusive
  managed-scope: example-schema
compatibility:
  semantic-profile: typedb-3.12.1/v1
  require:
    - schema.doc-meta
    - schema.annotations
migrations:
  directory: ../migrations/v2
  app-label: example
bindings:
  python:
    output: ../generated/python
  typescript:
    output: ../generated/typescript
  rust:
    output: ../generated/rust
secrets:
  typedb.credential:
    env: TYPEDB_CREDENTIAL
extensions:
  example.documentation:
    version: v1
artifacts:
  schema-authority:
    output: ../generated/schema-authority.json
"#;

struct CanonicalSource(Cell<usize>);

impl WorkspaceSourceService for CanonicalSource {
    fn canonicalize_workspace_root(&self, root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        self.0.set(self.0.get() + 1);
        Ok(root.to_path_buf())
    }
}

struct AcceptSecrets(Cell<usize>);

impl SecretReferenceService for AcceptSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

struct AcceptExtensions(Cell<usize>);

impl ExtensionRegistryService for AcceptExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        self.0.set(self.0.get() + 1);
        Ok(())
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

fn origin() -> ConfigOrigin {
    ConfigOrigin::new(root(), "config/typebridge.yaml", "virtual workspace config").unwrap()
}

fn services<'a>(
    source: &'a CanonicalSource,
    secrets: &'a AcceptSecrets,
    extensions: &'a AcceptExtensions,
) -> TypeBridgeConfigServices<'a> {
    TypeBridgeConfigServices::new(source, secrets, extensions)
}

fn programmatic(
    services: &TypeBridgeConfigServices<'_>,
) -> type_bridge_workspace::TypeBridgeConfig {
    TypeBridgeConfig::builder(root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .require_capabilities(CapabilitySet::from_iter([
            CapabilityId::new("schema.doc-meta").unwrap(),
            CapabilityId::new("schema.annotations").unwrap(),
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
        .secret(
            SecretSlot::new("typedb.credential").unwrap(),
            SecretReference::environment("TYPEDB_CREDENTIAL").unwrap(),
        )
        .require_extension(ExtensionRequirement::new("example.documentation", "v1").unwrap())
        .build(services)
        .unwrap()
}

#[test]
fn bytes_text_and_programmatic_builder_resolve_to_the_same_config() {
    let source = CanonicalSource(Cell::new(0));
    let secrets = AcceptSecrets(Cell::new(0));
    let extensions = AcceptExtensions(Cell::new(0));
    let service_set = services(&source, &secrets, &extensions);

    let from_text = TypeBridgeConfigSpec::parse_yaml(WORKSPACE_YAML, origin())
        .unwrap()
        .resolve(&service_set)
        .unwrap();
    let from_bytes = TypeBridgeConfigSpec::from_yaml_bytes(WORKSPACE_YAML.as_bytes(), origin())
        .unwrap()
        .resolve(&service_set)
        .unwrap();
    let from_builder = programmatic(&service_set);

    assert_eq!(from_text, from_bytes);
    assert_eq!(from_bytes, from_builder);
    assert_eq!(source.0.get(), 3);
    assert_eq!(secrets.0.get(), 3);
    assert_eq!(extensions.0.get(), 3);
}

#[test]
fn located_spec_retains_exact_source_comments_spans_and_origin() {
    let located = TypeBridgeConfigSpec::parse_yaml(WORKSPACE_YAML, origin()).unwrap();
    assert_eq!(located.spec().source(), WORKSPACE_YAML);
    assert_eq!(
        located.origin().manifest_path(),
        Path::new("config/typebridge.yaml")
    );
    assert_eq!(
        located.origin().manifest_absolute_path(),
        Path::new(VIRTUAL_ROOT).join("config/typebridge.yaml")
    );
    assert_eq!(
        located.origin().diagnostic_name(),
        "virtual workspace config"
    );
    assert_eq!(located.spec().comments().len(), 1);
    assert!(
        located.spec().comments()[0]
            .text()
            .contains("retained workspace comment")
    );
    assert_eq!(located.spec().comments()[0].span().line(), 1);
    assert_eq!(located.spec().root().entries()[0].key().span().line(), 2);
}

#[test]
fn manifest_relative_paths_are_normalized_but_cannot_escape_workspace_root() {
    let source = CanonicalSource(Cell::new(0));
    let secrets = AcceptSecrets(Cell::new(0));
    let extensions = AcceptExtensions(Cell::new(0));
    let config = TypeBridgeConfigSpec::parse_yaml(WORKSPACE_YAML, origin())
        .unwrap()
        .resolve(&services(&source, &secrets, &extensions))
        .unwrap();
    // Compare the spelling, not just the components: resolution must keep
    // the portable forward-slash form on every platform, or the confined-
    // path validators reject the resolver's own output on Windows.
    assert_eq!(
        config.schema_set().as_path().to_str(),
        Some("schema/schema.yaml")
    );
    assert_eq!(
        config.migration_v2_directory().as_path().to_str(),
        Some("migrations/v2")
    );
    assert_eq!(
        config.schema_authority_output().unwrap().as_path().to_str(),
        Some("generated/schema-authority.json")
    );

    let escaping = WORKSPACE_YAML.replace("../schema/schema.yaml", "../../schema/schema.yaml");
    let error = TypeBridgeConfigSpec::parse_yaml(escaping, origin())
        .unwrap()
        .resolve(&services(&source, &secrets, &extensions))
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::PathNotConfined);
    assert_eq!(error.detail(), Some("schema.root"));
    assert_eq!(error.origin(), Some("virtual workspace config"));
    assert_eq!(error.source_span().unwrap().line(), 4);
}

#[test]
fn optional_artifact_block_is_strict_and_source_aware() {
    let without_artifact = WORKSPACE_YAML.replace(
        "artifacts:\n  schema-authority:\n    output: ../generated/schema-authority.json\n",
        "",
    );
    let source = CanonicalSource(Cell::new(0));
    let secrets = AcceptSecrets(Cell::new(0));
    let extensions = AcceptExtensions(Cell::new(0));
    let config = TypeBridgeConfigSpec::parse_yaml(without_artifact, origin())
        .unwrap()
        .resolve(&services(&source, &secrets, &extensions))
        .unwrap();
    assert!(config.schema_authority_output().is_none());

    let unknown_artifact = WORKSPACE_YAML.replace(
        "artifacts:\n",
        "artifacts:\n  generated-lock:\n    output: ../generated/workspace.lock\n",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(unknown_artifact, origin()).unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::UnknownWorkspaceKey);
    assert_eq!(error.detail(), Some("artifacts.generated-lock"));
    assert!(error.source_span().is_some());

    let unknown_field = WORKSPACE_YAML.replace(
        "    output: ../generated/schema-authority.json",
        "    output: ../generated/schema-authority.json\n    format: json",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(unknown_field, origin()).unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::UnknownWorkspaceKey);
    assert_eq!(error.detail(), Some("artifacts.schema-authority.format"));
    assert!(error.source_span().is_some());

    let missing_output = WORKSPACE_YAML.replace(
        "  schema-authority:\n    output: ../generated/schema-authority.json",
        "  schema-authority: {}",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(missing_output, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::MissingWorkspaceField
    );
    assert_eq!(error.detail(), Some("artifacts.schema-authority.output"));
    assert!(error.source_span().is_some());

    let wrong_shape = WORKSPACE_YAML.replace(
        "  schema-authority:\n    output: ../generated/schema-authority.json",
        "  schema-authority: []",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(wrong_shape, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.detail(), Some("artifacts.schema-authority"));
    assert!(error.source_span().is_some());
}

#[test]
fn artifact_output_is_confined_portable_and_disjoint_after_manifest_resolution() {
    let source = CanonicalSource(Cell::new(0));
    let secrets = AcceptSecrets(Cell::new(0));
    let extensions = AcceptExtensions(Cell::new(0));
    let service_set = services(&source, &secrets, &extensions);

    let escaping = WORKSPACE_YAML.replace(
        "../generated/schema-authority.json",
        "../../schema-authority.json",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(escaping, origin())
        .unwrap()
        .resolve(&service_set)
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::PathNotConfined);
    assert_eq!(error.detail(), Some("artifacts.schema-authority.output"));
    assert!(error.source_span().is_some());

    let nonportable = WORKSPACE_YAML.replace(
        "../generated/schema-authority.json",
        "../generated/con.json",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(nonportable, origin())
        .unwrap()
        .resolve(&service_set)
        .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::PathNotConfined);
    assert_eq!(error.detail(), Some("schema_authority_output"));
    assert!(error.source_span().is_some());

    let adjacent_json = WORKSPACE_YAML.replace(
        "../generated/schema-authority.json",
        "../schema/schema-authority.json",
    );
    let config = TypeBridgeConfigSpec::parse_yaml(adjacent_json, origin())
        .unwrap()
        .resolve(&service_set)
        .expect("JSON authority beside schema YAML cannot enter schema discovery");
    assert_eq!(
        config.schema_authority_output().unwrap().as_path(),
        Path::new("schema/schema-authority.json")
    );

    for overlapping in [
        "../migrations/v2/schema-authority.json",
        "../generated/python/schema-authority.json",
    ] {
        let manifest = WORKSPACE_YAML.replace("../generated/schema-authority.json", overlapping);
        let error = TypeBridgeConfigSpec::parse_yaml(manifest, origin())
            .unwrap()
            .resolve(&service_set)
            .unwrap_err();
        assert_eq!(
            error.code(),
            WorkspaceConfigErrorCode::OverlappingWorkspacePath,
            "overlapping artifact {overlapping:?} was accepted"
        );
        assert!(error.source_span().is_some());
    }
}

#[test]
fn closed_wire_rejects_unknown_duplicate_missing_and_wrong_types_with_spans() {
    let unknown = WORKSPACE_YAML.replace(
        "  ownership: exclusive",
        "  ownership: exclusive\n  invented: value",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(unknown, origin()).unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::UnknownWorkspaceKey);
    assert_eq!(error.detail(), Some("schema.invented"));
    assert_eq!(error.origin(), Some("virtual workspace config"));
    assert_eq!(error.source_span().unwrap().line(), 6);

    let duplicate = WORKSPACE_YAML.replacen(
        "format: typebridge.workspace/v1",
        "format: typebridge.workspace/v1\nformat: typebridge.workspace/v1",
        1,
    );
    let error = TypeBridgeConfigSpec::parse_yaml(duplicate, origin()).unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::InvalidWorkspaceYaml);
    assert_eq!(error.detail(), Some("duplicate_yaml_key"));
    assert!(error.source_span().is_some());

    let missing = WORKSPACE_YAML.replace("  app-label: example\n", "");
    let error = TypeBridgeConfigSpec::parse_yaml(missing, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::MissingWorkspaceField
    );
    assert_eq!(error.detail(), Some("migrations.app-label"));
    assert!(error.source_span().is_some());

    let wrong_type = WORKSPACE_YAML.replace(
        "schema:\n  root: ../schema/schema.yaml\n  ownership: exclusive\n  managed-scope: example-schema",
        "schema: []",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(wrong_type, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.detail(), Some("schema"));
    assert!(error.source_span().is_some());
}

#[test]
fn set_like_capability_lists_reject_duplicates_at_the_second_span() {
    let duplicate_global =
        WORKSPACE_YAML.replace("    - schema.annotations", "    - schema.doc-meta");
    let error = TypeBridgeConfigSpec::parse_yaml(duplicate_global, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateCapabilityRequirement
    );
    assert_eq!(
        error.detail(),
        Some("compatibility.require:schema.doc-meta")
    );
    assert_eq!(error.source_span().unwrap().line(), 11);

    let duplicate_environment = format!(
        "{WORKSPACE_YAML}environments:\n  dev:\n    database: example\n    uri: localhost:1729\n    credential:\n      username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n    requirements:\n      - schema.doc-meta\n      - schema.doc-meta\n"
    );
    let duplicate_line = u32::try_from(duplicate_environment.lines().count()).unwrap();
    let error = TypeBridgeConfigSpec::parse_yaml(duplicate_environment, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::DuplicateCapabilityRequirement
    );
    assert_eq!(
        error.detail(),
        Some("environments.requirements:schema.doc-meta")
    );
    assert_eq!(error.source_span().unwrap().line(), duplicate_line);
}

#[test]
fn invalid_environment_uri_and_database_report_their_value_spans() {
    let source = CanonicalSource(Cell::new(0));
    let secrets = AcceptSecrets(Cell::new(0));
    let extensions = AcceptExtensions(Cell::new(0));
    let service_set = services(&source, &secrets, &extensions);
    let manifest = |database: &str, uri: &str| {
        format!(
            "{WORKSPACE_YAML}environments:\n  development:\n    database: {database}\n    uri: {uri}\n    credential:\n      username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n"
        )
    };
    let scalar_position = |source: &str, needle: &str| {
        source
            .lines()
            .enumerate()
            .find_map(|(line, text)| {
                text.find(needle).map(|column| {
                    (
                        u32::try_from(line + 1).unwrap(),
                        u32::try_from(column + 1).unwrap(),
                    )
                })
            })
            .expect("fixture contains the authored scalar")
    };

    let invalid_uri = manifest("example", "host:65536");
    let expected_uri = scalar_position(&invalid_uri, "host:65536");
    let error = TypeBridgeConfigSpec::parse_yaml(invalid_uri, origin())
        .unwrap()
        .resolve(&service_set)
        .expect_err("an out-of-range endpoint port is rejected");
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.origin(), Some("virtual workspace config"));
    let span = error.source_span().expect("URI diagnostic is spanned");
    assert_eq!((span.line(), span.column()), expected_uri);

    let empty_database = manifest("''", "localhost:1729");
    let expected_database = scalar_position(&empty_database, "''");
    let error = TypeBridgeConfigSpec::parse_yaml(empty_database, origin())
        .unwrap()
        .resolve(&service_set)
        .expect_err("an empty database is rejected");
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.origin(), Some("virtual workspace config"));
    let span = error.source_span().expect("database diagnostic is spanned");
    assert_eq!((span.line(), span.column()), expected_database);
}

#[test]
fn format_encoding_and_origin_are_explicit_and_fail_closed() {
    let unsupported = WORKSPACE_YAML.replace("typebridge.workspace/v1", "typebridge.workspace/v2");
    let error = TypeBridgeConfigSpec::parse_yaml(unsupported, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::UnsupportedWorkspaceFormat
    );
    assert_eq!(error.source_span().unwrap().line(), 2);

    let error = TypeBridgeConfigSpec::from_yaml_bytes(&[0xff], origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceEncoding
    );

    assert_eq!(
        ConfigOrigin::new(root(), "../typebridge.yaml", "bad origin")
            .unwrap_err()
            .code(),
        WorkspaceConfigErrorCode::PathNotConfined
    );
}

#[test]
fn secret_literals_and_unknown_binding_targets_are_rejected_before_resolution() {
    let literal = WORKSPACE_YAML.replace(
        "  typedb.credential:\n    env: TYPEDB_CREDENTIAL",
        "  typedb.credential: committed-secret",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(literal, origin()).unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::InvalidWorkspaceValue
    );
    assert_eq!(error.detail(), Some("secrets.*"));
    assert!(error.source_span().is_some());

    let unknown_target = WORKSPACE_YAML.replace(
        "bindings:\n",
        "bindings:\n  kotlin:\n    output: ../generated/kotlin\n",
    );
    let error = TypeBridgeConfigSpec::parse_yaml(unknown_target, origin()).unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::UnknownWorkspaceKey);
    assert_eq!(error.detail(), Some("bindings.kotlin"));
    assert!(error.source_span().is_some());
}
