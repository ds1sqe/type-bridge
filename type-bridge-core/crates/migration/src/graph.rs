//! Pure migration graph validation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{MigrationDependencySpec, MigrationGraph};

type MigrationKey = (String, String);

/// Applied migration record as loaded from migration state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedMigrationRecord {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Checksum recorded when this migration was applied.
    pub checksum: String,
    /// Optional application timestamp carried through the Python compatibility DTO.
    #[serde(default)]
    pub applied_at: Option<String>,
}

/// Stable validation error code for machine-readable assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCode {
    /// Migration app label is empty.
    EmptyAppLabel,
    /// Migration name is empty.
    EmptyName,
    /// More than one loaded migration has the same app/name identity.
    DuplicateMigration,
    /// More than one migration in an app uses the same numeric prefix.
    DuplicateMigrationNumber,
    /// A migration depends on itself.
    SelfDependency,
    /// A dependency target is not present in the loaded graph.
    MissingDependency,
    /// A dependency cycle was detected.
    DependencyCycle,
    /// More than one applied record has the same app/name identity.
    DuplicateAppliedRecord,
    /// Applied state references a migration not present in the loaded graph.
    UnknownAppliedMigration,
    /// An applied migration has an unapplied dependency.
    AppliedDependencyMissing,
}

/// Structured migration validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationValidationError {
    /// Stable validation code.
    pub code: ValidationCode,
    /// App label associated with the failing migration when available.
    pub app_label: String,
    /// Migration name associated with the failing migration when available.
    pub name: String,
    /// Human-readable validation message.
    pub message: String,
}

/// Validate graph and applied-state consistency, returning all deterministic errors.
pub fn validate_graph(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
) -> Vec<MigrationValidationError> {
    let mut errors = Vec::new();
    let graph_keys = validate_loaded_migrations(graph, &mut errors);
    validate_dependencies(graph, &graph_keys, &mut errors);
    validate_cycles(graph, &graph_keys, &mut errors);
    validate_applied_records(graph, applied, &graph_keys, &mut errors);
    errors
}

fn validate_loaded_migrations(
    graph: &MigrationGraph,
    errors: &mut Vec<MigrationValidationError>,
) -> BTreeSet<MigrationKey> {
    let mut keys = BTreeSet::new();
    let mut seen_keys = BTreeSet::new();
    let mut numbers: BTreeMap<(String, String), String> = BTreeMap::new();

    for migration in &graph.migrations {
        if migration.app_label.is_empty() {
            errors.push(error(
                ValidationCode::EmptyAppLabel,
                &migration.app_label,
                &migration.name,
                format!("Migration {} has an empty app label", migration.name),
            ));
        }
        if migration.name.is_empty() {
            errors.push(error(
                ValidationCode::EmptyName,
                &migration.app_label,
                &migration.name,
                format!("Migration {} has an empty name", migration.app_label),
            ));
        }

        let key = (migration.app_label.clone(), migration.name.clone());
        if !seen_keys.insert(key.clone()) {
            errors.push(error(
                ValidationCode::DuplicateMigration,
                &migration.app_label,
                &migration.name,
                format!(
                    "Migration {}.{} is defined more than once",
                    migration.app_label, migration.name
                ),
            ));
        }
        keys.insert(key);

        if let Some(number) = migration_number(&migration.name) {
            let number_key = (migration.app_label.clone(), number);
            if let Some(existing_name) = numbers.get(&number_key) {
                if existing_name != &migration.name {
                    errors.push(error(
                        ValidationCode::DuplicateMigrationNumber,
                        &migration.app_label,
                        &migration.name,
                        format!(
                            "Migration {}.{} uses the same number as {}.{}",
                            migration.app_label, migration.name, migration.app_label, existing_name
                        ),
                    ));
                }
            } else {
                numbers.insert(number_key, migration.name.clone());
            }
        }
    }

    keys
}

fn validate_dependencies(
    graph: &MigrationGraph,
    graph_keys: &BTreeSet<MigrationKey>,
    errors: &mut Vec<MigrationValidationError>,
) {
    for migration in &graph.migrations {
        let migration_key = (migration.app_label.clone(), migration.name.clone());
        for dependency in &migration.dependencies {
            let dependency_key = dependency_key(dependency);
            if dependency_key == migration_key {
                errors.push(error(
                    ValidationCode::SelfDependency,
                    &migration.app_label,
                    &migration.name,
                    format!(
                        "Migration {}.{} depends on itself",
                        migration.app_label, migration.name
                    ),
                ));
            } else if !graph_keys.contains(&dependency_key) {
                errors.push(error(
                    ValidationCode::MissingDependency,
                    &migration.app_label,
                    &migration.name,
                    format!(
                        "Migration {}.{} depends on {}.{} which does not exist",
                        migration.app_label,
                        migration.name,
                        dependency.app_label,
                        dependency.migration_name
                    ),
                ));
            }
        }
    }
}

