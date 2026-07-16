use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_schema::{
    SchemaSourceCapture, SchemaSourceObservation, SchemaSourceService,
    SchemaSourceServiceError, SystemSchemaSourceService,
};
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, MigrationV2Directory,
    SchemaSetPath, SecretReference, SecretReferenceService, TypeBridgeConfig,
    TypeBridgeConfigServices, TypeBridgeConfigSpec, TypeBridgeWorkspace,
    TypeBridgeWorkspaceError, TypeBridgeWorkspaceServices, WorkspaceConfigErrorCode,
    WorkspaceRoot, WorkspaceServiceError,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-workspace-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn root(&self) -> WorkspaceRoot {
        WorkspaceRoot::new(fs::canonicalize(&self.0).unwrap()).unwrap()
    }

    fn write(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }

    fn schema(&self, fragment: &str) {
        self.write(
            "schema/schema.yaml",
            "# schema-set authority\nformat: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
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

struct ExtensionPolicy {
    calls: AtomicUsize,
    reject: bool,
}

impl ExtensionRegistryService for ExtensionPolicy {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.reject {
            Err(WorkspaceServiceError::new("extension_unavailable"))
        } else {
            Ok(())
        }
    }
}

struct CountingSource {
    captures: AtomicUsize,
    system: SystemSchemaSourceService,
}

impl CountingSource {
    fn new() -> Self {
        Self {
            captures: AtomicUsize::new(0),
            system: SystemSchemaSourceService,
        }
    }
}

impl SchemaSourceService for CountingSource {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        self.system.canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.metadata(path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.symlink_metadata(path)
    }

    fn read_directory_names(
        &self,
        path: &Path,
    ) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        self.system.read_directory_names(path)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        self.captures.fetch_add(1, Ordering::Relaxed);
        self.system.capture_file(path, maximum_bytes)
    }
}

struct MutatingSource {
    captures: AtomicUsize,
    system: SystemSchemaSourceService,
    target: PathBuf,
}

impl SchemaSourceService for MutatingSource {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        self.system.canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.metadata(path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.symlink_metadata(path)
    }

    fn read_directory_names(
        &self,
        path: &Path,
    ) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        self.system.read_directory_names(path)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        let captured = self.system.capture_file(path, maximum_bytes)?;
        if path == self.target && self.captures.fetch_add(1, Ordering::SeqCst) == 0 {
            fs::write(path, "format: typebridge.schema/v2\nentities: {changed: {}}\n")
                .map_err(|_| SchemaSourceServiceError)?;
        }
        Ok(captured)
    }
}

struct ReselectingSource {
    directory: PathBuf,
    mutated: AtomicBool,
    system: SystemSchemaSourceService,
}

impl SchemaSourceService for ReselectingSource {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        self.system.canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.metadata(path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.symlink_metadata(path)
    }

    fn read_directory_names(
        &self,
        path: &Path,
    ) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        let names = self.system.read_directory_names(path)?;
        if path == self.directory && !self.mutated.swap(true, Ordering::SeqCst) {
            fs::write(
                path.join("added.yaml"),
                "format: typebridge.schema/v2\nentities: {added: {}}\n",
            )
            .map_err(|_| SchemaSourceServiceError)?;
        }
        Ok(names)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        self.system.capture_file(path, maximum_bytes)
    }
}

fn capabilities() -> CapabilitySet {
    ["schema.annotations", "schema.doc-meta", "schema.roles"]
        .into_iter()
        .map(|value| CapabilityId::new(value).unwrap())
        .collect()
}

fn programmatic_config<S: SchemaSourceService>(
    root: WorkspaceRoot,
    source: &S,
    secrets: &AcceptSecrets,
    extensions: &ExtensionPolicy,
) -> TypeBridgeConfig {
    TypeBridgeConfig::builder(root)
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .require_capability(CapabilityId::new("schema.doc-meta").unwrap())
        .build(&TypeBridgeConfigServices::new(source, secrets, extensions))
        .unwrap()
}

fn workspace_yaml() -> &'static str {
    "# retained workspace source\nformat: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: example-schema\ncompatibility:\n  semantic-profile: typedb-3.12.1/v1\n  require: [schema.doc-meta]\nmigrations:\n  directory: migrations/v2\n  app-label: example\n"
}

fn schema_code(error: &TypeBridgeWorkspaceError) -> &str {
    error
        .schema()
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .diagnostic()
        .code()
        .as_str()
}

#[test]
fn located_and_programmatic_configs_converge_on_one_source_workspace_state() {
    let directory = TempDirectory::new();
    directory.schema(
        "# retained fragment comment\nformat: typebridge.schema/v2\nentities: {person: {}}\n",
    );
    let source = CountingSource::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: false,
    };
    let available = capabilities();
    let workspace_services =
        TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available);

    let config = programmatic_config(directory.root(), &source, &secrets, &extensions);
    let programmatic = TypeBridgeWorkspace::from_config(config, &workspace_services).unwrap();
    let located = TypeBridgeConfigSpec::parse_yaml(
        workspace_yaml(),
        ConfigOrigin::new(directory.root(), "typebridge.yaml", "workspace fixture").unwrap(),
    )
    .unwrap();
    let located = TypeBridgeWorkspace::from_located_config(located, &workspace_services).unwrap();

    assert_eq!(
        programmatic
            .declared_schema()
            .declared_identity_fingerprint(),
        located.declared_schema().declared_identity_fingerprint()
    );
    assert_eq!(
        programmatic.resolved_schema().semantic_fingerprint(),
        located.resolved_schema().semantic_fingerprint()
    );
    assert_eq!(programmatic.managed_state(), located.managed_state());
    assert_eq!(
        programmatic.discovery_evidence(),
        located.discovery_evidence()
    );
    assert_eq!(programmatic.required_capabilities(), located.required_capabilities());
    assert!(programmatic.located_config().is_none());
    assert_eq!(
        located.located_config().unwrap().spec().source(),
        workspace_yaml()
    );
    assert_eq!(located.located_config().unwrap().spec().comments().len(), 1);
    assert_eq!(located.discovery().manifest().comments().len(), 1);
    assert!(!located.bound_managed_scope().selection().is_empty());
    assert_eq!(secrets.0.load(Ordering::Relaxed), 0);
    assert!(source.captures.load(Ordering::Relaxed) > 0);
}

