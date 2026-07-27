//! Validated, programmatic TypeBridge workspace configuration.
//!
//! This unpublished orchestration boundary deliberately stops before YAML
//! parsing, schema loading, history, persistence, provider/network I/O, secret
//! resolution, or compiled-runtime construction. The one bounded filesystem
//! observation is explicit custom TLS trust material: callers inject a local
//! source service that canonicalizes and proves the configured CA file before
//! an inert, fully validated [`TypeBridgeConfig`] is returned.

#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, ManagedScopeProfileId,
};
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::BindingTarget;
use type_bridge_contract::reserved::TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX;
use type_bridge_contract::schema::SourceSpan;
use type_bridge_contract::semantic_profile::SemanticProfile;
use type_bridge_schema::{SchemaSourceKind, SchemaSourceService};
use type_bridge_schema_migration::MigrationSafetyPolicy;

mod authority;
mod bundle;
mod lock;
mod migration;
mod workspace;
mod workspace_yaml;

pub use authority::{WorkspaceDirectoryAuthority, WorkspaceOutputDirectory};
pub use bundle::{
    BundleProjectionContext, BundleVerificationContext, MAX_SCHEMA_BUNDLE_BYTES,
    SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION, SCHEMA_BUNDLE_FINGERPRINT_DOMAIN,
    SchemaBundleError, SchemaBundleErrorCode, TYPEBRIDGE_SCHEMA_BUNDLE_V1, TypeBridgeRuntime,
    VerifiedSchemaBundle, build_verified_schema_bundle, decode_verified_schema_bundle,
    encode_verified_schema_bundle,
};
pub use lock::{
    MAX_WORKSPACE_LOCK_BYTES, TYPEBRIDGE_WORKSPACE_LOCK_V1, VerifiedWorkspaceLock, WorkspaceLock,
    WorkspaceLockError, WorkspaceLockErrorCode, generate_workspace_lock, verify_workspace_lock,
};
pub use migration::{MigrationDirectoryAuthority, MigrationPlanEntry};
pub use workspace::{TypeBridgeWorkspace, TypeBridgeWorkspaceError, TypeBridgeWorkspaceServices};
pub use workspace_yaml::{
    ConfigOrigin, LocatedConfigSpec, TYPEBRIDGE_WORKSPACE_V1_FORMAT, TypeBridgeConfigSpec,
};

/// The exact server-semantic profile accepted by the first V2 workspace.
pub const TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_ID: &str = "typedb-3.12.1/v1";

const MAX_SYMBOLIC_ID_BYTES: usize = 255;
const MAX_EXTENSION_VERSION_BYTES: usize = 64;

/// Stable categories returned while validating programmatic workspace policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkspaceConfigErrorCode {
    /// A workspace root was not absolute.
    WorkspaceRootNotAbsolute,
    /// A workspace root contained unresolved lexical components.
    WorkspaceRootNotCanonical,
    /// The injected source service could not canonicalize the root.
    WorkspaceRootCanonicalizationFailed,
    /// A path was not a portable, confined workspace-relative path.
    PathNotConfined,
    /// The schema-set path was not a lowercase YAML path.
    InvalidSchemaSetPath,
    /// The migration path did not identify a direct V2 history directory.
    InvalidMigrationV2Directory,
    /// A required builder field was absent.
    MissingRequiredField,
    /// A singleton builder field was assigned more than once.
    DuplicateRequiredField,
    /// The selected semantic profile is not the exact workspace profile.
    UnsupportedSemanticProfile,
    /// The exclusive managed-scope profile could not be bound.
    InvalidManagedScope,
    /// Two workspace-owned paths are equal or nested.
    OverlappingWorkspacePath,
    /// One binding output target appeared more than once.
    DuplicateOutputTarget,
    /// A symbolic secret slot appeared more than once.
    DuplicateSecretSlot,
    /// One extension handler appeared more than once.
    DuplicateExtensionHandler,
    /// A symbolic identifier was malformed.
    InvalidSymbolicIdentifier,
    /// One environment's managed database aliases another environment's journal.
    EnvironmentDatabaseCollision,
    /// A config origin was absent, escaped its root, or was malformed.
    InvalidConfigOrigin,
    /// Config bytes were not valid UTF-8.
    InvalidWorkspaceEncoding,
    /// The shared lossless YAML parser rejected the document.
    InvalidWorkspaceYaml,
    /// The workspace format discriminator is unsupported.
    UnsupportedWorkspaceFormat,
    /// A closed workspace mapping contained an unknown key.
    UnknownWorkspaceKey,
    /// A required workspace wire field was absent.
    MissingWorkspaceField,
    /// A workspace wire value had the wrong shape or spelling.
    InvalidWorkspaceValue,
    /// A set-like capability requirement appeared more than once.
    DuplicateCapabilityRequirement,
    /// A secret input was a literal rather than a reference.
    SecretLiteralRejected,
    /// An environment secret reference was malformed.
    InvalidSecretReference,
    /// A local secret-reference validator rejected a reference.
    SecretReferenceRejected,
    /// A local extension registry rejected a requirement.
    ExtensionRequirementRejected,
    /// An environment TLS value was not the canonical Boolean spelling.
    InvalidTlsBoolean,
    /// A custom root CA was supplied without explicitly enabling TLS.
    TlsRootCaRequiresTls,
    /// A custom root CA contradicted an explicit disabled TLS policy.
    TlsRootCaWithDisabledTls,
    /// A custom root CA path was not a readable, non-empty confined file.
    InvalidTlsRootCa,
}

