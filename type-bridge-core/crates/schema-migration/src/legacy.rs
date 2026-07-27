//! Legacy (v1) migration references recorded by the frontier bridge.
//!
//! Cutover from the legacy Python/JSON migration format creates one
//! immutable, zero-operation legacy-frontier-bridge canonical manifest per
//! managed scope. Only that node names legacy parents; it records each
//! legacy frontier compound identity with its tagged checksum, and its
//! verified source and target are the identical reconstructed legacy head.
//! Import verifies the ledger and live state against that head — it never
//! replays already-applied legacy steps.

use sha2::{Digest as _, Sha256};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{
    MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_schema::ManagedDeltaContext;

use crate::manifest::{
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
};

/// Tag identifying the legacy checksum algorithm: SHA-256 over the authored
/// Python migration source, hex-encoded and truncated to 16 characters.
pub const LEGACY_CHECKSUM_ALGORITHM: &str = "python-source-sha256/16";
/// Canonicalization/domain vocabulary for the complete released applied set.
pub const LEGACY_APPLIED_SET_CANONICALIZATION: &str = "typebridge.legacy-applied-set/v1";
/// Digest algorithm vocabulary for the complete released applied set.
pub const LEGACY_APPLIED_SET_ALGORITHM: &str = "sha256";

const LEGACY_CHECKSUM_LEN: usize = 16;

fn validate_legacy_component(value: String, kind: &'static str) -> Result<String, Diagnostic> {
    if !value.is_empty() && value.len() <= MAX_CANONICAL_STRING_BYTES {
        Ok(value)
    } else {
        Err(failure(
            "migration_legacy_identity_component_invalid",
            "legacy migration identity components must be nonempty bounded UTF-8",
        )
        .with_detail("component_kind", kind))
    }
}

macro_rules! legacy_component {
    ($name:ident, $doc:literal, $kind:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Construct one lossless released identity component.
            pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
                Ok(Self(validate_legacy_component(value.into(), $kind)?))
            }

            /// Return the exact released spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

legacy_component!(
    LegacyMigrationAppLabel,
    "A lossless bounded UTF-8 application label from released migration history.",
    "app_label"
);
legacy_component!(
    LegacyMigrationName,
    "A lossless bounded UTF-8 migration name from released migration history.",
    "name"
);

/// Lossless compound identity for a released migration frontier member.
///
/// This is deliberately distinct from canonical V2 [`MigrationId`]: released
/// V1 directory labels and sidecar names admitted UTF-8 spellings outside the
/// portable lowercase grammar required for newly authored V2 manifests.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LegacyMigrationId {
    app_label: LegacyMigrationAppLabel,
    name: LegacyMigrationName,
}

impl LegacyMigrationId {
    /// Construct a lossless bounded released identity.
    pub fn new(app_label: impl Into<String>, name: impl Into<String>) -> Result<Self, Diagnostic> {
        Ok(Self {
            app_label: LegacyMigrationAppLabel::new(app_label)?,
            name: LegacyMigrationName::new(name)?,
        })
    }

    /// Return the exact released application label.
    pub const fn app_label(&self) -> &LegacyMigrationAppLabel {
        &self.app_label
    }

    /// Return the exact released migration name.
    pub const fn name(&self) -> &LegacyMigrationName {
        &self.name
    }
}

impl From<MigrationId> for LegacyMigrationId {
    fn from(id: MigrationId) -> Self {
        Self {
            app_label: LegacyMigrationAppLabel(id.app_label().as_str().to_owned()),
            name: LegacyMigrationName(id.name().as_str().to_owned()),
        }
    }
}

/// A tagged legacy migration checksum.
///
/// The type is deliberately distinct from every canonical digest so the
/// legacy truncated Python checksum can never be confused with a full
/// canonical-manifest digest.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LegacyMigrationChecksum(String);

impl LegacyMigrationChecksum {
    /// Validate a canonical 16-character lowercase-hex legacy checksum.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.len() != LEGACY_CHECKSUM_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(failure(
                "migration_legacy_checksum_invalid",
                "legacy checksum must be exactly 16 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    /// Return the canonical checksum text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the tagged algorithm this checksum was computed under.
    pub const fn algorithm(&self) -> &'static str {
        LEGACY_CHECKSUM_ALGORITHM
    }
}

/// One legacy frontier migration named by the canonical bridge.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LegacyMigrationReference {
    id: LegacyMigrationId,
    checksum: LegacyMigrationChecksum,
}

impl LegacyMigrationReference {
    /// Bind one legacy compound identity to its tagged checksum.
    pub fn new(id: impl Into<LegacyMigrationId>, checksum: LegacyMigrationChecksum) -> Self {
        Self {
            id: id.into(),
            checksum,
        }
    }

