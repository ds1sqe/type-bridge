//! Domain-separated canonical fingerprint values.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::diagnostic::{Diagnostic, DiagnosticCategory};

/// The initial supported fingerprint algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintAlgorithm {
    /// SHA-256 with a 32-byte digest.
    Sha256,
}

/// A validated canonicalization identifier such as `typebridge.canonical-json/v1`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalizationVersion(String);
/// A validated fingerprint domain identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FingerprintDomain(String);
/// A validated semantic-profile identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticProfileId(String);

fn validate_name(
    value: String,
    kind: &'static str,
    require_version: bool,
) -> Result<String, Diagnostic> {
    let valid = !value.is_empty()
        && value.len() <= 255
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'/' | b'_' | b'-')
        })
        && !value.starts_with(['.', '/', '_', '-'])
        && !value.ends_with(['.', '/', '_', '-'])
        && (!require_version
            || value.rsplit_once("/v").is_some_and(|(_, version)| {
                !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit())
            }));
    if valid {
        Ok(value)
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::Integrity,
            "invalid_fingerprint_identifier",
            "fingerprint metadata identifier is malformed",
        )
        .with_detail("identifier_kind", kind))
    }
}

macro_rules! fingerprint_name {
    ($name:ident, $versioned:expr) => {
        impl $name {
            /// Validate and construct this identifier.
            pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
                Ok(Self(validate_name(
                    value.into(),
                    stringify!($name),
                    $versioned,
                )?))
            }
            /// Return its canonical spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}
fingerprint_name!(CanonicalizationVersion, true);
fingerprint_name!(FingerprintDomain, false);
fingerprint_name!(SemanticProfileId, true);

/// Require an exact canonicalization version before consuming canonical bytes.
pub fn ensure_canonicalization_version(
    actual: &CanonicalizationVersion,
    supported: &CanonicalizationVersion,
) -> Result<(), Diagnostic> {
    if actual == supported {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "unsupported_canonicalization_version",
            "canonicalization version is not supported",
        )
        .with_detail("actual", actual.as_str().to_owned())
        .with_detail("supported", supported.as_str().to_owned()))
    }
}

/// A validated 32-byte SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FingerprintDigest([u8; 32]);

impl FingerprintDigest {
    /// Parse exactly 64 lowercase hexadecimal characters.
    pub fn from_hex(value: &str) -> Result<Self, Diagnostic> {
        if value.len() != 64
            || value
                .bytes()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "invalid_fingerprint_digest",
                "fingerprint digest must be 64 lowercase hexadecimal characters",
            ));
        }
        let mut bytes = [0_u8; 32];
        for (index, output) in bytes.iter_mut().enumerate() {
            *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| {
                Diagnostic::stable(
                    DiagnosticCategory::Integrity,
                    "invalid_fingerprint_digest",
                    "fingerprint digest contains invalid hexadecimal",
                )
            })?;
        }
        Ok(Self(bytes))
    }
    /// Return lowercase hexadecimal text.
    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    /// Return the raw digest bytes.
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}
impl Serialize for FingerprintDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}
impl<'de> Deserialize<'de> for FingerprintDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_hex(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A complete self-describing domain-separated fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    domain: FingerprintDomain,
    algorithm: FingerprintAlgorithm,
    canonicalization: CanonicalizationVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_profile: Option<SemanticProfileId>,
    digest: FingerprintDigest,
}

impl Fingerprint {
    /// Compute SHA-256 over the domain, canonicalization, optional semantic profile, and canonical bytes.
    pub fn compute(
        domain: FingerprintDomain,
        canonicalization: CanonicalizationVersion,
        semantic_profile: Option<SemanticProfileId>,
        canonical_bytes: &[u8],
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"typebridge.fingerprint/v1\0");
        hash_field(&mut hasher, domain.as_str().as_bytes());
        hash_field(&mut hasher, canonicalization.as_str().as_bytes());
        match &semantic_profile {
            Some(profile) => {
                hasher.update([1]);
                hash_field(&mut hasher, profile.as_str().as_bytes());
            }
            None => hasher.update([0]),
        }
        hash_field(&mut hasher, canonical_bytes);
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&hasher.finalize());
        Self {
            domain,
            algorithm: FingerprintAlgorithm::Sha256,
            canonicalization,
            semantic_profile,
            digest: FingerprintDigest(digest),
        }
    }
    /// Return the fingerprint domain.
    pub fn domain(&self) -> &FingerprintDomain {
        &self.domain
    }
    /// Return the algorithm.
    pub const fn algorithm(&self) -> FingerprintAlgorithm {
        self.algorithm
    }
    /// Return the canonicalization identifier.
    pub fn canonicalization(&self) -> &CanonicalizationVersion {
        &self.canonicalization
    }
    /// Return the optional semantic profile.
    pub fn semantic_profile(&self) -> Option<&SemanticProfileId> {
        self.semantic_profile.as_ref()
    }
    /// Return the digest.
    pub const fn digest(&self) -> FingerprintDigest {
        self.digest
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_golden_and_domain_separated() {
        let canonicalization =
            CanonicalizationVersion::new("typebridge.canonical-json/v1").unwrap();
        let payload = br#"{"kind":"long","value":"9007199254740993"}"#;
        let first = Fingerprint::compute(
            FingerprintDomain::new("test.value").unwrap(),
            canonicalization.clone(),
            None,
            payload,
        );
        assert_eq!(
            first.digest().to_hex(),
            "cbe437dc731095f176ab19a4494c0ee53e491bded9a50627208a9bf022576ce9"
        );
        let other = Fingerprint::compute(
            FingerprintDomain::new("test.other").unwrap(),
            canonicalization,
            None,
            payload,
        );
        assert_ne!(first.digest(), other.digest());
    }

    #[test]
    fn canonicalization_versions_validate_and_fail_closed() {
        for value in [
            "",
            "typebridge.canonical-json",
            "Typebridge.canonical-json/v1",
            "typebridge.canonical-json/vx",
        ] {
            assert_eq!(
                CanonicalizationVersion::new(value)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "invalid_fingerprint_identifier",
            );
        }

        let supported = CanonicalizationVersion::new("typebridge.canonical-json/v1").unwrap();
        let unknown = CanonicalizationVersion::new("typebridge.canonical-json/v2").unwrap();
        assert!(ensure_canonicalization_version(&supported, &supported).is_ok());
        assert_eq!(
            ensure_canonicalization_version(&unknown, &supported)
                .unwrap_err()
                .code()
                .as_str(),
            "unsupported_canonicalization_version",
        );
    }
}