/// A structured programmatic workspace validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceConfigError {
    code: WorkspaceConfigErrorCode,
    detail: Option<String>,
    message: &'static str,
    origin: Option<String>,
    source_span: Option<Box<SourceSpan>>,
}

impl WorkspaceConfigError {
    fn new(code: WorkspaceConfigErrorCode, message: &'static str) -> Self {
        Self {
            code,
            detail: None,
            message,
            origin: None,
            source_span: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn with_source(
        mut self,
        origin: impl Into<String>,
        source_span: SourceSpan,
    ) -> Self {
        self.origin = Some(origin.into());
        self.source_span = Some(Box::new(source_span));
        self
    }

    /// Return the stable error category.
    #[must_use]
    pub const fn code(&self) -> WorkspaceConfigErrorCode {
        self.code
    }

    /// Return optional deterministic error context.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Return the immutable diagnostic origin captured during parsing.
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// Return the exact source span for a parsed-config failure, if available.
    #[must_use]
    pub fn source_span(&self) -> Option<&SourceSpan> {
        self.source_span.as_deref()
    }
}

impl fmt::Display for WorkspaceConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for WorkspaceConfigError {}

/// A stable failure reported by an injected, local-only config service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceServiceError {
    code: &'static str,
}

impl WorkspaceServiceError {
    /// Construct a service failure with a stable implementation-owned code.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    /// Return the stable service-owned code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl fmt::Display for WorkspaceServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl Error for WorkspaceServiceError {}

/// A narrow local source service for root and custom-CA path validation.
///
/// Config validation never reads schema documents or expands source patterns.
/// When a custom root CA is configured, the service additionally proves its
/// canonical confinement and bounded readability. Full schema source services
/// automatically satisfy this boundary.
pub trait WorkspaceSourceService {
    /// Return the canonical spelling of the supplied workspace root.
    fn canonicalize_workspace_root(&self, root: &Path) -> Result<PathBuf, WorkspaceServiceError>;

    /// Return the canonical physical spelling of one workspace-owned path.
    ///
    /// Implementations that only support virtual programmatic configs may
    /// retain the default rejection. The method is required only when a
    /// custom root CA path is configured.
    fn canonicalize_workspace_path(&self, _path: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        Err(WorkspaceServiceError::new(
            "workspace_path_canonicalization_unavailable",
        ))
    }

    /// Prove that one canonical path is a readable regular file and return
    /// its observed byte length.
    ///
    /// The default rejection keeps existing virtual services source
    /// compatible and is exercised only by custom-root configuration.
    fn readable_workspace_file_len(&self, _path: &Path) -> Result<u64, WorkspaceServiceError> {
        Err(WorkspaceServiceError::new(
            "workspace_file_observation_unavailable",
        ))
    }
}

impl<T> WorkspaceSourceService for T
where
    T: SchemaSourceService + ?Sized,
{
    fn canonicalize_workspace_root(&self, root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        self.canonicalize(root)
            .map_err(|_| WorkspaceServiceError::new("schema_source_canonicalize_failed"))
    }

    fn canonicalize_workspace_path(&self, path: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        self.canonicalize(path)
            .map_err(|_| WorkspaceServiceError::new("workspace_path_canonicalize_failed"))
    }

    fn readable_workspace_file_len(&self, path: &Path) -> Result<u64, WorkspaceServiceError> {
        let observation = self
            .metadata(path)
            .map_err(|_| WorkspaceServiceError::new("workspace_file_metadata_failed"))?;
        if observation.kind() != SchemaSourceKind::File {
            return Err(WorkspaceServiceError::new("workspace_path_is_not_a_file"));
        }
        let capture = self
            .capture_file(path, 0)
            .map_err(|_| WorkspaceServiceError::new("workspace_file_read_failed"))?;
        if capture.before() != &observation || capture.after() != &observation {
            return Err(WorkspaceServiceError::new(
                "workspace_file_changed_during_validation",
            ));
        }
        Ok(observation.len())
    }
}

/// A validator for symbolic secret references.
///
/// This service intentionally has no method that can resolve or read a secret.
pub trait SecretReferenceService {
    /// Validate that a symbolic reference is accepted by local policy.
    fn validate_reference(&self, reference: &SecretReference) -> Result<(), WorkspaceServiceError>;
}

/// A local registry for projection-only extension requirements.
///
/// Implementations must validate against locally supplied registry state and
/// must not perform network discovery during config construction.
pub trait ExtensionRegistryService {
    /// Validate one exact extension handler/version requirement.
    fn validate_requirement(
        &self,
        requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError>;
}

/// Explicit services used to validate a programmatic config hermetically.
pub struct TypeBridgeConfigServices<'a> {
    extensions: &'a dyn ExtensionRegistryService,
    secrets: &'a dyn SecretReferenceService,
    sources: &'a dyn WorkspaceSourceService,
}

impl<'a> TypeBridgeConfigServices<'a> {
    /// Construct a service set without ambient environment or network state.
    ///
    /// Filesystem access remains explicit through `sources` and is used only
    /// for canonical-root and optional custom-CA validation.
    #[must_use]
    pub const fn new(
        sources: &'a dyn WorkspaceSourceService,
        secrets: &'a dyn SecretReferenceService,
        extensions: &'a dyn ExtensionRegistryService,
    ) -> Self {
        Self {
            extensions,
            secrets,
            sources,
        }
    }
}

/// An explicit absolute workspace root whose canonical spelling is service-verified.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceRoot(PathBuf);

impl WorkspaceRoot {
    /// Validate the lexical portion of an explicit absolute workspace root.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, WorkspaceConfigError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootNotAbsolute,
                "workspace root must be explicit and absolute",
            ));
        }
        if path.to_str().is_none()
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootNotCanonical,
                "workspace root must have a portable canonical spelling",
            ));
        }
        Ok(Self(path))
    }

    /// Return the explicit root path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

