//! Pure migration planner.
//!
//! Lowers a validated [`MigrationGraph`] and applied-state into an ordered
//! [`ExecutionPlan`] of [`ExecutionStep`]s, each carrying its [`TxType`] and
//! the executable TypeQL to run.  No database connection, no async, no TypeDB
//! driver is touched here.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use type_bridge_orm::TxType;
use type_bridge_orm::schema::annotations::{
    AnnotationToken, AnnotationTokenDiff, diff_annotation_tokens, split_annotation_tokens,
};
use type_bridge_orm::schema::info::{
    AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry, RoleEntry,
    SchemaInfo,
};

use crate::checksum::check_checksum_drift;
use crate::error::MigrationError;
use crate::graph::{AppliedMigrationRecord, validate_graph};
use crate::spec::{MigrationGraph, OperationSpec, copy_attribute_typeql};

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

/// Authored operation kind from which an execution step was lowered.
///
/// This discriminant is carried separately from [`StepKind`]: `StepKind`
/// controls transaction dispatch, while `OperationKind` preserves the stable
/// artifact-level operation identity used by per-step recovery.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Arbitrary authored TypeQL.
    #[default]
    RunTypeql,
    /// Define a complete initial schema.
    DefineSchema,
    /// Add an attribute type.
    AddAttribute,
    /// Remove an attribute type.
    RemoveAttribute,
    /// Add an entity type.
    AddEntity,
    /// Remove an entity type.
    RemoveEntity,
    /// Add a relation type.
    AddRelation,
    /// Remove a relation type.
    RemoveRelation,
    /// Add an ownership capability.
    AddOwnership,
    /// Remove an ownership capability.
    RemoveOwnership,
    /// Modify ownership annotations.
    ModifyOwnership,
    /// Modify type annotations.
    ModifyTypeAnnotations,
    /// Modify role annotations.
    ModifyRoleAnnotations,
    /// Add a relation role.
    AddRole,
    /// Remove a relation role.
    RemoveRole,
    /// Add a role player capability.
    AddRolePlayer,
    /// Remove a role player capability.
    RemoveRolePlayer,
    /// Rename an attribute type.
    RenameAttribute,
    /// Copy attribute values as a backfill.
    CopyAttribute,
}

impl OperationKind {
    /// Stable snake-case token used in deterministic step identities.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunTypeql => "run_typeql",
            Self::DefineSchema => "define_schema",
            Self::AddAttribute => "add_attribute",
            Self::RemoveAttribute => "remove_attribute",
            Self::AddEntity => "add_entity",
            Self::RemoveEntity => "remove_entity",
            Self::AddRelation => "add_relation",
            Self::RemoveRelation => "remove_relation",
            Self::AddOwnership => "add_ownership",
            Self::RemoveOwnership => "remove_ownership",
            Self::ModifyOwnership => "modify_ownership",
            Self::ModifyTypeAnnotations => "modify_type_annotations",
            Self::ModifyRoleAnnotations => "modify_role_annotations",
            Self::AddRole => "add_role",
            Self::RemoveRole => "remove_role",
            Self::AddRolePlayer => "add_role_player",
            Self::RemoveRolePlayer => "remove_role_player",
            Self::RenameAttribute => "rename_attribute",
            Self::CopyAttribute => "copy_attribute",
        }
    }

    fn from_spec(operation: &OperationSpec) -> Self {
        match operation {
            OperationSpec::RunTypeql { .. } => Self::RunTypeql,
            OperationSpec::DefineSchema { .. } => Self::DefineSchema,
            OperationSpec::AddAttribute { .. } => Self::AddAttribute,
            OperationSpec::RemoveAttribute { .. } => Self::RemoveAttribute,
            OperationSpec::AddEntity { .. } => Self::AddEntity,
            OperationSpec::RemoveEntity { .. } => Self::RemoveEntity,
            OperationSpec::AddRelation { .. } => Self::AddRelation,
            OperationSpec::RemoveRelation { .. } => Self::RemoveRelation,
            OperationSpec::AddOwnership { .. } => Self::AddOwnership,
            OperationSpec::RemoveOwnership { .. } => Self::RemoveOwnership,
            OperationSpec::ModifyOwnership { .. } => Self::ModifyOwnership,
            OperationSpec::ModifyTypeAnnotations { .. } => Self::ModifyTypeAnnotations,
            OperationSpec::ModifyRoleAnnotations { .. } => Self::ModifyRoleAnnotations,
            OperationSpec::AddRole { .. } => Self::AddRole,
            OperationSpec::RemoveRole { .. } => Self::RemoveRole,
            OperationSpec::AddRolePlayer { .. } => Self::AddRolePlayer,
            OperationSpec::RemoveRolePlayer { .. } => Self::RemoveRolePlayer,
            OperationSpec::RenameAttribute { .. } => Self::RenameAttribute,
            OperationSpec::CopyAttribute { .. } => Self::CopyAttribute,
        }
    }
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
    /// Artifact operation kind that produced this step.
    ///
    /// Defaults to [`OperationKind::RunTypeql`] when deserializing legacy
    /// persisted plans that predate per-step recovery metadata.
    #[serde(default)]
    pub operation_kind: OperationKind,
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