fn validate_cycles(
    graph: &MigrationGraph,
    graph_keys: &BTreeSet<MigrationKey>,
    errors: &mut Vec<MigrationValidationError>,
) {
    let mut adjacency: BTreeMap<MigrationKey, Vec<MigrationKey>> = BTreeMap::new();
    for migration in &graph.migrations {
        let key = (migration.app_label.clone(), migration.name.clone());
        let dependencies = migration
            .dependencies
            .iter()
            .map(dependency_key)
            .filter(|dependency| graph_keys.contains(dependency))
            .collect();
        adjacency.insert(key, dependencies);
    }

    let mut state: BTreeMap<MigrationKey, VisitState> = BTreeMap::new();
    let mut stack = Vec::new();
    let mut reported = BTreeSet::new();

    for key in adjacency.keys() {
        detect_cycle(
            key,
            &adjacency,
            &mut state,
            &mut stack,
            &mut reported,
            errors,
        );
    }
}

fn detect_cycle(
    key: &MigrationKey,
    adjacency: &BTreeMap<MigrationKey, Vec<MigrationKey>>,
    state: &mut BTreeMap<MigrationKey, VisitState>,
    stack: &mut Vec<MigrationKey>,
    reported: &mut BTreeSet<String>,
    errors: &mut Vec<MigrationValidationError>,
) {
    match state.get(key) {
        Some(VisitState::Done) => return,
        Some(VisitState::Visiting) => {
            report_cycle(key, stack, reported, errors);
            return;
        }
        None => {}
    }

    state.insert(key.clone(), VisitState::Visiting);
    stack.push(key.clone());

    if let Some(dependencies) = adjacency.get(key) {
        for dependency in dependencies {
            detect_cycle(dependency, adjacency, state, stack, reported, errors);
        }
    }

    stack.pop();
    state.insert(key.clone(), VisitState::Done);
}

fn report_cycle(
    key: &MigrationKey,
    stack: &[MigrationKey],
    reported: &mut BTreeSet<String>,
    errors: &mut Vec<MigrationValidationError>,
) {
    let Some(start) = stack.iter().position(|stack_key| stack_key == key) else {
        return;
    };
    let mut cycle = stack[start..].to_vec();
    cycle.push(key.clone());
    let message_path = cycle
        .iter()
        .map(|(app_label, name)| format!("{app_label}.{name}"))
        .collect::<Vec<_>>()
        .join(" -> ");
    if reported.insert(message_path.clone()) {
        errors.push(error(
            ValidationCode::DependencyCycle,
            &key.0,
            &key.1,
            format!("Migration dependency cycle detected: {message_path}"),
        ));
    }
}

fn validate_applied_records(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
    graph_keys: &BTreeSet<MigrationKey>,
    errors: &mut Vec<MigrationValidationError>,
) {
    let mut applied_keys = BTreeSet::new();
    for record in applied {
        let key = (record.app_label.clone(), record.name.clone());
        if !applied_keys.insert(key.clone()) {
            errors.push(error(
                ValidationCode::DuplicateAppliedRecord,
                &record.app_label,
                &record.name,
                format!(
                    "Applied migration {}.{} is recorded more than once",
                    record.app_label, record.name
                ),
            ));
        }
        if !graph_keys.contains(&key) {
            errors.push(error(
                ValidationCode::UnknownAppliedMigration,
                &record.app_label,
                &record.name,
                format!(
                    "Applied migration {}.{} is not present in the loaded graph",
                    record.app_label, record.name
                ),
            ));
        }
    }

    for migration in &graph.migrations {
        let key = (migration.app_label.clone(), migration.name.clone());
        if !applied_keys.contains(&key) {
            continue;
        }
        for dependency in &migration.dependencies {
            let dependency_key = dependency_key(dependency);
            if graph_keys.contains(&dependency_key) && !applied_keys.contains(&dependency_key) {
                errors.push(error(
                    ValidationCode::AppliedDependencyMissing,
                    &migration.app_label,
                    &migration.name,
                    format!(
                        "Applied migration {}.{} depends on unapplied migration {}.{}",
                        migration.app_label,
                        migration.name,
                        dependency.app_label,
                        dependency.migration_name
                    ),
                ));
            }
        }
    }
}