pub(crate) fn confined_relative_path(
    path: impl Into<PathBuf>,
    subject: &'static str,
) -> Result<PathBuf, WorkspaceConfigError> {
    let path = path.into();
    let Some(portable) = path.to_str() else {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::PathNotConfined,
            "workspace-relative path must be valid UTF-8",
        )
        .with_detail(subject));
    };
    let invalid_spelling = portable.is_empty()
        || portable.contains(['\\', ':', '\0'])
        || portable.bytes().any(|byte| byte.is_ascii_control())
        || portable
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
    let invalid_components = path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)));
    if invalid_spelling || invalid_components {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::PathNotConfined,
            "workspace-relative path escapes or is not portable",
        )
        .with_detail(subject));
    }
    Ok(path)
}

/// A canonical, workspace-confined custom root CA file.
///
/// Construction resolves a portable workspace-relative path against the
/// canonical workspace root, follows symbolic links, rejects escapes, and
/// proves that the target is a readable non-empty regular file. The lower
/// transport layer re-validates the PEM certificate contents immediately
/// before any network I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRootCa(PathBuf);

impl WorkspaceRootCa {
    /// Resolve and validate a workspace-relative custom root CA path.
    pub fn new(
        workspace_root: &WorkspaceRoot,
        relative_path: impl Into<PathBuf>,
        sources: &dyn WorkspaceSourceService,
    ) -> Result<Self, WorkspaceConfigError> {
        let relative_path = confined_relative_path(relative_path, "environment.tls-root-ca")?;
        let canonical_root = sources
            .canonicalize_workspace_root(workspace_root.as_path())
            .map_err(|error| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                    "custom root CA validation could not canonicalize the workspace root",
                )
                .with_detail(error.code())
            })?;
        if canonical_root != workspace_root.as_path() {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootNotCanonical,
                "custom root CA validation requires a canonical workspace root",
            ));
        }

        // Extend component-wise: joining the whole portable relative path
        // onto a Windows verbatim (`\\?\`) root would keep its forward
        // slashes, which verbatim paths exempt from separator normalization.
        let mut candidate = canonical_root.clone();
        candidate.extend(relative_path.components());
        let canonical_path = sources
            .canonicalize_workspace_path(&candidate)
            .map_err(|error| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::InvalidTlsRootCa,
                    "custom root CA path cannot be canonicalized",
                )
                .with_detail(error.code())
            })?;
        if canonical_path == canonical_root
            || !canonical_path.starts_with(&canonical_root)
            || canonical_path.to_str().is_none()
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidTlsRootCa,
                "custom root CA path escapes the canonical workspace root",
            ));
        }

        let length = sources
            .readable_workspace_file_len(&canonical_path)
            .map_err(|error| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::InvalidTlsRootCa,
                    "custom root CA path must be a readable regular file",
                )
                .with_detail(error.code())
            })?;
        if length == 0 {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidTlsRootCa,
                "custom root CA file must not be empty",
            ));
        }
        Ok(Self(canonical_path))
    }

    /// Return the canonical absolute root CA path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Validated transport policy for one workspace environment.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceTransportPolicy {
    /// Plaintext HTTP and gRPC.
    #[default]
    Disabled,
    /// TLS using operating-system native trust roots.
    NativeRoots,
    /// TLS using one canonical workspace-confined custom root CA file.
    CustomRootCa(WorkspaceRootCa),
}

/// A confined path to one portable schema-set manifest.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchemaSetPath(PathBuf);

impl SchemaSetPath {
    /// Validate a confined lowercase `.yaml` schema-set path.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, WorkspaceConfigError> {
        let path = confined_relative_path(path, "schema_set")?;
        if path.extension().and_then(|extension| extension.to_str()) != Some("yaml") {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidSchemaSetPath,
                "schema-set path must end in lowercase .yaml",
            ));
        }
        Ok(Self(path))
    }

    /// Return the canonical workspace-relative path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A confined direct V2 migration-history directory.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MigrationV2Directory(PathBuf);

