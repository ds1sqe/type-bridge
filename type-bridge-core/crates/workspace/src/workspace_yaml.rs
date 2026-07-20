use std::path::{Path, PathBuf};

use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_contract::schema::{
    DocumentFingerprint, DocumentId, SchemaDiagnostics, SourceSpan,
};
use type_bridge_schema::{SchemaComment, SchemaDocument, YamlMapping, YamlNode};
use type_bridge_schema_migration::{MigrationSafetyPolicy, SafetyClass, SafetyPolicyDecision};

use crate::{
    ExtensionRequirement, MigrationV2Directory, OutputDirectory, SchemaSetPath, SecretReference,
    SecretSlot, TypeBridgeConfig, TypeBridgeConfigServices, WorkspaceConfigError,
    WorkspaceConfigErrorCode, WorkspaceEnvironment, WorkspaceRoot, confined_relative_path,
    workspace_paths_overlap,
};

/// The only accepted language-neutral workspace manifest discriminator.
pub const TYPEBRIDGE_WORKSPACE_V1_FORMAT: &str = "typebridge.workspace/v1";

const MAX_DIAGNOSTIC_NAME_BYTES: usize = 4_096;

/// An immutable source origin for one workspace manifest.
///
/// The manifest path is confined under the explicit workspace root. All wire
/// paths are resolved relative to the manifest's owning directory, never the
/// process current directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigOrigin {
    diagnostic_name: String,
    manifest_path: PathBuf,
    workspace_root: WorkspaceRoot,
}

impl ConfigOrigin {
    /// Construct a captured manifest origin under an explicit workspace root.
    pub fn new(
        workspace_root: WorkspaceRoot,
        manifest_path: impl Into<PathBuf>,
        diagnostic_name: impl Into<String>,
    ) -> Result<Self, WorkspaceConfigError> {
        let manifest_path = confined_relative_path(manifest_path, "workspace_manifest")?;
        if manifest_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("yaml")
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidConfigOrigin,
                "workspace manifest origin must end in lowercase .yaml",
            ));
        }
        let diagnostic_name = diagnostic_name.into();
        if diagnostic_name.is_empty()
            || diagnostic_name.len() > MAX_DIAGNOSTIC_NAME_BYTES
            || diagnostic_name.contains('\0')
            || diagnostic_name.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidConfigOrigin,
                "workspace config diagnostic origin is malformed",
            ));
        }
        Ok(Self {
            diagnostic_name,
            manifest_path,
            workspace_root,
        })
    }

    /// Return the explicit canonical workspace root.
    #[must_use]
    pub const fn workspace_root(&self) -> &WorkspaceRoot {
        &self.workspace_root
    }

    /// Return the manifest path relative to the workspace root.
    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Return the manifest path resolved under the workspace root.
    #[must_use]
    pub fn manifest_absolute_path(&self) -> PathBuf {
        self.workspace_root.as_path().join(&self.manifest_path)
    }

    /// Return the immutable diagnostic origin name.
    #[must_use]
    pub fn diagnostic_name(&self) -> &str {
        &self.diagnostic_name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SpannedString {
    span: SourceSpan,
    value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SecretWire {
    reference: SpannedString,
    slot: SpannedString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExtensionWire {
    handler: SpannedString,
    version: SpannedString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EnvironmentWire {
    database: SpannedString,
    http_port: Option<SpannedString>,
    migrate: Option<SpannedString>,
    password: SpannedString,
    requirements: Vec<SpannedString>,
    uri: SpannedString,
    username: SpannedString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceWire {
    app_label: SpannedString,
    capabilities: Vec<SpannedString>,
    destructive: Option<SpannedString>,
    environments: Vec<(SpannedString, EnvironmentWire)>,
    extensions: Vec<ExtensionWire>,
    managed_scope: SpannedString,
    migration_directory: SpannedString,
    outputs: Vec<(BindingTarget, SpannedString)>,
    schema_root: SpannedString,
    secrets: Vec<SecretWire>,
    semantic_profile: SpannedString,
}

/// A strict parsed workspace spec retaining exact source, comments, and spans.
///
/// The semantic wire is private and this trusted type implements no
/// deserialization contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeBridgeConfigSpec {
    document: SchemaDocument,
    wire: WorkspaceWire,
}

impl TypeBridgeConfigSpec {
    /// Parse UTF-8 workspace text and permanently attach its origin.
    pub fn parse_yaml(
        source: impl Into<String>,
        origin: ConfigOrigin,
    ) -> Result<LocatedConfigSpec, WorkspaceConfigError> {
        let source = source.into();
        let document_id = DocumentId::new(
            origin
                .manifest_path()
                .to_str()
                .expect("ConfigOrigin validates UTF-8 paths"),
        )
        .map_err(|error| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidConfigOrigin,
                "workspace manifest origin cannot identify a source document",
            )
            .with_detail(error.code().as_str())
        })?;
        let document = SchemaDocument::parse(document_id, source)
            .map_err(|diagnostics| yaml_diagnostics(diagnostics, &origin))?;
        let wire = parse_wire(document.root(), &origin)?;
        Ok(LocatedConfigSpec {
            origin,
            spec: Self { document, wire },
        })
    }

    /// Parse exact file bytes as UTF-8 and permanently attach their origin.
    pub fn from_yaml_bytes(
        bytes: &[u8],
        origin: ConfigOrigin,
    ) -> Result<LocatedConfigSpec, WorkspaceConfigError> {
        let source = std::str::from_utf8(bytes).map_err(|_| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceEncoding,
                "workspace manifest bytes must be valid UTF-8",
            )
        })?;
        Self::parse_yaml(source, origin)
    }

    /// Return source text exactly as supplied by the caller.
    #[must_use]
    pub fn source(&self) -> &str {
        self.document.source()
    }

    /// Return retained source comments in parser order.
    #[must_use]
    pub fn comments(&self) -> &[SchemaComment] {
        self.document.comments()
    }

    /// Return the exact authored-source fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &DocumentFingerprint {
        self.document.fingerprint()
    }

    /// Return the lossless root mapping and its exact spans.
    #[must_use]
    pub const fn root(&self) -> &YamlMapping {
        self.document.root()
    }
}

