use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{
    from_canonical_json_with_limits, to_canonical_json_with_limits,
};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain,
};
use type_bridge_contract::limits::CodecLimits;
use type_bridge_contract::managed_scope::SemanticProfileBinding;

use crate::{TypeBridgeConfig, TypeBridgeWorkspace};

/// The exact first canonical workspace lock format.
pub const TYPEBRIDGE_WORKSPACE_LOCK_V1: &str = "typebridge.workspace-lock/v1";
/// Maximum accepted canonical workspace lock bytes.
pub const MAX_WORKSPACE_LOCK_BYTES: usize = 1024 * 1024;

const WORKSPACE_CONFIG_IDENTITY_DOMAIN: &str = "typebridge.workspace.config-identity";
const WORKSPACE_CONFIG_IDENTITY_CANONICALIZATION: &str =
    "typebridge.workspace-config-identity/v1";
const WORKSPACE_LOCK_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_WORKSPACE_LOCK_BYTES,
    max_depth: 16,
    max_collection_len: 65_536,
    max_string_bytes: 4_096,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LockSourceWire {
    fingerprint: Fingerprint,
    path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthoredConfigWire {
    fingerprint: Fingerprint,
    path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceLockWire {
    authored_config: Option<AuthoredConfigWire>,
    bound_scope_id: String,
    declared_identity_fingerprint: Fingerprint,
    derived_capability_requirements: CapabilitySet,
    discovery_version: String,
    document_set_fingerprint: Fingerprint,
    lock_version: String,
    managed_declared_identity_fingerprint: Fingerprint,
    managed_scope_profile_fingerprint: Fingerprint,
    managed_scope_profile_id: String,
    managed_semantic_schema_fingerprint: Fingerprint,
    resolved_config_identity: Fingerprint,
    schema_set_manifest_fingerprint: Fingerprint,
    semantic_profile_fingerprint: Fingerprint,
    semantic_profile_id: String,
    semantic_schema_fingerprint: Fingerprint,
    sources: Vec<LockSourceWire>,
}

#[derive(Serialize)]
struct ResolvedConfigIdentityView {
    app_label: String,
    configured_capability_requirements: Vec<String>,
    managed_scope_id: String,
    managed_scope_profile_id: String,
    migration_v2_directory: String,
    schema_set: String,
    semantic_profile_id: String,
}

/// Stable error categories for explicit workspace lock generation and verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkspaceLockErrorCode {
    /// Canonical encoding, decoding, limits, or fingerprint construction failed.
    Contract,
    /// The decoded lock format is not supported.
    UnsupportedVersion,
    /// Canonical claims differ from the supplied source workspace.
    Stale,
}

/// A fail-closed workspace lock failure.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkspaceLockError {
    /// Canonical codec or fingerprint contract failure.
    Contract(Diagnostic),
    /// The lock format discriminator is unsupported.
    UnsupportedVersion,
    /// At least one lock claim is stale or tampered.
    Stale,
}

impl WorkspaceLockError {
    /// Return the stable failure category.
    #[must_use]
    pub const fn code(&self) -> WorkspaceLockErrorCode {
        match self {
            Self::Contract(_) => WorkspaceLockErrorCode::Contract,
            Self::UnsupportedVersion => WorkspaceLockErrorCode::UnsupportedVersion,
            Self::Stale => WorkspaceLockErrorCode::Stale,
        }
    }

    /// Return the nested contract diagnostic, if present.
    #[must_use]
    pub const fn contract(&self) -> Option<&Diagnostic> {
        match self {
            Self::Contract(error) => Some(error),
            Self::UnsupportedVersion | Self::Stale => None,
        }
    }
}

impl fmt::Display for WorkspaceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "workspace lock contract failed: {error}"),
            Self::UnsupportedVersion => formatter.write_str("workspace lock version is unsupported"),
            Self::Stale => formatter.write_str("workspace lock does not match the source workspace"),
        }
    }
}

impl Error for WorkspaceLockError {}

impl From<Diagnostic> for WorkspaceLockError {
    fn from(value: Diagnostic) -> Self {
        Self::Contract(value)
    }
}

/// Canonical current lock bytes produced explicitly from one source workspace.
///
/// This opaque value has no deserialization or persistence API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceLock {
    bytes: Vec<u8>,
}

impl WorkspaceLock {
    /// Return canonical lock bytes for caller-owned persistence.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Canonical lock bytes verified against one supplied source workspace.
///
/// Construction is possible only through [`verify_workspace_lock`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedWorkspaceLock {
    lock: WorkspaceLock,
}

impl VerifiedWorkspaceLock {
    /// Return the verified canonical lock artifact.
    #[must_use]
    pub const fn lock(&self) -> &WorkspaceLock {
        &self.lock
    }

    /// Return verified canonical bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.lock.bytes()
    }
}