/// Relation labels deleted wholesale by a `RemoveRelation` in this migration.
fn removed_relation_labels(operations: &[OperationSpec]) -> BTreeSet<&str> {
    operations
        .iter()
        .filter_map(|op| match op {
            OperationSpec::RemoveRelation { type_name } => Some(type_name.as_str()),
            _ => None,
        })
        .collect()
}

/// True when `op` is a relation-scoped granular removal shadowed by a
/// `RemoveRelation` of the same relation in the same migration.
///
/// Legacy v1.5.x artifacts decomposed whole-relation deletion into
/// `RemoveRolePlayer`/`RemoveRole`/`RemoveOwnership` before the final
/// `RemoveRelation`. Executed step-per-transaction, committing the last
/// role's removal violates TypeDB's commit-time rule that a concrete
/// relation must relate at least one role, stranding the migration after
/// partial schema changes (#168). `undefine <relation>` already cascades
/// roles, player capabilities, and ownerships in one schema transaction, so
/// the shadowed steps are dropped at plan time — the artifact bytes and
/// checksum are never touched.
fn shadowed_by_remove_relation(op: &OperationSpec, removed: &BTreeSet<&str>) -> bool {
    match op {
        OperationSpec::RemoveRole { relation_type, .. }
        | OperationSpec::RemoveRolePlayer { relation_type, .. } => {
            removed.contains(relation_type.as_str())
        }
        OperationSpec::RemoveOwnership { owner_type, .. } => removed.contains(owner_type.as_str()),
        _ => false,
    }
}

