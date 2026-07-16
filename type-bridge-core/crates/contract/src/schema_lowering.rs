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

/// The only schema-lowering profile admitted by this V2 contract revision.
pub const TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID: &str =
    "typedb-3.12.1-schema-lowering/v1";
/// Fingerprint domain for schema-lowering profile documents.
pub const SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN: &str =
    "typebridge.schema.lowering-profile";
/// Canonicalization version for schema-lowering profile documents.
pub const SCHEMA_LOWERING_PROFILE_CANONICALIZATION: &str =
    "typebridge.schema-lowering-profile/v1";

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
    /// Validates the fixed V2 schema-lowering profile identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, SchemaLoweringProfileValidationError> {
        let value = value.into();
        SemanticProfileId::new(value.clone()).map_err(|error| {
            SchemaLoweringProfileValidationError::new(format!(
                "invalid schema-lowering profile id: {error}"
            ))
        })?;
        if value != TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID {
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
    /// Fingerprints canonical schema-lowering profile bytes with the fixed metadata.
    pub fn compute(canonical_bytes: &[u8]) -> Self {
        let domain = FingerprintDomain::new(SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN)
            .expect("the fixed schema-lowering fingerprint domain is valid");
        let canonicalization =
            CanonicalizationVersion::new(SCHEMA_LOWERING_PROFILE_CANONICALIZATION)
                .expect("the fixed schema-lowering canonicalization is valid");
        let profile = SemanticProfileId::new(TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID)
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
            ("semantic_profile", TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID),
        ];
        for (field, expected_value) in expected {
            if value.get(field).and_then(serde_json::Value::as_str) != Some(expected_value) {
                return Err(SchemaLoweringProfileValidationError::new(format!(
                    "schema-lowering fingerprint has invalid {field}"
                )));
            }
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
    /// Bind the fixed profile identity to a complete canonical registry document.
    ///
    /// This validates canonical JSON and requires the document's top-level `id`
    /// to equal the frozen profile identity before computing its content fingerprint.
    pub fn from_canonical_profile_bytes(
        canonical_profile_bytes: &[u8],
    ) -> Result<Self, Diagnostic> {
        let value: serde_json::Value = from_canonical_json(canonical_profile_bytes)?;
        if value.get("id").and_then(serde_json::Value::as_str)
            != Some(TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID)
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "schema_lowering_profile_id_mismatch",
                "canonical schema-lowering profile bytes do not carry the frozen profile id",
            ));
        }
        Ok(Self {
            fingerprint: SchemaLoweringProfileFingerprint::compute(canonical_profile_bytes),
            id: SchemaLoweringProfileId::typedb_3_12_1(),
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
