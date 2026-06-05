//! Migration checksum calculation and drift detection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::graph::AppliedMigrationRecord;
use crate::{MigrationError, MigrationGraph};

/// Structured checksum drift details for an applied migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecksumDrift {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Checksum recorded when this migration was applied.
    pub stored_checksum: String,
    /// Current loaded migration checksum, or absent if the loaded graph has none.
    #[serde(default)]
    pub current_checksum: Option<String>,
    /// Human-readable drift message.
    pub message: String,
}

/// Calculate the migration-file checksum used for drift detection.
pub fn migration_file_checksum(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    format!("{digest:x}")[..16].to_string()
}

/// Return all checksum drift errors for applied records.
pub fn checksum_drift_errors(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
) -> Vec<ChecksumDrift> {
    let current_by_key: BTreeMap<(&str, &str), Option<&str>> = graph
        .migrations
        .iter()
        .map(|migration| {
            (
                (migration.app_label.as_str(), migration.name.as_str()),
                migration.checksum.as_deref(),
            )
        })
        .collect();

    let mut errors = Vec::new();
    for record in applied {
        let key = (record.app_label.as_str(), record.name.as_str());
        let Some(current) = current_by_key.get(&key) else {
            errors.push(ChecksumDrift {
                app_label: record.app_label.clone(),
                name: record.name.clone(),
                stored_checksum: record.checksum.clone(),
                current_checksum: None,
                message: format!(
                    "Applied migration {}.{} is not present in the loaded graph",
                    record.app_label, record.name
                ),
            });
            continue;
        };

        match current {
            Some(current_checksum) if *current_checksum == record.checksum => {}
            Some(current_checksum) => errors.push(ChecksumDrift {
                app_label: record.app_label.clone(),
                name: record.name.clone(),
                stored_checksum: record.checksum.clone(),
                current_checksum: Some((*current_checksum).to_string()),
                message: format!(
                    "Applied migration {}.{} checksum drifted: stored {}, current {}",
                    record.app_label, record.name, record.checksum, current_checksum
                ),
            }),
            None => errors.push(ChecksumDrift {
                app_label: record.app_label.clone(),
                name: record.name.clone(),
                stored_checksum: record.checksum.clone(),
                current_checksum: None,
                message: format!(
                    "Applied migration {}.{} has no current checksum",
                    record.app_label, record.name
                ),
            }),
        }
    }

    errors
}

/// Fail if any applied migration checksum has drifted from the loaded graph.
pub fn check_checksum_drift(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
) -> crate::Result<()> {
    let mut errors = checksum_drift_errors(graph, applied);
    if let Some(drift) = errors.pop() {
        Err(MigrationError::ChecksumDrift {
            message: drift.message.clone(),
            drift: Box::new(drift),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::AppliedMigrationRecord;
    use crate::{MigrationGraph, MigrationSpec};

    fn migration(name: &str, checksum: Option<&str>) -> MigrationSpec {
        MigrationSpec {
            app_label: "app".to_string(),
            name: name.to_string(),
            dependencies: Vec::new(),
            operations: Vec::new(),
            checksum: checksum.map(str::to_string),
            reversible: true,
        }
    }

    fn graph(migrations: Vec<MigrationSpec>) -> MigrationGraph {
        MigrationGraph { migrations }
    }

    fn applied(name: &str, checksum: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: "app".to_string(),
            name: name.to_string(),
            checksum: checksum.to_string(),
            applied_at: None,
        }
    }

    #[test]
    fn checksum_matches_python_sha256_prefix_behavior() {
        assert_eq!(
            migration_file_checksum("define attribute name, value string;\n"),
            "cdccf75a826f5ff7"
        );
    }

    #[test]
    fn matching_applied_checksum_has_no_drift() {
        let graph = graph(vec![migration("0001_initial", Some("abc"))]);

        assert_eq!(
            checksum_drift_errors(&graph, &[applied("0001_initial", "abc")]),
            Vec::new()
        );
        assert!(check_checksum_drift(&graph, &[applied("0001_initial", "abc")]).is_ok());
    }

    #[test]
    fn checksum_mismatch_reports_drift_details() {
        let graph = graph(vec![migration("0001_initial", Some("current"))]);
        let errors = checksum_drift_errors(&graph, &[applied("0001_initial", "stored")]);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].stored_checksum, "stored");
        assert_eq!(errors[0].current_checksum.as_deref(), Some("current"));
        assert!(check_checksum_drift(&graph, &[applied("0001_initial", "stored")]).is_err());
    }

    #[test]
    fn missing_current_checksum_for_applied_migration_is_drift() {
        let graph = graph(vec![migration("0001_initial", None)]);
        let errors = checksum_drift_errors(&graph, &[applied("0001_initial", "stored")]);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].current_checksum, None);
    }

    #[test]
    fn unknown_applied_migration_is_drift() {
        let graph = graph(vec![migration("0001_initial", Some("abc"))]);
        let errors = checksum_drift_errors(&graph, &[applied("0002_unknown", "stored")]);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "0002_unknown");
    }
}