/// Lower a slice of [`OperationSpec`] into [`ExecutionStep`]s.
fn assemble_steps(
    operations: &[OperationSpec],
    migration_reversible: bool,
) -> crate::Result<Vec<ExecutionStep>> {
    let removed_relations = removed_relation_labels(operations);
    let mut steps = Vec::with_capacity(operations.len());
    for op in operations {
        if shadowed_by_remove_relation(op, &removed_relations) {
            continue;
        }
        let mut op_steps: Vec<ExecutionStep> = match op {
            OperationSpec::RunTypeql { forward, reverse } => {
                let tx_type = run_typeql_tx_type(forward);
                vec![ExecutionStep {
                    tx_type,
                    kind: if tx_type == TxType::Write {
                        StepKind::Write
                    } else {
                        StepKind::Schema
                    },
                    operation_kind: OperationKind::RunTypeql,
                    forward: forward.clone(),
                    reverse: reverse.clone(),
                }]
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
                vec![ExecutionStep {
                    tx_type: TxType::Schema,
                    kind: StepKind::Schema,
                    operation_kind: OperationKind::DefineSchema,
                    forward,
                    // Model-initial migrations are non-reversible.
                    reverse: None,
                }]
            }
            OperationSpec::AddAttribute { attribute } => vec![schema_step(
                define_attribute(attribute)?,
                Some(undefine_attribute(&attribute.attr_name)),
            )],
            OperationSpec::RemoveAttribute { attr_name } => {
                vec![schema_step(undefine_attribute(attr_name), None)]
            }
            OperationSpec::AddEntity { entity } => vec![schema_step(
                define_entity(entity)?,
                Some(undefine_entity(&entity.type_name)),
            )],
            OperationSpec::RemoveEntity { type_name } => {
                vec![schema_step(undefine_entity(type_name), None)]
            }
            OperationSpec::AddRelation { relation } => vec![schema_step(
                define_relation(relation)?,
                Some(undefine_relation_with_players(relation)),
            )],
            OperationSpec::RemoveRelation { type_name } => {
                vec![schema_step(undefine_relation(type_name), None)]
            }
            OperationSpec::AddOwnership {
                owner_type,
                attribute,
            } => vec![schema_step(
                define_ownership(owner_type, attribute),
                Some(undefine_ownership(
                    owner_type,
                    &owned_attribute_type_ref(attribute),
                )),
            )],
            OperationSpec::RemoveOwnership {
                owner_type,
                attr_name,
            } => vec![schema_step(undefine_ownership(owner_type, attr_name), None)],
            OperationSpec::ModifyOwnership {
                owner_type,
                attr_name,
                old_annotations,
                new_annotations,
            } => annotation_token_steps(
                &format!("{owner_type} owns {attr_name}"),
                &diff_annotation_tokens(
                    &split_annotation_tokens(old_annotations),
                    &split_annotation_tokens(new_annotations),
                ),
            ),
            OperationSpec::ModifyTypeAnnotations {
                type_name,
                old_doc,
                new_doc,
                old_meta,
                new_meta,
            } => annotation_token_steps(
                type_name,
                &diff_annotation_tokens(
                    &doc_meta_tokens(old_doc.as_deref(), old_meta),
                    &doc_meta_tokens(new_doc.as_deref(), new_meta),
                ),
            ),
            OperationSpec::ModifyRoleAnnotations {
                relation_type,
                role_name,
                old_doc,
                new_doc,
                old_meta,
                new_meta,
            } => annotation_token_steps(
                &format!("{relation_type} relates {role_name}"),
                &diff_annotation_tokens(
                    &doc_meta_tokens(old_doc.as_deref(), old_meta),
                    &doc_meta_tokens(new_doc.as_deref(), new_meta),
                ),
            ),
            OperationSpec::AddRole {
                relation_type,
                role,
            } => vec![schema_step(
                define_role(relation_type, role),
                Some(undefine_role_with_players(relation_type, role)),
            )],
            OperationSpec::RemoveRole {
                relation_type,
                role_name,
            } => vec![schema_step(undefine_role(relation_type, role_name), None)],
            OperationSpec::AddRolePlayer {
                relation_type,
                role_name,
                player_type_name,
            } => vec![schema_step(
                define_role_player(relation_type, role_name, player_type_name),
                Some(undefine_role_player(
                    relation_type,
                    role_name,
                    player_type_name,
                )),
            )],
            OperationSpec::RemoveRolePlayer {
                relation_type,
                role_name,
                player_type_name,
            } => vec![schema_step(
                undefine_role_player(relation_type, role_name, player_type_name),
                Some(define_role_player(
                    relation_type,
                    role_name,
                    player_type_name,
                )),
            )],
            copy @ OperationSpec::CopyAttribute { .. } => {
                // Carried TypeQL (the frozen `CopyAttribute.to_typeql()` output)
                // executes verbatim; the structured portable form synthesizes the
                // identical template. `backfill.rs` composes its count queries
                // from this `forward` text's match clause.
                let (forward, reverse) = copy_attribute_typeql(copy)?;
                vec![ExecutionStep {
                    tx_type: TxType::Write,
                    kind: StepKind::Backfill,
                    operation_kind: OperationKind::CopyAttribute,
                    forward,
                    reverse,
                }]
            }
            other @ OperationSpec::RenameAttribute { .. } => {
                return Err(MigrationError::UnloweredOperation {
                    kind: op_kind_name(other).to_string(),
                });
            }
        };
        if !migration_reversible {
            for step in &mut op_steps {
                step.reverse = None;
            }
        }
        let operation_kind = OperationKind::from_spec(op);
        for step in &mut op_steps {
            step.operation_kind = operation_kind;
        }
        steps.append(&mut op_steps);
    }
    Ok(steps)
}

