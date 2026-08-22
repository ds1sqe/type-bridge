//! Deterministic, source-free bundles of replay-verified migration history.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::migration::{MigrationId, MigrationManifestDigest};
use type_bridge_contract::schema::{
    DeclaredSchema, decode_declared_schema, encode_declared_schema,
};
use type_bridge_schema::ManagedDeltaContext;

use crate::{
    MigrationHistoryGraph, VerifiedSchemaMigrationManifest, decode_verified_manifest,
    encode_verified_manifest,
};

/// Exact wire discriminator for the first immutable migration-history bundle.
pub const MIGRATION_HISTORY_BUNDLE_V1: &str = "typebridge.migration-history-bundle/v1";
/// Fingerprint domain for one exact verified history bundle content object.
pub const MIGRATION_HISTORY_BUNDLE_FINGERPRINT_DOMAIN: &str = "typebridge.migration.history-bundle";
/// Canonicalization identity for the first history-bundle wire.
pub const MIGRATION_HISTORY_BUNDLE_FINGERPRINT_CANONICALIZATION: &str =
    "typebridge.migration-history-bundle/v1";
/// Maximum accepted canonical history-bundle size: 16 MiB.
pub const MAX_MIGRATION_HISTORY_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

const BUNDLE_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_MIGRATION_HISTORY_BUNDLE_BYTES,
    max_depth: MAX_CANONICAL_DEPTH,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

/// One replay-verified historical manifest and its exact endpoint schemas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationHistoryBundleEntry {
    manifest: VerifiedSchemaMigrationManifest,
    manifest_digest: MigrationManifestDigest,
    source_schema: DeclaredSchema,
    target_schema: DeclaredSchema,
}

impl VerifiedMigrationHistoryBundleEntry {
    /// Return the verified manifest.
    pub const fn manifest(&self) -> &VerifiedSchemaMigrationManifest {
        &self.manifest
    }

    /// Return the raw digest of the exact canonical manifest bytes.
    pub const fn manifest_digest(&self) -> MigrationManifestDigest {
        self.manifest_digest
    }

    /// Return the historical declared schema required before this manifest.
    pub const fn source_schema(&self) -> &DeclaredSchema {
        &self.source_schema
    }

    /// Return the historical declared schema produced by this manifest.
    pub const fn target_schema(&self) -> &DeclaredSchema {
        &self.target_schema
    }
}

/// A deterministic immutable history snapshot admitted only after full replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationHistoryBundle {
    entries: Vec<VerifiedMigrationHistoryBundleEntry>,
    fingerprint: Fingerprint,
    heads: Vec<MigrationId>,
}

