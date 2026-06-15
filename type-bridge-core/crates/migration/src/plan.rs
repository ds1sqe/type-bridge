//! Pure migration planner.
//!
//! Lowers a validated [`MigrationGraph`] and applied-state into an ordered
//! [`ExecutionPlan`] of [`ExecutionStep`]s, each carrying its [`TxType`] and
//! the executable TypeQL to run.  No database connection, no async, no TypeDB
//! driver is touched here.

use serde::{Deserialize, Serialize};
use type_bridge_orm::TxType;
use type_bridge_orm::schema::info::{
    AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry, RoleEntry,
    SchemaInfo,
};

use crate::checksum::check_checksum_drift;
use crate::error::MigrationError;
use crate::graph::{AppliedMigrationRecord, validate_graph};
use crate::spec::{MigrationGraph, OperationSpec};

/// The kind of execution step, controlling how the executor dispatches it.
///
/// `Schema` and `Write` run the carried TypeQL directly.  `Backfill` is a
/// write-typed step that additionally derives matched/inserted/skipped counts
/// via bracketing `reduce $c = count;` read queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Schema DDL step — opened under a schema transaction.
    #[default]
    Schema,
    /// Data-write step — opened under a write transaction.
    Write,
    /// Backfill step — write transaction + bracketing count derivation.
    Backfill,
}

/// One executable step within a migration.
///
/// Carries the transaction type and the forward (and optional reverse) TypeQL.
/// Every step maps to exactly one TypeDB transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Transaction type to open for this step.
    pub tx_type: TxType,
    /// Discriminant controlling executor dispatch.
    ///
    /// Defaults to [`StepKind::Schema`] for backwards-compatible deserialization
    /// of steps persisted before this field was introduced.
    #[serde(default)]
    pub kind: StepKind,
    /// Forward (apply) TypeQL text.
    pub forward: String,
    /// Reverse (rollback) TypeQL text, or `None` when the step is
    /// non-reversible.
    pub reverse: Option<String>,
}

/// Whether a migration execution is an apply or a rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationAction {
    /// Apply the migration forward.
    Apply,
    /// Roll the migration back.
    Rollback,
}

/// One migration scheduled for execution, together with its assembled steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationExecution {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Apply or rollback.
    pub action: MigrationAction,
    /// Ordered execution steps.
    pub steps: Vec<ExecutionStep>,
    /// Whether every step in this migration has a reverse.
    ///
    /// `false` when the migration contains a `DefineSchema` op (model-initial
    /// schema; no reverse) or any op whose reverse is `None`.
    pub reversible: bool,
}

/// Ordered execution plan produced by [`plan`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Migrations to apply, in graph (dependency) order.
    pub to_apply: Vec<MigrationExecution>,
    /// Migrations to roll back, in reverse discovery order.
    pub to_rollback: Vec<MigrationExecution>,
}

/// Produce an ordered [`ExecutionPlan`] from a validated graph and applied state.
///
/// # Errors
///
/// - [`MigrationError::Planning`] – graph validation found one or more errors.
/// - [`MigrationError::ChecksumDrift`] – an applied migration's checksum has
///   drifted (hard gate; no plan is produced).
/// - [`MigrationError::UnloweredOperation`] – the graph contains an
///   [`OperationSpec`] variant that is intentionally unsupported by the Rust
///   planner.
pub fn plan(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
    target: Option<&str>,
) -> crate::Result<ExecutionPlan> {
    // Gate 1: structural graph validation (04).
    let errors = validate_graph(graph, applied);
    if !errors.is_empty() {
        return Err(MigrationError::Planning { errors });
    }

    // Gate 2: checksum drift (04 gate, hard stop before any step assembly).
    check_checksum_drift(graph, applied)?;

    // Build an applied-key set for O(1) membership checks.
    let applied_keys: std::collections::BTreeSet<(&str, &str)> = applied
        .iter()
        .map(|r| (r.app_label.as_str(), r.name.as_str()))
        .collect();

    let (to_apply, to_rollback) = if let Some(target_name) = target {
        // Find the target index by name (or app_label::name).
        let target_idx = graph
            .migrations
            .iter()
            .position(|m| {
                m.name == target_name || format!("{}::{}", m.app_label, m.name) == target_name
            })
            .ok_or_else(|| MigrationError::TargetNotFound {
                target: target_name.to_string(),
            })?;

        let mut apply = Vec::new();
        let mut rollback = Vec::new();

        for (i, migration) in graph.migrations.iter().enumerate() {
            let is_applied =
                applied_keys.contains(&(migration.app_label.as_str(), migration.name.as_str()));
            if i <= target_idx && !is_applied {
                apply.push(migration);
            } else if i > target_idx && is_applied {
                rollback.push(migration);
            }
        }

        // Rollbacks go in reverse order (matches Python _create_plan).
        rollback.reverse();
        (apply, rollback)
    } else {
        // Apply all pending migrations.
        let apply: Vec<_> = graph
            .migrations
            .iter()
            .filter(|m| !applied_keys.contains(&(m.app_label.as_str(), m.name.as_str())))
            .collect();
        (apply, Vec::new())
    };

    // Assemble MigrationExecution objects for the apply list.
    let mut apply_executions = Vec::with_capacity(to_apply.len());
    for migration in to_apply {
        let steps = assemble_steps(&migration.operations, migration.reversible)?;
        let reversible = steps.iter().all(|s| s.reverse.is_some());
        apply_executions.push(MigrationExecution {
            app_label: migration.app_label.clone(),
            name: migration.name.clone(),
            action: MigrationAction::Apply,
            steps,
            reversible,
        });
    }

    // Assemble MigrationExecution objects for the rollback list.
    let mut rollback_executions = Vec::with_capacity(to_rollback.len());
    for migration in to_rollback {
        let steps = assemble_steps(&migration.operations, migration.reversible)?;
        let reversible = steps.iter().all(|s| s.reverse.is_some());
        rollback_executions.push(MigrationExecution {
            app_label: migration.app_label.clone(),
            name: migration.name.clone(),
            action: MigrationAction::Rollback,
            steps,
            reversible,
        });
    }

    Ok(ExecutionPlan {
        to_apply: apply_executions,
        to_rollback: rollback_executions,
    })
}