/// A strict config spec permanently bound to the manifest that owns its paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedConfigSpec {
    origin: ConfigOrigin,
    spec: TypeBridgeConfigSpec,
}

impl LocatedConfigSpec {
    /// Return the immutable captured origin.
    #[must_use]
    pub const fn origin(&self) -> &ConfigOrigin {
        &self.origin
    }

    /// Return the lossless parsed spec.
    #[must_use]
    pub const fn spec(&self) -> &TypeBridgeConfigSpec {
        &self.spec
    }

    /// Resolve solely through validated nested types and the existing builder.
    pub fn resolve(
        self,
        services: &TypeBridgeConfigServices<'_>,
    ) -> Result<TypeBridgeConfig, WorkspaceConfigError> {
        let Self { origin, spec } = self;
        let TypeBridgeConfigSpec { document, wire } = spec;
        let schema_path = resolve_owned_path(&origin, &wire.schema_root, "schema.root")?;
        let migration_path =
            resolve_owned_path(&origin, &wire.migration_directory, "migrations.directory")?;

        let schema_set = SchemaSetPath::new(schema_path)
            .map_err(|error| sourced(error, &origin, &wire.schema_root.span))?;
        let migration_v2_directory = MigrationV2Directory::new(migration_path)
            .map_err(|error| sourced(error, &origin, &wire.migration_directory.span))?;
        let app_label = MigrationAppLabel::new(wire.app_label.value).map_err(|error| {
            contract_value(error, "migrations.app-label", &wire.app_label.span, &origin)
        })?;
        let managed_scope = ManagedScopeId::new(wire.managed_scope.value).map_err(|error| {
            contract_value(
                error,
                "schema.managed-scope",
                &wire.managed_scope.span,
                &origin,
            )
        })?;
        let semantic_profile =
            SemanticProfileId::new(wire.semantic_profile.value).map_err(|error| {
                contract_value(
                    error,
                    "compatibility.semantic-profile",
                    &wire.semantic_profile.span,
                    &origin,
                )
            })?;

        let mut builder = TypeBridgeConfig::builder(origin.workspace_root.clone())
            .schema_set(schema_set)
            .app_label(app_label)
            .exclusive_managed_scope(managed_scope)
            .semantic_profile(semantic_profile)
            .migration_v2_directory(migration_v2_directory);

        if let Some(destructive) = wire.destructive {
            // The manifest may only tighten the verifier's floor. Anything
            // spelling a standing allowance is the invalid permanent
            // `force = true` shape and is rejected here by name.
            let policy = match destructive.value.as_str() {
                "require-approval" => MigrationSafetyPolicy::default_policy(),
                "reject" => MigrationSafetyPolicy::default_policy()
                    .with_decision(SafetyClass::Destructive, SafetyPolicyDecision::Reject)
                    .expect("rejecting destructive work is always a valid tightening"),
                other => {
                    return Err(WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                        "migrations.destructive admits only require-approval or reject",
                    )
                    .with_detail(other.to_owned())
                    .with_source(
                        origin.manifest_path().display().to_string(),
                        destructive.span.clone(),
                    ));
                }
            };
            builder = builder.migration_policy(policy);
        }

