use std::error::Error;
use std::fmt;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Component, Path};
use std::sync::Arc;

#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt as _;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::Dir;
use cap_std::fs::OpenOptions;
#[cfg(windows)]
use cap_std::fs::OpenOptionsExt as _;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::managed_scope::ManagedScopeProfileId;
use type_bridge_contract::schema::{DeclaredSchema, ManagedSchemaState, SchemaDiagnostics};
use type_bridge_schema::{
    BoundManagedSchemaScope, DeltaError, ManagedDeltaContext, ManagedSchemaScope, ResolvedSchema,
    SchemaDiscoveryEvidence, SchemaDiscoveryLimits, SchemaDiscoverySnapshot, SchemaDocumentSet,
    SchemaSourceService, load_schema_set_with_source, managed_schema_state, normalize_documents,
    resolve_schema_with_capabilities,
};

use crate::{
    ExtensionRegistryService, LocatedConfigSpec, SecretReferenceService,
    TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_IDS, TypeBridgeConfig, TypeBridgeConfigServices,
    WorkspaceConfigError, WorkspaceConfigErrorCode, WorkspaceDirectoryAuthority,
    WorkspaceOutputDirectory, WorkspaceRoot, WorkspaceSourceService, WorkspaceTransportPolicy,
    authority::retain_canonical_root,
};

const MAX_WORKSPACE_ROOT_CA_BYTES: usize = 1024 * 1024;

/// Explicit local services and capabilities used to construct a source workspace.
///
/// No provider, credential resolver, history store, lock writer, or network
/// executor is present at this boundary.
pub struct TypeBridgeWorkspaceServices<'a> {
    available_capabilities: &'a CapabilitySet,
    config_source: &'a dyn WorkspaceSourceService,
    extensions: &'a dyn ExtensionRegistryService,
    root_authority: Option<&'a WorkspaceDirectoryAuthority>,
    schema_source: &'a dyn SchemaSourceService,
    secrets: &'a dyn SecretReferenceService,
}

impl<'a> TypeBridgeWorkspaceServices<'a> {
    /// Construct services around one retained workspace directory authority.
    ///
    /// This is the production constructor: config validation, schema capture,
    /// migration access, and generated outputs all derive from the same root
    /// descriptor.
    #[must_use]
    pub fn new(
        source: &'a WorkspaceDirectoryAuthority,
        secrets: &'a dyn SecretReferenceService,
        extensions: &'a dyn ExtensionRegistryService,
        available_capabilities: &'a CapabilitySet,
    ) -> Self {
        Self {
            available_capabilities,
            config_source: source,
            extensions,
            root_authority: Some(source),
            schema_source: source,
            secrets,
        }
    }

    /// Construct services around an explicitly injected observation service.
    ///
    /// This boundary supports virtual filesystems and adversarial source
    /// fixtures. The caller owns consistency between that service and the
    /// configured physical root; production filesystem callers should use
    /// [`Self::new`] with [`WorkspaceDirectoryAuthority`].
    #[must_use]
    pub fn with_source<S>(
        source: &'a S,
        secrets: &'a dyn SecretReferenceService,
        extensions: &'a dyn ExtensionRegistryService,
        available_capabilities: &'a CapabilitySet,
    ) -> Self
    where
        S: SchemaSourceService + 'a,
    {
        Self {
            available_capabilities,
            config_source: source,
            extensions,
            root_authority: None,
            schema_source: source,
            secrets,
        }
    }

    fn config_services(&self) -> TypeBridgeConfigServices<'_> {
        TypeBridgeConfigServices::new(self.config_source, self.secrets, self.extensions)
    }

    fn require_exact_authority_root(
        &self,
        configured_root: &WorkspaceRoot,
    ) -> Result<(), WorkspaceConfigError> {
        if self
            .root_authority
            .is_some_and(|authority| authority.root() != configured_root)
        {
            return Err(WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootNotCanonical,
                "workspace directory authority must exactly match the configured workspace root",
            )
            .with_detail("workspace_root_authority_mismatch"));
        }
        Ok(())
    }
}

/// A fail-closed source-workspace construction failure retaining nested evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TypeBridgeWorkspaceError {
    /// Programmatic or located config validation failed.
    Config(WorkspaceConfigError),
    /// A binding-neutral contract invariant failed.
    Contract(Diagnostic),
    /// Source discovery, normalization, or resolution failed with spans intact.
    Schema(SchemaDiagnostics),
}

