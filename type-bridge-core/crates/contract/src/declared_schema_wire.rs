//! Private canonical wire reconstruction for trusted declared schemas.

use serde::{Deserialize, Serialize};

use crate::capability::CapabilitySet;
use crate::codec::{FormatVersion, from_canonical_json, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::schema::{
    DeclaredIdentityFingerprint, DeclaredSchema, DocumentId, SourceSpan,
    SourcedSchemaFact,
};
use crate::schema_delta_wire::{FingerprintWire, SchemaFactWire};

const COMPILED_PROVENANCE_DOCUMENT: &str =
    "__typebridge_compiled__/declared-schema-v1";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSchemaWire {
    declared_identity: FingerprintWire,
    facts: Vec<SchemaFactWire>,
    format_version: FormatVersion,
    required_capabilities: CapabilitySet,
}

pub(crate) fn encode_declared_schema(
    schema: &DeclaredSchema,
) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(schema)
}

pub(crate) fn decode_declared_schema(bytes: &[u8]) -> Result<DeclaredSchema, Diagnostic> {
    let DeclaredSchemaWire {
        declared_identity,
        facts,
        format_version,
        required_capabilities,
    } = from_canonical_json(bytes)?;
    let expected_identity =
        DeclaredIdentityFingerprint::from_wire(declared_identity.rebuild()?)?;
    let sourced_facts = facts
        .into_iter()
        .enumerate()
        .map(|(index, wire)| {
            Ok(SourcedSchemaFact::new(
                wire.rebuild()?,
                compiled_provenance(index)?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let trusted = DeclaredSchema::from_facts(
        format_version,
        required_capabilities,
        sourced_facts,
    )
    .map_err(first_schema_diagnostic)?;

    if &expected_identity != trusted.declared_identity_fingerprint() {
        return Err(wire_diagnostic(
            DiagnosticCategory::Integrity,
            "declared_schema_fingerprint_mismatch",
            "declared schema fingerprint does not match its reconstructed identity",
        ));
    }
    if to_canonical_json(&trusted)? != bytes {
        return Err(wire_diagnostic(
            DiagnosticCategory::InvalidContract,
            "non_canonical_declared_schema",
            "declared schema bytes normalize after trusted reconstruction",
        ));
    }
    Ok(trusted)
}

fn compiled_provenance(index: usize) -> Result<SourceSpan, Diagnostic> {
    let byte = u64::try_from(index).map_err(|_| {
        wire_diagnostic(
            DiagnosticCategory::ResourceLimit,
            "declared_schema_provenance_overflow",
            "declared schema contains too many facts for compiled provenance",
        )
    })?;
    let column = u32::try_from(index + 1).map_err(|_| {
        wire_diagnostic(
            DiagnosticCategory::ResourceLimit,
            "declared_schema_provenance_overflow",
            "declared schema contains too many facts for compiled provenance",
        )
    })?;
    SourceSpan::new(
        DocumentId::new(COMPILED_PROVENANCE_DOCUMENT)?,
        byte,
        byte,
        1,
        column,
        1,
        column,
    )
}

fn first_schema_diagnostic(diagnostics: crate::schema::SchemaDiagnostics) -> Diagnostic {
    diagnostics
        .iter()
        .next()
        .map(|diagnostic| diagnostic.diagnostic().clone())
        .unwrap_or_else(|| {
            wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_declared_schema",
                "declared schema reconstruction failed without a diagnostic",
            )
        })
}

fn wire_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::stable(category, code, message)
}
