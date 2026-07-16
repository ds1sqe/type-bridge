use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{
    CodecVersion, FormatVersion, from_canonical_json_with_limits,
    to_canonical_json_with_limits,
};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH,
    MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, SemanticProfileBinding,
};
use type_bridge_contract::projection::{
    BindingTarget, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::{
    DeclaredSchema, ManagedSchemaState, SchemaDiagnostics, decode_declared_schema,
    encode_declared_schema,
};
use type_bridge_schema::{
    DeltaError, ManagedDeltaContext, ResolvedSchema, managed_schema_state, project,
    resolve_schema_with_capabilities,
};

use crate::{ExtensionRequirement, TypeBridgeWorkspace, WorkspaceConfigError};

/// The first closed compiled-schema bundle format.
pub const TYPEBRIDGE_SCHEMA_BUNDLE_V1: &str = "typebridge.schema-bundle/v1";
/// Fingerprint domain for the exact bundle content envelope.
pub const SCHEMA_BUNDLE_FINGERPRINT_DOMAIN: &str = "typebridge.schema.bundle";
/// Canonicalization identity for the first bundle content envelope.
pub const SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION: &str =
    "typebridge.schema-bundle/v1";
/// Maximum canonical compiled bundle size: 16 MiB.
pub const MAX_SCHEMA_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

const SCHEMA_BUNDLE_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_SCHEMA_BUNDLE_BYTES,
    max_depth: MAX_CANONICAL_DEPTH,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

/// Stable high-level failure classification for bundle construction and verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaBundleErrorCode {
    /// A nested contract codec or constructor rejected input.
    Contract,
    /// Schema resolution or projection reported source-neutral schema diagnostics.
    Schema,
    /// A bundle, codec, or schema-IR version is unsupported.
    UnsupportedVersion,
    /// Bundle policy differs from the explicit verification context.
    ContextMismatch,
    /// A digest, fingerprint, state, or independently derived value is stale.
    IntegrityMismatch,
    /// A projection target is missing, duplicated, or out of canonical order.
    ProjectionTargetMismatch,
    /// A required extension is not available in the verification context.
    ExtensionUnavailable,
    /// Projection bytes or their detached evidence failed independent verification.
    ProjectionMismatch,
    /// Projection resource evidence appeared before this bundle slice supports it.
    UnsupportedProjectionEvidence,
}

/// A fail-closed bundle failure retaining nested contract or schema diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaBundleError {
    code: SchemaBundleErrorCode,
    message: &'static str,
    config: Option<WorkspaceConfigError>,
    contract: Option<Diagnostic>,
    schema: Option<SchemaDiagnostics>,
}

impl SchemaBundleError {
    fn new(code: SchemaBundleErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            config: None,
            contract: None,
            schema: None,
        }
    }

    fn projection(error: Diagnostic) -> Self {
        Self {
            code: SchemaBundleErrorCode::ProjectionMismatch,
            message: "runtime projection verification failed",
            config: None,
            contract: Some(error),
            schema: None,
        }
    }

    /// Return the stable high-level failure code.
    #[must_use]
    pub const fn code(&self) -> SchemaBundleErrorCode {
        self.code
    }

    /// Return a nested contract diagnostic, when one caused the failure.
    #[must_use]
    pub const fn contract(&self) -> Option<&Diagnostic> {
        self.contract.as_ref()
    }

    /// Return a nested workspace-constructor diagnostic, when one caused the failure.
    #[must_use]
    pub const fn config(&self) -> Option<&WorkspaceConfigError> {
        self.config.as_ref()
    }

    /// Return nested schema diagnostics, when resolution or projection caused the failure.
    #[must_use]
    pub const fn schema(&self) -> Option<&SchemaDiagnostics> {
        self.schema.as_ref()
    }
}

impl fmt::Display for SchemaBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for SchemaBundleError {}

impl From<Diagnostic> for SchemaBundleError {
    fn from(value: Diagnostic) -> Self {
        Self {
            code: SchemaBundleErrorCode::Contract,
            message: "a compiled bundle contract is invalid",
            config: None,
            contract: Some(value),
            schema: None,
        }
    }
}

