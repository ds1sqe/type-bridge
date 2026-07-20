//! Adopted-genesis artifact parsing for legacy (v1) scope cutover.
//!
//! Adopting a released v1 database into canonical V2 history records the
//! pre-adoption TypeDB schema export verbatim as `adopted-genesis.typeql`
//! beside the canonical migration manifests. The adoption flow derives the
//! reconstructed legacy head from those bytes, and every later genesis
//! resolution re-parses the same bytes through the same function here — so
//! the schema every parentless manifest verifies against is fixed by one
//! immutable, reviewable artifact rather than rebuilt from a live
//! connection.
//!
//! A pre-adoption v1 export contains the user schema plus the frozen v1
//! migration-ledger schema and nothing else: the V2 control namespace is
//! installed only by canonical apply, which has not run yet. Parsing
//! therefore rejects any reserved-namespace fact outright, requires the
//! ledger partition to be absent or exactly the frozen contract, and
//! returns the remaining facts as the adopted genesis.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::reserved::is_typebridge_internal_label;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SourcedSchemaFact,
};

use crate::typeql_to_declared;

/// File name of the adopted-genesis artifact inside the canonical migration
/// directory.
pub const ADOPTED_GENESIS_FILE_NAME: &str = "adopted-genesis.typeql";

/// Frozen TypeQL rendering of the released v1 migration-ledger schema.
///
/// This is the exact `migration_state_schema().to_typeql()` output of the
/// released 1.5.x line, pinned here so offline surfaces can recognize the
/// ledger without depending on the provider-bearing v1 compat crate. The
/// v1 crate carries the equality test that keeps the two in lock-step; the
/// ledger is frozen, so a divergence there is a contract break, not a
/// refactor.
pub const LEGACY_LEDGER_SCHEMA_TYPEQL: &str = "\
define

attribute migration_app_label, value string;
attribute migration_applied_at, value datetime;
attribute migration_checksum, value string;
attribute migration_direction, value string;
attribute migration_error, value string;
attribute migration_executor_ip, value string;
attribute migration_executor_mac, value string;
attribute migration_finished_at, value datetime;
attribute migration_id, value string;
attribute migration_name, value string;
attribute migration_run_id, value string;
attribute migration_started_at, value datetime;
attribute migration_status, value string;

entity type_bridge_migration,
    owns migration_id @key,
    owns migration_app_label,
    owns migration_name,
    owns migration_applied_at,
    owns migration_checksum;
entity type_bridge_migration_run,
    owns migration_run_id @key,
    owns migration_app_label,
    owns migration_name,
    owns migration_checksum,
    owns migration_direction,
    owns migration_status,
    owns migration_started_at,
    owns migration_finished_at,
    owns migration_error,
    owns migration_executor_ip,
    owns migration_executor_mac;
";

/// The exact-match label set of the frozen legacy ledger.
///
/// Legacy reservation was never prefix-based, so classification is by exact
/// label. The set is derived from [`LEGACY_LEDGER_SCHEMA_TYPEQL`] labels.
static LEGACY_LEDGER_LABELS: LazyLock<BTreeSet<&'static str>> =
    LazyLock::new(|| {
        BTreeSet::from([
            "type_bridge_migration",
            "type_bridge_migration_run",
            "migration_id",
            "migration_app_label",
            "migration_name",
            "migration_applied_at",
            "migration_checksum",
            "migration_run_id",
            "migration_direction",
            "migration_status",
            "migration_started_at",
            "migration_finished_at",
            "migration_error",
            "migration_executor_ip",
            "migration_executor_mac",
        ])
    });

/// Return whether a schema label belongs to the frozen v1 ledger vocabulary.
#[must_use]
pub fn is_legacy_ledger_label(label: &str) -> bool {
    LEGACY_LEDGER_LABELS.contains(label)
}

