//! Legacy (v1) migration references recorded by the frontier bridge.
//!
//! Cutover from the legacy Python/JSON migration format creates one
//! immutable, zero-operation legacy-frontier-bridge canonical manifest per
//! managed scope. Only that node names legacy parents; it records each
//! legacy frontier compound identity with its tagged checksum, and its
//! verified source and target are the identical reconstructed legacy head.
//! Import verifies the ledger and live state against that head — it never
//! replays already-applied legacy steps.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_schema::ManagedDeltaContext;

use crate::manifest::{
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
};

/// Tag identifying the legacy checksum algorithm: SHA-256 over the authored
/// Python migration source, hex-encoded and truncated to 16 characters.
pub const LEGACY_CHECKSUM_ALGORITHM: &str = "python-source-sha256/16";

const LEGACY_CHECKSUM_LEN: usize = 16;

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
    id: MigrationId,
    checksum: LegacyMigrationChecksum,
}

impl LegacyMigrationReference {
    /// Bind one legacy compound identity to its tagged checksum.
    pub const fn new(id: MigrationId, checksum: LegacyMigrationChecksum) -> Self {
        Self { id, checksum }
    }

    /// Return the legacy compound migration identity.
    pub const fn id(&self) -> &MigrationId {
        &self.id
    }

    /// Return the tagged legacy checksum.
    pub const fn checksum(&self) -> &LegacyMigrationChecksum {
        &self.checksum
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
    reconstructed_head: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<VerifiedSchemaMigrationManifest, Diagnostic> {
    build_verified_manifest(
        SchemaMigrationDraft::legacy_bridge(id, legacy_frontier)?,
        (reconstructed_head, context),
    )
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static legacy diagnostic code is canonical"),
        message,
    )
}