impl From<SchemaDiagnostics> for SchemaBundleError {
    fn from(value: SchemaDiagnostics) -> Self {
        Self {
            code: SchemaBundleErrorCode::Schema,
            message: "compiled schema reconstruction failed",
            config: None,
            contract: None,
            schema: Some(value),
        }
    }
}

impl From<WorkspaceConfigError> for SchemaBundleError {
    fn from(value: WorkspaceConfigError) -> Self {
        Self {
            code: SchemaBundleErrorCode::Contract,
            message: "a compiled bundle workspace contract is invalid",
            config: Some(value),
            contract: None,
            schema: None,
        }
    }
}

impl From<DeltaError> for SchemaBundleError {
    fn from(value: DeltaError) -> Self {
        match value {
            DeltaError::Contract(error) => error.into(),
            DeltaError::Schema(error) => error.into(),
        }
    }
}

/// Exact target policy and handler evidence allowed for one compiled projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleProjectionContext {
    config: ProjectionConfig,
    handlers: Vec<ProjectionHandler>,
}

impl BundleProjectionContext {
    /// Validate one target's projection configuration and deterministic handler evidence.
    pub fn new(
        config: ProjectionConfig,
        mut handlers: Vec<ProjectionHandler>,
    ) -> Result<Self, SchemaBundleError> {
        handlers.sort_by(|left, right| {
            left.id()
                .cmp(right.id())
                .then(left.version().cmp(&right.version()))
        });
        if handlers
            .windows(2)
            .any(|pair| pair[0].id() == pair[1].id())
        {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ContextMismatch,
                "projection context contains duplicate handler identities",
            ));
        }
        let required = match config.target() {
            BindingTarget::Python => ProjectionHandler::python_v1(),
            BindingTarget::TypeScript => ProjectionHandler::typescript_v1(),
            BindingTarget::Rust => ProjectionHandler::rust_v1(),
        };
        if handlers.len() != 1 || handlers.first() != Some(&required) {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ContextMismatch,
                "projection context handler evidence differs from the exact shipped target set",
            ));
        }
        Ok(Self { config, handlers })
    }

    /// Return the exact target configuration.
    #[must_use]
    pub const fn config(&self) -> &ProjectionConfig {
        &self.config
    }

    /// Return the target selected by the projection configuration.
    #[must_use]
    pub const fn target(&self) -> BindingTarget {
        self.config.target()
    }

    /// Return canonical handler evidence.
    #[must_use]
    pub fn handlers(&self) -> &[ProjectionHandler] {
        &self.handlers
    }
}

/// Explicit capability, extension, scope, profile, and projection trust context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleVerificationContext {
    available_capabilities: CapabilitySet,
    available_extensions: BTreeSet<ExtensionRequirement>,
    managed_scope: ManagedScopeBinding,
    projections: BTreeMap<BindingTarget, BundleProjectionContext>,
    semantic_profile: SemanticProfileId,
}

impl BundleVerificationContext {
    /// Construct a closed verification context without consulting sources or providers.
    pub fn new(
        semantic_profile: SemanticProfileId,
        managed_scope: ManagedScopeBinding,
        available_capabilities: CapabilitySet,
        available_extensions: impl IntoIterator<Item = ExtensionRequirement>,
        projections: impl IntoIterator<Item = BundleProjectionContext>,
    ) -> Result<Self, SchemaBundleError> {
        SemanticProfileBinding::resolve(semantic_profile.clone())?;
        let mut extension_set = BTreeSet::new();
        for extension in available_extensions {
            if !extension_set.insert(extension) {
                return Err(SchemaBundleError::new(
                    SchemaBundleErrorCode::ContextMismatch,
                    "verification context contains a duplicate extension requirement",
                ));
            }
        }
        let mut projection_map = BTreeMap::new();
        for projection in projections {
            if projection_map.insert(projection.target(), projection).is_some() {
                return Err(SchemaBundleError::new(
                    SchemaBundleErrorCode::ProjectionTargetMismatch,
                    "verification context contains a duplicate projection target",
                ));
            }
        }
        Ok(Self {
            available_capabilities,
            available_extensions: extension_set,
            managed_scope,
            projections: projection_map,
            semantic_profile,
        })
    }

