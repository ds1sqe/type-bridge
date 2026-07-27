//! Pure migration graph validation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{MigrationDependencySpec, MigrationGraph};

type MigrationKey = (String, String);
type MigrationBackedge = (MigrationKey, MigrationKey);

const CYCLE_PATH_SEPARATOR: &str = " -> ";
// Bound aggregate diagnostic construction independently of graph density while
// retaining exact released paths for ordinary migration histories.
const MAX_CYCLE_DIAGNOSTIC_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_CYCLE_DIAGNOSTIC_PATHS: usize = 65_536;

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

    let mut traversal = CycleTraversal::new();

    for key in adjacency.keys() {
        detect_cycle(key, &adjacency, &mut traversal, errors);
    }
}

fn detect_cycle(
    key: &MigrationKey,
    adjacency: &BTreeMap<MigrationKey, Vec<MigrationKey>>,
    traversal: &mut CycleTraversal,
    errors: &mut Vec<MigrationValidationError>,
) {
    match traversal.state.get(key) {
        Some(VisitState::Done) => return,
        Some(VisitState::Visiting) => {
            report_cycle(key, traversal, errors);
            return;
        }
        None => {}
    }

    traversal.state.insert(key.clone(), VisitState::Visiting);
    traversal
        .stack_positions
        .insert(key.clone(), traversal.stack.len());
    traversal.stack.push(key.clone());
    // A node enters the frame stack only from the unseen state, so the
    // retained traversal is strictly bounded by the finite adjacency map.
    // Grow per component: reserving the entire graph for every disconnected
    // root would itself make an otherwise edgeless graph quadratic.
    let mut frames = Vec::new();
    frames.push(TraversalFrame {
        key: key.clone(),
        next_dependency: 0,
    });

    while let Some(frame) = frames.last_mut() {
        let dependency = adjacency
            .get(&frame.key)
            .and_then(|dependencies| dependencies.get(frame.next_dependency))
            .cloned();
        if let Some(dependency) = dependency {
            frame.next_dependency += 1;
            match traversal.state.get(&dependency) {
                Some(VisitState::Done) => {}
                Some(VisitState::Visiting) => {
                    report_cycle(&dependency, traversal, errors);
                }
                None => {
                    traversal
                        .state
                        .insert(dependency.clone(), VisitState::Visiting);
                    traversal
                        .stack_positions
                        .insert(dependency.clone(), traversal.stack.len());
                    traversal.stack.push(dependency.clone());
                    frames.push(TraversalFrame {
                        key: dependency,
                        next_dependency: 0,
                    });
                }
            }
            continue;
        }

        let Some(completed) = frames.pop() else {
            break;
        };
        debug_assert_eq!(traversal.stack.last(), Some(&completed.key));
        traversal.stack.pop();
        traversal.stack_positions.remove(&completed.key);
        traversal.state.insert(completed.key, VisitState::Done);
    }
}

fn report_cycle(
    key: &MigrationKey,
    traversal: &mut CycleTraversal,
    errors: &mut Vec<MigrationValidationError>,
) {
    if traversal.diagnostic_budget.exhausted {
        return;
    }
    let Some(&start) = traversal.stack_positions.get(key) else {
        return;
    };
    let Some(current) = traversal.stack.last() else {
        return;
    };
    // A node is discovered only once, so its active DFS path is fixed. The
    // current-node/ancestor pair therefore identifies the released path
    // exactly and filters only duplicate dependency edges.
    if !traversal
        .reported_backedges
        .insert((current.clone(), key.clone()))
    {
        return;
    }

    let cycle_nodes = &traversal.stack[start..];
    let path_bytes = cycle_path_bytes(cycle_nodes, key);
    if !traversal.diagnostic_budget.reserve(path_bytes) {
        errors.push(error(
            ValidationCode::DependencyCycle,
            &key.0,
            &key.1,
            format!(
                "Migration dependency cycle detected; additional cycle paths omitted after the \
                 bounded {MAX_CYCLE_DIAGNOSTIC_PATH_BYTES}-byte / \
                 {MAX_CYCLE_DIAGNOSTIC_PATHS}-path diagnostic budget was exhausted"
            ),
        ));
        return;
    }

    let mut message_path = String::with_capacity(path_bytes);
    for cycle_key in cycle_nodes {
        push_migration_key(&mut message_path, cycle_key);
        message_path.push_str(CYCLE_PATH_SEPARATOR);
    }
    push_migration_key(&mut message_path, key);
    // Released V1 deduplicated the rendered path, rather than the graph-node
    // identity. Retain that final check for unusual component spellings that
    // render to the same dotted path.
    if !traversal.reported_paths.insert(message_path.clone()) {
        return;
    }
    errors.push(error(
        ValidationCode::DependencyCycle,
        &key.0,
        &key.1,
        format!("Migration dependency cycle detected: {message_path}"),
    ));
}