        for capability in wire.capabilities {
            let value = CapabilityId::new(capability.value).map_err(|error| {
                contract_value(error, "compatibility.require", &capability.span, &origin)
            })?;
            builder = builder.require_capability(value);
        }
        for (target, output) in wire.outputs {
            let path = resolve_owned_path(&origin, &output, output_field(target))?;
            let directory = OutputDirectory::new(path)
                .map_err(|error| sourced(error, &origin, &output.span))?;
            builder = builder.output(target, directory);
        }
        for secret in wire.secrets {
            let slot = SecretSlot::new(secret.slot.value)
                .map_err(|error| sourced(error, &origin, &secret.slot.span))?;
            let reference = SecretReference::environment(secret.reference.value)
                .map_err(|error| sourced(error, &origin, &secret.reference.span))?;
            builder = builder.secret(slot, reference);
        }
        for extension in wire.extensions {
            let requirement =
                ExtensionRequirement::new(extension.handler.value, extension.version.value)
                    .map_err(|error| sourced(error, &origin, &extension.handler.span))?;
            builder = builder.require_extension(requirement);
        }
        for (name, wire_environment) in wire.environments {
            let username = SecretReference::parse_symbolic(&wire_environment.username.value)
                .map_err(|error| sourced(error, &origin, &wire_environment.username.span))?;
            let password = SecretReference::parse_symbolic(&wire_environment.password.value)
                .map_err(|error| sourced(error, &origin, &wire_environment.password.span))?;
            let mut environment = WorkspaceEnvironment::new(
                wire_environment.uri.value,
                wire_environment.database.value,
                username,
                password,
            )
            .map_err(|error| sourced(error, &origin, &name.span))?;
            if let Some(port) = wire_environment.http_port {
                let parsed = port
                    .value
                    .parse::<u16>()
                    .ok()
                    .filter(|value| value.to_string() == port.value);
                let Some(parsed) = parsed else {
                    return Err(WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                        "environments.http-port must be a canonical u16",
                    )
                    .with_source(origin.diagnostic_name.clone(), port.span.clone()));
                };
                environment = environment.with_http_port(parsed);
            }
            if let Some(migrate) = wire_environment.migrate {
                let value = match migrate.value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(WorkspaceConfigError::new(
                            WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                            "environments.migrate admits only true or false",
                        )
                        .with_source(origin.diagnostic_name.clone(), migrate.span.clone()));
                    }
                };
                environment = environment.with_migrate(value);
            }
            let requirements = wire_environment
                .requirements
                .into_iter()
                .map(|capability| {
                    CapabilityId::new(capability.value).map_err(|error| {
                        contract_value(
                            error,
                            "environments.requirements",
                            &capability.span,
                            &origin,
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            environment = environment.require_capabilities(requirements);
            builder = builder.environment(name.value, environment);
        }

        let config = builder
            .build(services)
            .map_err(|error| sourced(error, &origin, document.root().span()))?;
        validate_manifest_path_disjointness(&config, &origin, document.root().span())?;
        Ok(config)
    }
}

fn validate_manifest_path_disjointness(
    config: &TypeBridgeConfig,
    origin: &ConfigOrigin,
    span: &SourceSpan,
) -> Result<(), WorkspaceConfigError> {
    let manifest = origin.manifest_path();
    let mut owned_paths = vec![
        ("schema_set", config.schema_set().as_path()),
        (
            "migration_v2_directory",
            config.migration_v2_directory().as_path(),
        ),
    ];
    for (target, directory) in config.outputs() {
        let name = match target {
            BindingTarget::Python => "output.python",
            BindingTarget::TypeScript => "output.typescript",
            BindingTarget::Rust => "output.rust",
        };
        owned_paths.push((name, directory.as_path()));
    }
    for (name, path) in owned_paths {
        if workspace_paths_overlap(manifest, path) {
            return Err(sourced(
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::OverlappingWorkspacePath,
                    "workspace manifest cannot overlap an owned schema, history, or output path",
                )
                .with_detail(format!("workspace_manifest,{name}")),
                origin,
                span,
            ));
        }
    }
    Ok(())
}

fn yaml_diagnostics(diagnostics: SchemaDiagnostics, origin: &ConfigOrigin) -> WorkspaceConfigError {
    let first = diagnostics
        .iter()
        .next()
        .expect("SchemaDiagnostics is non-empty");
    let mut error = WorkspaceConfigError::new(
        WorkspaceConfigErrorCode::InvalidWorkspaceYaml,
        "lossless YAML parsing rejected the workspace manifest",
    )
    .with_detail(first.diagnostic().code().as_str());
    if let Some(span) = first.primary() {
        error = sourced(error, origin, span);
    }
    error
}

fn sourced(
    error: WorkspaceConfigError,
    origin: &ConfigOrigin,
    span: &SourceSpan,
) -> WorkspaceConfigError {
    error.with_source(origin.diagnostic_name.clone(), span.clone())
}

fn contract_value(
    error: Diagnostic,
    field: &str,
    span: &SourceSpan,
    origin: &ConfigOrigin,
) -> WorkspaceConfigError {
    sourced(
        WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::InvalidWorkspaceValue,
            "workspace field failed typed validation",
        )
        .with_detail(format!("{field}:{}", error.code().as_str())),
        origin,
        span,
    )
}