    /// Return capabilities available to schema resolution and bundle consumption.
    #[must_use]
    pub const fn available_capabilities(&self) -> &CapabilitySet {
        &self.available_capabilities
    }

    /// Return extensions available to bundle consumption.
    #[must_use]
    pub const fn available_extensions(&self) -> &BTreeSet<ExtensionRequirement> {
        &self.available_extensions
    }

    /// Return the expected durable managed scope and frozen profile binding.
    #[must_use]
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        &self.managed_scope
    }

    /// Return the expected semantic profile identity.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Look up exact projection verification policy by shipped target.
    #[must_use]
    pub fn projection(&self, target: BindingTarget) -> Option<&BundleProjectionContext> {
        self.projections.get(&target)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingTargetWire {
    Python,
    #[serde(rename = "typescript")]
    TypeScript,
    Rust,
}

impl BindingTargetWire {
    const fn rebuild(self) -> BindingTarget {
        match self {
            Self::Python => BindingTarget::Python,
            Self::TypeScript => BindingTarget::TypeScript,
            Self::Rust => BindingTarget::Rust,
        }
    }
}

impl From<BindingTarget> for BindingTargetWire {
    fn from(value: BindingTarget) -> Self {
        match value {
            BindingTarget::Python => Self::Python,
            BindingTarget::TypeScript => Self::TypeScript,
            BindingTarget::Rust => Self::Rust,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct ExtensionRequirementWire {
    handler_id: String,
    version: String,
}

impl ExtensionRequirementWire {
    fn from_requirement(value: &ExtensionRequirement) -> Self {
        Self {
            handler_id: value.handler_id().to_owned(),
            version: value.version().to_owned(),
        }
    }

    fn rebuild(self) -> Result<ExtensionRequirement, SchemaBundleError> {
        ExtensionRequirement::new(self.handler_id, self.version).map_err(SchemaBundleError::from)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectionEntryWire {
    binding_fingerprint: Value,
    canonical_projection: Value,
    config: Value,
    handler_evidence: Vec<Value>,
    target: BindingTargetWire,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaBundleContentWire {
    bundle_version: String,
    codec_version: u16,
    declared_schema: Value,
    expected_declared_identity: Value,
    expected_managed_declared_identity: Value,
    expected_managed_semantic_schema: Value,
    expected_semantic_schema: Value,
    managed_scope: Value,
    managed_state: Value,
    projections: Vec<ProjectionEntryWire>,
    required_capabilities: CapabilitySet,
    required_extensions: Vec<ExtensionRequirementWire>,
    schema_ir_version: u16,
    semantic_profile: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaBundleWire {
    bundle_fingerprint: Value,
    content: SchemaBundleContentWire,
}

/// Opaque constructor-verified compiled schema bundle.
#[derive(Debug)]
pub struct VerifiedSchemaBundle {
    bundle_fingerprint: Fingerprint,
    canonical_bytes: Vec<u8>,
    declared: DeclaredSchema,
    managed_scope: ManagedScopeBinding,
    managed_state: ManagedSchemaState,
    projections: BTreeMap<BindingTarget, RuntimeProjection>,
    required_capabilities: CapabilitySet,
    required_extensions: BTreeSet<ExtensionRequirement>,
    resolved: ResolvedSchema,
    semantic_profile: SemanticProfileBinding,
}

impl VerifiedSchemaBundle {
    /// Return the exact bundle-content fingerprint.
    #[must_use]
    pub const fn bundle_fingerprint(&self) -> &Fingerprint {
        &self.bundle_fingerprint
    }

    /// Return constructor-validated direct schema facts with compiled provenance.
    #[must_use]
    pub const fn declared_schema(&self) -> &DeclaredSchema {
        &self.declared
    }

    /// Return independently resolved effective schema semantics.
    #[must_use]
    pub const fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    /// Return the frozen semantic-profile identity and content fingerprint.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileBinding {
        &self.semantic_profile
    }

    /// Return the durable managed scope and frozen scope-profile binding.
    #[must_use]
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        &self.managed_scope
    }

    /// Return the independently recomputed managed schema state.
    #[must_use]
    pub const fn managed_state(&self) -> &ManagedSchemaState {
        &self.managed_state
    }

    /// Return exact compiled capability requirements.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return exact compiled extension requirements.
    #[must_use]
    pub const fn required_extensions(&self) -> &BTreeSet<ExtensionRequirement> {
        &self.required_extensions
    }

    /// Look up one independently verified runtime projection.
    #[must_use]
    pub fn projection(&self, target: BindingTarget) -> Option<&RuntimeProjection> {
        self.projections.get(&target)
    }

    /// Iterate verified projections in stable target order.
    pub fn projections(
        &self,
    ) -> impl ExactSizeIterator<Item = (BindingTarget, &RuntimeProjection)> {
        self.projections.iter().map(|(target, value)| (*target, value))
    }
}

/// Source-free runtime installed only from a verified compiled bundle.
#[derive(Debug)]
pub struct TypeBridgeRuntime {
    bundle: VerifiedSchemaBundle,
}

impl TypeBridgeRuntime {
    /// Decode and independently verify canonical bundle bytes without source access.
    pub fn from_bundle_bytes(
        bytes: &[u8],
        context: &BundleVerificationContext,
    ) -> Result<Self, SchemaBundleError> {
        Ok(Self {
            bundle: decode_verified_schema_bundle(bytes, context)?,
        })
    }

    /// Return the verified bundle fingerprint.
    #[must_use]
    pub const fn bundle_fingerprint(&self) -> &Fingerprint {
        self.bundle.bundle_fingerprint()
    }

    /// Return constructor-validated compiled declarations.
    #[must_use]
    pub const fn declared_schema(&self) -> &DeclaredSchema {
        self.bundle.declared_schema()
    }

    /// Return independently resolved compiled semantics.
    #[must_use]
    pub const fn resolved_schema(&self) -> &ResolvedSchema {
        self.bundle.resolved_schema()
    }

    /// Return the verified semantic-profile binding.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileBinding {
        self.bundle.semantic_profile()
    }

    /// Return the verified durable managed-scope binding.
    #[must_use]
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        self.bundle.managed_scope()
    }

    /// Return the recomputed managed state.
    #[must_use]
    pub const fn managed_state(&self) -> &ManagedSchemaState {
        self.bundle.managed_state()
    }

    /// Return exact runtime capability requirements.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        self.bundle.required_capabilities()
    }

    /// Return exact runtime extension requirements.
    #[must_use]
    pub const fn required_extensions(&self) -> &BTreeSet<ExtensionRequirement> {
        self.bundle.required_extensions()
    }

    /// Look up one verified target projection.
    #[must_use]
    pub fn projection(&self, target: BindingTarget) -> Option<&RuntimeProjection> {
        self.bundle.projection(target)
    }
}

/// Build and immediately reverify one source-free bundle from a source workspace.
pub fn build_verified_schema_bundle(
    workspace: &TypeBridgeWorkspace,
    context: &BundleVerificationContext,
) -> Result<VerifiedSchemaBundle, SchemaBundleError> {
    verify_workspace_context(workspace, context)?;
    let content = content_from_workspace(workspace, context)?;
    let bundle_fingerprint = compute_bundle_fingerprint(&content)?;
    let wire = SchemaBundleWire {
        bundle_fingerprint: canonical_value(&bundle_fingerprint)?,
        content,
    };
    let bytes = to_canonical_json_with_limits(&wire, SCHEMA_BUNDLE_LIMITS)?;
    decode_verified_schema_bundle(&bytes, context)
}

/// Return exact canonical bytes retained by an already verified bundle.
#[must_use]
pub fn encode_verified_schema_bundle(bundle: &VerifiedSchemaBundle) -> Vec<u8> {
    bundle.canonical_bytes.clone()
}

/// Decode, reconstruct, and independently verify one canonical compiled bundle.
pub fn decode_verified_schema_bundle(
    bytes: &[u8],
    context: &BundleVerificationContext,
) -> Result<VerifiedSchemaBundle, SchemaBundleError> {
    let wire: SchemaBundleWire =
        from_canonical_json_with_limits(bytes, SCHEMA_BUNDLE_LIMITS)?;
    let bundle_fingerprint = compute_bundle_fingerprint(&wire.content)?;
    if wire.bundle_fingerprint != canonical_value(&bundle_fingerprint)? {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::IntegrityMismatch,
            "bundle content fingerprint is stale",
        ));
    }
    let content = wire.content;
    if content.bundle_version != TYPEBRIDGE_SCHEMA_BUNDLE_V1
        || content.codec_version != CodecVersion::V1.get()
        || content.schema_ir_version != FormatVersion::V1.get()
    {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::UnsupportedVersion,
            "bundle, codec, or schema-IR version is unsupported",
        ));
    }

    let semantic_profile =
        SemanticProfileBinding::resolve(context.semantic_profile.clone())?;
    require_exact_value(
        &content.semantic_profile,
        &semantic_profile,
        "semantic profile binding differs from verification context",
    )?;
    require_exact_value(
        &content.managed_scope,
        &context.managed_scope,
        "managed scope binding differs from verification context",
    )?;
    content
        .required_capabilities
        .ensure_supported_by(&context.available_capabilities)?;
    let required_extensions = rebuild_extensions(&content.required_extensions)?;
    if !required_extensions.is_subset(&context.available_extensions) {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::ExtensionUnavailable,
            "bundle requires an extension absent from verification context",
        ));
    }

    let declared_bytes =
        to_canonical_json_with_limits(&content.declared_schema, SCHEMA_BUNDLE_LIMITS)?;
    let declared = decode_declared_schema(&declared_bytes)?;
    declared
        .required_capabilities()
        .ensure_supported_by(&content.required_capabilities)?;
    require_exact_value(
        &content.expected_declared_identity,
        declared.declared_identity_fingerprint(),
        "declared schema identity fingerprint is stale",
    )?;
    let resolved = resolve_schema_with_capabilities(
        &declared,
        context.semantic_profile(),
        context.available_capabilities(),
    )?;
    require_exact_value(
        &content.expected_semantic_schema,
        resolved.semantic_fingerprint(),
        "global semantic schema fingerprint is stale",
    )?;

    let managed_context = ManagedDeltaContext::new(
        context.managed_scope.id().clone(),
        context.semantic_profile.clone(),
        context.available_capabilities.clone(),
    );
    let managed_state = managed_schema_state(&declared, &managed_context)?;
    require_exact_value(
        &content.managed_state,
        &managed_state,
        "managed schema state is stale",
    )?;
    require_exact_value(
        &content.expected_managed_declared_identity,
        managed_state.managed_declared_identity(),
        "managed declared-identity fingerprint is stale",
    )?;
    require_exact_value(
        &content.expected_managed_semantic_schema,
        managed_state.managed_semantic_schema(),
        "managed semantic-schema fingerprint is stale",
    )?;

    let projections = verify_projections(&content.projections, &resolved, context)?;
    Ok(VerifiedSchemaBundle {
        bundle_fingerprint,
        canonical_bytes: bytes.to_vec(),
        declared,
        managed_scope: context.managed_scope.clone(),
        managed_state,
        projections,
        required_capabilities: content.required_capabilities,
        required_extensions,
        resolved,
        semantic_profile,
    })
}

