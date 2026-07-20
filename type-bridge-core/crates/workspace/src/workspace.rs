use std::error::Error;
use std::fmt;

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
    TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_ID, TypeBridgeConfig, TypeBridgeConfigServices,
    WorkspaceConfigError, WorkspaceConfigErrorCode, WorkspaceSourceService,
};

/// Explicit local services and capabilities used to construct a source workspace.
///
/// No provider, credential resolver, history store, lock writer, or network
/// executor is present at this boundary.
pub struct TypeBridgeWorkspaceServices<'a> {
    available_capabilities: &'a CapabilitySet,
    config_source: &'a dyn WorkspaceSourceService,
    extensions: &'a dyn ExtensionRegistryService,
    schema_source: &'a dyn SchemaSourceService,
    secrets: &'a dyn SecretReferenceService,
}

impl<'a> TypeBridgeWorkspaceServices<'a> {
    /// Construct services around one injected bounded schema source.
    #[must_use]
    pub fn new<S>(
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
            schema_source: source,
            secrets,
        }
    }

    fn config_services(&self) -> TypeBridgeConfigServices<'_> {
        TypeBridgeConfigServices::new(self.config_source, self.secrets, self.extensions)
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
    bound_scope: BoundManagedSchemaScope,
    config: TypeBridgeConfig,
    declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    discovery: SchemaDiscoverySnapshot,
    located_config: Option<LocatedConfigSpec>,
    managed_state: ManagedSchemaState,
    required_capabilities: CapabilitySet,
    resolved: ResolvedSchema,
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
        let retained = located.clone();
        let config = located.resolve(&services.config_services())?;
        Self::construct(config, Some(retained), services)
    }

    fn construct(
        config: TypeBridgeConfig,
        located_config: Option<LocatedConfigSpec>,
        services: &TypeBridgeWorkspaceServices<'_>,
    ) -> Result<Self, TypeBridgeWorkspaceError> {
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
            bound_scope,
            config,
            declared,
            delta_context: context,
            discovery,
            located_config,
            managed_state,
            required_capabilities,
            resolved,
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
}

fn validate_config_before_capture(
    config: &TypeBridgeConfig,
    services: &TypeBridgeWorkspaceServices<'_>,
) -> Result<(), TypeBridgeWorkspaceError> {
    if config.semantic_profile().as_str() != TYPEBRIDGE_WORKSPACE_SEMANTIC_PROFILE_ID {
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