fn output_field(target: BindingTarget) -> &'static str {
    match target {
        BindingTarget::Python => "bindings.python.output",
        BindingTarget::TypeScript => "bindings.typescript.output",
        BindingTarget::Rust => "bindings.rust.output",
    }
}

fn resolve_owned_path(
    origin: &ConfigOrigin,
    authored: &SpannedString,
    field: &str,
) -> Result<PathBuf, WorkspaceConfigError> {
    let value = &authored.value;
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains(['\\', ':', '\0'])
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::PathNotConfined,
                "workspace path is not a portable manifest-relative path",
            )
            .with_detail(field),
            origin,
            &authored.span,
        ));
    }

    let mut segments = origin
        .manifest_path
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .expect("ConfigOrigin validates UTF-8 paths")
                .to_owned()
        })
        .collect::<Vec<_>>();
    for segment in value.split('/') {
        match segment {
            "" => {
                return Err(sourced(
                    WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::PathNotConfined,
                        "workspace path contains an empty segment",
                    )
                    .with_detail(field),
                    origin,
                    &authored.span,
                ));
            }
            "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(sourced(
                        WorkspaceConfigError::new(
                            WorkspaceConfigErrorCode::PathNotConfined,
                            "workspace path escapes the captured workspace root",
                        )
                        .with_detail(field),
                        origin,
                        &authored.span,
                    ));
                }
            }
            component => segments.push(component.to_owned()),
        }
    }
    if segments.is_empty() {
        return Err(sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::PathNotConfined,
                "workspace path resolves to the workspace root rather than an owned path",
            )
            .with_detail(field),
            origin,
            &authored.span,
        ));
    }
    Ok(segments.into_iter().collect())
}

