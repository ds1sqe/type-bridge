//! Legacy (v1) frontier extraction and applied-ledger continuity checks.
//!
//! The archival v1 crate stays the only reader of legacy artifacts: this
//! module adapts its checked loader, drift rule, and state store into the
//! canonical import flow without re-encoding legacy semantics. Import never
//! replays legacy operations — it verifies files, ledger, and live state,
//! then applies the zero-operation bridge as a pure journal checkpoint.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_migration::{
    AppliedMigrationRecord, MigrationGraph, checksum_drift_errors, validate_graph,
};
use type_bridge_schema_migration::{
    LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationId, LegacyMigrationReference,
};

/// Extract the canonical legacy frontier from one checked legacy graph.
///
/// The frontier is the set of graph heads — migrations no other migration
/// depends on — each bound to its tagged checksum. A head without a recorded
/// checksum cannot enter a bridge and fails closed.
pub fn extract_legacy_frontier(
    graph: &MigrationGraph,
) -> Result<Vec<LegacyMigrationReference>, Diagnostic> {
    let mut references = Vec::new();
    for migration in &graph.migrations {
        let is_head = !graph.migrations.iter().any(|candidate| {
            candidate.dependencies.iter().any(|dependency| {
                dependency.app_label == migration.app_label
                    && dependency.migration_name == migration.name
            })
        });
        if !is_head {
            continue;
        }
        references.push(reference_from_parts(
            &migration.app_label,
            &migration.name,
            migration.checksum.as_deref(),
        )?);
    }
    if references.is_empty() {
        return Err(failure(
            "migration_legacy_import_empty_history",
            "legacy migration directory contains no frontier to import",
        ));
    }
    references.sort();
    Ok(references)
}