/// Lower an annotation-set change on `subject` into execution steps.
///
/// TypeDB 3.12 semantics (verified live): `define`/`undefine` blocks accept
/// multiple statements per query, while `redefine` mutates exactly one schema
/// element per query; parameterless annotations (`@key`, `@unique`,
/// `@distinct`) can only be defined or undefined, never redefined. Removals
/// therefore group into one `undefine` step, additions into one `define`
/// step, and each value change becomes its own `redefine` step. Removals run
/// first — adding `@key` while a conflicting explicit `@card` is still
/// declared fails schema validation. Reverse steps restore the prior state
/// with the mirrored verb.
fn annotation_token_steps(subject: &str, diff: &AnnotationTokenDiff) -> Vec<ExecutionStep> {
    let mut steps = Vec::new();
    if !diff.removed.is_empty() {
        let forward = typeql_block(
            "undefine",
            diff.removed
                .iter()
                .map(|token| format!("{} from {subject};", undefine_annotation_ref(token)))
                .collect(),
        );
        let reverse = typeql_block(
            "define",
            diff.removed
                .iter()
                .map(|token| format!("{subject} {};", token.render()))
                .collect(),
        );
        steps.push(schema_step(forward, Some(reverse)));
    }
    for (old_token, new_token) in &diff.changed {
        steps.push(schema_step(
            typeql_block(
                "redefine",
                vec![format!("{subject} {};", new_token.render())],
            ),
            Some(typeql_block(
                "redefine",
                vec![format!("{subject} {};", old_token.render())],
            )),
        ));
    }
    if !diff.added.is_empty() {
        let forward = typeql_block(
            "define",
            diff.added
                .iter()
                .map(|token| format!("{subject} {};", token.render()))
                .collect(),
        );
        let reverse = typeql_block(
            "undefine",
            diff.added
                .iter()
                .map(|token| format!("{} from {subject};", undefine_annotation_ref(token)))
                .collect(),
        );
        steps.push(schema_step(forward, Some(reverse)));
    }
    steps
}

/// The `@...` reference used in `undefine <ref> from <subject>`.
///
/// `@meta` must use the keyed form `@meta("key")`; every other annotation
/// (parameterless or parameterized) undefines by bare name.
fn undefine_annotation_ref(token: &AnnotationToken) -> String {
    if token.name == "meta"
        && let Some(key) = token.meta_key()
    {
        return format!(
            "@meta({})",
            type_bridge_orm::schema::annotations::escaped_string_literal(&key)
        );
    }
    format!("@{}", token.name)
}

/// Build the `@doc`/`@meta` token list for one side of a type or role
/// annotation change.
fn doc_meta_tokens(
    doc: Option<&str>,
    meta: &std::collections::BTreeMap<String, String>,
) -> Vec<AnnotationToken> {
    let mut tokens = Vec::new();
    if let Some(doc) = doc {
        tokens.push(AnnotationToken::doc(doc));
    }
    for (key, value) in meta {
        tokens.push(AnnotationToken::meta(key, value));
    }
    tokens
}

