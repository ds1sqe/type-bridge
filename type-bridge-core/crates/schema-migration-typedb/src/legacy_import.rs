//! Legacy (v1) frontier extraction and applied-ledger continuity checks.
//!
//! The archival v1 crate stays the only reader of legacy artifacts: this
//! module adapts its checked loader, drift rule, and state store into the
//! canonical import flow without re-encoding legacy semantics. Import never
//! replays legacy operations — it verifies files, ledger, and live state,
//! then applies the zero-operation bridge as a pure journal checkpoint.

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName,
};
use type_bridge_migration::{
    AppliedMigrationRecord, MigrationGraph, checksum_drift_errors,
};
use type_bridge_schema_migration::{
    LegacyMigrationChecksum, LegacyMigrationReference,
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
        let checksum = migration.checksum.as_deref().ok_or_else(|| {
            failure(
                "migration_legacy_import_missing_checksum",
                "legacy frontier migration carries no recorded checksum",
            )
            .with_detail("migration", legacy_key(&migration.app_label, &migration.name))
        })?;
        references.push(LegacyMigrationReference::new(
            legacy_migration_id(&migration.app_label, &migration.name)?,
            LegacyMigrationChecksum::new(checksum)?,
        ));
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
        let is_applied = applied.iter().any(|record| {
            record.app_label == migration.app_label && record.name == migration.name
        });
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

fn legacy_migration_id(app_label: &str, name: &str) -> Result<MigrationId, Diagnostic> {
    Ok(MigrationId::from_components(
        MigrationAppLabel::new(app_label.to_owned())?,
        MigrationName::new(name.to_owned())?,
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

    fn spec(
        name: &str,
        dependencies: Vec<(&str, &str)>,
        checksum: Option<&str>,
    ) -> MigrationSpec {
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

        let pending = verify_legacy_continuity(
            &graph,
            &[applied("0001_initial", "0123456789abcdef")],
        )
        .expect_err("pending legacy work blocks import");
        assert_eq!(
            pending.code().as_str(),
            "migration_legacy_import_pending_migration"
        );
    }
}