fn verify_workspace_context(
    workspace: &TypeBridgeWorkspace,
    context: &BundleVerificationContext,
) -> Result<(), SchemaBundleError> {
    if workspace.config().semantic_profile() != context.semantic_profile()
        || workspace.config().managed_scope() != context.managed_scope()
    {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::ContextMismatch,
            "workspace profile or managed scope differs from bundle context",
        ));
    }
    workspace
        .required_capabilities()
        .ensure_supported_by(context.available_capabilities())?;
    if !workspace
        .config()
        .extensions()
        .is_subset(context.available_extensions())
    {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::ExtensionUnavailable,
            "workspace requires an extension absent from bundle context",
        ));
    }
    let output_targets = workspace
        .config()
        .outputs()
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let context_targets = context.projections.keys().copied().collect::<BTreeSet<_>>();
    if output_targets != context_targets {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::ProjectionTargetMismatch,
            "configured output targets and bundle projection contexts differ",
        ));
    }
    Ok(())
}

fn content_from_workspace(
    workspace: &TypeBridgeWorkspace,
    context: &BundleVerificationContext,
) -> Result<SchemaBundleContentWire, SchemaBundleError> {
    let declared_bytes = encode_declared_schema(workspace.declared_schema())?;
    let declared_schema =
        from_canonical_json_with_limits(&declared_bytes, SCHEMA_BUNDLE_LIMITS)?;
    let semantic_profile =
        SemanticProfileBinding::resolve(context.semantic_profile.clone())?;
    let mut projections = Vec::with_capacity(context.projections.len());
    for (&target, projection_context) in &context.projections {
        let projection = project(
            workspace.resolved_schema(),
            target,
            projection_context.config(),
            projection_context.handlers(),
            &[],
        )?;
        projections.push(ProjectionEntryWire {
            binding_fingerprint: canonical_value(
                projection.projection_fingerprint().as_fingerprint(),
            )?,
            canonical_projection: canonical_value(&projection)?,
            config: canonical_value(projection_context.config())?,
            handler_evidence: canonical_values(projection_context.handlers())?,
            target: target.into(),
        });
    }
    Ok(SchemaBundleContentWire {
        bundle_version: TYPEBRIDGE_SCHEMA_BUNDLE_V1.to_owned(),
        codec_version: CodecVersion::V1.get(),
        declared_schema,
        expected_declared_identity: canonical_value(
            workspace.declared_schema().declared_identity_fingerprint(),
        )?,
        expected_managed_declared_identity: canonical_value(
            workspace.managed_state().managed_declared_identity(),
        )?,
        expected_managed_semantic_schema: canonical_value(
            workspace.managed_state().managed_semantic_schema(),
        )?,
        expected_semantic_schema: canonical_value(
            workspace.resolved_schema().semantic_fingerprint(),
        )?,
        managed_scope: canonical_value(context.managed_scope())?,
        managed_state: canonical_value(workspace.managed_state())?,
        projections,
        required_capabilities: workspace.required_capabilities().clone(),
        required_extensions: workspace
            .config()
            .extensions()
            .iter()
            .map(ExtensionRequirementWire::from_requirement)
            .collect(),
        schema_ir_version: workspace.declared_schema().format().get(),
        semantic_profile: canonical_value(&semantic_profile)?,
    })
}

