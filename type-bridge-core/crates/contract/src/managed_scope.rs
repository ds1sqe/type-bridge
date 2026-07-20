//! Durable managed-scope identities and frozen profile fingerprints.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::codec::to_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use crate::limits::MAX_CANONICAL_STRING_BYTES;
use crate::semantic_profile::{InterfaceKind, SemanticProfile};

/// The only managed-scope profile supported by the first workspace format.
pub const EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID: &str = "typebridge.managed-scope/exclusive/v1";
/// Fingerprint domain for frozen managed-scope profile definitions.
pub const MANAGED_SCOPE_PROFILE_FINGERPRINT_DOMAIN: &str =
    "typebridge.schema.managed-scope-profile";
/// Canonicalization contract for frozen managed-scope profile definitions.
pub const MANAGED_SCOPE_PROFILE_CANONICALIZATION: &str = "typebridge.managed-scope-profile/v1";
/// Fingerprint domain for frozen semantic-profile definitions.
pub const SEMANTIC_PROFILE_FINGERPRINT_DOMAIN: &str = "typebridge.schema.semantic-profile";
/// Canonicalization contract for frozen semantic-profile definitions.
pub const SEMANTIC_PROFILE_CANONICALIZATION: &str = "typebridge.semantic-profile/v1";

/// A durable deployment-managed schema scope identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagedScopeId(String);

impl ManagedScopeId {
    /// Validate and construct a durable managed-scope identity.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "malformed_managed_scope_id",
                "managed scope ID is empty or exceeds the canonical string limit",
            )
            .with_detail(
                "maximum_bytes",
                i64::try_from(MAX_CANONICAL_STRING_BYTES).unwrap_or(i64::MAX),
            ));
        }
        Ok(Self(value))
    }

    /// Return the canonical identity spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ManagedScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ManagedScopeId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ManagedScopeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A registry-owned managed-scope profile identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagedScopeProfileId(String);

impl ManagedScopeProfileId {
    /// Return the frozen exclusive-profile identity.
    #[must_use]
    pub fn exclusive() -> Self {
        Self(EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID.to_owned())
    }

    /// Validate a managed-scope profile identity against the closed registry.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value != EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID {
            return Err(Diagnostic::stable(
                DiagnosticCategory::UnsupportedCapability,
                "unsupported_managed_scope_profile",
                "managed scope profile is not present in the frozen registry",
            ));
        }
        Ok(Self(value))
    }

    /// Return the canonical profile identity spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ManagedScopeProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ManagedScopeProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ManagedScopeProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Serialize)]
struct ExclusiveManagedScopeProfileView {
    internal_facts: &'static str,
    non_internal_facts: &'static str,
    profile_id: &'static str,
}

/// Return the byte-exact frozen exclusive-profile definition.
pub fn exclusive_managed_scope_profile_bytes() -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(&ExclusiveManagedScopeProfileView {
        internal_facts: "excluded",
        non_internal_facts: "managed",
        profile_id: EXCLUSIVE_MANAGED_SCOPE_PROFILE_ID,
    })
}

/// Content fingerprint of one frozen managed-scope profile definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ManagedScopeProfileFingerprint(Fingerprint);

impl ManagedScopeProfileFingerprint {
    /// Compute the fingerprint of the frozen exclusive profile.
    pub fn exclusive() -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(MANAGED_SCOPE_PROFILE_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(MANAGED_SCOPE_PROFILE_CANONICALIZATION)?,
            None,
            &exclusive_managed_scope_profile_bytes()?,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// A registry-validated managed-scope profile identity and content fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedScopeProfileBinding {
    fingerprint: ManagedScopeProfileFingerprint,
    id: ManagedScopeProfileId,
}

impl ManagedScopeProfileBinding {
    /// Resolve the frozen exclusive managed-scope profile.
    pub fn exclusive() -> Result<Self, Diagnostic> {
        Ok(Self {
            fingerprint: ManagedScopeProfileFingerprint::exclusive()?,
            id: ManagedScopeProfileId::exclusive(),
        })
    }

    /// Return the profile identity.
    pub const fn id(&self) -> &ManagedScopeProfileId {
        &self.id
    }

    /// Return the profile content fingerprint.
    pub const fn fingerprint(&self) -> &ManagedScopeProfileFingerprint {
        &self.fingerprint
    }
}

/// Durable identity binding attached explicitly to one managed fact selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedScopeBinding {
    id: ManagedScopeId,
    profile: ManagedScopeProfileBinding,
}

impl ManagedScopeBinding {
    /// Bind a durable scope identity to the frozen exclusive profile.
    pub fn exclusive(id: ManagedScopeId) -> Result<Self, Diagnostic> {
        Ok(Self {
            id,
            profile: ManagedScopeProfileBinding::exclusive()?,
        })
    }

    /// Return the durable scope identity.
    pub const fn id(&self) -> &ManagedScopeId {
        &self.id
    }

    /// Return the frozen scope-profile binding.
    pub const fn profile(&self) -> &ManagedScopeProfileBinding {
        &self.profile
    }
}

#[derive(Serialize)]
struct SemanticProfileView<'a> {
    id: &'a SemanticProfileId,
    key_owns_default: crate::value::Cardinality,
    owns_default: crate::value::Cardinality,
    plays_default: crate::value::Cardinality,
    relates_default: crate::value::Cardinality,
}

/// Return byte-exact content for one frozen semantic profile.
pub fn semantic_profile_canonical_bytes(profile: &SemanticProfile) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(&SemanticProfileView {
        id: profile.id(),
        key_owns_default: profile.effective_cardinality(InterfaceKind::Owns, None, true),
        owns_default: profile.default_cardinality(InterfaceKind::Owns),
        plays_default: profile.default_cardinality(InterfaceKind::Plays),
        relates_default: profile.default_cardinality(InterfaceKind::Relates),
    })
}

/// Content fingerprint of one frozen semantic-profile definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SemanticProfileFingerprint(Fingerprint);

impl SemanticProfileFingerprint {
    /// Compute a content fingerprint from a resolved frozen profile.
    pub fn compute(profile: &SemanticProfile) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(SEMANTIC_PROFILE_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(SEMANTIC_PROFILE_CANONICALIZATION)?,
            None,
            &semantic_profile_canonical_bytes(profile)?,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// A registry-resolved semantic-profile identity and exact content fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SemanticProfileBinding {
    fingerprint: SemanticProfileFingerprint,
    id: SemanticProfileId,
}

impl SemanticProfileBinding {
    /// Resolve one supported semantic profile and bind its frozen content.
    pub fn resolve(id: SemanticProfileId) -> Result<Self, Diagnostic> {
        let profile = SemanticProfile::resolve(&id)?;
        Ok(Self {
            fingerprint: profile.content_fingerprint()?,
            id,
        })
    }

    /// Resolve the frozen TypeDB 3.12.1 semantic profile.
    pub fn typedb_3_12_1() -> Result<Self, Diagnostic> {
        Self::resolve(SemanticProfileId::new("typedb-3.12.1/v1")?)
    }

    /// Return the exact semantic-profile identity.
    pub const fn id(&self) -> &SemanticProfileId {
        &self.id
    }

    /// Return the registry-owned semantic-profile content fingerprint.
    pub const fn fingerprint(&self) -> &SemanticProfileFingerprint {
        &self.fingerprint
    }
}

impl SemanticProfile {
    /// Compute the content fingerprint of this frozen profile definition.
    pub fn content_fingerprint(&self) -> Result<SemanticProfileFingerprint, Diagnostic> {
        SemanticProfileFingerprint::compute(self)
    }
}