impl TypeBridgeWorkspaceError {
    /// Return an underlying config failure, if this is a config error.
    #[must_use]
    pub const fn config(&self) -> Option<&WorkspaceConfigError> {
        match self {
            Self::Config(error) => Some(error),
            Self::Contract(_) | Self::Schema(_) => None,
        }
    }

    /// Return an underlying contract failure, if this is a contract error.
    #[must_use]
    pub const fn contract(&self) -> Option<&Diagnostic> {
        match self {
            Self::Contract(error) => Some(error),
            Self::Config(_) | Self::Schema(_) => None,
        }
    }

    /// Return source-aware schema diagnostics, if this is a schema error.
    #[must_use]
    pub const fn schema(&self) -> Option<&SchemaDiagnostics> {
        match self {
            Self::Schema(error) => Some(error),
            Self::Config(_) | Self::Contract(_) => None,
        }
    }
}

impl fmt::Display for TypeBridgeWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "workspace config is invalid: {error}"),
            Self::Contract(error) => write!(formatter, "workspace contract is invalid: {error}"),
            Self::Schema(_) => {
                formatter.write_str("workspace schema source or semantics are invalid")
            }
        }
    }
}

impl Error for TypeBridgeWorkspaceError {}

impl From<WorkspaceConfigError> for TypeBridgeWorkspaceError {
    fn from(value: WorkspaceConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<Diagnostic> for TypeBridgeWorkspaceError {
    fn from(value: Diagnostic) -> Self {
        Self::Contract(value)
    }
}

impl From<SchemaDiagnostics> for TypeBridgeWorkspaceError {
    fn from(value: SchemaDiagnostics) -> Self {
        Self::Schema(value)
    }
}

impl From<DeltaError> for TypeBridgeWorkspaceError {
    fn from(value: DeltaError) -> Self {
        match value {
            DeltaError::Contract(error) => Self::Contract(error),
            DeltaError::Schema(error) => Self::Schema(error),
        }
    }
}

/// An opaque source-authority workspace with resolved schema and managed state.
pub struct TypeBridgeWorkspace {
    authority_owner: Arc<()>,
    bound_scope: BoundManagedSchemaScope,
    config: TypeBridgeConfig,
    declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    discovery: SchemaDiscoverySnapshot,
    located_config: Option<LocatedConfigSpec>,
    managed_state: ManagedSchemaState,
    required_capabilities: CapabilitySet,
    resolved: ResolvedSchema,
    root_directory: Dir,
}

impl TypeBridgeWorkspace {
    /// Load a source workspace from an already validated programmatic config.
    pub fn from_config(
        config: TypeBridgeConfig,
        services: &TypeBridgeWorkspaceServices<'_>,
    ) -> Result<Self, TypeBridgeWorkspaceError> {
        Self::construct(config, None, services)
    }

    /// Resolve a located lossless spec and load the same source-workspace pipeline.
    pub fn from_located_config(
        located: LocatedConfigSpec,
        services: &TypeBridgeWorkspaceServices<'_>,
    ) -> Result<Self, TypeBridgeWorkspaceError> {
        services.require_exact_authority_root(located.origin().workspace_root())?;
        let retained = located.clone();
        let config = located.resolve(&services.config_services())?;
        Self::construct(config, Some(retained), services)
    }

    fn construct(
        config: TypeBridgeConfig,
        located_config: Option<LocatedConfigSpec>,
        services: &TypeBridgeWorkspaceServices<'_>,
    ) -> Result<Self, TypeBridgeWorkspaceError> {
        services.require_exact_authority_root(config.workspace_root())?;
        let root_directory = match services.root_authority {
            Some(authority) => authority.directory().try_clone().map_err(|_| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                    "workspace root cannot be retained as directory authority",
                )
                .with_detail("workspace_root_authority_unavailable")
            })?,
            None => retain_canonical_root(config.workspace_root().as_path()).map_err(|_| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                    "workspace root cannot be retained as directory authority",
                )
                .with_detail("workspace_root_authority_unavailable")
            })?,
        };
        validate_config_before_capture(&config, services)?;

        let discovery = load_schema_set_with_source(
            config.schema_set_absolute_path(),
            services.schema_source,
            SchemaDiscoveryLimits::default(),
        )?;
        let declared = normalize_documents(discovery.documents())?;

        let required_capabilities = declared
            .required_capabilities()
            .iter()
            .chain(config.required_capabilities().iter())
            .cloned()
            .collect::<CapabilitySet>();
        required_capabilities.ensure_supported_by(services.available_capabilities)?;

        let resolved = resolve_schema_with_capabilities(
            &declared,
            config.semantic_profile(),
            services.available_capabilities,
        )?;
        let scope_id = config.managed_scope().id().clone();
        let bound_scope = ManagedSchemaScope::bind_exclusive(scope_id.clone(), &declared)?;
        let context = ManagedDeltaContext::new(
            scope_id,
            config.semantic_profile().clone(),
            services.available_capabilities.clone(),
        );
        let managed_state = managed_schema_state(&declared, &context)?;

        Ok(Self {
            authority_owner: Arc::new(()),
            bound_scope,
            config,
            declared,
            delta_context: context,
            discovery,
            located_config,
            managed_state,
            required_capabilities,
            resolved,
            root_directory,
        })
    }

    /// Return the validated inert workspace policy.
    #[must_use]
    pub const fn config(&self) -> &TypeBridgeConfig {
        &self.config
    }

    /// Return the retained located workspace manifest for source-backed construction.
    #[must_use]
    pub const fn located_config(&self) -> Option<&LocatedConfigSpec> {
        self.located_config.as_ref()
    }

    /// Return the atomic schema-set discovery snapshot and exact source authority.
    #[must_use]
    pub const fn discovery(&self) -> &SchemaDiscoverySnapshot {
        &self.discovery
    }

    /// Return the exact lossless schema documents selected by the manifest.
    #[must_use]
    pub const fn documents(&self) -> &SchemaDocumentSet {
        self.discovery.documents()
    }

    /// Return deterministic discovery evidence and source fingerprints.
    #[must_use]
    pub const fn discovery_evidence(&self) -> &SchemaDiscoveryEvidence {
        self.discovery.evidence()
    }

    /// Return normalized direct schema facts with source spans.
    #[must_use]
    pub const fn declared_schema(&self) -> &DeclaredSchema {
        &self.declared
    }

    /// Return the fully validated effective schema.
    #[must_use]
    pub const fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    /// Return the exclusive managed fact selection and durable binding.
    #[must_use]
    pub const fn bound_managed_scope(&self) -> &BoundManagedSchemaScope {
        &self.bound_scope
    }

    /// Return the exact managed schema state used by offline migration planning.
    #[must_use]
    pub const fn managed_state(&self) -> &ManagedSchemaState {
        &self.managed_state
    }

    /// Return schema-derived plus additive configured capability requirements.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the exact managed-delta context this workspace plans under.
    #[must_use]
    pub const fn delta_context(&self) -> &ManagedDeltaContext {
        &self.delta_context
    }

    /// Return the retained workspace-root capability used by later file operations.
    ///
    /// Consumers must keep every operation relative to this handle. The path in
    /// [`TypeBridgeConfig::workspace_root`](crate::TypeBridgeConfig::workspace_root)
    /// remains diagnostic-only after construction and may have been renamed or
    /// replaced by another directory entry.
    pub(crate) const fn root_directory(&self) -> &Dir {
        &self.root_directory
    }

    /// Clone the retained workspace root into a generated-output authority.
    pub fn output_root(&self) -> Result<WorkspaceOutputDirectory, String> {
        WorkspaceOutputDirectory::from_root(
            &self.root_directory,
            self.config.workspace_root().as_path(),
        )
    }

    /// Capture one configured environment's custom trust root through this
    /// workspace's retained directory authority.
    ///
    /// The environment is selected from the immutable validated config by
    /// name. The canonical diagnostic path is converted back to a confined
    /// relative identity and every component is opened without following a
    /// symlink. The returned bytes come from two bounded reads of the same
    /// regular-file handle and may be passed directly to a transport parser.
    #[doc(hidden)]
    pub fn capture_environment_custom_root_ca(
        &self,
        environment_name: &str,
    ) -> Result<Option<Arc<[u8]>>, WorkspaceConfigError> {
        let environment = self.config.environment(environment_name).ok_or_else(|| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::InvalidWorkspaceValue,
                "custom root CA capture requires an environment from this workspace config",
            )
            .with_detail("workspace_tls_environment_unknown")
        })?;
        let WorkspaceTransportPolicy::CustomRootCa(root_ca) = environment.transport_policy() else {
            return Ok(None);
        };
        let workspace_root = self.config.workspace_root().as_path();
        let relative = root_ca
            .as_path()
            .strip_prefix(workspace_root)
            .map_err(|_| workspace_root_ca_authority_mismatch())?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || workspace_root.join(relative) != root_ca.as_path()
        {
            return Err(workspace_root_ca_authority_mismatch());
        }
        capture_workspace_root_ca(&self.root_directory, relative)
    }

    pub(crate) const fn authority_owner(&self) -> &Arc<()> {
        &self.authority_owner
    }
}