/// Lower a slice of [`OperationSpec`] into [`ExecutionStep`]s.
fn assemble_steps(
    operations: &[OperationSpec],
    migration_reversible: bool,
) -> crate::Result<Vec<ExecutionStep>> {
    let mut steps = Vec::with_capacity(operations.len());
    for op in operations {
        let mut step = match op {
            OperationSpec::RunTypeql { forward, reverse } => {
                let tx_type = run_typeql_tx_type(forward);
                ExecutionStep {
                    tx_type,
                    kind: if tx_type == TxType::Write {
                        StepKind::Write
                    } else {
                        StepKind::Schema
                    },
                    forward: forward.clone(),
                    reverse: reverse.clone(),
                }
            }
            OperationSpec::DefineSchema { schema } => {
                // Route through the existing canonical Rust generator
                // (SchemaInfo::to_typeql → generator::generate_define_block).
                // This is the only TypeQL generation in plan.rs — no per-variant
                // re-derivation for any other OperationSpec (invariant 2).
                let forward = schema
                    .to_typeql()
                    .map_err(|e| MigrationError::SchemaGeneration {
                        message: e.to_string(),
                    })?;
                ExecutionStep {
                    tx_type: TxType::Schema,
                    kind: StepKind::Schema,
                    forward,
                    // Model-initial migrations are non-reversible.
                    reverse: None,
                }
            }
            OperationSpec::AddAttribute { attribute } => schema_step(
                define_attribute(attribute)?,
                Some(undefine_attribute(&attribute.attr_name)),
            ),
            OperationSpec::RemoveAttribute { attr_name } => {
                schema_step(undefine_attribute(attr_name), None)
            }
            OperationSpec::AddEntity { entity } => schema_step(
                define_entity(entity)?,
                Some(undefine_entity(&entity.type_name)),
            ),
            OperationSpec::RemoveEntity { type_name } => {
                schema_step(undefine_entity(type_name), None)
            }
            OperationSpec::AddRelation { relation } => schema_step(
                define_relation(relation)?,
                Some(undefine_relation_with_players(relation)),
            ),
            OperationSpec::RemoveRelation { type_name } => {
                schema_step(undefine_relation(type_name), None)
            }
            OperationSpec::AddOwnership {
                owner_type,
                attribute,
            } => schema_step(
                define_ownership(owner_type, attribute),
                Some(undefine_ownership(
                    owner_type,
                    &owned_attribute_type_ref(attribute),
                )),
            ),
            OperationSpec::RemoveOwnership {
                owner_type,
                attr_name,
            } => schema_step(undefine_ownership(owner_type, attr_name), None),
            OperationSpec::ModifyOwnership {
                owner_type,
                attr_name,
                old_annotations,
                new_annotations,
            } => schema_step(
                redefine_ownership(owner_type, attr_name, new_annotations),
                Some(redefine_ownership(owner_type, attr_name, old_annotations)),
            ),
            OperationSpec::AddRole {
                relation_type,
                role,
            } => schema_step(
                define_role(relation_type, role),
                Some(undefine_role_with_players(relation_type, role)),
            ),
            OperationSpec::RemoveRole {
                relation_type,
                role_name,
            } => schema_step(undefine_role(relation_type, role_name), None),
            OperationSpec::AddRolePlayer {
                relation_type,
                role_name,
                player_type_name,
            } => schema_step(
                define_role_player(relation_type, role_name, player_type_name),
                Some(undefine_role_player(
                    relation_type,
                    role_name,
                    player_type_name,
                )),
            ),
            OperationSpec::RemoveRolePlayer {
                relation_type,
                role_name,
                player_type_name,
            } => schema_step(
                undefine_role_player(relation_type, role_name, player_type_name),
                Some(define_role_player(
                    relation_type,
                    role_name,
                    player_type_name,
                )),
            ),
            OperationSpec::CopyAttribute { forward, reverse } => {
                // The backfill TypeQL is carried verbatim from the frozen
                // `CopyAttribute.to_typeql()` (invariant 2: no re-synthesis here).
                // `backfill.rs` composes its count queries from this `forward`
                // text's match clause.
                ExecutionStep {
                    tx_type: TxType::Write,
                    kind: StepKind::Backfill,
                    forward: forward.clone(),
                    reverse: reverse.clone(),
                }
            }
            other @ OperationSpec::RenameAttribute { .. } => {
                return Err(MigrationError::UnloweredOperation {
                    kind: op_kind_name(other).to_string(),
                });
            }
        };
        if !migration_reversible {
            step.reverse = None;
        }
        steps.push(step);
    }
    Ok(steps)
}

