//! Constructor-verified, source-free schema authority artifacts.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{
    CodecVersion, FormatVersion, from_canonical_json, to_canonical_json,
};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::limits::MAX_CANONICAL_BYTES;
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, ManagedScopeProfileId, SemanticProfileBinding,
};
use type_bridge_contract::migration::CONDITIONAL_RESOLUTION_CAPABILITY;
use type_bridge_contract::migration_assertion_capability_vocabulary;
use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
use type_bridge_contract::schema::{
    DeclaredSchema, ManagedSchemaState, SchemaDiagnostics, decode_declared_schema,
    encode_declared_schema,
};
use type_bridge_contract::schema_delta::{
    SCHEMA_REDEFINE_CAPABILITY, schema_transition_capability_vocabulary,
};

use crate::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, DeltaError, ManagedDeltaContext, ResolvedSchema,
    managed_schema_state, resolve_schema_with_capabilities,
};

/// The first source-free compiled schema-authority envelope.
pub const TYPEBRIDGE_SCHEMA_AUTHORITY_V1: &str = "typebridge.schema-authority/v1";
/// Fingerprint domain for one exact canonical schema-authority content envelope.
pub const SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN: &str = "typebridge.schema.authority";
/// Canonicalization identity for the first schema-authority content envelope.
pub const SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION: &str = "typebridge.schema-authority/v1";
/// Maximum canonical schema-authority size under the shared contract codec.
pub const MAX_SCHEMA_AUTHORITY_BYTES: usize = MAX_CANONICAL_BYTES;

/// Return every capability understood by generated packages and the generic server.
///
/// The authority may bind query, schema-resolution, and workspace migration
/// requirements. Consumers use this closed vocabulary only to reconstruct and
/// validate that envelope; execution surfaces still advertise their narrower
/// operation-specific capabilities.
#[must_use]
pub fn schema_authority_capability_vocabulary() -> CapabilitySet {
    let mut capabilities = query_plan_v2_capability_vocabulary();
    for capability in schema_transition_capability_vocabulary()
        .into_iter()
        .chain(migration_assertion_capability_vocabulary())
    {
        capabilities.insert(capability);
    }
    for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
        capabilities.insert(
            CapabilityId::new(*capability).expect("built-in schema capability ID is canonical"),
        );
    }
    for capability in [
        SCHEMA_REDEFINE_CAPABILITY,
        CONDITIONAL_RESOLUTION_CAPABILITY,
    ] {
        capabilities.insert(
            CapabilityId::new(capability).expect("static authority capability ID is canonical"),
        );
    }
    capabilities
}

/// Stable high-level classification for schema-authority failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaAuthorityErrorCode {
    /// Canonical bytes or a nested contract value are malformed.
    Contract,
    /// Declared-schema construction or semantic resolution failed.
    Schema,
    /// The authority, codec, or schema-IR version is unsupported.
    UnsupportedVersion,
    /// A required capability is absent from the available capability set.
    UnsupportedCapability,
    /// A canonical structural ceiling was exceeded.
    ResourceLimit,
    /// A bound fingerprint, scope, profile, or state is stale.
    IntegrityMismatch,
}

/// A fail-closed schema-authority failure retaining nested diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaAuthorityError {
    code: SchemaAuthorityErrorCode,
    message: &'static str,
    contract: Option<Box<Diagnostic>>,
    schema: Option<Box<SchemaDiagnostics>>,
}

impl SchemaAuthorityError {
    fn new(code: SchemaAuthorityErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            contract: None,
            schema: None,
        }
    }

    /// Return the stable high-level failure classification.
    #[must_use]
    pub const fn code(&self) -> SchemaAuthorityErrorCode {
        self.code
    }

    /// Return the nested contract diagnostic, when one caused the failure.
    #[must_use]
    pub fn contract(&self) -> Option<&Diagnostic> {
        self.contract.as_deref()
    }

    /// Return nested schema diagnostics, when reconstruction or resolution failed.
    #[must_use]
    pub fn schema(&self) -> Option<&SchemaDiagnostics> {
        self.schema.as_deref()
    }
}

