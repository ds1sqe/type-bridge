//! Validated identity and fingerprint primitives for schema-lowering profiles.

use std::error::Error;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::codec::from_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};

/// Schema-lowering profile for the TypeDB 3.12.1 semantic profile.
pub const TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID: &str = "typedb-3.12.1-schema-lowering/v1";
/// Schema-lowering profile for the TypeDB 3.13.6 semantic profile.
pub const TYPEDB_3_13_6_SCHEMA_LOWERING_PROFILE_ID: &str = "typedb-3.13.6-schema-lowering/v1";
/// Fingerprint domain for schema-lowering profile documents.
pub const SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN: &str = "typebridge.schema.lowering-profile";
/// Canonicalization version for schema-lowering profile documents.
pub const SCHEMA_LOWERING_PROFILE_CANONICALIZATION: &str = "typebridge.schema-lowering-profile/v1";

/// The semantic-to-lowering profile mapping; this is its only definition.
///
/// `typedb-3.11.5/v1` has no registry of its own and keeps lowering through
/// the 3.12.1 registry, as its existing manifests are bound to it.
const SCHEMA_LOWERING_PROFILES: [(&str, &str); 3] = [
    ("typedb-3.11.5/v1", TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID),
    ("typedb-3.12.1/v1", TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID),
    ("typedb-3.13.6/v1", TYPEDB_3_13_6_SCHEMA_LOWERING_PROFILE_ID),
];

/// Validation failure for a schema-lowering profile identity or fingerprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoweringProfileValidationError {
    message: String,
}

impl SchemaLoweringProfileValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SchemaLoweringProfileValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SchemaLoweringProfileValidationError {}

/// A validated schema-lowering profile identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SchemaLoweringProfileId(String);

impl SchemaLoweringProfileId {
    /// Validates one of the closed V2 schema-lowering profile identifiers.
    pub fn new(value: impl Into<String>) -> Result<Self, SchemaLoweringProfileValidationError> {
        let value = value.into();
        SemanticProfileId::new(value.clone()).map_err(|error| {
            SchemaLoweringProfileValidationError::new(format!(
                "invalid schema-lowering profile id: {error}"
            ))
        })?;
        if !SCHEMA_LOWERING_PROFILES
            .iter()
            .any(|(_, lowering)| *lowering == value)
        {
            return Err(SchemaLoweringProfileValidationError::new(format!(
                "unsupported schema-lowering profile id: {value}"
            )));
        }
        Ok(Self(value))
    }

    /// Returns the fixed TypeDB 3.12.1 profile identifier.
    pub fn typedb_3_12_1() -> Self {
        Self::new(TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID)
            .expect("the fixed schema-lowering profile id is valid")
    }

    /// Returns the fixed TypeDB 3.13.6 profile identifier.
    pub fn typedb_3_13_6() -> Self {
        Self::new(TYPEDB_3_13_6_SCHEMA_LOWERING_PROFILE_ID)
            .expect("the fixed schema-lowering profile id is valid")
    }

    /// Returns the lowering profile for a semantic profile, if one exists.
    pub fn for_semantic_profile(semantic_profile: &SemanticProfileId) -> Option<Self> {
        SCHEMA_LOWERING_PROFILES
            .iter()
            .find(|(semantic, _)| *semantic == semantic_profile.as_str())
            .map(|(_, lowering)| Self((*lowering).to_owned()))
    }