    /// Return the legacy compound migration identity.
    pub const fn id(&self) -> &LegacyMigrationId {
        &self.id
    }

    /// Return the tagged legacy checksum.
    pub const fn checksum(&self) -> &LegacyMigrationChecksum {
        &self.checksum
    }
}

/// Digest binding every semantic row in the released applied ledger.
///
/// The preimage excludes `applied_at`: released insertion timestamps do not
/// change which migrations/checksums are applied. Exact UTF-8 identity and
/// checksum triples are sorted, count-prefixed, and length-delimited so
/// component boundaries cannot collide.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LegacyAppliedSetDigest(String);

impl LegacyAppliedSetDigest {
    /// Compute the domain-separated digest of one complete applied set.
    pub fn compute(
        references: impl IntoIterator<Item = LegacyMigrationReference>,
    ) -> Result<Self, Diagnostic> {
        let mut collected = Vec::new();
        let mut preimage_bytes = LEGACY_APPLIED_SET_CANONICALIZATION
            .len()
            .checked_add(1 + std::mem::size_of::<u64>())
            .ok_or_else(applied_set_too_large)?;
        for reference in references {
            if collected.len() == MAX_CANONICAL_COLLECTION_LEN {
                return Err(failure(
                    "migration_legacy_applied_set_too_many_rows",
                    "legacy applied-set digest exceeds the canonical collection ceiling",
                ));
            }
            for field in [
                reference.id().app_label().as_str().as_bytes(),
                reference.id().name().as_str().as_bytes(),
                reference.checksum().as_str().as_bytes(),
            ] {
                preimage_bytes = preimage_bytes
                    .checked_add(std::mem::size_of::<u64>())
                    .and_then(|bytes| bytes.checked_add(field.len()))
                    .ok_or_else(applied_set_too_large)?;
            }
            if preimage_bytes > MAX_CANONICAL_BYTES {
                return Err(applied_set_too_large());
            }
            collected.push(reference);
        }
        let mut references = collected;
        references.sort();
        if references.is_empty() {
            return Err(failure(
                "migration_legacy_applied_set_empty",
                "legacy applied-set digest requires at least one migration",
            ));
        }
        if references
            .windows(2)
            .any(|pair| pair[0].id() == pair[1].id())
        {
            return Err(failure(
                "migration_legacy_applied_set_duplicate",
                "legacy applied-set digest contains a duplicate migration identity",
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(LEGACY_APPLIED_SET_CANONICALIZATION.as_bytes());
        hasher.update([0]);
        let row_count = u64::try_from(references.len()).map_err(|_| applied_set_too_large())?;
        hasher.update(row_count.to_be_bytes());
        for reference in &references {
            hash_field(&mut hasher, reference.id().app_label().as_str().as_bytes());
            hash_field(&mut hasher, reference.id().name().as_str().as_bytes());
            hash_field(&mut hasher, reference.checksum().as_str().as_bytes());
        }
        Ok(Self(hex_digest(hasher.finalize())))
    }

    /// Validate a persisted lowercase SHA-256 digest.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(failure(
                "migration_legacy_applied_set_digest_invalid",
                "legacy applied-set digest must be 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    /// Return the fixed digest algorithm vocabulary.
    pub const fn algorithm(&self) -> &'static str {
        LEGACY_APPLIED_SET_ALGORITHM
    }

    /// Return the fixed preimage canonicalization/domain vocabulary.
    pub const fn canonicalization(&self) -> &'static str {
        LEGACY_APPLIED_SET_CANONICALIZATION
    }

    /// Return the lowercase digest text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Build the verified zero-operation bridge for one legacy frontier.
///
/// `reconstructed_head` is the managed schema the completed legacy history
/// reaches; the bridge verifies against it as its genesis source and records
/// an identical target, so applying the bridge executes nothing and the
/// coordinator's live-state gate proves the database sits at that exact head.
pub fn build_legacy_frontier_bridge(
    id: MigrationId,
    legacy_frontier: Vec<LegacyMigrationReference>,
    legacy_applied_set: LegacyAppliedSetDigest,
    reconstructed_head: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<VerifiedSchemaMigrationManifest, Diagnostic> {
    build_verified_manifest(
        SchemaMigrationDraft::legacy_bridge(id, legacy_frontier, legacy_applied_set)?,
        (reconstructed_head, context),
    )
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn applied_set_too_large() -> Diagnostic {
    failure(
        "migration_legacy_applied_set_too_large",
        "legacy applied-set digest exceeds the canonical byte ceiling",
    )
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static legacy diagnostic code is canonical"),
        message,
    )
}