impl MigrationV2Directory {
    /// Validate a confined directory whose final component is exactly `v2`.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, WorkspaceConfigError> {
        let path = confined_relative_path(path, "migration_v2_directory")?;
        if path.file_name().and_then(|name| name.to_str()) != Some("v2") {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidMigrationV2Directory,
                "canonical migration directory must identify the V2 history directly",
            ));
        }
        Ok(Self(path))
    }

    /// Return the canonical workspace-relative directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A confined output directory for one shipped binding target.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OutputDirectory(PathBuf);

impl OutputDirectory {
    /// Validate a confined output directory.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, WorkspaceConfigError> {
        Ok(Self(confined_relative_path(path, "binding_output")?))
    }

    /// Return the canonical workspace-relative directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

const fn output_field_name(target: BindingTarget) -> &'static str {
    match target {
        BindingTarget::Python => "output.python",
        BindingTarget::TypeScript => "output.typescript",
        BindingTarget::Rust => "output.rust",
    }
}

pub(crate) fn workspace_paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn valid_namespaced_id(value: &str) -> bool {
    let mut count = 0_usize;
    let valid = value.split('.').all(|segment| {
        count += 1;
        let mut bytes = segment.bytes();
        bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
    });
    valid && count >= 2 && value.len() <= MAX_SYMBOLIC_ID_BYTES
}

/// A deterministic logical slot containing one symbolic secret reference.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SecretSlot(String);

impl SecretSlot {
    /// Validate a lowercase namespaced secret slot such as `typedb.credential`.
    pub fn new(value: impl Into<String>) -> Result<Self, WorkspaceConfigError> {
        let value = value.into();
        if !valid_namespaced_id(&value) {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidSymbolicIdentifier,
                "secret slot must be a bounded lowercase namespaced identifier",
            ));
        }
        Ok(Self(value))
    }

    /// Return the canonical slot spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A retained environment-variable reference, never a resolved secret value.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SecretReference {
    environment_variable: String,
}

impl SecretReference {
    /// Construct an environment reference without reading the environment.
    pub fn environment(variable: impl Into<String>) -> Result<Self, WorkspaceConfigError> {
        let variable = variable.into();
        let mut bytes = variable.bytes();
        let valid = variable.len() <= MAX_SYMBOLIC_ID_BYTES
            && bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if !valid {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidSecretReference,
                "environment reference contains an invalid variable name",
            ));
        }
        Ok(Self {
            environment_variable: variable,
        })
    }

    /// Parse the sole shipped symbolic spelling, `env:VARIABLE`.
    ///
    /// Inputs without a symbolic scheme are rejected as literals.
    pub fn parse_symbolic(value: impl AsRef<str>) -> Result<Self, WorkspaceConfigError> {
        let value = value.as_ref();
        let Some(variable) = value.strip_prefix("env:") else {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::SecretLiteralRejected,
                "secret literals are forbidden; use a symbolic reference",
            ));
        };
        Self::environment(variable)
    }

    /// Return the retained environment variable name without resolving it.
    #[must_use]
    pub fn environment_variable(&self) -> &str {
        &self.environment_variable
    }
}

/// One exact local extension handler/version requirement.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExtensionRequirement {
    handler_id: String,
    version: String,
}

impl ExtensionRequirement {
    /// Validate one namespaced handler ID and bounded version spelling.
    pub fn new(
        handler_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, WorkspaceConfigError> {
        let handler_id = handler_id.into();
        let version = version.into();
        let valid_version = !version.is_empty()
            && version.len() <= MAX_EXTENSION_VERSION_BYTES
            && version.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_')
            });
        if !valid_namespaced_id(&handler_id) || !valid_version {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidSymbolicIdentifier,
                "extension requirement has an invalid handler ID or version",
            ));
        }
        Ok(Self {
            handler_id,
            version,
        })
    }

    /// Return the stable extension handler ID.
    #[must_use]
    pub fn handler_id(&self) -> &str {
        &self.handler_id
    }

    /// Return the exact required handler version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// One named, inert deployment environment.
///
/// Environments carry connection identity and policy only: credentials are
/// symbolic environment references, never committed values, and production
/// application stays opt-in through the explicit `migrate` flag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEnvironment {
    database: String,
    http_port: Option<u16>,
    migrate: bool,
    password: SecretReference,
    requirements: CapabilitySet,
    transport_policy: WorkspaceTransportPolicy,
    uri: String,
    username: SecretReference,
}

fn invalid_environment_uri() -> WorkspaceConfigError {
    WorkspaceConfigError::new(
        WorkspaceConfigErrorCode::InvalidWorkspaceValue,
        "environment uri must be a comma-separated list of host:port or [IPv6]:port endpoints without credentials, schemes, or control characters",
    )
}

fn valid_endpoint_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