    /// Returns the wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaLoweringProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SchemaLoweringProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SchemaLoweringProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// A fingerprint whose metadata is pinned to the schema-lowering domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoweringProfileFingerprint(Fingerprint);

impl SchemaLoweringProfileFingerprint {
    /// Fingerprints canonical schema-lowering profile bytes under the
    /// TypeDB 3.12.1 lowering profile, the only profile 2.2 compiled.
    #[deprecated(
        since = "2.3.0",
        note = "scopes the fingerprint to the 3.12.1 profile only; use compute_for_profile; removed in 3.0"
    )]
    pub fn compute(canonical_bytes: &[u8]) -> Self {
        let profile_id = SchemaLoweringProfileId::new(TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID)
            .expect("the fixed 3.12.1 schema-lowering profile id is valid");
        Self::compute_for_profile(&profile_id, canonical_bytes)
    }

    /// Fingerprints canonical schema-lowering profile bytes with the fixed
    /// metadata; the lowering profile id is the fingerprint's profile scope.
    pub fn compute_for_profile(
        profile_id: &SchemaLoweringProfileId,
        canonical_bytes: &[u8],
    ) -> Self {
        let domain = FingerprintDomain::new(SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN)
            .expect("the fixed schema-lowering fingerprint domain is valid");
        let canonicalization =
            CanonicalizationVersion::new(SCHEMA_LOWERING_PROFILE_CANONICALIZATION)
                .expect("the fixed schema-lowering canonicalization is valid");
        let profile = SemanticProfileId::new(profile_id.as_str())
            .expect("the fixed schema-lowering profile id is a valid semantic profile id");
        Self(Fingerprint::compute(
            domain,
            canonicalization,
            Some(profile),
            canonical_bytes,
        ))
    }

    /// Borrows the generic contract fingerprint.
    pub fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    fn validate_metadata(
        fingerprint: &Fingerprint,
    ) -> Result<(), SchemaLoweringProfileValidationError> {
        let value = serde_json::to_value(fingerprint).map_err(|error| {
            SchemaLoweringProfileValidationError::new(format!(
                "cannot inspect schema-lowering fingerprint: {error}"
            ))
        })?;
        let expected = [
            ("domain", SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN),
            ("canonicalization", SCHEMA_LOWERING_PROFILE_CANONICALIZATION),
        ];
        for (field, expected_value) in expected {
            if value.get(field).and_then(serde_json::Value::as_str) != Some(expected_value) {
                return Err(SchemaLoweringProfileValidationError::new(format!(
                    "schema-lowering fingerprint has invalid {field}"
                )));
            }
        }
        let profile = value
            .get("semantic_profile")
            .and_then(serde_json::Value::as_str);
        if profile.is_none_or(|profile| SchemaLoweringProfileId::new(profile).is_err()) {
            return Err(SchemaLoweringProfileValidationError::new(
                "schema-lowering fingerprint has invalid semantic_profile",
            ));
        }
        Ok(())
    }
}

impl Serialize for SchemaLoweringProfileFingerprint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SchemaLoweringProfileFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fingerprint = Fingerprint::deserialize(deserializer)?;
        Self::validate_metadata(&fingerprint).map_err(D::Error::custom)?;
        Ok(Self(fingerprint))
    }
}

/// A registry-resolved schema-lowering profile and exact content fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaLoweringProfileBinding {
    fingerprint: SchemaLoweringProfileFingerprint,
    id: SchemaLoweringProfileId,
}

impl SchemaLoweringProfileBinding {
    /// Bind a closed profile identity to a complete canonical registry document.
    ///
    /// This validates canonical JSON and requires the document's top-level `id`
    /// to be one of the closed profile identities before computing its content
    /// fingerprint under that identity.
    pub fn from_canonical_profile_bytes(
        canonical_profile_bytes: &[u8],
    ) -> Result<Self, Diagnostic> {
        let value: serde_json::Value = from_canonical_json(canonical_profile_bytes)?;
        let Some(id) = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| SchemaLoweringProfileId::new(id).ok())
        else {
            return Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "schema_lowering_profile_id_mismatch",
                "canonical schema-lowering profile bytes do not carry a frozen profile id",
            ));
        };
        Ok(Self {
            fingerprint: SchemaLoweringProfileFingerprint::compute_for_profile(
                &id,
                canonical_profile_bytes,
            ),
            id,
        })
    }

    /// Return the exact lowering-profile identity.
    pub const fn id(&self) -> &SchemaLoweringProfileId {
        &self.id
    }

    /// Return the registry-owned lowering-profile content fingerprint.
    pub const fn fingerprint(&self) -> &SchemaLoweringProfileFingerprint {
        &self.fingerprint
    }
}
