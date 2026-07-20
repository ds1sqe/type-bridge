//! Domain-separated fingerprints for schema comparison scopes.

use serde::Serialize;

use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};

const SCHEMA_CANONICALIZATION: &str = "typebridge.schema-canonical-json/v1";
const SCHEMA_DOCUMENT_SET_CANONICALIZATION: &str = "typebridge.schema-document-set/v1";
const MANAGED_DECLARED_CANONICALIZATION: &str = "typebridge.managed-declared/v1";
const MANAGED_SEMANTIC_CANONICALIZATION: &str = "typebridge.managed-semantic/v1";

/// Fingerprint of an ordered schema document set's portable paths and exact-source digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaDocumentSetFingerprint(Fingerprint);

impl SchemaDocumentSetFingerprint {
    /// Compute a document-set fingerprint from canonical path-and-digest bytes.
    pub fn compute(canonical_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.document-set")?,
            CanonicalizationVersion::new(SCHEMA_DOCUMENT_SET_CANONICALIZATION)?,
            None,
            canonical_bytes,
        )))
    }

    /// Return the generic fingerprint metadata and digest.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Fingerprint of canonical direct schema semantics under one semantic profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SemanticSchemaFingerprint(Fingerprint);

impl SemanticSchemaFingerprint {
    /// Compute a semantic schema fingerprint from canonical direct-semantic bytes.
    pub fn compute(profile: SemanticProfileId, canonical_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.semantic")?,
            CanonicalizationVersion::new(SCHEMA_CANONICALIZATION)?,
            Some(profile),
            canonical_bytes,
        )))
    }

    /// Return the generic fingerprint metadata and digest.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Fingerprint of declared identity after deterministic managed-scope filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ManagedDeclaredIdentityFingerprint(Fingerprint);

impl ManagedDeclaredIdentityFingerprint {
    /// Compute a managed declared-identity fingerprint from canonical filtered bytes.
    pub fn compute(canonical_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.managed-declared-identity")?,
            CanonicalizationVersion::new(MANAGED_DECLARED_CANONICALIZATION)?,
            None,
            canonical_bytes,
        )))
    }

    /// Return the generic fingerprint metadata and digest.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    pub(crate) fn from_wire(fingerprint: Fingerprint) -> Result<Self, Diagnostic> {
        if fingerprint.domain().as_str() != "typebridge.schema.managed-declared-identity"
            || fingerprint.canonicalization().as_str() != MANAGED_DECLARED_CANONICALIZATION
            || fingerprint.semantic_profile().is_some()
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "invalid_managed_declared_identity_fingerprint",
                "managed declared-identity fingerprint metadata is invalid",
            ));
        }
        Ok(Self(fingerprint))
    }
}

/// Fingerprint of direct semantics after deterministic managed-scope filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ManagedSemanticSchemaFingerprint(Fingerprint);

impl ManagedSemanticSchemaFingerprint {
    /// Compute a managed semantic fingerprint from canonical filtered bytes.
    pub fn compute(profile: SemanticProfileId, canonical_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.managed-semantic")?,
            CanonicalizationVersion::new(MANAGED_SEMANTIC_CANONICALIZATION)?,
            Some(profile),
            canonical_bytes,
        )))
    }

    /// Return the generic fingerprint metadata and digest.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    pub(crate) fn from_wire(fingerprint: Fingerprint) -> Result<Self, Diagnostic> {
        if fingerprint.domain().as_str() != "typebridge.schema.managed-semantic"
            || fingerprint.canonicalization().as_str() != MANAGED_SEMANTIC_CANONICALIZATION
            || fingerprint.semantic_profile().is_none()
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "invalid_managed_semantic_schema_fingerprint",
                "managed semantic-schema fingerprint metadata is invalid",
            ));
        }
        Ok(Self(fingerprint))
    }
}

impl SemanticSchemaFingerprint {
    /// Adopt a decoded generic fingerprint only after checking the exact semantic-schema domain.
    pub(crate) fn from_wire(fingerprint: Fingerprint) -> Result<Self, Diagnostic> {
        if fingerprint.domain().as_str() != "typebridge.schema.semantic"
            || fingerprint.canonicalization().as_str() != SCHEMA_CANONICALIZATION
            || fingerprint.semantic_profile().is_none()
        {
            return Err(crate::diagnostic::Diagnostic::stable(
                crate::diagnostic::DiagnosticCategory::Integrity,
                "invalid_semantic_schema_fingerprint",
                "semantic schema fingerprint wire metadata is inconsistent",
            ));
        }
        Ok(Self(fingerprint))
    }
}