/// Digest every checksum-bound node in one checked legacy graph.
pub fn extract_legacy_applied_set_digest(
    graph: &MigrationGraph,
) -> Result<LegacyAppliedSetDigest, Diagnostic> {
    LegacyAppliedSetDigest::compute(
        graph
            .migrations
            .iter()
            .map(|migration| {
                reference_from_parts(
                    &migration.app_label,
                    &migration.name,
                    migration.checksum.as_deref(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
}

/// Digest semantic rows from the released applied ledger.
///
/// Row order and `applied_at` are deliberately ignored; exact application
/// label, migration name, and checksum spellings remain bound.
pub fn digest_legacy_applied_records(
    applied: &[AppliedMigrationRecord],
) -> Result<LegacyAppliedSetDigest, Diagnostic> {
    LegacyAppliedSetDigest::compute(
        applied
            .iter()
            .map(|record| {
                reference_from_parts(
                    &record.app_label,
                    &record.name,
                    Some(record.checksum.as_str()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
}

/// Verify checksum continuity between legacy files and the applied ledger.
///
/// Every loaded legacy migration must be applied with its exact recorded
/// checksum, and every applied record must exist in the loaded graph: the
/// drift rule is the released v1 rule, and an unfinished legacy history is
/// rejected — pending legacy work completes under v1 before import.
pub fn verify_legacy_continuity(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
) -> Result<(), Diagnostic> {
    if let Some(first) = validate_graph(graph, applied).first() {
        return Err(failure(
            "migration_legacy_import_invalid_graph",
            "legacy dependency graph or applied ledger is structurally invalid",
        )
        .with_detail("migration", legacy_key(&first.app_label, &first.name))
        .with_detail("validation", format!("{:?}", first.code)));
    }
    let drift = checksum_drift_errors(graph, applied);
    if let Some(first) = drift.first() {
        return Err(failure(
            "migration_legacy_import_checksum_drift",
            "legacy applied ledger disagrees with the loaded legacy files",
        )
        .with_detail("migration", legacy_key(&first.app_label, &first.name))
        .with_detail("stored_checksum", first.stored_checksum.clone()));
    }
    for migration in &graph.migrations {
        let is_applied = applied
            .iter()
            .any(|record| record.app_label == migration.app_label && record.name == migration.name);
        if !is_applied {
            return Err(failure(
                "migration_legacy_import_pending_migration",
                "legacy history has unapplied migrations; complete them under v1 first",
            )
            .with_detail(
                "migration",
                legacy_key(&migration.app_label, &migration.name),
            ));
        }
    }
    Ok(())
}

fn legacy_migration_id(app_label: &str, name: &str) -> Result<LegacyMigrationId, Diagnostic> {
    LegacyMigrationId::new(app_label.to_owned(), name.to_owned())
}

fn reference_from_parts(
    app_label: &str,
    name: &str,
    checksum: Option<&str>,
) -> Result<LegacyMigrationReference, Diagnostic> {
    let checksum = checksum.ok_or_else(|| {
        failure(
            "migration_legacy_import_missing_checksum",
            "legacy migration carries no recorded checksum",
        )
        .with_detail("migration", legacy_key(app_label, name))
    })?;
    Ok(LegacyMigrationReference::new(
        legacy_migration_id(app_label, name)?,
        LegacyMigrationChecksum::new(checksum)?,
    ))
}

fn legacy_key(app_label: &str, name: &str) -> String {
    format!("{app_label}:{name}")
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static legacy import diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use type_bridge_migration::{MigrationDependencySpec, MigrationSpec};

    use super::*;

    fn spec(name: &str, dependencies: Vec<(&str, &str)>, checksum: Option<&str>) -> MigrationSpec {
        MigrationSpec {
            app_label: "example".to_owned(),
            name: name.to_owned(),
            dependencies: dependencies
                .into_iter()
                .map(|(app_label, migration_name)| MigrationDependencySpec {
                    app_label: app_label.to_owned(),
                    migration_name: migration_name.to_owned(),
                })
                .collect(),
            operations: Vec::new(),
            checksum: checksum.map(str::to_owned),
            source_sha256: None,
            reversible: true,
        }
    }

    fn applied(name: &str, checksum: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: "example".to_owned(),
            name: name.to_owned(),
            checksum: checksum.to_owned(),
            applied_at: None,
        }
    }

    #[test]
    fn frontier_is_the_checked_head_set() {
        let graph = MigrationGraph {
            migrations: vec![
                spec("0001_initial", vec![], Some("0123456789abcdef")),
                spec(
                    "0002_addresses",
                    vec![("example", "0001_initial")],
                    Some("fedcba9876543210"),
                ),
            ],
        };
        let frontier = extract_legacy_frontier(&graph).expect("legacy frontier");
        assert_eq!(frontier.len(), 1);
        assert_eq!(frontier[0].id().name().as_str(), "0002_addresses");
        assert_eq!(frontier[0].checksum().as_str(), "fedcba9876543210");

        let unchecksummed = MigrationGraph {
            migrations: vec![spec("0001_initial", vec![], None)],
        };
        let missing = extract_legacy_frontier(&unchecksummed)
            .expect_err("a frontier head requires a checksum");
        assert_eq!(
            missing.code().as_str(),
            "migration_legacy_import_missing_checksum"
        );
    }

    #[test]
    fn frontier_preserves_nonportable_released_identity_spelling() {
        let mut migration = spec("0002_A:B with space", Vec::new(), Some("0123456789abcdef"));
        migration.app_label = "Legacy App: Ω".to_owned();
        let frontier = extract_legacy_frontier(&MigrationGraph {
            migrations: vec![migration],
        })
        .expect("released identity extracts losslessly");

        assert_eq!(frontier[0].id().app_label().as_str(), "Legacy App: Ω");
        assert_eq!(frontier[0].id().name().as_str(), "0002_A:B with space");
    }

    #[test]
    fn continuity_requires_a_fully_applied_undrifted_ledger() {
        let graph = MigrationGraph {
            migrations: vec![
                spec("0001_initial", vec![], Some("0123456789abcdef")),
                spec(
                    "0002_addresses",
                    vec![("example", "0001_initial")],
                    Some("fedcba9876543210"),
                ),
            ],
        };
        verify_legacy_continuity(
            &graph,
            &[
                applied("0001_initial", "0123456789abcdef"),
                applied("0002_addresses", "fedcba9876543210"),
            ],
        )
        .expect("continuous ledger imports");

        let drift = verify_legacy_continuity(
            &graph,
            &[
                applied("0001_initial", "aaaaaaaaaaaaaaaa"),
                applied("0002_addresses", "fedcba9876543210"),
            ],
        )
        .expect_err("a drifted checksum blocks import");
        assert_eq!(
            drift.code().as_str(),
            "migration_legacy_import_checksum_drift"
        );

        let pending =
            verify_legacy_continuity(&graph, &[applied("0001_initial", "0123456789abcdef")])
                .expect_err("pending legacy work blocks import");
        assert_eq!(
            pending.code().as_str(),
            "migration_legacy_import_pending_migration"
        );
    }

    #[test]
    fn complete_applied_digest_ignores_order_and_timestamp_but_detects_every_row_drift() {
        let graph = MigrationGraph {
            migrations: vec![
                spec("0001_initial", vec![], Some("0123456789abcdef")),
                spec(
                    "0002_addresses",
                    vec![("example", "0001_initial")],
                    Some("fedcba9876543210"),
                ),
            ],
        };
        let expected = extract_legacy_applied_set_digest(&graph).expect("graph digest");
        let mut first = applied("0001_initial", "0123456789abcdef");
        first.applied_at = Some("2020-01-01T00:00:00.000000".to_owned());
        let mut second = applied("0002_addresses", "fedcba9876543210");
        second.applied_at = Some("2099-12-31T23:59:59.999999".to_owned());
        assert_eq!(
            digest_legacy_applied_records(&[second.clone(), first.clone()])
                .expect("reordered ledger digest"),
            expected,
        );

        assert_ne!(
            digest_legacy_applied_records(&[first.clone()]).expect("missing row digest"),
            expected,
        );
        let mut extra = applied("0003_extra", "aaaaaaaaaaaaaaaa");
        extra.app_label = "应用 Ω".to_owned();
        extra.name = "迁移 🚀".to_owned();
        assert_ne!(
            digest_legacy_applied_records(&[first.clone(), second.clone(), extra])
                .expect("extra UTF-8 row digest"),
            expected,
        );
        second.checksum = "aaaaaaaaaaaaaaaa".to_owned();
        assert_ne!(
            digest_legacy_applied_records(&[first, second]).expect("checksum drift digest"),
            expected,
        );
    }

    #[test]
    fn applied_noop_merge_history_preserves_its_checksum_bound_frontier() {
        let graph = MigrationGraph {
            migrations: vec![
                spec("0001_initial", vec![], Some("1111111111111111")),
                spec(
                    "0002_left_empty",
                    vec![("example", "0001_initial")],
                    Some("2222222222222222"),
                ),
                spec(
                    "0003_right_empty",
                    vec![("example", "0001_initial")],
                    Some("3333333333333333"),
                ),
                spec(
                    "0004_empty_merge",
                    vec![
                        ("example", "0002_left_empty"),
                        ("example", "0003_right_empty"),
                    ],
                    Some("4444444444444444"),
                ),
            ],
        };
        let applied = [
            applied("0001_initial", "1111111111111111"),
            applied("0002_left_empty", "2222222222222222"),
            applied("0003_right_empty", "3333333333333333"),
            applied("0004_empty_merge", "4444444444444444"),
        ];

        verify_legacy_continuity(&graph, &applied)
            .expect("a fully applied no-op merge history remains continuous");
        let frontier = extract_legacy_frontier(&graph).expect("merge frontier extracts");
        assert_eq!(frontier.len(), 1);
        assert_eq!(frontier[0].id().name().as_str(), "0004_empty_merge");
        assert_eq!(frontier[0].checksum().as_str(), "4444444444444444");
    }

    #[test]
    fn continuity_runs_the_frozen_graph_validator_first() {
        let graph = MigrationGraph {
            migrations: vec![spec(
                "0001_initial",
                vec![("example", "9999_missing")],
                Some("0123456789abcdef"),
            )],
        };
        let error =
            verify_legacy_continuity(&graph, &[applied("0001_initial", "0123456789abcdef")])
                .expect_err("missing dependency must fail before frontier extraction");
        assert_eq!(
            error.code().as_str(),
            "migration_legacy_import_invalid_graph"
        );
    }
}
