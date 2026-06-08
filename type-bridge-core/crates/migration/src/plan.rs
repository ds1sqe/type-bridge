//! Pure migration planner.
//!
//! Lowers a validated [`MigrationGraph`] and applied-state into an ordered
//! [`ExecutionPlan`] of [`ExecutionStep`]s, each carrying its [`TxType`] and
//! the executable TypeQL to run.  No database connection, no async, no TypeDB
//! driver is touched here.

use serde::{Deserialize, Serialize};
use type_bridge_orm::TxType;

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
///   [`OperationSpec`] variant that has not been lowered to `RunTypeql` or
///   `DefineSchema` (granular typed ops must be lowered by the Python executor
///   before they reach this planner — that is Phase 3).
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
        let steps = assemble_steps(&migration.operations)?;
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
        let steps = assemble_steps(&migration.operations)?;
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
///
/// `RunTypeql`, `DefineSchema`, and `CopyAttribute` are handled; any other
/// variant returns [`MigrationError::UnloweredOperation`].
fn assemble_steps(operations: &[OperationSpec]) -> crate::Result<Vec<ExecutionStep>> {
    let mut steps = Vec::with_capacity(operations.len());
    for op in operations {
        let step = match op {
            OperationSpec::RunTypeql { forward, reverse } => ExecutionStep {
                tx_type: TxType::Schema,
                kind: StepKind::Schema,
                forward: forward.clone(),
                reverse: reverse.clone(),
            },
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
            other => {
                return Err(MigrationError::UnloweredOperation {
                    kind: op_kind_name(other).to_string(),
                });
            }
        };
        steps.push(step);
    }
    Ok(steps)
}

/// Return a stable string name for an [`OperationSpec`] variant.
///
/// Used only in error messages; no TypeQL is produced from these variants.
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
        AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, SchemaInfo,
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
            AttributeSchemaEntry {
                attr_name: "name".to_string(),
                value_type: ValueType::String,
            },
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
                }],
                plays_cardinalities: BTreeMap::new(),
            },
        );
        OperationSpec::DefineSchema { schema }
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

    // ── test: unlowered operation returns Err ─────────────────────────────────

    #[test]
    fn unlowered_op_returns_err() {
        use type_bridge_orm::schema::info::AttributeSchemaEntry;

        let g = graph(vec![migration(
            "0001_add_attr",
            vec![OperationSpec::AddAttribute {
                attribute: AttributeSchemaEntry {
                    attr_name: "score".to_string(),
                    value_type: ValueType::Long,
                },
            }],
            vec![],
        )]);

        let err = plan(&g, &[], None).expect_err("should fail for unlowered op");
        match err {
            MigrationError::UnloweredOperation { kind } => {
                assert_eq!(kind, "AddAttribute");
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