fn parse_wire(
    root: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<WorkspaceWire, WorkspaceConfigError> {
    let mut format = None;
    let mut schema = None;
    let mut compatibility = None;
    let mut migrations = None;
    let mut bindings = None;
    let mut secrets = None;
    let mut extensions = None;
    let mut environments = None;
    for entry in root.entries() {
        match entry.key().value() {
            "format" => format = Some(entry.value()),
            "schema" => schema = Some(entry.value()),
            "compatibility" => compatibility = Some(entry.value()),
            "migrations" => migrations = Some(entry.value()),
            "bindings" => bindings = Some(entry.value()),
            "secrets" => secrets = Some(entry.value()),
            "extensions" => extensions = Some(entry.value()),
            "environments" => environments = Some(entry.value()),
            unknown => return Err(unknown_key("root", unknown, entry.key().span(), origin)),
        }
    }

    let format = scalar(required(format, "format", root, origin)?, "format", origin)?;
    if format.value != TYPEBRIDGE_WORKSPACE_V1_FORMAT {
        return Err(sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::UnsupportedWorkspaceFormat,
                "workspace manifest format is not supported",
            )
            .with_detail(format.value),
            origin,
            &format.span,
        ));
    }

    let (schema_root, managed_scope) = parse_schema(
        mapping(required(schema, "schema", root, origin)?, "schema", origin)?,
        origin,
    )?;
    let (semantic_profile, capabilities) = parse_compatibility(
        mapping(
            required(compatibility, "compatibility", root, origin)?,
            "compatibility",
            origin,
        )?,
        origin,
    )?;
    let (migration_directory, app_label, destructive) = parse_migrations(
        mapping(
            required(migrations, "migrations", root, origin)?,
            "migrations",
            origin,
        )?,
        origin,
    )?;
    let outputs = bindings
        .map(|node| {
            mapping(node, "bindings", origin).and_then(|value| parse_bindings(value, origin))
        })
        .transpose()?
        .unwrap_or_default();
    let secrets = secrets
        .map(|node| mapping(node, "secrets", origin).and_then(|value| parse_secrets(value, origin)))
        .transpose()?
        .unwrap_or_default();
    let extensions = extensions
        .map(|node| {
            mapping(node, "extensions", origin).and_then(|value| parse_extensions(value, origin))
        })
        .transpose()?
        .unwrap_or_default();
    let environments = environments
        .map(|node| {
            mapping(node, "environments", origin)
                .and_then(|value| parse_environments(value, origin))
        })
        .transpose()?
        .unwrap_or_default();

    Ok(WorkspaceWire {
        app_label,
        capabilities,
        destructive,
        environments,
        extensions,
        managed_scope,
        migration_directory,
        outputs,
        schema_root,
        secrets,
        semantic_profile,
    })
}

fn parse_schema(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<(SpannedString, SpannedString), WorkspaceConfigError> {
    let mut root = None;
    let mut ownership = None;
    let mut managed_scope = None;
    for entry in value.entries() {
        match entry.key().value() {
            "root" => root = Some(entry.value()),
            "ownership" => ownership = Some(entry.value()),
            "managed-scope" => managed_scope = Some(entry.value()),
            unknown => return Err(unknown_key("schema", unknown, entry.key().span(), origin)),
        }
    }
    let root = scalar(
        required(root, "schema.root", value, origin)?,
        "schema.root",
        origin,
    )?;
    let ownership = scalar(
        required(ownership, "schema.ownership", value, origin)?,
        "schema.ownership",
        origin,
    )?;
    if ownership.value != "exclusive" {
        return Err(sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                "workspace V1 supports only exclusive schema ownership",
            )
            .with_detail("schema.ownership"),
            origin,
            &ownership.span,
        ));
    }
    let managed_scope = scalar(
        required(managed_scope, "schema.managed-scope", value, origin)?,
        "schema.managed-scope",
        origin,
    )?;
    Ok((root, managed_scope))
}