fn workspace_root_ca_authority_mismatch() -> WorkspaceConfigError {
    WorkspaceConfigError::new(
        WorkspaceConfigErrorCode::InvalidTlsRootCa,
        "custom root CA identity is not owned by this workspace configuration",
    )
    .with_detail("workspace_tls_root_ca_authority_mismatch")
}

fn workspace_root_ca_capture_failure(detail: &'static str) -> WorkspaceConfigError {
    WorkspaceConfigError::new(
        WorkspaceConfigErrorCode::InvalidTlsRootCa,
        "custom root CA cannot be captured through the retained workspace authority",
    )
    .with_detail(detail)
}

fn capture_workspace_root_ca(
    root: &Dir,
    relative: &Path,
) -> Result<Option<Arc<[u8]>>, WorkspaceConfigError> {
    let mut directory = root
        .try_clone()
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    let mut components = relative.components().peekable();
    let mut file_name = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(workspace_root_ca_authority_mismatch());
        };
        if components.peek().is_none() {
            file_name = Some(name.to_owned());
        } else {
            directory = directory
                .open_dir_nofollow(name)
                .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
        }
    }
    let file_name = file_name.ok_or_else(workspace_root_ca_authority_mismatch)?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    options.nonblock(true);
    #[cfg(windows)]
    {
        // Retain read sharing for driver consumers while excluding concurrent
        // writes, deletion, and replacement during the bounded capture.
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    let mut file = directory
        .open_with(Path::new(&file_name), &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    let before = file
        .metadata()
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    if !before.is_file() {
        return Err(workspace_root_ca_capture_failure(
            "tls_custom_root_ca_not_file",
        ));
    }
    if before.len() > u64::try_from(MAX_WORKSPACE_ROOT_CA_BYTES).unwrap_or(u64::MAX) {
        return Err(workspace_root_ca_capture_failure(
            "tls_custom_root_ca_too_large",
        ));
    }

    let read_limit = u64::try_from(MAX_WORKSPACE_ROOT_CA_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    (&mut file)
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    if bytes.len() > MAX_WORKSPACE_ROOT_CA_BYTES {
        return Err(workspace_root_ca_capture_failure(
            "tls_custom_root_ca_too_large",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    let mut verification = Vec::new();
    (&mut file)
        .take(read_limit)
        .read_to_end(&mut verification)
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    let after = file
        .metadata()
        .map_err(|_| workspace_root_ca_capture_failure("tls_custom_root_ca_unreadable"))?;
    let timestamps_match = match (before.modified(), after.modified()) {
        (Ok(before), Ok(after)) => before == after,
        (Err(_), Err(_)) => true,
        _ => false,
    };
    if before.len() != after.len()
        || before.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || bytes != verification
        || !timestamps_match
    {
        return Err(workspace_root_ca_capture_failure(
            "tls_custom_root_ca_unreadable",
        ));
    }
    Ok(Some(bytes.into()))
}

fn validate_config_before_capture(
    config: &TypeBridgeConfig,
    services: &TypeBridgeWorkspaceServices<'_>,
) -> Result<(), TypeBridgeWorkspaceError> {
    if !TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_IDS.contains(&config.semantic_profile().as_str()) {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::UnsupportedSemanticProfile,
            "workspace config semantic profile changed after validation",
        )
        .into());
    }
    if config.managed_scope().profile().id() != &ManagedScopeProfileId::exclusive() {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::InvalidManagedScope,
            "workspace config managed-scope profile changed after validation",
        )
        .into());
    }

    let canonical_root = services
        .config_source
        .canonicalize_workspace_root(config.workspace_root().as_path())
        .map_err(|error| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                "workspace source service cannot revalidate the config root",
            )
            .with_detail(error.code())
        })?;
    if canonical_root != config.workspace_root().as_path() {
        return Err(WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::WorkspaceRootNotCanonical,
            "workspace root differs under the construction source service",
        )
        .into());
    }

    for requirement in config.extensions() {
        services
            .extensions
            .validate_requirement(requirement)
            .map_err(|error| {
                WorkspaceConfigError::new(
                    WorkspaceConfigErrorCode::ExtensionRequirementRejected,
                    "workspace extension is unavailable before source capture",
                )
                .with_detail(error.code())
            })?;
    }
    config
        .required_capabilities()
        .ensure_supported_by(services.available_capabilities)?;
    Ok(())
}