#[test]
fn discovery_content_drift_and_reselection_fail_closed_through_injected_sources() {
    let drift = TempDirectory::new();
    drift.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let target = fs::canonicalize(drift.0.join("schema/fragments/model.yaml")).unwrap();
    let source = MutatingSource {
        captures: AtomicUsize::new(0),
        system: SystemSchemaSourceService,
        target,
    };
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: false,
    };
    let available = capabilities();
    let config = programmatic_config(drift.root(), &source, &secrets, &extensions);
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .err()
    .expect("content drift must reject workspace construction");
    assert_eq!(schema_code(&error), "schema_discovery_snapshot_changed");

    let reselection = TempDirectory::new();
    reselection.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let source = ReselectingSource {
        directory: fs::canonicalize(reselection.0.join("schema/fragments")).unwrap(),
        mutated: AtomicBool::new(false),
        system: SystemSchemaSourceService,
    };
    let config = programmatic_config(reselection.root(), &source, &secrets, &extensions);
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .err()
    .expect("source reselection must reject workspace construction");
    assert!(schema_code(&error).starts_with("schema_discovery_"));
}

#[test]
fn unknown_references_and_inheritance_cycles_remain_source_aware_failures() {
    let source = CountingSource::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: false,
    };
    let available = capabilities();

    let unknown = TempDirectory::new();
    unknown.schema(
        "format: typebridge.schema/v2\nentities:\n  child: {sub: missing}\n",
    );
    let config = programmatic_config(unknown.root(), &source, &secrets, &extensions);
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .err()
    .expect("unknown schema reference must reject workspace construction");
    assert!(schema_code(&error).contains("unknown"));
    assert!(
        error
            .schema()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .primary()
            .is_some()
    );

    let cycle = TempDirectory::new();
    cycle.schema(
        "format: typebridge.schema/v2\nentities:\n  a: {sub: b}\n  b: {sub: a}\n",
    );
    let config = programmatic_config(cycle.root(), &source, &secrets, &extensions);
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .err()
    .expect("inheritance cycle must reject workspace construction");
    assert!(schema_code(&error).contains("cycle"));
}

#[test]
fn unavailable_config_constraints_fail_before_schema_capture() {
    let directory = TempDirectory::new();
    directory.schema("format: typebridge.schema/v2\nentities: {person: {}}\n");
    let source = CountingSource::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let accept_extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: false,
    };
    let missing = CapabilitySet::new();
    let config = programmatic_config(
        directory.root(),
        &source,
        &secrets,
        &accept_extensions,
    );
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(
            &source,
            &secrets,
            &accept_extensions,
            &missing,
        ),
    )
    .err()
    .expect("missing configured capability must reject before capture");
    assert_eq!(
        error.contract().unwrap().code().as_str(),
        "unsupported_required_capability"
    );
    assert_eq!(source.captures.load(Ordering::Relaxed), 0);

    let config = TypeBridgeConfig::builder(directory.root())
        .schema_set(SchemaSetPath::new("schema/schema.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .require_extension(ExtensionRequirement::new("example.docs", "v1").unwrap())
        .build(&TypeBridgeConfigServices::new(
            &source,
            &secrets,
            &accept_extensions,
        ))
        .unwrap();
    let reject_extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: true,
    };
    let error = TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(
            &source,
            &secrets,
            &reject_extensions,
            &capabilities(),
        ),
    )
    .err()
    .expect("missing extension must reject before capture");
    assert_eq!(
        error.config().unwrap().code(),
        WorkspaceConfigErrorCode::ExtensionRequirementRejected
    );
    assert_eq!(source.captures.load(Ordering::Relaxed), 0);
}

#[test]
fn located_manifest_cannot_overlap_schema_history_or_output_authority() {
    let directory = TempDirectory::new();
    let source = CountingSource::new();
    let secrets = AcceptSecrets(AtomicUsize::new(0));
    let extensions = ExtensionPolicy {
        calls: AtomicUsize::new(0),
        reject: false,
    };
    let overlapping = workspace_yaml().replace("root: schema/schema.yaml", "root: schema.yaml");
    let located = TypeBridgeConfigSpec::parse_yaml(
        overlapping,
        ConfigOrigin::new(
            directory.root(),
            "schema/schema.yaml",
            "overlap fixture",
        )
        .unwrap(),
    )
    .unwrap();
    let error = located
        .resolve(&TypeBridgeConfigServices::new(
            &source,
            &secrets,
            &extensions,
        ))
        .unwrap_err();
    assert_eq!(
        error.code(),
        WorkspaceConfigErrorCode::OverlappingWorkspacePath
    );
    assert_eq!(error.detail(), Some("workspace_manifest,schema_set"));
    assert_eq!(source.captures.load(Ordering::Relaxed), 0);
}