/// Explicitly generate canonical current lock bytes without writing them.
pub fn generate_workspace_lock(
    workspace: &TypeBridgeWorkspace,
) -> Result<WorkspaceLock, WorkspaceLockError> {
    let wire = derive_lock_wire(workspace)?;
    let bytes = to_canonical_json_with_limits(&wire, WORKSPACE_LOCK_LIMITS)?;
    Ok(WorkspaceLock { bytes })
}

/// Verify canonical lock bytes against a freshly constructed source workspace.
pub fn verify_workspace_lock(
    bytes: &[u8],
    workspace: &TypeBridgeWorkspace,
) -> Result<VerifiedWorkspaceLock, WorkspaceLockError> {
    let decoded: WorkspaceLockWire =
        from_canonical_json_with_limits(bytes, WORKSPACE_LOCK_LIMITS)?;
    if decoded.lock_version != TYPEBRIDGE_WORKSPACE_LOCK_V1 {
        return Err(WorkspaceLockError::UnsupportedVersion);
    }
    let expected = derive_lock_wire(workspace)?;
    if decoded != expected {
        return Err(WorkspaceLockError::Stale);
    }
    Ok(VerifiedWorkspaceLock {
        lock: WorkspaceLock {
            bytes: bytes.to_vec(),
        },
    })
}

fn derive_lock_wire(
    workspace: &TypeBridgeWorkspace,
) -> Result<WorkspaceLockWire, WorkspaceLockError> {
    let evidence = workspace.discovery_evidence();
    let managed_state = workspace.managed_state();
    let scope = workspace.bound_managed_scope().binding();
    let semantic_profile =
        SemanticProfileBinding::resolve(workspace.config().semantic_profile().clone())?;
    let authored_config = workspace.located_config().map(|located| AuthoredConfigWire {
        fingerprint: located.spec().fingerprint().as_fingerprint().clone(),
        path: located
            .origin()
            .manifest_path()
            .to_str()
            .expect("ConfigOrigin validates UTF-8 paths")
            .to_owned(),
    });
    let sources = evidence
        .sources()
        .iter()
        .map(|source| LockSourceWire {
            fingerprint: source.fingerprint().as_fingerprint().clone(),
            path: source.path().as_str().to_owned(),
        })
        .collect();

    Ok(WorkspaceLockWire {
        authored_config,
        bound_scope_id: scope.id().as_str().to_owned(),
        declared_identity_fingerprint: managed_state
            .declared_identity()
            .as_fingerprint()
            .clone(),
        derived_capability_requirements: workspace.required_capabilities().clone(),
        discovery_version: evidence.discovery_version().as_str().to_owned(),
        document_set_fingerprint: evidence
            .document_set_fingerprint()
            .as_fingerprint()
            .clone(),
        lock_version: TYPEBRIDGE_WORKSPACE_LOCK_V1.to_owned(),
        managed_declared_identity_fingerprint: managed_state
            .managed_declared_identity()
            .as_fingerprint()
            .clone(),
        managed_scope_profile_fingerprint: scope
            .profile()
            .fingerprint()
            .as_fingerprint()
            .clone(),
        managed_scope_profile_id: scope.profile().id().as_str().to_owned(),
        managed_semantic_schema_fingerprint: managed_state
            .managed_semantic_schema()
            .as_fingerprint()
            .clone(),
        resolved_config_identity: resolved_config_identity(workspace.config())?,
        schema_set_manifest_fingerprint: evidence
            .manifest_fingerprint()
            .as_fingerprint()
            .clone(),
        semantic_profile_fingerprint: semantic_profile
            .fingerprint()
            .as_fingerprint()
            .clone(),
        semantic_profile_id: semantic_profile.id().as_str().to_owned(),
        semantic_schema_fingerprint: workspace
            .resolved_schema()
            .semantic_fingerprint()
            .as_fingerprint()
            .clone(),
        sources,
    })
}

fn resolved_config_identity(config: &TypeBridgeConfig) -> Result<Fingerprint, WorkspaceLockError> {
    let view = ResolvedConfigIdentityView {
        app_label: config.app_label().as_str().to_owned(),
        configured_capability_requirements: config
            .required_capabilities()
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect(),
        managed_scope_id: config.managed_scope().id().as_str().to_owned(),
        managed_scope_profile_id: config
            .managed_scope()
            .profile()
            .id()
            .as_str()
            .to_owned(),
        migration_v2_directory: config
            .migration_v2_directory()
            .as_path()
            .to_str()
            .expect("MigrationV2Directory validates UTF-8 paths")
            .to_owned(),
        schema_set: config
            .schema_set()
            .as_path()
            .to_str()
            .expect("SchemaSetPath validates UTF-8 paths")
            .to_owned(),
        semantic_profile_id: config.semantic_profile().as_str().to_owned(),
    };
    let bytes = to_canonical_json_with_limits(&view, WORKSPACE_LOCK_LIMITS)?;
    Ok(Fingerprint::compute(
        FingerprintDomain::new(WORKSPACE_CONFIG_IDENTITY_DOMAIN)?,
        CanonicalizationVersion::new(WORKSPACE_CONFIG_IDENTITY_CANONICALIZATION)?,
        None,
        &bytes,
    ))
}