fn valid_endpoint_host(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn valid_environment_endpoint(endpoint: &str) -> bool {
    if let Some(bracketed) = endpoint.strip_prefix('[') {
        let Some((address, port)) = bracketed.split_once("]:") else {
            return false;
        };
        !address.is_empty()
            && !port.contains(['[', ']', ':'])
            && address.parse::<Ipv6Addr>().is_ok()
            && valid_endpoint_port(port)
    } else {
        let Some((host, port)) = endpoint.rsplit_once(':') else {
            return false;
        };
        !host.contains(['[', ']', ':']) && valid_endpoint_host(host) && valid_endpoint_port(port)
    }
}

pub(crate) fn validate_environment_uri(uri: &str) -> Result<(), WorkspaceConfigError> {
    if uri.is_empty() || !uri.split(',').all(valid_environment_endpoint) {
        return Err(invalid_environment_uri());
    }
    Ok(())
}

fn normalized_environment_endpoints(uri: &str) -> Result<BTreeSet<String>, WorkspaceConfigError> {
    uri.split(',')
        .map(|endpoint| {
            if let Some(bracketed) = endpoint.strip_prefix('[') {
                let (address, port) = bracketed.split_once("]:")?;
                let address = address.parse::<Ipv6Addr>().ok()?;
                let port = port.parse::<u16>().ok()?;
                Some(format!("[{address}]:{port}"))
            } else {
                let (host, port) = endpoint.rsplit_once(':')?;
                let host = host.strip_suffix('.').unwrap_or(host);
                let host = host
                    .parse::<Ipv4Addr>()
                    .map_or_else(|_| host.to_ascii_lowercase(), |address| address.to_string());
                let port = port.parse::<u16>().ok()?;
                Some(format!("{host}:{port}"))
            }
        })
        .collect::<Option<BTreeSet<_>>>()
        .ok_or_else(invalid_environment_uri)
}

pub(crate) fn validate_environment_database(database: &str) -> Result<(), WorkspaceConfigError> {
    if database.is_empty() {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::InvalidWorkspaceValue,
            "environment database must be non-empty",
        ));
    }
    Ok(())
}

impl WorkspaceEnvironment {
    fn from_validated(
        uri: String,
        database: String,
        username: SecretReference,
        password: SecretReference,
    ) -> Self {
        Self {
            database,
            http_port: None,
            migrate: false,
            password,
            requirements: CapabilitySet::new(),
            transport_policy: WorkspaceTransportPolicy::Disabled,
            uri,
            username,
        }
    }

    /// Construct one environment with mandatory connection identity.
    ///
    /// The uri is one or more comma-separated TypeDB endpoints: `host:port`
    /// or bracketed `[IPv6]:port`, with every port in `1..=65535`. Userinfo,
    /// schemes, paths, query strings, whitespace, and control characters are
    /// rejected: credentials stay symbolic by construction, so an address
    /// echoed by driver errors or tracing can never leak them.
    pub fn new(
        uri: impl Into<String>,
        database: impl Into<String>,
        username: SecretReference,
        password: SecretReference,
    ) -> Result<Self, WorkspaceConfigError> {
        let uri = uri.into();
        let database = database.into();
        validate_environment_uri(&uri)?;
        validate_environment_database(&database)?;
        Ok(Self::from_validated(uri, database, username, password))
    }

    /// Select an explicit provider HTTP port.
    #[must_use]
    pub fn with_http_port(mut self, port: u16) -> Self {
        self.http_port = Some(port);
        self
    }

    /// Opt this environment into migration application.
    #[must_use]
    pub const fn with_migrate(mut self, migrate: bool) -> Self {
        self.migrate = migrate;
        self
    }

    /// Select the validated transport policy for this environment.
    #[must_use]
    pub fn with_transport_policy(mut self, policy: WorkspaceTransportPolicy) -> Self {
        self.transport_policy = policy;
        self
    }

    /// Add environment-specific capability requirements.
    #[must_use]
    pub fn require_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = CapabilityId>,
    ) -> Self {
        for capability in capabilities {
            self.requirements.insert(capability);
        }
        self
    }

    /// Return the provider address.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Return the managed database name.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Return the explicit provider HTTP port, when configured.
    #[must_use]
    pub const fn http_port(&self) -> Option<u16> {
        self.http_port
    }

    /// Return the symbolic username reference.
    #[must_use]
    pub const fn username(&self) -> &SecretReference {
        &self.username
    }

    /// Return the symbolic password reference.
    #[must_use]
    pub const fn password(&self) -> &SecretReference {
        &self.password
    }

    /// Return whether migration application is opted in.
    #[must_use]
    pub const fn migrate(&self) -> bool {
        self.migrate
    }

    /// Return the validated transport policy.
    #[must_use]
    pub const fn transport_policy(&self) -> &WorkspaceTransportPolicy {
        &self.transport_policy
    }

    /// Return environment-specific capability requirements.
    #[must_use]
    pub const fn requirements(&self) -> &CapabilitySet {
        &self.requirements
    }
}