impl fmt::Display for SchemaAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for SchemaAuthorityError {}

impl From<Diagnostic> for SchemaAuthorityError {
    fn from(value: Diagnostic) -> Self {
        let code = match value.category() {
            DiagnosticCategory::InvalidContract => SchemaAuthorityErrorCode::Contract,
            DiagnosticCategory::UnsupportedCapability => {
                SchemaAuthorityErrorCode::UnsupportedCapability
            }
            DiagnosticCategory::ResourceLimit => SchemaAuthorityErrorCode::ResourceLimit,
            DiagnosticCategory::Integrity => SchemaAuthorityErrorCode::IntegrityMismatch,
        };
        Self {
            code,
            message: "a schema-authority contract is invalid",
            contract: Some(Box::new(value)),
            schema: None,
        }
    }
}

impl From<SchemaDiagnostics> for SchemaAuthorityError {
    fn from(value: SchemaDiagnostics) -> Self {
        Self {
            code: SchemaAuthorityErrorCode::Schema,
            message: "schema-authority reconstruction failed",
            contract: None,
            schema: Some(Box::new(value)),
        }
    }
}

impl From<DeltaError> for SchemaAuthorityError {
    fn from(value: DeltaError) -> Self {
        match value {
            DeltaError::Contract(error) => error.into(),
            DeltaError::Schema(error) => error.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SemanticProfileBindingWire {
    fingerprint: Value,
    id: SemanticProfileId,
}

impl SemanticProfileBindingWire {
    fn from_binding(binding: &SemanticProfileBinding) -> Result<Self, SchemaAuthorityError> {
        Ok(Self {
            fingerprint: canonical_value(binding.fingerprint().as_fingerprint())?,
            id: binding.id().clone(),
        })
    }

    fn rebuild(self) -> Result<SemanticProfileBinding, SchemaAuthorityError> {
        let trusted = SemanticProfileBinding::resolve(self.id.clone())?;
        require_exact_value(
            &self.fingerprint,
            trusted.fingerprint().as_fingerprint(),
            "semantic-profile fingerprint is stale",
        )?;
        Ok(trusted)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedScopeProfileBindingWire {
    fingerprint: Value,
    id: ManagedScopeProfileId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedScopeBindingWire {
    id: ManagedScopeId,
    profile: ManagedScopeProfileBindingWire,
}

impl ManagedScopeBindingWire {
    fn from_binding(binding: &ManagedScopeBinding) -> Result<Self, SchemaAuthorityError> {
        Ok(Self {
            id: binding.id().clone(),
            profile: ManagedScopeProfileBindingWire {
                fingerprint: canonical_value(binding.profile().fingerprint().as_fingerprint())?,
                id: binding.profile().id().clone(),
            },
        })
    }

    fn rebuild(self) -> Result<ManagedScopeBinding, SchemaAuthorityError> {
        let trusted = ManagedScopeBinding::exclusive(self.id.clone())?;
        if self.profile.id != *trusted.profile().id() {
            return Err(integrity_failure("managed-scope profile identity is stale"));
        }
        require_exact_value(
            &self.profile.fingerprint,
            trusted.profile().fingerprint().as_fingerprint(),
            "managed-scope profile fingerprint is stale",
        )?;
        Ok(trusted)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaAuthorityContentWire {
    authority_version: String,
    codec_version: u16,
    declared_identity: Value,
    declared_schema: Value,
    managed_scope: ManagedScopeBindingWire,
    managed_state: Value,
    required_capabilities: CapabilitySet,
    schema_ir_version: u16,
    semantic_profile: SemanticProfileBindingWire,
    semantic_schema: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaAuthorityWire {
    authority_fingerprint: Value,
    content: SchemaAuthorityContentWire,
}

/// Opaque source-free authority accepted only after complete reconstruction.
#[derive(Clone, Debug)]
pub struct VerifiedSchemaAuthority {
    authority_fingerprint: Fingerprint,
    canonical_bytes: Vec<u8>,
    declared: DeclaredSchema,
    managed_scope: ManagedScopeBinding,
    managed_state: ManagedSchemaState,
    required_capabilities: CapabilitySet,
    resolved: ResolvedSchema,
    semantic_profile: SemanticProfileBinding,
}

impl VerifiedSchemaAuthority {
    /// Return the exact fingerprint of the authority content envelope.
    #[must_use]
    pub const fn authority_fingerprint(&self) -> &Fingerprint {
        &self.authority_fingerprint
    }

    /// Return the constructor-validated direct declaration.
    #[must_use]
    pub const fn declared_schema(&self) -> &DeclaredSchema {
        &self.declared
    }

    /// Return independently reconstructed effective schema semantics.
    #[must_use]
    pub const fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    /// Return the registry-resolved semantic-profile identity and content binding.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileBinding {
        &self.semantic_profile
    }

    /// Return the registry-resolved durable managed-scope binding.
    #[must_use]
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        &self.managed_scope
    }

    /// Return the independently reconstructed managed schema state.
    #[must_use]
    pub const fn managed_state(&self) -> &ManagedSchemaState {
        &self.managed_state
    }

    /// Return the exact additive capability requirements bound by this artifact.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }
}

/// Build and immediately reverify one source-free schema authority.
///
/// `required_capabilities` may add workspace/runtime requirements to the
/// declaration's own requirements, but it may neither omit a declaration
/// requirement nor claim a capability absent from `context`.
pub fn build_schema_authority(
    declared: &DeclaredSchema,
    required_capabilities: &CapabilitySet,
    context: &ManagedDeltaContext,
) -> Result<VerifiedSchemaAuthority, SchemaAuthorityError> {
    declared
        .required_capabilities()
        .ensure_supported_by(required_capabilities)?;
    required_capabilities.ensure_supported_by(context.available_capabilities())?;

    let semantic_profile = SemanticProfileBinding::resolve(context.semantic_profile().clone())?;
    let managed_scope = ManagedScopeBinding::exclusive(context.scope_id().clone())
        .map_err(SchemaAuthorityError::from)?;
    let resolved = resolve_schema_with_capabilities(
        declared,
        context.semantic_profile(),
        context.available_capabilities(),
    )?;
    let managed_state = managed_schema_state(declared, context)?;
    let declared_bytes = encode_declared_schema(declared)?;
    let content = SchemaAuthorityContentWire {
        authority_version: TYPEBRIDGE_SCHEMA_AUTHORITY_V1.to_owned(),
        codec_version: CodecVersion::V1.get(),
        declared_identity: canonical_value(declared.declared_identity_fingerprint())?,
        declared_schema: from_canonical_json(&declared_bytes)?,
        managed_scope: ManagedScopeBindingWire::from_binding(&managed_scope)?,
        managed_state: canonical_value(&managed_state)?,
        required_capabilities: required_capabilities.clone(),
        schema_ir_version: declared.format().get(),
        semantic_profile: SemanticProfileBindingWire::from_binding(&semantic_profile)?,
        semantic_schema: canonical_value(resolved.semantic_fingerprint())?,
    };
    let authority_fingerprint = compute_authority_fingerprint(&content)?;
    let wire = SchemaAuthorityWire {
        authority_fingerprint: canonical_value(&authority_fingerprint)?,
        content,
    };
    let bytes = to_canonical_json(&wire)?;
    decode_schema_authority(&bytes, context.available_capabilities())
}

/// Return exact canonical bytes retained by an already verified authority.
#[must_use]
pub fn encode_schema_authority(authority: &VerifiedSchemaAuthority) -> Vec<u8> {
    authority.canonical_bytes.clone()
}

/// Decode canonical bytes and independently reconstruct every bound schema view.
///
/// The caller supplies only its available capability set. Scope and semantic
/// profile come from the artifact and are accepted only through their frozen
/// registries; no source workspace or repeated deployment configuration is
/// consulted.
pub fn decode_schema_authority(
    bytes: &[u8],
    available_capabilities: &CapabilitySet,
) -> Result<VerifiedSchemaAuthority, SchemaAuthorityError> {
    let wire: SchemaAuthorityWire = from_canonical_json(bytes)?;
    if to_canonical_json(&wire)? != bytes {
        return Err(SchemaAuthorityError::new(
            SchemaAuthorityErrorCode::Contract,
            "schema-authority bytes normalize after typed reconstruction",
        ));
    }
    if wire.content.authority_version != TYPEBRIDGE_SCHEMA_AUTHORITY_V1
        || wire.content.codec_version != CodecVersion::V1.get()
        || wire.content.schema_ir_version != FormatVersion::V1.get()
    {
        return Err(SchemaAuthorityError::new(
            SchemaAuthorityErrorCode::UnsupportedVersion,
            "schema-authority, codec, or schema-IR version is unsupported",
        ));
    }

    let authority_fingerprint = compute_authority_fingerprint(&wire.content)?;
    require_exact_value(
        &wire.authority_fingerprint,
        &authority_fingerprint,
        "schema-authority content fingerprint is stale",
    )?;

    let SchemaAuthorityContentWire {
        authority_version: _,
        codec_version: _,
        declared_identity,
        declared_schema,
        managed_scope,
        managed_state,
        required_capabilities,
        schema_ir_version: _,
        semantic_profile,
        semantic_schema,
    } = wire.content;

    required_capabilities.ensure_supported_by(available_capabilities)?;
    let semantic_profile = semantic_profile.rebuild()?;
    let managed_scope = managed_scope.rebuild()?;
    let declared_bytes = to_canonical_json(&declared_schema)?;
    let declared = decode_declared_schema(&declared_bytes)?;
    declared
        .required_capabilities()
        .ensure_supported_by(&required_capabilities)?;
    require_exact_value(
        &declared_identity,
        declared.declared_identity_fingerprint(),
        "declared-schema identity fingerprint is stale",
    )?;

    let resolved =
        resolve_schema_with_capabilities(&declared, semantic_profile.id(), available_capabilities)?;
    require_exact_value(
        &semantic_schema,
        resolved.semantic_fingerprint(),
        "global semantic-schema fingerprint is stale",
    )?;

    let managed_context = ManagedDeltaContext::new(
        managed_scope.id().clone(),
        semantic_profile.id().clone(),
        available_capabilities.clone(),
    );
    let rebuilt_managed_state = managed_schema_state(&declared, &managed_context)?;
    require_exact_value(
        &managed_state,
        &rebuilt_managed_state,
        "managed schema state is stale",
    )?;

    Ok(VerifiedSchemaAuthority {
        authority_fingerprint,
        canonical_bytes: bytes.to_vec(),
        declared,
        managed_scope,
        managed_state: rebuilt_managed_state,
        required_capabilities,
        resolved,
        semantic_profile,
    })
}

fn compute_authority_fingerprint(
    content: &SchemaAuthorityContentWire,
) -> Result<Fingerprint, SchemaAuthorityError> {
    Ok(Fingerprint::compute(
        FingerprintDomain::new(SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN)?,
        CanonicalizationVersion::new(SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION)?,
        None,
        &to_canonical_json(content)?,
    ))
}

fn canonical_value<T: Serialize>(value: &T) -> Result<Value, SchemaAuthorityError> {
    Ok(from_canonical_json(&to_canonical_json(value)?)?)
}

fn require_exact_value<T: Serialize>(
    actual: &Value,
    expected: &T,
    message: &'static str,
) -> Result<(), SchemaAuthorityError> {
    if actual == &canonical_value(expected)? {
        Ok(())
    } else {
        Err(integrity_failure(message))
    }
}

fn integrity_failure(message: &'static str) -> SchemaAuthorityError {
    SchemaAuthorityError::new(SchemaAuthorityErrorCode::IntegrityMismatch, message)
}