fn dependency_key(dependency: &MigrationDependencySpec) -> MigrationKey {
    (
        dependency.app_label.clone(),
        dependency.migration_name.clone(),
    )
}

fn migration_number(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.len() < 5 || bytes[4] != b'_' {
        return None;
    }
    let prefix = &bytes[..4];
    if prefix.iter().all(u8::is_ascii_digit) {
        Some(name[..4].to_string())
    } else {
        None
    }
}

fn error(
    code: ValidationCode,
    app_label: &str,
    name: &str,
    message: String,
) -> MigrationValidationError {
    MigrationValidationError {
        code,
        app_label: app_label.to_string(),
        name: name.to_string(),
        message,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Done,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MigrationGraph, MigrationSpec};

    fn migration(name: &str, dependencies: Vec<(&str, &str)>) -> MigrationSpec {
        MigrationSpec {
            app_label: "app".to_string(),
            name: name.to_string(),
            dependencies: dependencies
                .into_iter()
                .map(|(app_label, migration_name)| MigrationDependencySpec {
                    app_label: app_label.to_string(),
                    migration_name: migration_name.to_string(),
                })
                .collect(),
            operations: Vec::new(),
            checksum: Some(format!("{name}-checksum")),
            reversible: true,
        }
    }

    fn graph(migrations: Vec<MigrationSpec>) -> MigrationGraph {
        MigrationGraph { migrations }
    }

    fn applied(name: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: "app".to_string(),
            name: name.to_string(),
            checksum: format!("{name}-checksum"),
            applied_at: Some("2026-06-05T00:00:00".to_string()),
        }
    }

    fn codes(errors: &[MigrationValidationError]) -> Vec<ValidationCode> {
        errors.iter().map(|error| error.code).collect()
    }

    #[test]
    fn valid_graph_has_no_errors() {
        let graph = graph(vec![
            migration("0001_initial", vec![]),
            migration("0002_next", vec![("app", "0001_initial")]),
        ]);

        assert_eq!(
            validate_graph(&graph, &[applied("0001_initial")]),
            Vec::new()
        );
    }

    #[test]
    fn duplicate_migration_identity_is_reported() {
        let graph = graph(vec![
            migration("0001_initial", vec![]),
            migration("0001_initial", vec![]),
        ]);

        assert_eq!(
            codes(&validate_graph(&graph, &[])),
            vec![ValidationCode::DuplicateMigration]
        );
    }

    #[test]
    fn duplicate_migration_number_is_reported() {
        let graph = graph(vec![
            migration("0001_initial", vec![]),
            migration("0001_second", vec![]),
        ]);

        assert_eq!(
            codes(&validate_graph(&graph, &[])),
            vec![ValidationCode::DuplicateMigrationNumber]
        );
    }

    #[test]
    fn missing_dependency_is_reported() {
        let graph = graph(vec![migration("0002_next", vec![("app", "0001_initial")])]);

        assert_eq!(
            codes(&validate_graph(&graph, &[])),
            vec![ValidationCode::MissingDependency]
        );
    }

    #[test]
    fn self_dependency_is_reported() {
        let graph = graph(vec![migration(
            "0001_initial",
            vec![("app", "0001_initial")],
        )]);

        assert_eq!(
            codes(&validate_graph(&graph, &[])),
            vec![
                ValidationCode::SelfDependency,
                ValidationCode::DependencyCycle
            ]
        );
    }

    #[test]
    fn dependency_cycle_is_reported() {
        let graph = graph(vec![
            migration("0001_initial", vec![("app", "0002_next")]),
            migration("0002_next", vec![("app", "0001_initial")]),
        ]);

        assert_eq!(
            codes(&validate_graph(&graph, &[])),
            vec![ValidationCode::DependencyCycle]
        );
    }

    #[test]
    fn unknown_applied_record_is_reported() {
        let graph = graph(vec![migration("0001_initial", vec![])]);

        assert_eq!(
            codes(&validate_graph(&graph, &[applied("0002_unknown")])),
            vec![ValidationCode::UnknownAppliedMigration]
        );
    }

    #[test]
    fn applied_dependency_gap_is_reported() {
        let graph = graph(vec![
            migration("0001_initial", vec![]),
            migration("0002_next", vec![("app", "0001_initial")]),
        ]);

        assert_eq!(
            codes(&validate_graph(&graph, &[applied("0002_next")])),
            vec![ValidationCode::AppliedDependencyMissing]
        );
    }
}