/// An inert validated workspace policy produced only by its builder.
///
/// This trusted type intentionally implements no deserialization contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeBridgeConfig {
    app_label: MigrationAppLabel,
    environments: BTreeMap<String, WorkspaceEnvironment>,
    extensions: BTreeSet<ExtensionRequirement>,
    managed_scope: ManagedScopeBinding,
    migration_policy: MigrationSafetyPolicy,
    migration_v2_directory: MigrationV2Directory,
    outputs: BTreeMap<BindingTarget, OutputDirectory>,
    required_capabilities: CapabilitySet,
    schema_set: SchemaSetPath,
    secret_references: BTreeMap<SecretSlot, SecretReference>,
    semantic_profile: SemanticProfileId,
    workspace_root: WorkspaceRoot,
}

impl TypeBridgeConfig {
    /// Begin typed programmatic construction at one explicit workspace root.
    #[must_use]
    pub fn builder(workspace_root: WorkspaceRoot) -> TypeBridgeConfigBuilder {
        TypeBridgeConfigBuilder::new(workspace_root)
    }

    /// Return the service-verified canonical workspace root.
    #[must_use]
    pub const fn workspace_root(&self) -> &WorkspaceRoot {
        &self.workspace_root
    }

    /// Return the confined schema-set manifest path.
    #[must_use]
    pub const fn schema_set(&self) -> &SchemaSetPath {
        &self.schema_set
    }

    /// Return the schema-set manifest path resolved under the root.
    #[must_use]
    pub fn schema_set_absolute_path(&self) -> PathBuf {
        // Extend component-wise: joining the whole portable relative path
        // onto a Windows verbatim (`\\?\`) root would keep its forward
        // slashes, which verbatim paths exempt from separator normalization.
        let mut path = self.workspace_root.as_path().to_path_buf();
        path.extend(self.schema_set.as_path().components());
        path
    }

    /// Return the validated migration application label.
    #[must_use]
    pub const fn app_label(&self) -> &MigrationAppLabel {
        &self.app_label
    }

    /// Return the durable scope bound to the frozen exclusive profile.
    #[must_use]
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        &self.managed_scope
    }

    /// Return the exact server-semantic profile.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Return the confined canonical V2 migration directory.
    #[must_use]
    pub const fn migration_v2_directory(&self) -> &MigrationV2Directory {
        &self.migration_v2_directory
    }

    /// Return every named deployment environment.
    #[must_use]
    pub const fn environments(&self) -> &BTreeMap<String, WorkspaceEnvironment> {
        &self.environments
    }

    /// Return one named deployment environment.
    #[must_use]
    pub fn environment(&self, name: &str) -> Option<&WorkspaceEnvironment> {
        self.environments.get(name)
    }

    /// Return the explicit apply-side migration safety policy.
    ///
    /// The policy can tighten the verifier's classification but never loosen
    /// it: a standing allowance for destructive or opaque work is rejected by
    /// construction, so no configuration spells a permanent `force = true`.
    #[must_use]
    pub const fn migration_policy(&self) -> &MigrationSafetyPolicy {
        &self.migration_policy
    }

    /// Return the V2 migration directory resolved under the root.
    #[must_use]
    pub fn migration_v2_absolute_path(&self) -> PathBuf {
        self.workspace_root
            .as_path()
            .join(self.migration_v2_directory.as_path())
    }

    /// Return additive workspace capability requirements.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return independently configured shipped output targets.
    #[must_use]
    pub const fn outputs(&self) -> &BTreeMap<BindingTarget, OutputDirectory> {
        &self.outputs
    }

    /// Return retained symbolic references without resolving secret values.
    #[must_use]
    pub const fn secret_references(&self) -> &BTreeMap<SecretSlot, SecretReference> {
        &self.secret_references
    }

    /// Return exact locally validated extension requirements.
    #[must_use]
    pub const fn extensions(&self) -> &BTreeSet<ExtensionRequirement> {
        &self.extensions
    }
}

/// A consuming typed builder for [`TypeBridgeConfig`].
///
/// The builder accepts only validated nested values. `build` performs the
/// remaining cross-field and injected-service checks without loading schema or
/// consulting any provider.
pub struct TypeBridgeConfigBuilder {
    app_label: Option<MigrationAppLabel>,
    environments: Vec<(String, WorkspaceEnvironment)>,
    duplicate_required_fields: BTreeSet<&'static str>,
    extensions: Vec<ExtensionRequirement>,
    managed_scope_id: Option<ManagedScopeId>,
    migration_policy: Option<MigrationSafetyPolicy>,
    migration_v2_directory: Option<MigrationV2Directory>,
    outputs: Vec<(BindingTarget, OutputDirectory)>,
    required_capabilities: CapabilitySet,
    schema_set: Option<SchemaSetPath>,
    secrets: Vec<(SecretSlot, SecretReference)>,
    semantic_profile: Option<SemanticProfileId>,
    workspace_root: WorkspaceRoot,
}

impl TypeBridgeConfigBuilder {
    fn new(workspace_root: WorkspaceRoot) -> Self {
        Self {
            app_label: None,
            environments: Vec::new(),
            duplicate_required_fields: BTreeSet::new(),
            extensions: Vec::new(),
            managed_scope_id: None,
            migration_policy: None,
            migration_v2_directory: None,
            outputs: Vec::new(),
            required_capabilities: CapabilitySet::new(),
            schema_set: None,
            secrets: Vec::new(),
            semantic_profile: None,
            workspace_root,
        }
    }