impl VerifiedMigrationHistoryBundle {
    /// Build a bundle from one already replay-verified graph.
    pub fn from_graph(graph: &MigrationHistoryGraph) -> Result<Self, Diagnostic> {
        let entries = graph
            .topological_order()
            .iter()
            .map(|id| {
                let manifest = graph.manifest(id).ok_or_else(|| {
                    failure(
                        DiagnosticCategory::Integrity,
                        "migration_history_bundle_missing_manifest",
                        "verified graph order names a missing manifest",
                    )
                })?;
                let manifest_bytes = encode_verified_manifest(manifest)?;
                Ok(VerifiedMigrationHistoryBundleEntry {
                    manifest: manifest.clone(),
                    manifest_digest: MigrationManifestDigest::compute(&manifest_bytes),
                    source_schema: manifest.source_schema().clone(),
                    target_schema: manifest.target_schema().clone(),
                })
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let heads = graph.heads().to_vec();
        let content = content_wire(&entries, &heads)?;
        let fingerprint = fingerprint_content(&content)?;
        Ok(Self {
            entries,
            fingerprint,
            heads,
        })
    }

    /// Return entries in deterministic topological order.
    pub fn entries(&self) -> &[VerifiedMigrationHistoryBundleEntry] {
        &self.entries
    }

    /// Return the exact bundle content fingerprint.
    pub const fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Return graph heads in canonical compound-identity order.
    pub fn heads(&self) -> &[MigrationId] {
        &self.heads
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BundleWire {
    content: BundleContentWire,
    fingerprint: Fingerprint,
    format: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BundleContentWire {
    entries: Vec<BundleEntryWire>,
    heads: Vec<MigrationIdCandidate>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BundleEntryWire {
    manifest: Value,
    manifest_digest: String,
    source_schema: Value,
    target_schema: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MigrationIdCandidate {
    app_label: String,
    name: String,
}

/// Encode one verified history bundle into bounded canonical bytes.
pub fn encode_verified_migration_history_bundle(
    bundle: &VerifiedMigrationHistoryBundle,
) -> Result<Vec<u8>, Diagnostic> {
    let content = content_wire(&bundle.entries, &bundle.heads)?;
    let derived = fingerprint_content(&content)?;
    if derived != bundle.fingerprint {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_history_bundle_stale_fingerprint",
            "history bundle fingerprint differs from its exact content",
        ));
    }
    to_canonical_json_with_limits(
        &BundleWire {
            content,
            fingerprint: derived,
            format: MIGRATION_HISTORY_BUNDLE_V1.to_owned(),
        },
        BUNDLE_LIMITS,
    )
}

/// Decode, reconstruct, replay, and independently verify a history bundle.
pub fn decode_verified_migration_history_bundle(
    bytes: &[u8],
    context: &ManagedDeltaContext,
) -> Result<VerifiedMigrationHistoryBundle, Diagnostic> {
    let wire: BundleWire = from_canonical_json_with_limits(bytes, BUNDLE_LIMITS)?;
    if wire.format != MIGRATION_HISTORY_BUNDLE_V1 {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "unsupported_migration_history_bundle_format",
            "migration history bundle format is not supported",
        ));
    }
    if fingerprint_content(&wire.content)? != wire.fingerprint {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_history_bundle_fingerprint_mismatch",
            "migration history bundle content does not match its fingerprint",
        ));
    }

    let mut manifests = Vec::with_capacity(wire.content.entries.len());
    for candidate in &wire.content.entries {
        let source_bytes = to_canonical_json_with_limits(&candidate.source_schema, BUNDLE_LIMITS)?;
        let target_bytes = to_canonical_json_with_limits(&candidate.target_schema, BUNDLE_LIMITS)?;
        let source_schema = decode_declared_schema(&source_bytes)?;
        let target_schema = decode_declared_schema(&target_bytes)?;
        let manifest_bytes = to_canonical_json_with_limits(&candidate.manifest, BUNDLE_LIMITS)?;
        let observed_digest = MigrationManifestDigest::compute(&manifest_bytes);
        let expected_digest = MigrationManifestDigest::from_hex(&candidate.manifest_digest)?;
        if observed_digest != expected_digest {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_history_bundle_manifest_digest_mismatch",
                "bundled manifest bytes do not match their detached digest",
            ));
        }
        let manifest = decode_verified_manifest(&manifest_bytes, (&source_schema, context))?;
        if encode_declared_schema(manifest.target_schema())?
            != encode_declared_schema(&target_schema)?
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_history_bundle_target_schema_mismatch",
                "bundled target schema differs from replay-derived manifest output",
            ));
        }
        manifests.push(manifest);
    }

    let graph = MigrationHistoryGraph::from_verified(manifests)?;
    let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph)?;
    let expected_heads = wire
        .content
        .heads
        .into_iter()
        .map(|id| MigrationId::new(id.app_label, id.name))
        .collect::<Result<Vec<_>, _>>()?;
    if bundle.heads != expected_heads || bundle.fingerprint != wire.fingerprint {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_history_bundle_graph_mismatch",
            "bundled ordering, heads, or identity differ from the reconstructed graph",
        ));
    }
    if encode_verified_migration_history_bundle(&bundle)? != bytes {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_history_bundle_noncanonical",
            "migration history bundle bytes are not the unique canonical encoding",
        ));
    }
    Ok(bundle)
}

fn content_wire(
    entries: &[VerifiedMigrationHistoryBundleEntry],
    heads: &[MigrationId],
) -> Result<BundleContentWire, Diagnostic> {
    let entries = entries
        .iter()
        .map(|entry| {
            let manifest_bytes = encode_verified_manifest(&entry.manifest)?;
            let source_bytes = encode_declared_schema(&entry.source_schema)?;
            let target_bytes = encode_declared_schema(&entry.target_schema)?;
            Ok(BundleEntryWire {
                manifest: serde_json::from_slice(&manifest_bytes).map_err(codec_failure)?,
                manifest_digest: MigrationManifestDigest::compute(&manifest_bytes).to_hex(),
                source_schema: serde_json::from_slice(&source_bytes).map_err(codec_failure)?,
                target_schema: serde_json::from_slice(&target_bytes).map_err(codec_failure)?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let heads = heads
        .iter()
        .map(|id| MigrationIdCandidate {
            app_label: id.app_label().as_str().to_owned(),
            name: id.name().as_str().to_owned(),
        })
        .collect();
    Ok(BundleContentWire { entries, heads })
}

fn fingerprint_content(content: &BundleContentWire) -> Result<Fingerprint, Diagnostic> {
    Ok(Fingerprint::compute(
        FingerprintDomain::new(MIGRATION_HISTORY_BUNDLE_FINGERPRINT_DOMAIN)?,
        CanonicalizationVersion::new(MIGRATION_HISTORY_BUNDLE_FINGERPRINT_CANONICALIZATION)?,
        None,
        &to_canonical_json_with_limits(content, BUNDLE_LIMITS)?,
    ))
}

fn codec_failure(_: serde_json::Error) -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_history_bundle_internal_codec_failure",
        "verified canonical migration material could not be reconstructed",
    )
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static bundle diagnostic code is canonical"),
        message,
    )
}