fn parse_compatibility(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<(SpannedString, Vec<SpannedString>), WorkspaceConfigError> {
    let mut semantic_profile = None;
    let mut require = None;
    for entry in value.entries() {
        match entry.key().value() {
            "semantic-profile" => semantic_profile = Some(entry.value()),
            "require" => require = Some(entry.value()),
            unknown => {
                return Err(unknown_key(
                    "compatibility",
                    unknown,
                    entry.key().span(),
                    origin,
                ));
            }
        }
    }
    let semantic_profile = scalar(
        required(
            semantic_profile,
            "compatibility.semantic-profile",
            value,
            origin,
        )?,
        "compatibility.semantic-profile",
        origin,
    )?;
    let capabilities = require
        .map(|node| string_sequence(node, "compatibility.require", origin))
        .transpose()?
        .unwrap_or_default();
    Ok((semantic_profile, capabilities))
}

fn parse_migrations(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<(SpannedString, SpannedString, Option<SpannedString>), WorkspaceConfigError> {
    let mut directory = None;
    let mut app_label = None;
    let mut destructive = None;
    for entry in value.entries() {
        match entry.key().value() {
            "directory" => directory = Some(entry.value()),
            "app-label" => app_label = Some(entry.value()),
            "destructive" => destructive = Some(entry.value()),
            unknown => {
                return Err(unknown_key(
                    "migrations",
                    unknown,
                    entry.key().span(),
                    origin,
                ));
            }
        }
    }
    let destructive = destructive
        .map(|value| scalar(value, "migrations.destructive", origin))
        .transpose()?;
    Ok((
        scalar(
            required(directory, "migrations.directory", value, origin)?,
            "migrations.directory",
            origin,
        )?,
        scalar(
            required(app_label, "migrations.app-label", value, origin)?,
            "migrations.app-label",
            origin,
        )?,
        destructive,
    ))
}

fn parse_environments(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<Vec<(SpannedString, EnvironmentWire)>, WorkspaceConfigError> {
    let mut environments = Vec::new();
    for entry in value.entries() {
        let name = SpannedString {
            span: entry.key().span().clone(),
            value: entry.key().value().to_owned(),
        };
        let body = mapping(entry.value(), "environments", origin)?;
        environments.push((name, parse_environment(body, origin)?));
    }
    Ok(environments)
}

fn parse_environment(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<EnvironmentWire, WorkspaceConfigError> {
    let mut database = None;
    let mut uri = None;
    let mut http_port = None;
    let mut migrate = None;
    let mut credential = None;
    let mut requirements = None;
    for entry in value.entries() {
        match entry.key().value() {
            "database" => database = Some(entry.value()),
            "uri" => uri = Some(entry.value()),
            "http-port" => http_port = Some(entry.value()),
            "migrate" => migrate = Some(entry.value()),
            "credential" => credential = Some(entry.value()),
            "requirements" => requirements = Some(entry.value()),
            unknown => {
                return Err(unknown_key(
                    "environments",
                    unknown,
                    entry.key().span(),
                    origin,
                ));
            }
        }
    }
    let credential = mapping(
        required(credential, "environments.credential", value, origin)?,
        "environments.credential",
        origin,
    )?;
    let mut username = None;
    let mut password = None;
    for entry in credential.entries() {
        match entry.key().value() {
            "username" => username = Some(entry.value()),
            "password" => password = Some(entry.value()),
            unknown => {
                return Err(unknown_key(
                    "environments.credential",
                    unknown,
                    entry.key().span(),
                    origin,
                ));
            }
        }
    }
    Ok(EnvironmentWire {
        database: scalar(
            required(database, "environments.database", value, origin)?,
            "environments.database",
            origin,
        )?,
        http_port: http_port
            .map(|node| scalar(node, "environments.http-port", origin))
            .transpose()?,
        migrate: migrate
            .map(|node| scalar(node, "environments.migrate", origin))
            .transpose()?,
        password: scalar(
            required(
                password,
                "environments.credential.password",
                credential,
                origin,
            )?,
            "environments.credential.password",
            origin,
        )?,
        requirements: requirements
            .map(|node| string_sequence(node, "environments.requirements", origin))
            .transpose()?
            .unwrap_or_default(),
        uri: scalar(
            required(uri, "environments.uri", value, origin)?,
            "environments.uri",
            origin,
        )?,
        username: scalar(
            required(
                username,
                "environments.credential.username",
                credential,
                origin,
            )?,
            "environments.credential.username",
            origin,
        )?,
    })
}

fn parse_bindings(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<Vec<(BindingTarget, SpannedString)>, WorkspaceConfigError> {
    let mut outputs = Vec::new();
    for entry in value.entries() {
        let target = match entry.key().value() {
            "python" => BindingTarget::Python,
            "typescript" => BindingTarget::TypeScript,
            "rust" => BindingTarget::Rust,
            unknown => return Err(unknown_key("bindings", unknown, entry.key().span(), origin)),
        };
        let binding = mapping(entry.value(), output_field(target), origin)?;
        let mut output = None;
        for field in binding.entries() {
            match field.key().value() {
                "output" => output = Some(field.value()),
                unknown => {
                    return Err(unknown_key(
                        output_field(target),
                        unknown,
                        field.key().span(),
                        origin,
                    ));
                }
            }
        }
        outputs.push((
            target,
            scalar(
                required(output, output_field(target), binding, origin)?,
                output_field(target),
                origin,
            )?,
        ));
    }
    Ok(outputs)
}

fn parse_secrets(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<Vec<SecretWire>, WorkspaceConfigError> {
    let mut secrets = Vec::new();
    for entry in value.entries() {
        let slot = SpannedString {
            span: entry.key().span().clone(),
            value: entry.key().value().to_owned(),
        };
        let secret = mapping(entry.value(), "secrets.*", origin)?;
        let mut environment = None;
        for field in secret.entries() {
            match field.key().value() {
                "env" => environment = Some(field.value()),
                unknown => {
                    return Err(unknown_key(
                        "secrets.*",
                        unknown,
                        field.key().span(),
                        origin,
                    ));
                }
            }
        }
        let reference = scalar(
            required(environment, "secrets.*.env", secret, origin)?,
            "secrets.*.env",
            origin,
        )?;
        secrets.push(SecretWire { reference, slot });
    }
    Ok(secrets)
}

fn parse_extensions(
    value: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<Vec<ExtensionWire>, WorkspaceConfigError> {
    let mut extensions = Vec::new();
    for entry in value.entries() {
        let handler = SpannedString {
            span: entry.key().span().clone(),
            value: entry.key().value().to_owned(),
        };
        let extension = mapping(entry.value(), "extensions.*", origin)?;
        let mut version = None;
        for field in extension.entries() {
            match field.key().value() {
                "version" => version = Some(field.value()),
                unknown => {
                    return Err(unknown_key(
                        "extensions.*",
                        unknown,
                        field.key().span(),
                        origin,
                    ));
                }
            }
        }
        extensions.push(ExtensionWire {
            handler,
            version: scalar(
                required(version, "extensions.*.version", extension, origin)?,
                "extensions.*.version",
                origin,
            )?,
        });
    }
    Ok(extensions)
}

fn required<'a>(
    value: Option<&'a YamlNode>,
    field: &str,
    owner: &YamlMapping,
    origin: &ConfigOrigin,
) -> Result<&'a YamlNode, WorkspaceConfigError> {
    value.ok_or_else(|| {
        sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::MissingWorkspaceField,
                "workspace manifest requires a closed wire field",
            )
            .with_detail(field),
            origin,
            owner.span(),
        )
    })
}

fn mapping<'a>(
    node: &'a YamlNode,
    field: &str,
    origin: &ConfigOrigin,
) -> Result<&'a YamlMapping, WorkspaceConfigError> {
    node.as_mapping().ok_or_else(|| {
        sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                "workspace field must be a mapping",
            )
            .with_detail(field),
            origin,
            node.span(),
        )
    })
}