/// Parse adopted-genesis bytes into the reconstructed legacy head.
///
/// The same function serves both ends of the adoption contract: the adopt
/// flow runs it over the live pre-adoption export before storing those
/// bytes, and genesis resolution runs it over the stored artifact — the
/// derived schema is identical by construction. Reserved V2 control facts
/// fail closed (a pre-adoption database cannot carry them), and a partial
/// ledger partition is indistinguishable from corruption and is rejected.
pub fn parse_adopted_genesis(
    document: DocumentId,
    source: &str,
) -> Result<DeclaredSchema, Diagnostic> {
    let full = typeql_to_declared(document, source).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "adopted_genesis_invalid",
            "adopted-genesis TypeQL cannot be normalized into V2 facts",
        )
    })?;

    let mut user = Vec::new();
    let mut ledger = Vec::new();
    for fact in full.facts() {
        let id = fact.id();
        let source = full.source(&id).cloned().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_missing_provenance",
                "normalized adopted-genesis fact has no source provenance",
            )
        })?;
        let id_value = serde_json::to_value(&id).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "adopted_genesis_identity_encode_failed",
                "adopted-genesis fact identity cannot be inspected for reserved labels",
            )
        })?;
        if value_mentions_label(&id_value, is_typebridge_internal_label) {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_reserved_namespace",
                "adopted genesis mentions the reserved V2 control namespace; a \
                 pre-adoption v1 database cannot carry canonical control state",
            ));
        }
        let sourced = SourcedSchemaFact::new(fact.clone(), source);
        if value_mentions_label(&id_value, is_legacy_ledger_label) {
            ledger.push(sourced);
        } else {
            user.push(sourced);
        }
    }

    if !ledger.is_empty() {
        let ledger = DeclaredSchema::from_facts(
            full.format(),
            CapabilitySet::new(),
            ledger,
        )
        .map_err(|_| ledger_mismatch())?;
        let expected_document =
            DocumentId::new("typebridge-legacy-ledger-schema.typeql")?;
        let expected =
            typeql_to_declared(expected_document, LEGACY_LEDGER_SCHEMA_TYPEQL)
                .map_err(|_| {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "adopted_genesis_frozen_ledger_invalid",
                        "frozen legacy migration-ledger schema cannot be normalized",
                    )
                })?;
        if ledger.declared_identity_fingerprint()
            != expected.declared_identity_fingerprint()
        {
            return Err(ledger_mismatch());
        }
    }

    DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), user).map_err(
        |_| {
            failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_cross_reference",
                "adopted-genesis user facts reference the frozen legacy ledger",
            )
        },
    )
}

fn ledger_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "adopted_genesis_legacy_ledger_mismatch",
        "legacy migration-ledger facts differ from the frozen v1 contract",
    )
}

fn value_mentions_label(value: &Value, predicate: fn(&str) -> bool) -> bool {
    match value {
        Value::String(value) => predicate(value),
        Value::Array(values) => {
            values.iter().any(|value| value_mentions_label(value, predicate))
        }
        Value::Object(values) => values
            .values()
            .any(|value| value_mentions_label(value, predicate)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static adopted-genesis diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> DocumentId {
        DocumentId::new("adopted-genesis.typeql").expect("document id")
    }

    #[test]
    fn user_only_export_parses_to_the_user_schema() {
        let genesis = parse_adopted_genesis(
            document(),
            "define\nentity person;\nentity company;\n",
        )
        .expect("user-only export");
        let direct = typeql_to_declared(
            document(),
            "define\nentity person;\nentity company;\n",
        )
        .expect("direct parse");
        assert_eq!(
            genesis.declared_identity_fingerprint(),
            direct.declared_identity_fingerprint(),
        );
    }

    #[test]
    fn the_exact_frozen_ledger_partition_is_dropped() {
        let source = format!(
            "{}\nentity person;\n",
            LEGACY_LEDGER_SCHEMA_TYPEQL.trim_end(),
        );
        let genesis =
            parse_adopted_genesis(document(), &source).expect("v1 export");
        let expected = typeql_to_declared(document(), "define\nentity person;\n")
            .expect("user parse");
        assert_eq!(
            genesis.declared_identity_fingerprint(),
            expected.declared_identity_fingerprint(),
        );
    }

    #[test]
    fn a_partial_ledger_partition_fails_closed() {
        let error = parse_adopted_genesis(
            document(),
            "define\nattribute migration_id, value string;\nentity person;\n",
        )
        .expect_err("partial ledger is corruption");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_legacy_ledger_mismatch",
        );
    }

    #[test]
    fn reserved_control_facts_fail_closed() {
        let error = parse_adopted_genesis(
            document(),
            "define\nentity typebridge-internal-v2-migration-control;\n",
        )
        .expect_err("reserved namespace cannot be adopted");
        assert_eq!(error.code().as_str(), "adopted_genesis_reserved_namespace");
    }

    #[test]
    fn the_frozen_ledger_constant_normalizes() {
        typeql_to_declared(document(), LEGACY_LEDGER_SCHEMA_TYPEQL)
            .expect("frozen ledger parses");
    }
}