fn schema_step(forward: String, reverse: Option<String>) -> ExecutionStep {
    ExecutionStep {
        tx_type: TxType::Schema,
        kind: StepKind::Schema,
        forward,
        reverse,
    }
}

fn run_typeql_tx_type(forward: &str) -> TxType {
    let first_statement = forward
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if first_statement.starts_with("define")
        || first_statement.starts_with("undefine")
        || first_statement.starts_with("redefine")
    {
        TxType::Schema
    } else {
        TxType::Write
    }
}

fn schema_to_typeql(schema: &SchemaInfo) -> crate::Result<String> {
    schema
        .to_typeql()
        .map_err(|e| MigrationError::SchemaGeneration {
            message: e.to_string(),
        })
}

fn define_attribute(attribute: &AttributeSchemaEntry) -> crate::Result<String> {
    let mut schema = SchemaInfo::default();
    schema
        .attributes
        .insert(attribute.attr_name.clone(), attribute.clone());
    schema_to_typeql(&schema)
}

fn undefine_attribute(attr_name: &str) -> String {
    format!("undefine\nattribute {attr_name};")
}

fn define_entity(entity: &EntitySchemaEntry) -> crate::Result<String> {
    let mut schema = SchemaInfo::default();
    schema
        .entities
        .insert(entity.type_name.clone(), entity.clone());
    schema_to_typeql(&schema)
}

fn undefine_entity(type_name: &str) -> String {
    format!("undefine\nentity {type_name};")
}

fn define_relation(relation: &RelationSchemaEntry) -> crate::Result<String> {
    let mut schema = SchemaInfo::default();
    schema
        .relations
        .insert(relation.type_name.clone(), relation.clone());
    schema_to_typeql(&schema)
}

fn undefine_relation(type_name: &str) -> String {
    format!("undefine\nrelation {type_name};")
}

fn undefine_relation_with_players(relation: &RelationSchemaEntry) -> String {
    let mut statements = Vec::new();
    for role in &relation.roles {
        for player_type_name in &role.player_type_names {
            statements.push(format!(
                "{player_type_name} plays {}:{};",
                relation.type_name, role.role_name
            ));
        }
    }
    statements.push(format!("relation {};", relation.type_name));
    typeql_block("undefine", statements)
}

fn define_ownership(owner_type: &str, attribute: &OwnedAttributeEntry) -> String {
    let attr_ref = owned_attribute_type_ref(attribute);
    let flags = annotation_suffix(&attribute.flags_string());
    typeql_block(
        "define",
        vec![format!("{owner_type} owns {attr_ref}{flags};")],
    )
}

fn undefine_ownership(owner_type: &str, attr_name: &str) -> String {
    typeql_block("undefine", vec![format!("{owner_type} owns {attr_name};")])
}

fn redefine_ownership(owner_type: &str, attr_name: &str, annotations: &str) -> String {
    let suffix = annotation_suffix(annotations);
    typeql_block(
        "redefine",
        vec![format!("{owner_type} owns {attr_name}{suffix};")],
    )
}

fn owned_attribute_type_ref(attribute: &OwnedAttributeEntry) -> String {
    if attribute.is_ordered {
        format!("{}[]", attribute.attr_name)
    } else {
        attribute.attr_name.clone()
    }
}