    fn mark_duplicate<T>(
        slot: &mut Option<T>,
        value: T,
        field: &'static str,
        duplicates: &mut BTreeSet<&'static str>,
    ) {
        if slot.replace(value).is_some() {
            duplicates.insert(field);
        }
    }

    /// Select the portable schema-set manifest.
    #[must_use]
    pub fn schema_set(mut self, path: SchemaSetPath) -> Self {
        Self::mark_duplicate(
            &mut self.schema_set,
            path,
            "schema_set",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Select the validated migration application label.
    #[must_use]
    pub fn app_label(mut self, app_label: MigrationAppLabel) -> Self {
        Self::mark_duplicate(
            &mut self.app_label,
            app_label,
            "app_label",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Bind the durable managed-scope identity to the sole exclusive profile.
    #[must_use]
    pub fn exclusive_managed_scope(mut self, scope_id: ManagedScopeId) -> Self {
        Self::mark_duplicate(
            &mut self.managed_scope_id,
            scope_id,
            "managed_scope",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Select the exact server-semantic profile.
    #[must_use]
    pub fn semantic_profile(mut self, profile: SemanticProfileId) -> Self {
        Self::mark_duplicate(
            &mut self.semantic_profile,
            profile,
            "semantic_profile",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Select the confined direct V2 migration directory.
    #[must_use]
    pub fn migration_v2_directory(mut self, directory: MigrationV2Directory) -> Self {
        Self::mark_duplicate(
            &mut self.migration_v2_directory,
            directory,
            "migration_v2_directory",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Replace the default apply-side migration safety policy.
    ///
    /// [`MigrationSafetyPolicy`] construction already rejects every standing
    /// allowance for destructive or opaque work, so no builder input spells a
    /// permanent `force = true`.
    #[must_use]
    pub fn migration_policy(mut self, policy: MigrationSafetyPolicy) -> Self {
        Self::mark_duplicate(
            &mut self.migration_policy,
            policy,
            "migration_policy",
            &mut self.duplicate_required_fields,
        );
        self
    }

    /// Add one open capability requirement without replacing prior requirements.
    #[must_use]
    pub fn require_capability(mut self, capability: CapabilityId) -> Self {
        self.required_capabilities.insert(capability);
        self
    }

    /// Add capability requirements without replacing prior requirements.
    #[must_use]
    pub fn require_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = CapabilityId>,
    ) -> Self {
        for capability in capabilities {
            self.required_capabilities.insert(capability);
        }
        self
    }

    /// Add one independently regenerated shipped binding target.
    #[must_use]
    pub fn output(mut self, target: BindingTarget, directory: OutputDirectory) -> Self {
        self.outputs.push((target, directory));
        self
    }

    /// Add one named deployment environment.
    #[must_use]
    pub fn environment(
        mut self,
        name: impl Into<String>,
        environment: WorkspaceEnvironment,
    ) -> Self {
        self.environments.push((name.into(), environment));
        self
    }

    /// Add one retained symbolic secret reference.
    #[must_use]
    pub fn secret(mut self, slot: SecretSlot, reference: SecretReference) -> Self {
        self.secrets.push((slot, reference));
        self
    }

    /// Add one exact local extension requirement.
    #[must_use]
    pub fn require_extension(mut self, requirement: ExtensionRequirement) -> Self {
        self.extensions.push(requirement);
        self
    }

    /// Validate the complete config using only the explicitly injected services.
    pub fn build(
        self,
        services: &TypeBridgeConfigServices<'_>,
    ) -> Result<TypeBridgeConfig, WorkspaceConfigError> {
        if let Some(field) = self.duplicate_required_fields.iter().next() {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::DuplicateRequiredField,
                "a singleton workspace field was assigned more than once",
            )
            .with_detail(*field));
        }

        fn required<T>(value: Option<T>, field: &'static str) -> Result<T, WorkspaceConfigError> {
            value.ok_or_else(|| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::MissingRequiredField,
                    "required workspace field is missing",
                )
                .with_detail(field)
            })
        }
        let schema_set = required(self.schema_set, "schema_set")?;
        let app_label = required(self.app_label, "app_label")?;
        let managed_scope_id = required(self.managed_scope_id, "managed_scope")?;
        let semantic_profile = required(self.semantic_profile, "semantic_profile")?;
        let migration_v2_directory =
            required(self.migration_v2_directory, "migration_v2_directory")?;

        if semantic_profile.as_str() != TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_ID
            || SemanticProfile::resolve(&semantic_profile).is_err()
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::UnsupportedSemanticProfile,
                "workspace requires the exact frozen TypeDB 3.12.1 semantic profile",
            )
            .with_detail(semantic_profile.as_str()));
        }

        let canonical_root = services
            .sources
            .canonicalize_workspace_root(self.workspace_root.as_path())
            .map_err(|error| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                    "injected source service could not canonicalize workspace root",
                )
                .with_detail(error.code())
            })?;
        if canonical_root != self.workspace_root.as_path() {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootNotCanonical,
                "explicit workspace root differs from its canonical spelling",
            ));
        }

        let managed_scope = ManagedScopeBinding::exclusive(managed_scope_id).map_err(|_| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidManagedScope,
                "managed scope could not bind to the exclusive profile",
            )
        })?;
        debug_assert_eq!(
            managed_scope.profile().id(),
            &ManagedScopeProfileId::exclusive()
        );

        let mut outputs = BTreeMap::new();
        for (target, directory) in self.outputs {
            if outputs.insert(target, directory).is_some() {
                return Err(WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::DuplicateOutputTarget,
                    "binding output target is configured more than once",
                ));
            }
        }

        let mut workspace_paths: Vec<(&'static str, &Path)> = vec![
            ("schema_set", schema_set.as_path()),
            ("migration_v2_directory", migration_v2_directory.as_path()),
        ];
        for (target, directory) in &outputs {
            workspace_paths.push((output_field_name(*target), directory.as_path()));
        }
        for left_index in 0..workspace_paths.len() {
            for right_index in (left_index + 1)..workspace_paths.len() {
                let (left_name, left_path) = workspace_paths[left_index];
                let (right_name, right_path) = workspace_paths[right_index];
                if workspace_paths_overlap(left_path, right_path) {
                    return Err(WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::OverlappingWorkspacePath,
                        "workspace-owned paths must be pairwise disjoint",
                    )
                    .with_detail(format!("{left_name},{right_name}")));
                }
            }
        }

        let mut environments = BTreeMap::new();
        for (name, environment) in self.environments {
            if name.is_empty()
                || name.len() > MAX_SYMBOLIC_ID_BYTES
                || !name.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
            {
                return Err(WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::InvalidSymbolicIdentifier,
                    "environment names must be bounded lowercase identifiers",
                )
                .with_detail(name));
            }
            for reference in [environment.username(), environment.password()] {
                services
                    .secrets
                    .validate_reference(reference)
                    .map_err(|error| {
                        WorkspaceConfigError::new(
                            WorkspaceConfigErrorCode::SecretReferenceRejected,
                            "local secret-reference service rejected an environment credential",
                        )
                        .with_detail(error.code())
                    })?;
            }
            if environments.insert(name, environment).is_some() {
                return Err(WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::DuplicateRequiredField,
                    "environment name is configured more than once",
                ));
            }
        }

        let environment_namespaces = environments
            .iter()
            .map(|(name, environment)| {
                normalized_environment_endpoints(environment.uri())
                    .map(|endpoints| (name, environment, endpoints))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for left_index in 0..environment_namespaces.len() {
            for right_index in (left_index + 1)..environment_namespaces.len() {
                let (left_name, left, left_endpoints) = &environment_namespaces[left_index];
                let (right_name, right, right_endpoints) = &environment_namespaces[right_index];
                if left_endpoints.is_disjoint(right_endpoints) {
                    continue;
                }
                let left_journal =
                    format!("{}{TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX}", left.database());
                let right_journal =
                    format!("{}{TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX}", right.database());
                if left_journal == right.database() || right_journal == left.database() {
                    return Err(WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::EnvironmentDatabaseCollision,
                        "one environment's managed database aliases another environment's reserved migration journal on overlapping TypeDB endpoint sets",
                    )
                    .with_detail(format!("{left_name},{right_name}")));
                }
            }
        }

        let mut secret_references = BTreeMap::new();
        for (slot, reference) in self.secrets {
            if secret_references.insert(slot, reference).is_some() {
                return Err(WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::DuplicateSecretSlot,
                    "symbolic secret slot is configured more than once",
                ));
            }
        }

        let mut extensions_by_handler = BTreeMap::new();
        for requirement in self.extensions {
            if extensions_by_handler
                .insert(requirement.handler_id.clone(), requirement)
                .is_some()
            {
                return Err(WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::DuplicateExtensionHandler,
                    "extension handler is required more than once",
                ));
            }
        }
        let extensions = extensions_by_handler.into_values().collect::<BTreeSet<_>>();

        for reference in secret_references.values() {
            services
                .secrets
                .validate_reference(reference)
                .map_err(|error| {
                    WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::SecretReferenceRejected,
                        "local secret-reference service rejected a symbolic reference",
                    )
                    .with_detail(error.code())
                })?;
        }
        for requirement in &extensions {
            services
                .extensions
                .validate_requirement(requirement)
                .map_err(|error| {
                    WorkspaceConfigError::new(
                        WorkspaceConfigErrorCode::ExtensionRequirementRejected,
                        "local extension registry rejected a handler requirement",
                    )
                    .with_detail(error.code())
                })?;
        }

        Ok(TypeBridgeConfig {
            app_label,
            environments,
            extensions,
            managed_scope,
            migration_policy: self
                .migration_policy
                .unwrap_or_else(MigrationSafetyPolicy::default_policy),
            migration_v2_directory,
            outputs,
            required_capabilities: self.required_capabilities,
            schema_set,
            secret_references,
            semantic_profile,
            workspace_root: self.workspace_root,
        })
    }
}