fn scalar(
    node: &YamlNode,
    field: &str,
    origin: &ConfigOrigin,
) -> Result<SpannedString, WorkspaceConfigError> {
    let value = node.as_scalar().ok_or_else(|| {
        sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                "workspace field must be a string scalar",
            )
            .with_detail(field),
            origin,
            node.span(),
        )
    })?;
    Ok(SpannedString {
        span: value.span().clone(),
        value: value.value().to_owned(),
    })
}

fn string_sequence(
    node: &YamlNode,
    field: &str,
    origin: &ConfigOrigin,
) -> Result<Vec<SpannedString>, WorkspaceConfigError> {
    let sequence = node.as_sequence().ok_or_else(|| {
        sourced(
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                "workspace field must be a string sequence",
            )
            .with_detail(field),
            origin,
            node.span(),
        )
    })?;
    sequence
        .items()
        .iter()
        .map(|item| scalar(item, field, origin))
        .collect()
}

fn unknown_key(
    owner: &str,
    key: &str,
    span: &SourceSpan,
    origin: &ConfigOrigin,
) -> WorkspaceConfigError {
    sourced(
        WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::UnknownWorkspaceKey,
            "workspace manifest contains an unknown closed-wire key",
        )
        .with_detail(format!("{owner}.{key}")),
        origin,
        span,
    )
}