fn verify_projections(
    entries: &[ProjectionEntryWire],
    resolved: &ResolvedSchema,
    context: &BundleVerificationContext,
) -> Result<BTreeMap<BindingTarget, RuntimeProjection>, SchemaBundleError> {
    let expected_semantic = to_canonical_json_with_limits(
        resolved.semantic_fingerprint().as_fingerprint(),
        SCHEMA_BUNDLE_LIMITS,
    )?;
    let mut projections = BTreeMap::new();
    let mut previous = None;
    for entry in entries {
        let target = entry.target.rebuild();
        if previous.is_some_and(|value| value >= target) {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ProjectionTargetMismatch,
                "bundle projection entries are duplicated or not canonically ordered",
            ));
        }
        previous = Some(target);
        let expected = context.projection(target).ok_or_else(|| {
            SchemaBundleError::new(
                SchemaBundleErrorCode::ProjectionTargetMismatch,
                "bundle contains a projection target absent from context",
            )
        })?;
        if entry.config != canonical_value(expected.config())?
            || entry.handler_evidence != canonical_values(expected.handlers())?
        {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ContextMismatch,
                "projection configuration or handler evidence differs from context",
            ));
        }
        let projection_bytes = to_canonical_json_with_limits(
            &entry.canonical_projection,
            SCHEMA_BUNDLE_LIMITS,
        )?;
        let binding_bytes = to_canonical_json_with_limits(
            &entry.binding_fingerprint,
            SCHEMA_BUNDLE_LIMITS,
        )?;
        let decoded = decode_runtime_projection_verified(
            &projection_bytes,
            &expected_semantic,
            &binding_bytes,
        )
        .map_err(SchemaBundleError::projection)?;
        if decoded.target() != target
            || decoded.config() != expected.config()
            || decoded.generator_handlers() != expected.handlers()
            || !decoded.code_resources().is_empty()
            || entry.binding_fingerprint
                != canonical_value(decoded.projection_fingerprint().as_fingerprint())?
        {
            return Err(SchemaBundleError::new(
                if decoded.code_resources().is_empty() {
                    SchemaBundleErrorCode::ProjectionMismatch
                } else {
                    SchemaBundleErrorCode::UnsupportedProjectionEvidence
                },
                "decoded projection evidence differs from the closed bundle contract",
            ));
        }
        let recomputed = project(
            resolved,
            target,
            expected.config(),
            expected.handlers(),
            &[],
        )?;
        let recomputed_bytes =
            to_canonical_json_with_limits(&recomputed, SCHEMA_BUNDLE_LIMITS)?;
        if projection_bytes != recomputed_bytes || decoded != recomputed {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ProjectionMismatch,
                "projection bytes differ from pure re-projection",
            ));
        }
        projections.insert(target, decoded);
    }
    if projections.keys().copied().collect::<BTreeSet<_>>()
        != context.projections.keys().copied().collect::<BTreeSet<_>>()
    {
        return Err(SchemaBundleError::new(
            SchemaBundleErrorCode::ProjectionTargetMismatch,
            "bundle omits a projection required by verification context",
        ));
    }
    Ok(projections)
}