fn define_role(relation_type: &str, role: &RoleEntry) -> String {
    let mut statements = vec![format!(
        "{relation_type} relates {};",
        role_definition(role)
    )];
    for player_type_name in &role.player_type_names {
        statements.push(format!(
            "{player_type_name} plays {relation_type}:{};",
            role.role_name
        ));
    }
    typeql_block("define", statements)
}

fn undefine_role(relation_type: &str, role_name: &str) -> String {
    typeql_block(
        "undefine",
        vec![format!("{relation_type} relates {role_name};")],
    )
}

fn undefine_role_with_players(relation_type: &str, role: &RoleEntry) -> String {
    let mut statements = Vec::new();
    for player_type_name in &role.player_type_names {
        statements.push(format!(
            "{player_type_name} plays {relation_type}:{};",
            role.role_name
        ));
    }
    statements.push(format!("{relation_type} relates {};", role_type_ref(role)));
    typeql_block("undefine", statements)
}

fn define_role_player(relation_type: &str, role_name: &str, player_type_name: &str) -> String {
    typeql_block(
        "define",
        vec![format!(
            "{player_type_name} plays {relation_type}:{role_name};"
        )],
    )
}

fn undefine_role_player(relation_type: &str, role_name: &str, player_type_name: &str) -> String {
    typeql_block(
        "undefine",
        vec![format!(
            "{player_type_name} plays {relation_type}:{role_name};"
        )],
    )
}

fn role_definition(role: &RoleEntry) -> String {
    let mut definition = role_type_ref(role);
    if let Some(ref parent_role) = role.overrides {
        definition.push_str(&format!(" as {parent_role}"));
    }
    if role.is_abstract {
        definition.push_str(" @abstract");
    }
    if role.distinct {
        definition.push_str(" @distinct");
    }
    if let Some((min, max)) = role.cardinality {
        definition.push(' ');
        definition.push_str(&card_annotation(min, max));
    }
    definition
}

fn role_type_ref(role: &RoleEntry) -> String {
    if role.ordered {
        format!("{}[]", role.role_name)
    } else {
        role.role_name.clone()
    }
}

fn card_annotation(min: u32, max: Option<u32>) -> String {
    let max_str = max.map(|value| value.to_string()).unwrap_or_default();
    format!("@card({min}..{max_str})")
}

fn annotation_suffix(annotations: &str) -> String {
    let trimmed = annotations.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(" {trimmed}")
    }
}

fn typeql_block(keyword: &str, statements: Vec<String>) -> String {
    format!("{keyword}\n{}", statements.join("\n"))
}