fn schema_step(forward: String, reverse: Option<String>) -> ExecutionStep {
    ExecutionStep {
        tx_type: TxType::Schema,
        kind: StepKind::Schema,
        operation_kind: OperationKind::RunTypeql,
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
    format!("undefine\n{attr_name};")
}

fn define_entity(entity: &EntitySchemaEntry) -> crate::Result<String> {
    let mut schema = SchemaInfo::default();
    schema
        .entities
        .insert(entity.type_name.clone(), entity.clone());
    schema_to_typeql(&schema)
}

fn undefine_entity(type_name: &str) -> String {
    format!("undefine\n{type_name};")
}

fn define_relation(relation: &RelationSchemaEntry) -> crate::Result<String> {
    let mut schema = SchemaInfo::default();
    schema
        .relations
        .insert(relation.type_name.clone(), relation.clone());
    schema_to_typeql(&schema)
}

fn undefine_relation(type_name: &str) -> String {
    format!("undefine\n{type_name};")
}

fn undefine_relation_with_players(relation: &RelationSchemaEntry) -> String {
    let mut statements = Vec::new();
    for role in &relation.roles {
        for player_type_name in &role.player_type_names {
            statements.push(format!(
                "plays {}:{} from {player_type_name};",
                relation.type_name, role.role_name
            ));
        }
    }
    statements.push(format!("{};", relation.type_name));
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
    typeql_block(
        "undefine",
        vec![format!("owns {attr_name} from {owner_type};")],
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
        vec![format!("relates {role_name} from {relation_type};")],
    )
}

fn undefine_role_with_players(relation_type: &str, role: &RoleEntry) -> String {
    let mut statements = Vec::new();
    for player_type_name in &role.player_type_names {
        statements.push(format!(
            "plays {relation_type}:{} from {player_type_name};",
            role.role_name
        ));
    }
    statements.push(format!(
        "relates {} from {relation_type};",
        role_type_ref(role)
    ));
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
            "plays {relation_type}:{role_name} from {player_type_name};"
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
        OperationSpec::ModifyTypeAnnotations { .. } => "ModifyTypeAnnotations",
        OperationSpec::ModifyRoleAnnotations { .. } => "ModifyRoleAnnotations",
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
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
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
            doc: None,
            meta: Default::default(),
        }
    }

    fn entity_entry(type_name: &str) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: type_name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![owned_attr("name", ValueType::String, vec![Annotation::Key])],
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
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
                doc: None,
                meta: Default::default(),
            }],
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
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
        assert_eq!(steps[0].reverse.as_deref(), Some("undefine\nscore;"));
        assert_eq!(steps[1].forward, "undefine\nlegacy-score;");
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
        assert_eq!(steps[0].reverse.as_deref(), Some("undefine\nperson;"));
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
                .contains("plays employment:employee from person;")
        );
        assert!(steps[1].reverse.as_deref().unwrap().contains("employment;"));
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
            Some("undefine\nowns email from person;")
        );
        assert_eq!(steps[1].forward, "undefine\nowns legacy-email from person;");
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
    fn modify_ownership_decomposes_parameterless_transitions() {
        // @key can never be redefined (REX28): swapping @card(0..1) for
        // @key must lower to an undefine step followed by a define step,
        // removals first (defining @key beside a conflicting explicit
        // @card fails schema validation).
        let g = graph(vec![migration(
            "0002_key",
            vec![OperationSpec::ModifyOwnership {
                owner_type: "person".to_string(),
                attr_name: "nickname".to_string(),
                old_annotations: "@card(0..1)".to_string(),
                new_annotations: "@key".to_string(),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].forward,
            "undefine\n@card from person owns nickname;"
        );
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("define\nperson owns nickname @card(0..1);")
        );
        assert_eq!(steps[1].forward, "define\nperson owns nickname @key;");
        assert_eq!(
            steps[1].reverse.as_deref(),
            Some("undefine\n@key from person owns nickname;")
        );
    }

    #[test]
    fn modify_ownership_from_plain_defines_and_identical_sets_lower_to_nothing() {
        let g = graph(vec![migration(
            "0002_tighten",
            vec![
                OperationSpec::ModifyOwnership {
                    owner_type: "person".to_string(),
                    attr_name: "nickname".to_string(),
                    old_annotations: String::new(),
                    new_annotations: "@key".to_string(),
                },
                OperationSpec::ModifyOwnership {
                    owner_type: "person".to_string(),
                    attr_name: "email".to_string(),
                    old_annotations: "@unique".to_string(),
                    new_annotations: "@unique".to_string(),
                },
            ],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;

        // The no-op transition contributes zero steps.
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].forward, "define\nperson owns nickname @key;");
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\n@key from person owns nickname;")
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
            doc: None,
            meta: Default::default(),
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
            Some(
                "undefine\nplays employment:reviewer from person;\nrelates reviewer from employment;"
            )
        );
        assert_eq!(
            steps[1].forward,
            "undefine\nrelates legacy from employment;"
        );
        assert!(steps[1].reverse.is_none());
        assert_eq!(
            steps[2].forward,
            "define\ncontractor plays employment:employee;"
        );
        assert_eq!(
            steps[2].reverse.as_deref(),
            Some("undefine\nplays employment:employee from contractor;")
        );
        assert_eq!(
            steps[3].forward,
            "undefine\nplays employment:employee from company;"
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
                owner: None,
                source: None,
                dest: None,
                filter: None,
                forward: Some(forward.to_string()),
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
    fn structured_copy_attribute_lowers_to_the_same_backfill_step() {
        // The structured portable form synthesizes the exact TypeQL the
        // carried form would have contained.
        let g = graph(vec![migration(
            "0002_backfill",
            vec![OperationSpec::CopyAttribute {
                owner: Some("person".to_string()),
                source: Some("old-name".to_string()),
                dest: Some("new-name".to_string()),
                filter: None,
                forward: None,
                reverse: None,
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        let step = &result.to_apply[0].steps[0];
        assert_eq!(step.tx_type, TxType::Write);
        assert_eq!(step.kind, StepKind::Backfill);
        assert_eq!(
            step.forward,
            "match\n  $x isa person, has old-name $v;\n  \
             not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;"
        );
        assert_eq!(
            step.reverse.as_deref(),
            Some("match $x isa person, has new-name $v;\ndelete $v of $x;")
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
        assert_eq!(step.operation_kind, OperationKind::RunTypeql);
    }

    // ── test: whole-relation removal normalization (#168) ───────────────────

    /// The exact operation shape v1.5.5/v1.5.6 generators authored for a
    /// whole-relation deletion: granular unwind, then `RemoveRelation`.
    fn legacy_remove_relation_ops(relation: &str) -> Vec<OperationSpec> {
        vec![
            OperationSpec::RemoveRolePlayer {
                relation_type: relation.to_string(),
                role_name: "subject".to_string(),
                player_type_name: "person".to_string(),
            },
            OperationSpec::RemoveRole {
                relation_type: relation.to_string(),
                role_name: "subject".to_string(),
            },
            OperationSpec::RemoveRolePlayer {
                relation_type: relation.to_string(),
                role_name: "badge".to_string(),
                player_type_name: "temporary-badge".to_string(),
            },
            OperationSpec::RemoveRole {
                relation_type: relation.to_string(),
                role_name: "badge".to_string(),
            },
            OperationSpec::RemoveOwnership {
                owner_type: relation.to_string(),
                attr_name: "legacy-link-id".to_string(),
            },
            OperationSpec::RemoveRelation {
                type_name: relation.to_string(),
            },
        ]
    }

    #[test]
    fn legacy_decomposed_relation_removal_normalizes_to_single_step() {
        let g = graph(vec![migration(
            "0005_remove_legacy_link",
            legacy_remove_relation_ops("legacy-link"),
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        let exec = &result.to_apply[0];
        assert_eq!(
            exec.steps.len(),
            1,
            "granular removals shadowed by RemoveRelation must be dropped"
        );
        assert_eq!(exec.steps[0].forward, "undefine\nlegacy-link;");
        assert_eq!(exec.steps[0].tx_type, TxType::Schema);
    }

    #[test]
    fn surviving_relation_granular_removals_are_kept() {
        // No RemoveRelation for `employment`: granular ops must lower 1:1.
        let ops = vec![
            OperationSpec::RemoveRolePlayer {
                relation_type: "employment".to_string(),
                role_name: "employee".to_string(),
                player_type_name: "contractor".to_string(),
            },
            OperationSpec::RemoveRole {
                relation_type: "employment".to_string(),
                role_name: "reviewer".to_string(),
            },
            OperationSpec::RemoveOwnership {
                owner_type: "employment".to_string(),
                attr_name: "note".to_string(),
            },
        ];
        let g = graph(vec![migration("0002_trim_employment", ops, vec![])]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        assert_eq!(result.to_apply[0].steps.len(), 3);
    }

    #[test]
    fn normalization_is_scoped_to_the_removed_relation() {
        // One relation is removed wholesale while another one is trimmed in
        // the same migration; only ops scoped to the removed relation drop.
        let mut ops = legacy_remove_relation_ops("legacy-link");
        ops.push(OperationSpec::RemoveRolePlayer {
            relation_type: "employment".to_string(),
            role_name: "employee".to_string(),
            player_type_name: "contractor".to_string(),
        });
        ops.push(OperationSpec::RemoveOwnership {
            owner_type: "person".to_string(),
            attr_name: "nickname".to_string(),
        });
        let g = graph(vec![migration("0006_mixed_removals", ops, vec![])]);

        let result = plan(&g, &[], None).expect("plan should succeed");

        let forwards: Vec<&str> = result.to_apply[0]
            .steps
            .iter()
            .map(|s| s.forward.as_str())
            .collect();
        assert_eq!(
            forwards,
            vec![
                "undefine\nlegacy-link;",
                "undefine\nplays employment:employee from contractor;",
                "undefine\nowns nickname from person;",
            ]
        );
    }

    #[test]
    fn modify_ownership_lowers_per_annotation_steps() {
        // Mixed add/update/remove including parameterless @key/@unique, which
        // can never be redefined (REX28) — they must go through define/undefine.
        let g = graph(vec![migration(
            "0001_annotations",
            vec![OperationSpec::ModifyOwnership {
                owner_type: "person".to_string(),
                attr_name: "name".to_string(),
                old_annotations: "@key @doc(\"old doc\") @meta(\"x\", \"1\")".to_string(),
                new_annotations: "@unique @doc(\"new doc\") @meta(\"y\", \"2\")".to_string(),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;
        assert_eq!(steps.len(), 3);

        // Removals run first: adding @unique while the conflicting @key is
        // still declared would fail schema validation.
        assert_eq!(
            steps[0].forward,
            "undefine\n@key from person owns name;\n@meta(\"x\") from person owns name;"
        );
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("define\nperson owns name @key;\nperson owns name @meta(\"x\", \"1\");")
        );
        assert_eq!(
            steps[1].forward,
            "redefine\nperson owns name @doc(\"new doc\");"
        );
        assert_eq!(
            steps[1].reverse.as_deref(),
            Some("redefine\nperson owns name @doc(\"old doc\");")
        );
        assert_eq!(
            steps[2].forward,
            "define\nperson owns name @meta(\"y\", \"2\");\nperson owns name @unique;"
        );
        assert_eq!(
            steps[2].reverse.as_deref(),
            Some("undefine\n@meta(\"y\") from person owns name;\n@unique from person owns name;")
        );
        // added order: "meta:y" < "unique" in identity order.
    }

    #[test]
    fn modify_ownership_with_identical_annotations_lowers_to_no_steps() {
        let g = graph(vec![migration(
            "0001_noop",
            vec![OperationSpec::ModifyOwnership {
                owner_type: "person".to_string(),
                attr_name: "name".to_string(),
                old_annotations: "@key @doc(\"same\")".to_string(),
                new_annotations: "@key @doc(\"same\")".to_string(),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        assert!(result.to_apply[0].steps.is_empty());
    }

    #[test]
    fn modify_type_annotations_lowers_add_update_remove() {
        let g = graph(vec![migration(
            "0001_type_annotations",
            vec![OperationSpec::ModifyTypeAnnotations {
                type_name: "person".to_string(),
                old_doc: Some("old type doc".to_string()),
                new_doc: Some("new type doc".to_string()),
                old_meta: BTreeMap::from([("gone".to_string(), "1".to_string())]),
                new_meta: BTreeMap::from([("added".to_string(), "2".to_string())]),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].forward, "undefine\n@meta(\"gone\") from person;");
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("define\nperson @meta(\"gone\", \"1\");")
        );
        assert_eq!(steps[1].forward, "redefine\nperson @doc(\"new type doc\");");
        assert_eq!(
            steps[1].reverse.as_deref(),
            Some("redefine\nperson @doc(\"old type doc\");")
        );
        assert_eq!(steps[2].forward, "define\nperson @meta(\"added\", \"2\");");
        assert_eq!(
            steps[2].reverse.as_deref(),
            Some("undefine\n@meta(\"added\") from person;")
        );
    }

    #[test]
    fn modify_role_annotations_lowers_on_relates_subject() {
        let g = graph(vec![migration(
            "0001_role_annotations",
            vec![OperationSpec::ModifyRoleAnnotations {
                relation_type: "employment".to_string(),
                role_name: "employee".to_string(),
                old_doc: None,
                new_doc: Some("The employed party.".to_string()),
                old_meta: BTreeMap::new(),
                new_meta: BTreeMap::new(),
            }],
            vec![],
        )]);

        let result = plan(&g, &[], None).expect("plan should succeed");
        let steps = &result.to_apply[0].steps;
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].forward,
            "define\nemployment relates employee @doc(\"The employed party.\");"
        );
        assert_eq!(
            steps[0].reverse.as_deref(),
            Some("undefine\n@doc from employment relates employee;")
        );
    }
}