fn rebuild_extensions(
    wires: &[ExtensionRequirementWire],
) -> Result<BTreeSet<ExtensionRequirement>, SchemaBundleError> {
    let mut extensions = BTreeSet::new();
    let mut previous = None;
    for wire in wires {
        let extension = wire.clone().rebuild()?;
        if previous.as_ref().is_some_and(|value| value >= &extension)
            || !extensions.insert(extension.clone())
        {
            return Err(SchemaBundleError::new(
                SchemaBundleErrorCode::ContextMismatch,
                "bundle extension requirements are duplicated or not canonically ordered",
            ));
        }
        previous = Some(extension);
    }
    Ok(extensions)
}

fn compute_bundle_fingerprint(
    content: &SchemaBundleContentWire,
) -> Result<Fingerprint, SchemaBundleError> {
    let bytes = to_canonical_json_with_limits(content, SCHEMA_BUNDLE_LIMITS)?;
    Ok(Fingerprint::compute(
        FingerprintDomain::new(SCHEMA_BUNDLE_FINGERPRINT_DOMAIN)?,
        CanonicalizationVersion::new(SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION)?,
        None,
        &bytes,
    ))
}

fn require_exact_value<T: Serialize>(
    actual: &Value,
    expected: &T,
    message: &'static str,
) -> Result<(), SchemaBundleError> {
    if actual == &canonical_value(expected)? {
        Ok(())
    } else {
        Err(SchemaBundleError::new(
            SchemaBundleErrorCode::IntegrityMismatch,
            message,
        ))
    }
}

fn canonical_value<T: Serialize>(value: &T) -> Result<Value, SchemaBundleError> {
    let bytes = to_canonical_json_with_limits(value, SCHEMA_BUNDLE_LIMITS)?;
    Ok(from_canonical_json_with_limits(
        &bytes,
        SCHEMA_BUNDLE_LIMITS,
    )?)
}

fn canonical_values<T: Serialize>(values: &[T]) -> Result<Vec<Value>, SchemaBundleError> {
    values.iter().map(canonical_value).collect()
}