/// Return a stable string name for an [`OperationSpec`] variant.
///
/// Used only in error messages.
fn op_kind_name(op: &OperationSpec) -> &'static str {
    match op {
        OperationSpec::RunTypeql { .. } => "RunTypeql",
        OperationSpec::DefineSchema { .. } => "DefineSchema",
        OperationSpec::AddAttribute { .. } => "AddAttribute",
        OperationSpec::RemoveAttribute { .. } => "RemoveAttribute",
        OperationSpec::AddEntity { .. } => "AddEntity",
        OperationSpec::RemoveEntity { .. } => "RemoveEntity",
        OperationSpec::AddRelation { .. } => "AddRelation",
        OperationSpec::RemoveRelation { .. } => "RemoveRelation",
        OperationSpec::AddOwnership { .. } => "AddOwnership",
        OperationSpec::RemoveOwnership { .. } => "RemoveOwnership",
        OperationSpec::ModifyOwnership { .. } => "ModifyOwnership",
        OperationSpec::AddRole { .. } => "AddRole",
        OperationSpec::RemoveRole { .. } => "RemoveRole",
        OperationSpec::AddRolePlayer { .. } => "AddRolePlayer",
        OperationSpec::RemoveRolePlayer { .. } => "RemoveRolePlayer",
        OperationSpec::RenameAttribute { .. } => "RenameAttribute",
        OperationSpec::CopyAttribute { .. } => "CopyAttribute",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use type_bridge_orm::schema::info::{
        AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry,
        RoleEntry, SchemaInfo,
    };
    use type_bridge_orm::{Annotation, ValueType};

    use crate::graph::AppliedMigrationRecord;
    use crate::spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn run_typeql(forward: &str, reverse: Option<&str>) -> OperationSpec {
        OperationSpec::RunTypeql {
            forward: forward.to_string(),
            reverse: reverse.map(str::to_string),
        }
    }

    fn define_schema_op() -> OperationSpec {
        let mut schema = SchemaInfo::default();
        schema.attributes.insert(
            "name".to_string(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        schema.entities.insert(
            "person".to_string(),
            EntitySchemaEntry {
                type_name: "person".to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".to_string(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_ordered: false,
                }],
                plays_cardinalities: BTreeMap::new(),
            },
        );
        OperationSpec::DefineSchema { schema }
    }

    fn owned_attr(
        attr_name: &str,
        value_type: ValueType,
        annotations: Vec<Annotation>,
    ) -> OwnedAttributeEntry {
        OwnedAttributeEntry {
            attr_name: attr_name.to_string(),
            value_type,
            annotations,
            is_ordered: false,
        }
    }

    fn entity_entry(type_name: &str) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: type_name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![owned_attr("name", ValueType::String, vec![Annotation::Key])],
            plays_cardinalities: BTreeMap::new(),
        }
    }

    fn relation_entry(type_name: &str) -> RelationSchemaEntry {
        RelationSchemaEntry {
            type_name: type_name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![RoleEntry {
                role_name: "employee".to_string(),
                player_type_names: vec!["person".to_string()],
                cardinality: None,
                overrides: None,
                is_abstract: false,
                ordered: false,
                distinct: false,
            }],
            plays_cardinalities: BTreeMap::new(),
        }
    }

    fn migration(name: &str, ops: Vec<OperationSpec>, deps: Vec<(&str, &str)>) -> MigrationSpec {
        MigrationSpec {
            app_label: "app".to_string(),
            name: name.to_string(),
            dependencies: deps
                .into_iter()
                .map(|(app, dep_name)| MigrationDependencySpec {
                    app_label: app.to_string(),
                    migration_name: dep_name.to_string(),
                })
                .collect(),
            operations: ops,
            checksum: Some(format!("{name}-csum")),
            reversible: true,
        }
    }

    fn applied(name: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: "app".to_string(),
            name: name.to_string(),
            checksum: format!("{name}-csum"),
            applied_at: None,
        }
    }

    fn graph(migrations: Vec<MigrationSpec>) -> MigrationGraph {
        MigrationGraph { migrations }
    }

    // ── test: pending-only ordering (target=None) ────────────────────────────

    #[test]
    fn pending_only_all_pending_applies_in_order() {
        let g = graph(vec![
            migration(
                "0001_initial",
                vec![run_typeql("define attribute a, value string;", None)],
                vec![],
            ),
            migration(
                "0002_add",
                vec![run_typeql(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                vec![("app", "0001_initial")],
            ),
        ]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        assert_eq!(result.to_apply.len(), 2);
        assert_eq!(result.to_rollback.len(), 0);
        assert_eq!(result.to_apply[0].name, "0001_initial");
        assert_eq!(result.to_apply[1].name, "0002_add");
    }

    #[test]
    fn pending_only_already_applied_excluded() {
        let g = graph(vec![
            migration(
                "0001_initial",
                vec![run_typeql("define attribute a, value string;", None)],
                vec![],
            ),
            migration(
                "0002_add",
                vec![run_typeql(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                vec![("app", "0001_initial")],
            ),
        ]);

        let result = plan(&g, &[applied("0001_initial")], None).expect("plan should succeed");

        assert_eq!(result.to_apply.len(), 1);
        assert_eq!(result.to_apply[0].name, "0002_add");
        assert_eq!(result.to_rollback.len(), 0);
    }

    // ── test: target-based apply/rollback split ───────────────────────────────

    #[test]
    fn target_applies_up_to_and_including_target() {
        let g = graph(vec![
            migration(
                "0001_initial",
                vec![run_typeql("define attribute a, value string;", None)],
                vec![],
            ),
            migration(
                "0002_add",
                vec![run_typeql(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                vec![("app", "0001_initial")],
            ),
            migration(
                "0003_more",
                vec![run_typeql(
                    "define attribute c, value string;",
                    Some("undefine attribute c;"),
                )],
                vec![("app", "0002_add")],
            ),
        ]);

        // target = 0002_add; none applied yet.
        let result = plan(&g, &[], Some("0002_add")).expect("plan should succeed");

        assert_eq!(result.to_apply.len(), 2);
        assert_eq!(result.to_apply[0].name, "0001_initial");
        assert_eq!(result.to_apply[1].name, "0002_add");
        assert_eq!(result.to_rollback.len(), 0);
    }

    #[test]
    fn target_rolls_back_past_target_in_reverse_order() {
        let g = graph(vec![
            migration(
                "0001_initial",
                vec![run_typeql("define attribute a, value string;", None)],
                vec![],
            ),
            migration(
                "0002_add",
                vec![run_typeql(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                vec![("app", "0001_initial")],
            ),
            migration(
                "0003_more",
                vec![run_typeql(
                    "define attribute c, value string;",
                    Some("undefine attribute c;"),
                )],
                vec![("app", "0002_add")],
            ),
        ]);

        // All three applied; target = 0001_initial → rollback 0002 and 0003.
        let result = plan(
            &g,
            &[
                applied("0001_initial"),
                applied("0002_add"),
                applied("0003_more"),
            ],
            Some("0001_initial"),
        )
        .expect("plan should succeed");

        assert_eq!(result.to_apply.len(), 0);
        // rollback list must be in reverse order: 0003, then 0002
        assert_eq!(result.to_rollback.len(), 2);
        assert_eq!(result.to_rollback[0].name, "0003_more");
        assert_eq!(result.to_rollback[1].name, "0002_add");
    }

    // ── test: rollback reverse ordering for steps ─────────────────────────────

    #[test]
    fn rollback_execution_action_is_rollback() {
        let g = graph(vec![
            migration(
                "0001_initial",
                vec![run_typeql("define attribute a, value string;", None)],
                vec![],
            ),
            migration(
                "0002_add",
                vec![run_typeql(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                vec![("app", "0001_initial")],
            ),
        ]);

        let result = plan(
            &g,
            &[applied("0001_initial"), applied("0002_add")],
            Some("0001_initial"),
        )
        .expect("plan should succeed");

        assert_eq!(result.to_rollback[0].action, MigrationAction::Rollback);
    }

    // ── test: DefineSchema carries non-empty TypeQL from the generator ─────────

    #[test]
    fn define_schema_step_carries_typeql_from_generator() {
        let g = graph(vec![migration(
            "0001_initial",
            vec![define_schema_op()],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        let exec = &result.to_apply[0];
        assert_eq!(exec.steps.len(), 1);
        let step = &exec.steps[0];
        // Forward must be non-empty TypeQL produced by SchemaInfo::to_typeql().
        assert!(
            !step.forward.is_empty(),
            "DefineSchema forward must be non-empty"
        );
        assert!(
            step.forward.contains("define"),
            "DefineSchema forward must contain 'define'"
        );
        // DefineSchema is non-reversible — no reverse.
        assert!(step.reverse.is_none());
    }

    #[test]
    fn define_schema_step_tx_type_is_schema() {
        let g = graph(vec![migration(
            "0001_initial",
            vec![define_schema_op()],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert_eq!(result.to_apply[0].steps[0].tx_type, TxType::Schema);
    }

    // ── test: per-step TxType is Schema for RunTypeql ─────────────────────────

    #[test]
    fn run_typeql_step_tx_type_is_schema() {
        let g = graph(vec![migration(
            "0001_add",
            vec![run_typeql("define attribute a, value string;", None)],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert_eq!(result.to_apply[0].steps[0].tx_type, TxType::Schema);
    }

    #[test]
    fn data_run_typeql_step_tx_type_is_write() {
        let g = graph(vec![migration(
            "0002_seed",
            vec![run_typeql(
                r#"match $a isa account, has account-id "acct-001";
insert $a has email "ops@example.com";"#,
                Some(
                    r#"match $a isa account, has email "ops@example.com";
delete $a has email "ops@example.com";"#,
                ),
            )],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let step = &result.to_apply[0].steps[0];
        assert_eq!(step.tx_type, TxType::Write);
        assert_eq!(step.kind, StepKind::Write);
    }

    // ── tests: typed OperationSpec variants lower in Rust ─────────────────────

    #[test]
    fn typed_attribute_operations_lower_to_schema_steps() {
        let g = graph(vec![migration(
            "0001_attrs",
            vec![
                OperationSpec::AddAttribute {
                    attribute: AttributeSchemaEntry::new("score", ValueType::Long),
                },
                OperationSpec::RemoveAttribute {
                    attr_name: "legacy-score".to_string(),
                },
            ],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        assert_eq!(steps.len(), 2);
        assert!(steps[0].forward.contains("attribute score, value integer;"));
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\nattribute score;")
        );
        assert_eq!(steps[1].forward, "undefine\nattribute legacy-score;");
        assert!(steps[1].reverse.is_none());
        assert!(!result.to_apply[0].reversible);
    }

    #[test]
    fn typed_entity_and_relation_operations_lower_to_schema_steps() {
        let g = graph(vec![migration(
            "0001_types",
            vec![
                OperationSpec::AddEntity {
                    entity: entity_entry("person"),
                },
                OperationSpec::AddRelation {
                    relation: relation_entry("employment"),
                },
            ],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        assert!(steps[0].forward.contains("entity person,"));
        assert!(steps[0].forward.contains("owns name @key;"));
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\nentity person;")
        );
        assert!(steps[1].forward.contains("relation employment,"));
        assert!(steps[1].forward.contains("relates employee;"));
        assert!(
            steps[1]
                .forward
                .contains("person plays employment:employee;")
        );
        assert!(
            steps[1]
                .reverse
                .as_deref()
                .unwrap()
                .contains("person plays employment:employee;")
        );
        assert!(
            steps[1]
                .reverse
                .as_deref()
                .unwrap()
                .contains("relation employment;")
        );
    }

    #[test]
    fn typed_ownership_operations_lower_to_schema_steps() {
        let g = graph(vec![migration(
            "0001_ownership",
            vec![
                OperationSpec::AddOwnership {
                    owner_type: "person".to_string(),
                    attribute: owned_attr("email", ValueType::String, vec![Annotation::Key]),
                },
                OperationSpec::RemoveOwnership {
                    owner_type: "person".to_string(),
                    attr_name: "legacy-email".to_string(),
                },
                OperationSpec::ModifyOwnership {
                    owner_type: "person".to_string(),
                    attr_name: "nickname".to_string(),
                    old_annotations: "@card(0..1)".to_string(),
                    new_annotations: "@card(1..1)".to_string(),
                },
            ],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        assert_eq!(steps[0].forward, "define\nperson owns email @key;");
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\nperson owns email;")
        );
        assert_eq!(steps[1].forward, "undefine\nperson owns legacy-email;");
        assert!(steps[1].reverse.is_none());
        assert_eq!(
            steps[2].forward,
            "redefine\nperson owns nickname @card(1..1);"
        );
        assert_eq!(
            steps[2].reverse.as_deref(),
            Some("redefine\nperson owns nickname @card(0..1);")
        );
    }

    #[test]
    fn typed_role_operations_lower_to_schema_steps() {
        let role = RoleEntry {
            role_name: "reviewer".to_string(),
            player_type_names: vec!["person".to_string()],
            cardinality: Some((0, Some(2))),
            overrides: None,
            is_abstract: false,
            ordered: false,
            distinct: false,
        };
        let g = graph(vec![migration(
            "0001_roles",
            vec![
                OperationSpec::AddRole {
                    relation_type: "employment".to_string(),
                    role,
                },
                OperationSpec::RemoveRole {
                    relation_type: "employment".to_string(),
                    role_name: "legacy".to_string(),
                },
                OperationSpec::AddRolePlayer {
                    relation_type: "employment".to_string(),
                    role_name: "employee".to_string(),
                    player_type_name: "contractor".to_string(),
                },
                OperationSpec::RemoveRolePlayer {
                    relation_type: "employment".to_string(),
                    role_name: "employee".to_string(),
                    player_type_name: "company".to_string(),
                },
            ],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        assert_eq!(
            steps[0].forward,
            "define\nemployment relates reviewer @card(0..2);\nperson plays employment:reviewer;"
        );
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\nperson plays employment:reviewer;\nemployment relates reviewer;")
        );
        assert_eq!(steps[1].forward, "undefine\nemployment relates legacy;");
        assert!(steps[1].reverse.is_none());
        assert_eq!(
            steps[2].forward,
            "define\ncontractor plays employment:employee;"
        );
        assert_eq!(
            steps[2].reverse.as_deref(),
            Some("undefine\ncontractor plays employment:employee;")
        );
        assert_eq!(
            steps[3].forward,
            "undefine\ncompany plays employment:employee;"
        );
        assert_eq!(
            steps[3].reverse.as_deref(),
            Some("define\ncompany plays employment:employee;")
        );
    }

    #[test]
    fn migration_reversible_flag_drops_typed_operation_reverses() {
        let mut spec = migration(
            "0001_non_reversible",
            vec![OperationSpec::AddAttribute {
                attribute: AttributeSchemaEntry::new("score", ValueType::Long),
            }],
            vec![],
        );
        spec.reversible = false;
        let g = graph(vec![spec]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        assert!(result.to_apply[0].steps[0].reverse.is_none());
        assert!(!result.to_apply[0].reversible);
    }

    // ── test: intentionally unsupported operation returns Err ────────────────

    #[test]
    fn unlowered_op_returns_err() {
        let g = graph(vec![migration(
            "0001_rename_attr",
            vec![OperationSpec::RenameAttribute {
                old_name: "old-score".to_string(),
                new_name: "new-score".to_string(),
                value_type: "string".to_string(),
            }],
            vec![],
        )]);

        let err = plan(&g, &[], None).expect_err("should fail for unsupported op");
        match err {
            MigrationError::UnloweredOperation { kind } => {
                assert_eq!(kind, "RenameAttribute");
            }
            other => panic!("expected UnloweredOperation, got {other:?}"),
        }
    }

    // ── test: validation failure short-circuits ────────────────────────────────

    #[test]
    fn validation_failure_returns_planning_error() {
        // 0002 depends on 0001 which is not in the graph.
        let g = graph(vec![migration(
            "0002_next",
            vec![run_typeql("define attribute b, value string;", None)],
            vec![("app", "0001_initial")],
        )]);

        let err = plan(&g, &[], None).expect_err("should fail on validation error");
        assert!(
            matches!(err, MigrationError::Planning { .. }),
            "expected Planning error, got {err:?}"
        );
    }

    // ── test: checksum drift short-circuits ───────────────────────────────────

    #[test]
    fn checksum_drift_returns_error() {
        let g = graph(vec![migration(
            "0001_initial",
            vec![run_typeql("define attribute a, value string;", None)],
            vec![],
        )]);

        // Record "0001_initial" with a wrong checksum.
        let bad_applied = AppliedMigrationRecord {
            app_label: "app".to_string(),
            name: "0001_initial".to_string(),
            checksum: "wrong-checksum".to_string(),
            applied_at: None,
        };

        let err = plan(&g, &[bad_applied], None).expect_err("should fail on drift");
        assert!(
            matches!(err, MigrationError::ChecksumDrift { .. }),
            "expected ChecksumDrift error, got {err:?}"
        );
    }

    // ── test: reversible flag ─────────────────────────────────────────────────

    #[test]
    fn migration_with_no_reverse_is_marked_not_reversible() {
        let g = graph(vec![migration(
            "0001_initial",
            // reverse is None
            vec![run_typeql("define attribute a, value string;", None)],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert!(!result.to_apply[0].reversible);
    }

    #[test]
    fn migration_with_all_reverses_is_reversible() {
        let g = graph(vec![migration(
            "0001_add",
            vec![run_typeql(
                "define attribute a, value string;",
                Some("undefine attribute a;"),
            )],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert!(result.to_apply[0].reversible);
    }

    #[test]
    fn define_schema_migration_is_not_reversible() {
        // DefineSchema never has a reverse.
        let g = graph(vec![migration(
            "0001_initial",
            vec![define_schema_op()],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert!(!result.to_apply[0].reversible);
    }

    // ── test: target not found returns TargetNotFound ─────────────────────────

    #[test]
    fn unknown_target_returns_target_not_found_error() {
        let g = graph(vec![migration(
            "0001_initial",
            vec![run_typeql("define attribute a, value string;", None)],
            vec![],
        )]);

        let err = plan(&g, &[], Some("nonexistent_migration"))
            .expect_err("should fail for missing target");
        assert!(
            matches!(err, MigrationError::TargetNotFound { .. }),
            "expected TargetNotFound, got {err:?}"
        );
    }

    // ── test: CopyAttribute lowers to Write-typed Backfill step ───────────────

    #[test]
    fn copy_attribute_lowers_to_write_typed_backfill_step() {
        // The carried forward/reverse mirror `CopyAttribute.to_typeql()` /
        // `to_rollback_typeql()`; assemble_steps must pass them through verbatim
        // under a Write/Backfill step (no re-synthesis — invariant 2).
        let forward = "match\n  $x isa person, has old-name $v;\n  \
            not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;";
        let reverse = "match $x isa person, has new-name $v;\ndelete $v of $x;";
        let g = graph(vec![migration(
            "0002_backfill",
            vec![OperationSpec::CopyAttribute {
                forward: forward.to_string(),
                reverse: Some(reverse.to_string()),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        let exec = &result.to_apply[0];
        assert_eq!(exec.steps.len(), 1);
        let step = &exec.steps[0];

        assert_eq!(
            step.tx_type,
            TxType::Write,
            "CopyAttribute step must use Write tx"
        );
        assert_eq!(
            step.kind,
            StepKind::Backfill,
            "CopyAttribute step kind must be Backfill"
        );
        // The carried strings are passed through unchanged.
        assert_eq!(step.forward, forward, "forward must be carried verbatim");
        assert_eq!(
            step.reverse.as_deref(),
            Some(reverse),
            "reverse must be carried verbatim"
        );
    }

    #[test]
    fn step_kind_default_is_schema_for_serde_backcompat() {
        // Simulate a legacy JSON step without the `kind` field.
        let json =
            r#"{"tx_type":"Schema","forward":"define attribute a, value string;","reverse":null}"#;
        let step: ExecutionStep =
            serde_json::from_str(json).expect("should deserialize legacy step");
        assert_eq!(
            step.kind,
            StepKind::Schema,
            "missing `kind` field must default to Schema for backward compat"
        );
    }
}