fn cycle_path_bytes(cycle_nodes: &[MigrationKey], repeated_key: &MigrationKey) -> usize {
    cycle_nodes.iter().fold(
        migration_key_display_bytes(repeated_key),
        |path_bytes, key| {
            path_bytes
                .saturating_add(migration_key_display_bytes(key))
                .saturating_add(CYCLE_PATH_SEPARATOR.len())
        },
    )
}

fn migration_key_display_bytes(key: &MigrationKey) -> usize {
    key.0.len().saturating_add(1).saturating_add(key.1.len())
}

fn push_migration_key(output: &mut String, key: &MigrationKey) {
    output.push_str(&key.0);
    output.push('.');
    output.push_str(&key.1);
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

#[derive(Debug)]
struct TraversalFrame {
    key: MigrationKey,
    next_dependency: usize,
}

#[derive(Debug)]
struct CycleTraversal {
    state: BTreeMap<MigrationKey, VisitState>,
    stack: Vec<MigrationKey>,
    stack_positions: BTreeMap<MigrationKey, usize>,
    reported_backedges: BTreeSet<MigrationBackedge>,
    reported_paths: BTreeSet<String>,
    diagnostic_budget: CycleDiagnosticBudget,
}

impl CycleTraversal {
    fn new() -> Self {
        Self {
            state: BTreeMap::new(),
            stack: Vec::new(),
            stack_positions: BTreeMap::new(),
            reported_backedges: BTreeSet::new(),
            reported_paths: BTreeSet::new(),
            diagnostic_budget: CycleDiagnosticBudget::new(),
        }
    }
}

#[derive(Debug)]
struct CycleDiagnosticBudget {
    remaining_path_bytes: usize,
    remaining_paths: usize,
    exhausted: bool,
}

impl CycleDiagnosticBudget {
    fn new() -> Self {
        Self {
            remaining_path_bytes: MAX_CYCLE_DIAGNOSTIC_PATH_BYTES,
            remaining_paths: MAX_CYCLE_DIAGNOSTIC_PATHS,
            exhausted: false,
        }
    }

    fn reserve(&mut self, path_bytes: usize) -> bool {
        if self.exhausted || self.remaining_paths == 0 || path_bytes > self.remaining_path_bytes {
            self.exhausted = true;
            return false;
        }
        self.remaining_path_bytes -= path_bytes;
        self.remaining_paths -= 1;
        true
    }
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
            source_sha256: None,
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
    fn small_cycle_diagnostic_preserves_the_exact_deterministic_path() {
        let graph = graph(vec![
            migration("0001_initial", vec![("app", "0002_next")]),
            migration("0002_next", vec![("app", "0001_initial")]),
        ]);

        assert_eq!(
            validate_graph(&graph, &[]),
            vec![MigrationValidationError {
                code: ValidationCode::DependencyCycle,
                app_label: "app".to_owned(),
                name: "0001_initial".to_owned(),
                message: "Migration dependency cycle detected: app.0001_initial -> \
                          app.0002_next -> app.0001_initial"
                    .to_owned(),
            }],
        );
    }

    #[test]
    fn overlapping_cycles_preserve_each_released_deterministic_path() {
        let graph = graph(vec![
            migration("0001_a", vec![("app", "0002_b"), ("app", "0003_c")]),
            migration("0002_b", vec![("app", "0001_a")]),
            migration("0003_c", vec![("app", "0001_a")]),
        ]);

        assert_eq!(
            validate_graph(&graph, &[]),
            vec![
                MigrationValidationError {
                    code: ValidationCode::DependencyCycle,
                    app_label: "app".to_owned(),
                    name: "0001_a".to_owned(),
                    message: "Migration dependency cycle detected: app.0001_a -> app.0002_b -> \
                              app.0001_a"
                        .to_owned(),
                },
                MigrationValidationError {
                    code: ValidationCode::DependencyCycle,
                    app_label: "app".to_owned(),
                    name: "0001_a".to_owned(),
                    message: "Migration dependency cycle detected: app.0001_a -> app.0003_c -> \
                              app.0001_a"
                        .to_owned(),
                },
            ],
        );
    }

    #[test]
    fn duplicate_dependency_edges_do_not_duplicate_a_released_cycle_path() {
        let graph = graph(vec![
            migration("0001_a", vec![("app", "0002_b"), ("app", "0002_b")]),
            migration("0002_b", vec![("app", "0001_a")]),
        ]);

        assert_eq!(
            validate_graph(&graph, &[]),
            vec![MigrationValidationError {
                code: ValidationCode::DependencyCycle,
                app_label: "app".to_owned(),
                name: "0001_a".to_owned(),
                message: "Migration dependency cycle detected: app.0001_a -> app.0002_b -> \
                          app.0001_a"
                    .to_owned(),
            }],
        );
    }

    #[test]
    fn directory_ceiling_disconnected_graph_remains_near_linear() {
        const NODE_COUNT: usize = 65_536;
        let migrations = (0..NODE_COUNT)
            .map(|index| migration(&format!("node_{index:05}"), vec![]))
            .collect();
        let started = std::time::Instant::now();

        let errors = validate_graph(&graph(migrations), &[]);

        assert!(errors.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn directory_ceiling_scale_cycle_returns_typed_validation_without_recursion() {
        const NODE_COUNT: usize = 65_536;
        let migrations = (0..NODE_COUNT)
            .map(|index| {
                let name = format!("node_{index:05}");
                let dependency = format!("node_{:05}", (index + 1) % NODE_COUNT);
                migration(&name, vec![("app", &dependency)])
            })
            .collect();
        let started = std::time::Instant::now();

        let errors = validate_graph(&graph(migrations), &[]);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ValidationCode::DependencyCycle);
        assert_eq!(errors[0].app_label, "app");
        assert_eq!(errors[0].name, "node_00000");
        assert!(
            errors[0].message.starts_with(
                "Migration dependency cycle detected: app.node_00000 -> app.node_00001"
            )
        );
        assert!(
            errors[0]
                .message
                .ends_with("app.node_65535 -> app.node_00000")
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn overlapping_deep_backedges_are_amortized_at_the_directory_ceiling() {
        const NODE_COUNT: usize = 65_536;
        let migrations = (0..NODE_COUNT)
            .map(|index| {
                let name = format!("node_{index:05}");
                let dependencies = if index + 1 < NODE_COUNT {
                    vec![("app", format!("node_{:05}", index + 1))]
                } else {
                    std::iter::once(("app", format!("node_{:05}", NODE_COUNT - 2)))
                        .chain(
                            (0..NODE_COUNT - 2).map(|target| ("app", format!("node_{target:05}"))),
                        )
                        .collect()
                };
                MigrationSpec {
                    app_label: "app".to_owned(),
                    name: name.clone(),
                    dependencies: dependencies
                        .into_iter()
                        .map(|(app_label, migration_name)| MigrationDependencySpec {
                            app_label: app_label.to_owned(),
                            migration_name,
                        })
                        .collect(),
                    operations: Vec::new(),
                    checksum: Some(format!("{name}-checksum")),
                    source_sha256: None,
                    reversible: true,
                }
            })
            .collect();
        let started = std::time::Instant::now();

        let errors = validate_graph(&graph(migrations), &[]);

        assert!(errors.len() > 2);
        assert!(
            errors
                .iter()
                .all(|error| error.code == ValidationCode::DependencyCycle)
        );
        assert_eq!(
            errors[0].message,
            "Migration dependency cycle detected: app.node_65534 -> app.node_65535 -> \
             app.node_65534"
        );
        assert!(
            errors[1].message.starts_with(
                "Migration dependency cycle detected: app.node_00000 -> app.node_00001"
            )
        );
        assert_eq!(
            errors.last().map(|error| error.message.as_str()),
            Some(
                "Migration dependency cycle detected; additional cycle paths omitted after the \
                 bounded 16777216-byte / 65536-path diagnostic budget was exhausted"
            )
        );
        let rendered_cycle_bytes = errors
            .iter()
            .filter_map(|error| {
                error
                    .message
                    .strip_prefix("Migration dependency cycle detected: ")
                    .map(str::len)
            })
            .sum::<usize>();
        assert!(rendered_cycle_bytes <= MAX_CYCLE_DIAGNOSTIC_PATH_BYTES);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
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
