//! Dynamic rows and inputs used by runtime descriptor managers.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use type_bridge_core_lib::ast::{
    Clause, Constraint, FetchItem, FunctionCallValue, Pattern, ReduceAssignment, RolePlayer,
    SortField, Statement, Value,
};

use crate::descriptor::{EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor};
use crate::error::{OrmError, Result};
use crate::expr::SortDir;
use crate::filter::Filter;
use crate::value::AttributeValue;

/// Runtime attribute values keyed by TypeDB attribute name or descriptor field name.
pub type DynamicAttributeMap = Vec<(String, AttributeValue)>;

/// Dynamic entity row hydrated from a fetch document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicEntityRow {
    /// TypeDB internal identifier, when present in fetch output.
    pub iid: Option<String>,
    /// Concrete TypeDB type label from polymorphic fetch, when present.
    pub type_name: Option<String>,
    /// Declared attributes that were present in the document.
    pub attributes: DynamicAttributeMap,
}

/// Dynamic role player hydrated from relation fetch output or provided for relation insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRolePlayer {
    /// Relation role name.
    pub role_name: String,
    /// Player entity IID, when known.
    pub player_iid: Option<String>,
    /// Player entity type name, when known.
    pub player_type_name: Option<String>,
    /// Player owned attributes, keyed by TypeDB attribute type name.
    pub attributes: Vec<(String, JsonValue)>,
}

/// Runtime relation row hydrated from a fetch document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRelationRow {
    /// TypeDB internal identifier, when present in fetch output.
    pub iid: Option<String>,
    /// Concrete TypeDB type label from polymorphic fetch, when present.
    pub type_name: Option<String>,
    /// Declared attributes that were present in the document.
    pub attributes: DynamicAttributeMap,
    /// Role players when the document shape exposes them.
    pub role_players: Vec<DynamicRolePlayer>,
}

/// Runtime role player reference used for dynamic relation inserts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRolePlayerInput {
    /// Relation role name.
    pub role_name: String,
    /// Entity type name for the player.
    pub player_type_name: String,
    /// IID-based identification, preferred when available.
    pub iid: Option<String>,
    /// Key-attribute fallback identification.
    pub key: Option<(String, AttributeValue)>,
}

/// Runtime aggregate requested by a language binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicAggregate {
    /// Stable result key exposed to bindings.
    pub result_key: String,
    /// TypeDB reduce function name, such as `count`, `sum`, `mean`, `min`, or `max`.
    pub function: String,
    /// Attribute type name or field name for attribute-backed aggregates.
    pub attr_name: Option<String>,
}

/// Runtime comparison operator requested by a language binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicComparisonOp {
    /// Equality.
    Eq,
    /// Not equal.
    Neq,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// String contains.
    Contains,
    /// String regex match.
    Like,
    /// Anchored prefix match: the caller passes the raw literal; Rust adds
    /// the `^` anchor, escapes regex metacharacters, and emits a `like` query.
    StartsWith,
    /// Anchored suffix match: the caller passes the raw literal; Rust adds
    /// the `$` anchor, escapes regex metacharacters, and emits a `like` query.
    EndsWith,
}

impl DynamicComparisonOp {
    fn typeql_operator(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Neq => "!=",
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Contains => "contains",
            Self::Like => "like",
            Self::StartsWith => "like",
            Self::EndsWith => "like",
        }
    }
}

/// Runtime expression tree requested by a language binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DynamicExpr {
    /// Attribute comparison or string operator.
    Compare {
        /// Attribute type name.
        attr_name: String,
        /// Typed comparison operator.
        operator: DynamicComparisonOp,
        /// Typed value to compare against.
        value: AttributeValue,
    },
    /// IID lookup for the current thing variable.
    Iid {
        /// TypeDB internal identifier.
        iid: String,
    },
    /// Presence/absence lookup for an owned attribute.
    IsNull {
        /// Attribute type name.
        attr_name: String,
        /// `true` means the owner must not have this attribute.
        is_null: bool,
    },
    /// All child expressions must match.
    And {
        /// Child expressions.
        exprs: Vec<DynamicExpr>,
    },
    /// At least one child expression must match.
    Or {
        /// Child expressions.
        exprs: Vec<DynamicExpr>,
    },
    /// The child expression must not match.
    Not {
        /// Child expression.
        expr: Box<DynamicExpr>,
    },
    /// Apply an expression to a relation role-player variable.
    RolePlayer {
        /// Relation role name.
        role_name: String,
        /// Expression applied to that role player.
        expr: Box<DynamicExpr>,
    },
}

/// Runtime sort requested by a language binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DynamicSort {
    /// Sort by an attribute owned by the queried thing.
    Attribute {
        /// Attribute type name.
        attr_name: String,
        /// Sort direction.
        direction: SortDir,
    },
    /// Sort a relation query by one role player's attribute.
    RolePlayerAttribute {
        /// Relation role name.
        role_name: String,
        /// Player attribute type name.
        attr_name: String,
        /// Sort direction.
        direction: SortDir,
    },
}

/// Escape regex metacharacters in a raw literal for use in a TypeDB `like` pattern.
///
/// Escapes the characters: `\ ^ $ . * + ? ( ) [ ] { } |`
fn escape_regex_literal(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    for ch in raw.chars() {
        if r"\.^$*+?()[]{}|".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl DynamicExpr {
    pub(crate) fn to_patterns(&self, thing_var: &str, counter: &mut usize) -> Result<Vec<Pattern>> {
        match self {
            Self::Compare {
                attr_name,
                operator,
                value,
            } => {
                let attr_var = next_dynamic_var("$dyn_attr", counter);
                // For StartsWith/EndsWith, transform the raw literal into an anchored
                // like-regex pattern. Rust owns all anchoring and escaping; the caller
                // provides only the raw literal.
                let effective_value = match operator {
                    DynamicComparisonOp::StartsWith => {
                        let raw = match value {
                            AttributeValue::String(s) => s.as_str(),
                            _ => {
                                return Err(OrmError::QueryExecution(
                                    "DynamicComparisonOp::StartsWith requires a String value"
                                        .into(),
                                ));
                            }
                        };
                        AttributeValue::String(format!("^{}.*", escape_regex_literal(raw)))
                    }
                    DynamicComparisonOp::EndsWith => {
                        let raw = match value {
                            AttributeValue::String(s) => s.as_str(),
                            _ => {
                                return Err(OrmError::QueryExecution(
                                    "DynamicComparisonOp::EndsWith requires a String value".into(),
                                ));
                            }
                        };
                        AttributeValue::String(format!(".*{}$", escape_regex_literal(raw)))
                    }
                    _ => value.clone(),
                };
                Ok(vec![
                    Pattern::Has {
                        thing_var: thing_var.to_string(),
                        attr_type: attr_name.clone(),
                        attr_var: attr_var.clone(),
                    },
                    Pattern::ValueComparison {
                        var: attr_var,
                        operator: operator.typeql_operator().to_string(),
                        value: effective_value.to_ast_value(),
                    },
                ])
            }
            Self::Iid { iid } => Ok(vec![Pattern::Iid {
                variable: thing_var.to_string(),
                iid: iid.clone(),
            }]),
            Self::IsNull { attr_name, is_null } => {
                let attr_var = next_dynamic_var("$dyn_attr", counter);
                let pattern = Pattern::Has {
                    thing_var: thing_var.to_string(),
                    attr_type: attr_name.clone(),
                    attr_var,
                };
                if *is_null {
                    Ok(vec![Pattern::Not(vec![pattern])])
                } else {
                    Ok(vec![pattern])
                }
            }
            Self::And { exprs } => {
                let mut patterns = Vec::new();
                for expr in exprs {
                    patterns.extend(expr.to_patterns(thing_var, counter)?);
                }
                Ok(patterns)
            }
            Self::Or { exprs } => {
                let mut branches = Vec::with_capacity(exprs.len());
                for expr in exprs {
                    branches.push(expr.to_patterns(thing_var, counter)?);
                }
                Ok(vec![Pattern::Or(branches)])
            }
            Self::Not { expr } => Ok(vec![Pattern::Not(expr.to_patterns(thing_var, counter)?)]),
            Self::RolePlayer { role_name, expr } => {
                expr.to_patterns(&role_player_expr_var(role_name), counter)
            }
        }
    }

    pub(crate) fn collect_roles(&self, roles: &mut HashSet<String>) {
        match self {
            Self::RolePlayer { role_name, expr } => {
                roles.insert(role_name.clone());
                expr.collect_roles(roles);
            }
            Self::And { exprs } | Self::Or { exprs } => {
                for expr in exprs {
                    expr.collect_roles(roles);
                }
            }
            Self::Not { expr } => expr.collect_roles(roles),
            _ => {}
        }
    }
}

impl DynamicSort {
    pub(crate) fn role_name(&self) -> Option<&str> {
        match self {
            Self::RolePlayerAttribute { role_name, .. } => Some(role_name),
            Self::Attribute { .. } => None,
        }
    }

    fn direction(&self) -> SortDir {
        match self {
            Self::Attribute { direction, .. } | Self::RolePlayerAttribute { direction, .. } => {
                *direction
            }
        }
    }
}

pub(crate) fn entity_insert_clauses(
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Vec<Clause> {
    entity_write_with_iid_clauses(ClauseKind::Insert, descriptor, attributes, var)
}

pub(crate) fn entity_put_clauses(
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<Vec<Clause>> {
    let key_attrs: Vec<_> = descriptor
        .owned_attributes
        .iter()
        .filter(|attr| attr.is_key())
        .collect();
    if key_attrs.is_empty()
        || !key_attrs
            .iter()
            .any(|key_attr| find_attribute_value(attributes, key_attr).is_some())
    {
        return Ok(entity_write_with_iid_clauses(
            ClauseKind::Put,
            descriptor,
            attributes,
            var,
        ));
    }

    let mut put_statements = vec![Statement::Isa {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
    }];
    let mut has_non_key_attributes = false;

    for (attr_name, value) in attributes {
        let attr = descriptor
            .owned_attributes
            .iter()
            .find(|attr| attr_name == &attr.field_name || attr_name == &attr.attr_name)
            .ok_or_else(|| {
                OrmError::QueryExecution(format!(
                    "Dynamic put for {} references unknown attribute {attr_name}",
                    descriptor.type_name
                ))
            })?;

        if attr.is_key() {
            put_statements.push(Statement::Has {
                subject_var: var.to_string(),
                attr_name: attr.attr_name.clone(),
                value: value.to_ast_value(),
            });
        } else {
            has_non_key_attributes = true;
        }
    }

    let mut clauses = vec![Clause::Put(put_statements)];
    if has_non_key_attributes {
        let constraints = entity_identification_constraints(descriptor, None, attributes)?;
        let mut match_patterns = vec![Pattern::Entity {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            constraints,
            is_strict: false,
        }];
        let mutation = update_mutation_clauses(
            &descriptor.type_name,
            descriptor
                .owned_attributes
                .iter()
                .map(|attr| (attr, attr.is_key())),
            attributes,
            var,
        )?;
        match_patterns.extend(mutation.match_patterns);
        clauses.push(Clause::Match(match_patterns));
        if !mutation.delete_statements.is_empty() {
            clauses.push(Clause::Delete(mutation.delete_statements));
        }
        clauses.push(Clause::Insert(mutation.insert_statements));
    }
    clauses.push(Clause::Fetch(vec![FetchItem::Function {
        key: "iid".to_string(),
        func_name: "iid".to_string(),
        var: var.to_string(),
    }]));
    Ok(clauses)
}

pub(crate) fn entity_update_clauses(
    descriptor: &EntityDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<Vec<Clause>> {
    let constraints = entity_identification_constraints(descriptor, iid, attributes)?;
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints,
        is_strict: false,
    }];
    let mutation = update_mutation_clauses(
        &descriptor.type_name,
        descriptor
            .owned_attributes
            .iter()
            .map(|attr| (attr, attr.is_key())),
        attributes,
        var,
    )?;
    match_patterns.extend(mutation.match_patterns);

    let mut clauses = vec![Clause::Match(match_patterns)];
    if !mutation.delete_statements.is_empty() {
        clauses.push(Clause::Delete(mutation.delete_statements));
    }
    clauses.push(Clause::Insert(mutation.insert_statements));
    Ok(clauses)
}

fn entity_write_with_iid_clauses(
    kind: ClauseKind,
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Vec<Clause> {
    let mut statements = vec![Statement::Isa {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
    }];
    statements.extend(attribute_statements(
        var,
        &descriptor.owned_attributes,
        attributes,
    ));
    let write_clause = match kind {
        ClauseKind::Insert => Clause::Insert(statements),
        ClauseKind::Put => Clause::Put(statements),
    };
    vec![
        write_clause,
        Clause::Fetch(vec![FetchItem::Function {
            key: "iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        }]),
    ]
}

enum ClauseKind {
    Insert,
    Put,
}

pub(crate) fn entity_fetch_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    entity_fetch_with_filter_patterns(descriptor, constraints, extra_patterns, var)
}

pub(crate) fn entity_expr_fetch_clauses(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    sorts: &[DynamicSort],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<Vec<Clause>> {
    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: "$t".to_string(),
        constraints: vec![],
        is_strict: true,
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }
    match_patterns.push(Pattern::SubType {
        variable: "$t".to_string(),
        parent_type: descriptor.type_name.clone(),
    });

    let sort_fields = dynamic_sort_patterns(var, sorts, &mut match_patterns)?;
    let mut clauses = vec![Clause::Match(match_patterns)];
    append_sort_limit_offset_fetch(
        &mut clauses,
        sort_fields,
        limit,
        offset,
        polymorphic_fetch_items(var),
    );
    Ok(clauses)
}

pub(crate) fn entity_fetch_by_iid_clauses(
    descriptor: &EntityDescriptor,
    iid: &str,
    var: &str,
) -> Vec<Clause> {
    entity_fetch_with_constraints_clauses(descriptor, vec![Constraint::Iid(iid.to_string())], var)
}

fn entity_fetch_with_constraints_clauses(
    descriptor: &EntityDescriptor,
    constraints: Vec<Constraint>,
    var: &str,
) -> Vec<Clause> {
    entity_fetch_with_filter_patterns(descriptor, constraints, vec![], var)
}

fn entity_fetch_with_filter_patterns(
    descriptor: &EntityDescriptor,
    constraints: Vec<Constraint>,
    extra_patterns: Vec<Pattern>,
    var: &str,
) -> Vec<Clause> {
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: "$t".to_string(),
        constraints,
        is_strict: true,
    }];
    match_patterns.extend(extra_patterns);
    match_patterns.push(Pattern::SubType {
        variable: "$t".to_string(),
        parent_type: descriptor.type_name.clone(),
    });

    vec![Clause::Match(match_patterns), polymorphic_fetch_items(var)]
}

pub(crate) fn entity_count_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints,
        is_strict: false,
    }];
    match_patterns.extend(extra_patterns);

    vec![Clause::Match(match_patterns), count_clause(var)]
}

pub(crate) fn entity_expr_count_clauses(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    var: &str,
) -> Result<Vec<Clause>> {
    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints: vec![],
        is_strict: false,
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }
    Ok(vec![Clause::Match(match_patterns), count_clause(var)])
}

pub(crate) fn entity_expr_aggregate_clauses(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints: vec![],
        is_strict: false,
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }
    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ])
}

pub(crate) fn entity_expr_group_by_aggregate_clauses(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    if group_fields.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic group-by aggregate for {} requires at least one group field",
            descriptor.type_name
        )));
    }

    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints: vec![],
        is_strict: false,
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }

    let mut group_vars = Vec::with_capacity(group_fields.len());
    for (index, field) in group_fields.iter().enumerate() {
        let attr = resolve_attribute_descriptor(
            &descriptor.type_name,
            &descriptor.owned_attributes,
            field,
        )?;
        let group_var = format!("$group{index}");
        match_patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: attr.attr_name.clone(),
            attr_var: group_var.clone(),
        });
        group_vars.push(group_var);
    }

    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: Some(group_vars.join(", ")),
        },
    ])
}

pub(crate) fn entity_aggregate_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    let mut match_patterns = vec![Pattern::Entity {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        constraints,
        is_strict: false,
    }];
    match_patterns.extend(extra_patterns);
    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ])
}

pub(crate) fn entity_group_by_aggregate_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    group_by_aggregate_clauses(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        &filters,
        group_fields,
        aggregates,
        var,
        |constraints| Pattern::Entity {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            constraints,
            is_strict: false,
        },
    )
}

pub(crate) fn entity_delete_by_iid_clauses(
    descriptor: &EntityDescriptor,
    iid: &str,
    var: &str,
) -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Entity {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            constraints: vec![Constraint::Iid(iid.to_string())],
            is_strict: false,
        }]),
        Clause::Delete(vec![Statement::DeleteThing(var.to_string())]),
    ]
}

pub(crate) fn relation_insert_clauses(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Vec<Clause> {
    relation_write_with_iid_clauses(
        ClauseKind::Insert,
        descriptor,
        attributes,
        role_players,
        var,
    )
}

pub(crate) fn relation_put_clauses(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Vec<Clause> {
    relation_write_with_iid_clauses(ClauseKind::Put, descriptor, attributes, role_players, var)
}

pub(crate) fn relation_update_clauses(
    descriptor: &RelationDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<Vec<Clause>> {
    let mut match_patterns = relation_identification_patterns(descriptor, iid, role_players, var)?;
    let mutation = update_mutation_clauses(
        &descriptor.type_name,
        descriptor
            .owned_attributes
            .iter()
            .map(|attr| (attr, attr.is_key())),
        attributes,
        var,
    )?;
    match_patterns.extend(mutation.match_patterns);

    let mut clauses = vec![Clause::Match(match_patterns)];
    if !mutation.delete_statements.is_empty() {
        clauses.push(Clause::Delete(mutation.delete_statements));
    }
    clauses.push(Clause::Insert(mutation.insert_statements));
    Ok(clauses)
}

fn relation_write_with_iid_clauses(
    kind: ClauseKind,
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Vec<Clause> {
    let match_patterns: Vec<_> = role_players
        .iter()
        .enumerate()
        .map(|(index, player)| role_player_match_pattern(player, &format!("$rp{index}")))
        .collect();

    let mut clauses = Vec::new();
    if !match_patterns.is_empty() {
        clauses.push(Clause::Match(match_patterns));
    }
    let relation_statement = Statement::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players: role_player_bindings(role_players),
        include_variable: true,
        attributes: attribute_statements(var, &descriptor.owned_attributes, attributes),
    };
    clauses.push(match kind {
        ClauseKind::Insert => Clause::Insert(vec![relation_statement]),
        ClauseKind::Put => Clause::Put(vec![relation_statement]),
    });
    clauses.push(Clause::Fetch(vec![FetchItem::Function {
        key: "iid".to_string(),
        func_name: "iid".to_string(),
        var: var.to_string(),
    }]));
    clauses
}

pub(crate) fn relation_fetch_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    relation_fetch_with_role_filters(descriptor, constraints, extra_patterns, &[], var)
}

pub(crate) fn relation_fetch_with_role_filters_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    role_filters: &[DynamicRolePlayerInput],
    var: &str,
) -> Vec<Clause> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    relation_fetch_with_role_filters(descriptor, constraints, extra_patterns, role_filters, var)
}

pub(crate) fn relation_expr_fetch_clauses(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    sorts: &[DynamicSort],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<Vec<Clause>> {
    let mut role_names = HashSet::new();
    for expression in expressions {
        expression.collect_roles(&mut role_names);
    }
    for sort in sorts {
        if let Some(role_name) = sort.role_name() {
            role_names.insert(role_name.to_string());
        }
    }

    // Same role partition as `relation_fetch_with_role_filters`: a role is
    // required when declared with minimum cardinality one or referenced by an
    // expression/sort (dereferencing a player implies its presence); every
    // other role is matched optionally so an unfilled role does not exclude
    // the relation, while a filled one still hydrates.
    let mut required_bindings: Vec<DynamicRoleBinding> = Vec::new();
    let mut optional_bindings: Vec<DynamicRoleBinding> = Vec::new();
    for (index, role) in descriptor.roles.iter().enumerate() {
        let referenced = role_names.contains(&role.role_name);
        let binding = DynamicRoleBinding {
            index,
            role_name: role.role_name.clone(),
            var_name: if referenced {
                role_player_expr_var(&role.role_name)
            } else {
                role_player_var(index)
            },
        };
        if referenced || is_required_role(role.cardinality) {
            required_bindings.push(binding);
        } else {
            optional_bindings.push(binding);
        }
    }

    let role_players = required_bindings
        .iter()
        .map(|binding| RolePlayer {
            role: binding.role_name.clone(),
            player_var: binding.var_name.clone(),
        })
        .collect();

    let mut counter = 0;
    let mut match_patterns = vec![
        Pattern::Relation {
            variable: var.to_string(),
            type_name: "$t".to_string(),
            role_players,
            constraints: vec![],
        },
        Pattern::SubType {
            variable: "$t".to_string(),
            parent_type: descriptor.type_name.clone(),
        },
    ];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }
    for binding in &required_bindings {
        match_patterns.push(Pattern::Entity {
            variable: binding.var_name.clone(),
            type_name: format!("{}_type", binding.var_name),
            constraints: vec![],
            is_strict: true,
        });
    }
    for binding in &optional_bindings {
        match_patterns.push(Pattern::Try(vec![
            Pattern::Relation {
                variable: var.to_string(),
                type_name: "$t".to_string(),
                role_players: vec![RolePlayer {
                    role: binding.role_name.clone(),
                    player_var: binding.var_name.clone(),
                }],
                constraints: vec![],
            },
            Pattern::Entity {
                variable: binding.var_name.clone(),
                type_name: format!("{}_type", binding.var_name),
                constraints: vec![],
                is_strict: true,
            },
        ]));
    }
    let sort_fields = dynamic_sort_patterns(var, sorts, &mut match_patterns)?;

    let mut included_roles = required_bindings;
    included_roles.extend(optional_bindings);
    included_roles.sort_by_key(|binding| binding.index);

    let mut clauses = vec![Clause::Match(match_patterns)];
    append_sort_limit_offset_fetch(
        &mut clauses,
        sort_fields,
        limit,
        offset,
        relation_fetch_items_for_bindings(var, &included_roles),
    );
    Ok(clauses)
}

pub(crate) fn relation_fetch_by_iid_clauses(
    descriptor: &RelationDescriptor,
    iid: &str,
    var: &str,
) -> Vec<Clause> {
    relation_fetch_with_constraints_clauses(descriptor, vec![Constraint::Iid(iid.to_string())], var)
}

fn relation_fetch_with_constraints_clauses(
    descriptor: &RelationDescriptor,
    constraints: Vec<Constraint>,
    var: &str,
) -> Vec<Clause> {
    relation_fetch_with_filter_patterns(descriptor, constraints, vec![], var)
}

fn relation_fetch_with_filter_patterns(
    descriptor: &RelationDescriptor,
    constraints: Vec<Constraint>,
    extra_patterns: Vec<Pattern>,
    var: &str,
) -> Vec<Clause> {
    relation_fetch_with_role_filters(descriptor, constraints, extra_patterns, &[], var)
}

fn relation_fetch_with_role_filters(
    descriptor: &RelationDescriptor,
    constraints: Vec<Constraint>,
    extra_patterns: Vec<Pattern>,
    role_filters: &[DynamicRolePlayerInput],
    var: &str,
) -> Vec<Clause> {
    // A role is matched in the main (mandatory) `links` pattern when it is
    // either declared with a minimum cardinality of one, or named by a role
    // filter (filtering on a player implies its presence). Every other role is
    // optional: the relation must still match when the role is unfilled, but a
    // filled role's player is fetched and hydrated like a required one.
    let mut required_role_indices: Vec<usize> = descriptor
        .roles
        .iter()
        .enumerate()
        .filter(|(_, role)| is_required_role(role.cardinality))
        .map(|(index, _)| index)
        .collect();

    let mut role_filter_patterns = Vec::new();
    for (filter_index, role_filter) in role_filters.iter().enumerate() {
        let filter_var = descriptor
            .roles
            .iter()
            .enumerate()
            .find(|(_, role)| role.role_name == role_filter.role_name)
            .map(|(index, _)| {
                if !required_role_indices.contains(&index) {
                    required_role_indices.push(index);
                }
                role_player_var(index)
            })
            .unwrap_or_else(|| {
                // A filter naming a role that is not in the descriptor's role
                // set still constrains the relation; bind it to a private var.
                format!("$rpf{filter_index}")
            });
        role_filter_patterns.push(role_player_match_pattern(role_filter, &filter_var));
    }

    let required_role_players: Vec<RolePlayer> = descriptor
        .roles
        .iter()
        .enumerate()
        .filter(|(index, _)| required_role_indices.contains(index))
        .map(|(index, role)| RolePlayer {
            role: role.role_name.clone(),
            player_var: role_player_var(index),
        })
        .collect();

    let mut match_patterns = vec![
        Pattern::Relation {
            variable: var.to_string(),
            type_name: "$t".to_string(),
            role_players: required_role_players,
            constraints,
        },
        Pattern::SubType {
            variable: "$t".to_string(),
            parent_type: descriptor.type_name.clone(),
        },
    ];
    match_patterns.extend(extra_patterns);
    match_patterns.extend(role_filter_patterns);

    // Required roles bind their player type in the main match; optional roles
    // bind theirs inside a `try` block so an unfilled role does not exclude the
    // relation, while a filled one still yields the player's iid/type/attributes.
    for (index, role) in descriptor.roles.iter().enumerate() {
        if required_role_indices.contains(&index) {
            match_patterns.push(Pattern::Entity {
                variable: role_player_var(index),
                type_name: role_player_type_var(index),
                constraints: vec![],
                is_strict: true,
            });
        } else {
            match_patterns.push(Pattern::Try(vec![
                Pattern::Relation {
                    variable: var.to_string(),
                    type_name: "$t".to_string(),
                    role_players: vec![RolePlayer {
                        role: role.role_name.clone(),
                        player_var: role_player_var(index),
                    }],
                    constraints: vec![],
                },
                Pattern::Entity {
                    variable: role_player_var(index),
                    type_name: role_player_type_var(index),
                    constraints: vec![],
                    is_strict: true,
                },
            ]));
        }
    }

    // Every role contributes fetch items; absent optional-role keys are
    // tolerated by hydration, which skips a role whose iid key is missing.
    let included_role_indices: Vec<usize> = (0..descriptor.roles.len()).collect();

    vec![
        Clause::Match(match_patterns),
        relation_fetch_items(descriptor, var, &included_role_indices),
    ]
}

pub(crate) fn relation_count_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    let mut match_patterns = vec![Pattern::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players: vec![],
        constraints,
    }];
    match_patterns.extend(extra_patterns);

    vec![Clause::Match(match_patterns), count_clause(var)]
}

pub(crate) fn relation_expr_count_clauses(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    var: &str,
) -> Result<Vec<Clause>> {
    let mut role_names = HashSet::new();
    for expression in expressions {
        expression.collect_roles(&mut role_names);
    }
    let role_players = dynamic_relation_role_bindings(descriptor, &role_names)
        .into_iter()
        .map(|binding| RolePlayer {
            role: binding.role_name,
            player_var: binding.var_name,
        })
        .collect();

    let mut counter = 0;
    let mut match_patterns = vec![
        Pattern::Relation {
            variable: var.to_string(),
            type_name: "$t".to_string(),
            role_players,
            constraints: vec![],
        },
        Pattern::SubType {
            variable: "$t".to_string(),
            parent_type: descriptor.type_name.clone(),
        },
    ];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }

    Ok(vec![Clause::Match(match_patterns), count_clause(var)])
}

pub(crate) fn relation_expr_aggregate_clauses(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    // Collect role names referenced in expression tree so we can bind role
    // player variables if needed (mirrors relation_expr_count_clauses).
    let mut role_names = HashSet::new();
    for expression in expressions {
        expression.collect_roles(&mut role_names);
    }
    let role_players = dynamic_relation_role_bindings(descriptor, &role_names)
        .into_iter()
        .map(|binding| RolePlayer {
            role: binding.role_name,
            player_var: binding.var_name,
        })
        .collect();

    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players,
        constraints: vec![],
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }
    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ])
}

pub(crate) fn relation_expr_group_by_aggregate_clauses(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    if group_fields.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic group-by aggregate for {} requires at least one group field",
            descriptor.type_name
        )));
    }

    // Collect role names referenced in expression tree.
    let mut role_names = HashSet::new();
    for expression in expressions {
        expression.collect_roles(&mut role_names);
    }
    let role_players = dynamic_relation_role_bindings(descriptor, &role_names)
        .into_iter()
        .map(|binding| RolePlayer {
            role: binding.role_name,
            player_var: binding.var_name,
        })
        .collect();

    let mut counter = 0;
    let mut match_patterns = vec![Pattern::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players,
        constraints: vec![],
    }];
    for expression in expressions {
        match_patterns.extend(expression.to_patterns(var, &mut counter)?);
    }

    let mut group_vars = Vec::with_capacity(group_fields.len());
    for (index, field) in group_fields.iter().enumerate() {
        let attr = resolve_attribute_descriptor(
            &descriptor.type_name,
            &descriptor.owned_attributes,
            field,
        )?;
        let group_var = format!("$group{index}");
        match_patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: attr.attr_name.clone(),
            attr_var: group_var.clone(),
        });
        group_vars.push(group_var);
    }

    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: Some(group_vars.join(", ")),
        },
    ])
}

pub(crate) fn relation_aggregate_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    let (constraints, extra_patterns) = filter_match_parts(&filters, var);
    let mut match_patterns = vec![Pattern::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players: vec![],
        constraints,
    }];
    match_patterns.extend(extra_patterns);
    let assignments = aggregate_assignments(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        aggregates,
        var,
        &mut match_patterns,
    )?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ])
}

pub(crate) fn relation_group_by_aggregate_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<Vec<Clause>> {
    let filters = normalize_filters(&descriptor.owned_attributes, filters);
    group_by_aggregate_clauses(
        &descriptor.type_name,
        &descriptor.owned_attributes,
        &filters,
        group_fields,
        aggregates,
        var,
        |constraints| Pattern::Relation {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            role_players: vec![],
            constraints,
        },
    )
}

pub(crate) fn relation_delete_by_iid_clauses(
    descriptor: &RelationDescriptor,
    iid: &str,
    var: &str,
) -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Relation {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            role_players: vec![],
            constraints: vec![Constraint::Iid(iid.to_string())],
        }]),
        Clause::Delete(vec![Statement::DeleteThing(var.to_string())]),
    ]
}

fn attribute_statements(
    var: &str,
    descriptors: &[OwnedAttributeDescriptor],
    attributes: &DynamicAttributeMap,
) -> Vec<Statement> {
    attributes
        .iter()
        .map(|(attr_name, value)| Statement::Has {
            subject_var: var.to_string(),
            attr_name: resolve_input_attribute_name(descriptors, attr_name),
            value: value.to_ast_value(),
        })
        .collect()
}

fn resolve_input_attribute_name(
    descriptors: &[OwnedAttributeDescriptor],
    input_name: &str,
) -> String {
    descriptors
        .iter()
        .find(|attr| input_name == attr.field_name || input_name == attr.attr_name)
        .map(|attr| attr.attr_name.clone())
        .unwrap_or_else(|| input_name.to_string())
}

fn entity_identification_constraints(
    descriptor: &EntityDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
) -> Result<Vec<Constraint>> {
    if let Some(iid) = iid.filter(|iid| !iid.is_empty()) {
        return Ok(vec![Constraint::Iid(iid.to_string())]);
    }

    let Some(key) = descriptor.key_attribute() else {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic update for {} requires an IID or @key attribute",
            descriptor.type_name
        )));
    };
    let Some((_, value)) = find_attribute_value(attributes, key) else {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic update for {} requires key attribute {}",
            descriptor.type_name, key.attr_name
        )));
    };
    Ok(vec![Constraint::Has {
        attr_name: key.attr_name.clone(),
        value: value.to_ast_value(),
    }])
}

fn relation_identification_patterns(
    descriptor: &RelationDescriptor,
    iid: Option<&str>,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<Vec<Pattern>> {
    if let Some(iid) = iid.filter(|iid| !iid.is_empty()) {
        return Ok(vec![Pattern::Relation {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            role_players: vec![],
            constraints: vec![Constraint::Iid(iid.to_string())],
        }]);
    }

    if role_players.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic update for {} requires an IID or role players",
            descriptor.type_name
        )));
    }

    let mut patterns: Vec<_> = role_players
        .iter()
        .enumerate()
        .map(|(index, player)| role_player_match_pattern(player, &format!("$rp{index}")))
        .collect();
    patterns.push(Pattern::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players: role_player_bindings(role_players),
        constraints: vec![],
    });
    Ok(patterns)
}

struct UpdateMutation {
    match_patterns: Vec<Pattern>,
    delete_statements: Vec<Statement>,
    insert_statements: Vec<Statement>,
}

fn update_mutation_clauses<'a>(
    type_name: &str,
    descriptors: impl Iterator<Item = (&'a OwnedAttributeDescriptor, bool)>,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<UpdateMutation> {
    let descriptors: Vec<_> = descriptors.collect();
    let mut match_patterns = Vec::new();
    let mut delete_statements = Vec::new();
    let mut insert_statements = Vec::new();
    let mut deletion_attrs: Vec<String> = Vec::new();

    for (name, value) in attributes {
        let Some((attr, is_key)) = descriptors.iter().find(|(attr, _)| {
            name.as_str() == attr.attr_name.as_str() || name.as_str() == attr.field_name.as_str()
        }) else {
            continue;
        };
        if *is_key {
            continue;
        }

        if !deletion_attrs.contains(&attr.attr_name) {
            let old_var = format!("$old_attr_{}", deletion_attrs.len());
            deletion_attrs.push(attr.attr_name.clone());
            match_patterns.push(Pattern::Raw(format!(
                "try {{ {var} has {} {old_var}; }}",
                attr.attr_name
            )));
            delete_statements.push(Statement::Raw(format!("try {{ {old_var} of {var}; }}")));
        }

        insert_statements.push(Statement::Has {
            subject_var: var.to_string(),
            attr_name: attr.attr_name.clone(),
            value: value.to_ast_value(),
        });
    }

    if insert_statements.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic update for {type_name} has no non-key attributes"
        )));
    }

    Ok(UpdateMutation {
        match_patterns,
        delete_statements,
        insert_statements,
    })
}

fn find_attribute_value<'a>(
    attributes: &'a DynamicAttributeMap,
    descriptor: &OwnedAttributeDescriptor,
) -> Option<&'a (String, AttributeValue)> {
    attributes.iter().find(|(name, _)| {
        name.as_str() == descriptor.attr_name.as_str()
            || name.as_str() == descriptor.field_name.as_str()
    })
}

fn group_by_aggregate_clauses(
    type_name: &str,
    descriptors: &[OwnedAttributeDescriptor],
    filters: &[Filter],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
    base_pattern: impl FnOnce(Vec<Constraint>) -> Pattern,
) -> Result<Vec<Clause>> {
    if group_fields.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic group-by aggregate for {type_name} requires at least one group field"
        )));
    }

    let (constraints, extra_patterns) = filter_match_parts(filters, var);
    let mut match_patterns = vec![base_pattern(constraints)];
    match_patterns.extend(extra_patterns);
    let mut group_vars = Vec::with_capacity(group_fields.len());
    for (index, field) in group_fields.iter().enumerate() {
        let attr = resolve_attribute_descriptor(type_name, descriptors, field)?;
        let group_var = format!("$group{index}");
        match_patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: attr.attr_name.clone(),
            attr_var: group_var.clone(),
        });
        group_vars.push(group_var);
    }

    let assignments =
        aggregate_assignments(type_name, descriptors, aggregates, var, &mut match_patterns)?;
    Ok(vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: Some(group_vars.join(", ")),
        },
    ])
}

fn aggregate_assignments(
    type_name: &str,
    descriptors: &[OwnedAttributeDescriptor],
    aggregates: &[DynamicAggregate],
    var: &str,
    match_patterns: &mut Vec<Pattern>,
) -> Result<Vec<ReduceAssignment>> {
    if aggregates.is_empty() {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic aggregate for {type_name} requires at least one aggregate"
        )));
    }

    let mut assignments = Vec::with_capacity(aggregates.len());
    let mut attr_vars: HashMap<String, String> = HashMap::new();
    for (index, aggregate) in aggregates.iter().enumerate() {
        validate_variable_key(type_name, &aggregate.result_key)?;
        let function = validate_aggregate_function(type_name, &aggregate.function)?;
        let args = if function == "count" {
            vec![Value::Variable(var.to_string())]
        } else {
            let attr_name = aggregate.attr_name.as_deref().ok_or_else(|| {
                OrmError::QueryExecution(format!(
                    "Dynamic aggregate {function} for {type_name} requires an attribute"
                ))
            })?;
            let attr = resolve_attribute_descriptor(type_name, descriptors, attr_name)?;
            let attr_var = attr_vars
                .entry(attr.attr_name.clone())
                .or_insert_with(|| {
                    let attr_var = format!("$agg{index}");
                    match_patterns.push(Pattern::Has {
                        thing_var: var.to_string(),
                        attr_type: attr.attr_name.clone(),
                        attr_var: attr_var.clone(),
                    });
                    attr_var
                })
                .clone();
            vec![Value::Variable(attr_var)]
        };

        assignments.push(ReduceAssignment {
            variable: format!("${}", aggregate.result_key),
            expression: Value::FunctionCall(FunctionCallValue {
                function: function.to_string(),
                args,
            }),
        });
    }
    Ok(assignments)
}

fn resolve_attribute_descriptor<'a>(
    type_name: &str,
    descriptors: &'a [OwnedAttributeDescriptor],
    name: &str,
) -> Result<&'a OwnedAttributeDescriptor> {
    descriptors
        .iter()
        .find(|attr| attr.field_name == name || attr.attr_name == name)
        .ok_or_else(|| {
            OrmError::QueryExecution(format!(
                "Dynamic aggregate for {type_name} references unknown attribute {name}"
            ))
        })
}

fn validate_aggregate_function<'a>(type_name: &str, function: &'a str) -> Result<&'a str> {
    match function {
        "count" | "sum" | "mean" | "min" | "max" | "median" | "std" => Ok(function),
        other => Err(OrmError::QueryExecution(format!(
            "Dynamic aggregate for {type_name} uses unsupported function {other}"
        ))),
    }
}

fn validate_variable_key(type_name: &str, key: &str) -> Result<()> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic aggregate for {type_name} has an empty result key"
        )));
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic aggregate result key {key} for {type_name} is not a valid variable name"
        )));
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(OrmError::QueryExecution(format!(
            "Dynamic aggregate result key {key} for {type_name} is not a valid variable name"
        )));
    }
    Ok(())
}

fn filter_match_parts(filters: &[Filter], var: &str) -> (Vec<Constraint>, Vec<Pattern>) {
    let mut constraints = Vec::new();
    let mut patterns = Vec::new();
    for (index, filter) in filters.iter().enumerate() {
        if filter.operator == "==" {
            constraints.push(Constraint::Has {
                attr_name: filter.attr_name.clone(),
                value: filter.value.to_ast_value(),
            });
            continue;
        }
        let attr_var = format!("$filter{index}");
        patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: filter.attr_name.clone(),
            attr_var: attr_var.clone(),
        });
        patterns.push(Pattern::ValueComparison {
            var: attr_var,
            operator: filter.operator.clone(),
            value: filter.value.to_ast_value(),
        });
    }
    (constraints, patterns)
}

fn normalize_filters(descriptors: &[OwnedAttributeDescriptor], filters: &[Filter]) -> Vec<Filter> {
    filters
        .iter()
        .map(|filter| {
            let Some(attr) = descriptors.iter().find(|attr| {
                filter.attr_name == attr.field_name || filter.attr_name == attr.attr_name
            }) else {
                return filter.clone();
            };
            let mut normalized = filter.clone();
            normalized.attr_name = attr.attr_name.clone();
            normalized
        })
        .collect()
}

fn polymorphic_fetch_items(var: &str) -> Clause {
    Clause::Fetch(vec![
        FetchItem::Function {
            key: "_iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        },
        FetchItem::Function {
            key: "_type".to_string(),
            func_name: "label".to_string(),
            var: "$t".to_string(),
        },
        FetchItem::NestedWildcard {
            key: "attributes".to_string(),
            var: var.to_string(),
        },
    ])
}

fn relation_fetch_items(
    descriptor: &RelationDescriptor,
    var: &str,
    included_role_indices: &[usize],
) -> Clause {
    let mut items = match polymorphic_fetch_items(var) {
        Clause::Fetch(items) => items,
        _ => unreachable!("polymorphic_fetch_items always returns a fetch clause"),
    };

    for (index, _role) in descriptor.roles.iter().enumerate() {
        if !included_role_indices.contains(&index) {
            continue;
        }
        let var = role_player_var(index);
        items.push(FetchItem::Function {
            key: format!("_role_{index}_iid"),
            func_name: "iid".to_string(),
            var: var.clone(),
        });
        items.push(FetchItem::Function {
            key: format!("_role_{index}_type"),
            func_name: "label".to_string(),
            var: role_player_type_var(index),
        });
        items.push(FetchItem::NestedWildcard {
            key: format!("_role_{index}_attributes"),
            var,
        });
    }

    Clause::Fetch(items)
}

#[derive(Debug, Clone)]
struct DynamicRoleBinding {
    index: usize,
    role_name: String,
    var_name: String,
}

// Binds only the roles a count/aggregate query genuinely constrains: roles
// referenced by an expression, plus explicit min>=1 roles (whose presence the
// engine already guarantees). A role with no explicit minimum must NOT be
// bound here — requiring it would silently drop relations that leave the role
// unfilled.
fn dynamic_relation_role_bindings(
    descriptor: &RelationDescriptor,
    role_names: &HashSet<String>,
) -> Vec<DynamicRoleBinding> {
    let mut bindings = Vec::new();
    for (index, role) in descriptor.roles.iter().enumerate() {
        let referenced = role_names.contains(&role.role_name);
        if referenced || is_required_role(role.cardinality) {
            bindings.push(DynamicRoleBinding {
                index,
                role_name: role.role_name.clone(),
                var_name: if referenced {
                    role_player_expr_var(&role.role_name)
                } else {
                    role_player_var(index)
                },
            });
        }
    }
    bindings
}

fn relation_fetch_items_for_bindings(var: &str, bindings: &[DynamicRoleBinding]) -> Clause {
    let mut items = match polymorphic_fetch_items(var) {
        Clause::Fetch(items) => items,
        _ => unreachable!("polymorphic_fetch_items always returns a fetch clause"),
    };

    for binding in bindings {
        items.push(FetchItem::Function {
            key: format!("_role_{}_iid", binding.index),
            func_name: "iid".to_string(),
            var: binding.var_name.clone(),
        });
        items.push(FetchItem::Function {
            key: format!("_role_{}_type", binding.index),
            func_name: "label".to_string(),
            var: format!("{}_type", binding.var_name),
        });
        items.push(FetchItem::NestedWildcard {
            key: format!("_role_{}_attributes", binding.index),
            var: binding.var_name.clone(),
        });
    }

    Clause::Fetch(items)
}

fn dynamic_sort_patterns(
    thing_var: &str,
    sorts: &[DynamicSort],
    match_patterns: &mut Vec<Pattern>,
) -> Result<Vec<SortField>> {
    let mut sort_fields = Vec::with_capacity(sorts.len());
    for (index, sort) in sorts.iter().enumerate() {
        let (owner_var, attr_name) = match sort {
            DynamicSort::Attribute { attr_name, .. } => (thing_var.to_string(), attr_name),
            DynamicSort::RolePlayerAttribute {
                role_name,
                attr_name,
                ..
            } => (role_player_expr_var(role_name), attr_name),
        };
        if attr_name.is_empty() {
            return Err(OrmError::QueryExecution(
                "Dynamic sort attribute name cannot be empty".into(),
            ));
        }
        let sort_var = format!("$dyn_sort{index}");
        match_patterns.push(Pattern::Has {
            thing_var: owner_var,
            attr_type: attr_name.clone(),
            attr_var: sort_var.clone(),
        });
        sort_fields.push(SortField {
            variable: sort_var,
            ascending: sort.direction() == SortDir::Asc,
        });
    }
    Ok(sort_fields)
}

fn append_sort_limit_offset_fetch(
    clauses: &mut Vec<Clause>,
    sort_fields: Vec<SortField>,
    limit: Option<u64>,
    offset: Option<u64>,
    fetch: Clause,
) {
    if !sort_fields.is_empty() {
        clauses.push(Clause::Sort(sort_fields));
    }
    if let Some(offset) = offset {
        clauses.push(Clause::Offset(offset));
    }
    if let Some(limit) = limit {
        clauses.push(Clause::Limit(limit));
    }
    clauses.push(fetch);
}

fn role_player_var(index: usize) -> String {
    format!("$rp{index}")
}

fn role_player_expr_var(role_name: &str) -> String {
    format!("${role_name}")
}

fn role_player_type_var(index: usize) -> String {
    format!("$rp{index}_type")
}

fn next_dynamic_var(prefix: &str, counter: &mut usize) -> String {
    let var = format!("{prefix}{counter}");
    *counter += 1;
    var
}

/// A role is *required* for matching only when it carries an explicit
/// cardinality whose minimum is at least one. A `None` cardinality inherits
/// TypeDB's default `relates`, which permits zero players, so it is optional;
/// an explicit `(0, _)` is optional by definition.
fn is_required_role(cardinality: Option<(u32, Option<u32>)>) -> bool {
    matches!(cardinality, Some((min, _)) if min >= 1)
}

fn count_clause(var: &str) -> Clause {
    Clause::Reduce {
        assignments: vec![ReduceAssignment {
            variable: "$count".to_string(),
            expression: Value::FunctionCall(FunctionCallValue {
                function: "count".into(),
                args: vec![Value::Variable(var.to_string())],
            }),
        }],
        group_by: None,
    }
}

fn role_player_match_pattern(player: &DynamicRolePlayerInput, var: &str) -> Pattern {
    let mut constraints = Vec::new();
    if let Some(iid) = &player.iid {
        constraints.push(Constraint::Iid(iid.clone()));
    } else if let Some((attr_name, value)) = &player.key {
        constraints.push(Constraint::Has {
            attr_name: attr_name.clone(),
            value: value.to_ast_value(),
        });
    }

    Pattern::Entity {
        variable: var.to_string(),
        type_name: player.player_type_name.clone(),
        constraints,
        is_strict: false,
    }
}

fn role_player_bindings(role_players: &[DynamicRolePlayerInput]) -> Vec<RolePlayer> {
    role_players
        .iter()
        .enumerate()
        .map(|(index, player)| RolePlayer {
            role: player.role_name.clone(),
            player_var: format!("$rp{index}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::RoleDescriptor;
    use type_bridge_core_lib::compiler::QueryCompiler;

    fn role(name: &str, cardinality: Option<(u32, Option<u32>)>) -> RoleDescriptor {
        RoleDescriptor {
            role_name: name.to_string(),
            player_type_names: vec!["account".to_string()],
            cardinality,
        }
    }

    /// Build a relation descriptor whose role set exercises every fetch
    /// partition: a required role (explicit min one), an unbounded-required
    /// role, a `None`-cardinality role, and an explicit `(0, n)` role.
    fn descriptor() -> RelationDescriptor {
        RelationDescriptor {
            type_name: "interaction".to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![
                role("sender", Some((1, Some(1)))),
                role("receiver", Some((1, None))),
                role("participant", None),
                role("observer", Some((0, Some(5)))),
            ],
        }
    }

    fn compile(clauses: &[Clause]) -> String {
        QueryCompiler::new().compile(clauses)
    }

    #[test]
    fn required_roles_stay_in_main_links_pattern() {
        let clauses = relation_fetch_by_iid_clauses(&descriptor(), "0xabc", "$r");
        let query = compile(&clauses);

        // Both min>=1 roles bind in the mandatory relation pattern, outside any
        // `try` block.
        let main_pattern = query
            .lines()
            .find(|line| line.contains("isa $t (") && line.contains("sender"))
            .expect("main relation pattern present");
        assert!(main_pattern.contains("sender: $rp0"));
        assert!(main_pattern.contains("receiver: $rp1"));
        assert!(!main_pattern.contains("participant"));
        assert!(!main_pattern.contains("observer"));
    }

    #[test]
    fn none_cardinality_role_renders_optional() {
        let clauses = relation_fetch_by_iid_clauses(&descriptor(), "0xabc", "$r");
        let query = compile(&clauses);

        // `participant` (None cardinality) must be matched inside a `try` block,
        // not in the mandatory pattern.
        assert!(query.contains("try { $r isa $t (participant: $rp2)"));
        assert!(query.contains("$rp2 isa! $rp2_type"));
    }

    #[test]
    fn explicit_zero_min_role_is_fetched_optionally() {
        let clauses = relation_fetch_by_iid_clauses(&descriptor(), "0xabc", "$r");
        let query = compile(&clauses);

        // The explicit `(0, 5)` role is no longer dropped: it is matched
        // optionally and its player iid/type/attributes are fetched.
        assert!(query.contains("try { $r isa $t (observer: $rp3)"));
        assert!(query.contains("\"_role_3_iid\""));
        assert!(query.contains("\"_role_3_type\""));
        assert!(query.contains("\"_role_3_attributes\""));
    }

    #[test]
    fn all_roles_emit_fetch_items() {
        let clauses = relation_fetch_by_iid_clauses(&descriptor(), "0xabc", "$r");
        let query = compile(&clauses);

        // Every role, required or optional, contributes its fetch keys.
        for index in 0..4 {
            assert!(query.contains(&format!("\"_role_{index}_iid\"")));
            assert!(query.contains(&format!("\"_role_{index}_attributes\"")));
        }
    }

    #[test]
    fn role_filter_forces_optional_role_required() {
        let role_filters = vec![DynamicRolePlayerInput {
            role_name: "participant".to_string(),
            player_type_name: "account".to_string(),
            iid: Some("0xdef".to_string()),
            key: None,
        }];
        let clauses = relation_fetch_with_role_filters(
            &descriptor(),
            vec![],
            vec![],
            &role_filters,
            "$r",
        );
        let query = compile(&clauses);

        // Filtering on `participant` (otherwise optional) pulls it into the
        // mandatory pattern; it must not appear inside a `try` block.
        let main_pattern = query
            .lines()
            .find(|line| line.contains("isa $t (") && line.contains("participant"))
            .expect("main relation pattern binds the filtered role");
        assert!(main_pattern.contains("participant: $rp2"));
        assert!(!query.contains("try { $r isa $t (participant"));
        // The filter constraint on the player is still emitted.
        assert!(query.contains("iid 0xdef"));
    }

    #[test]
    fn expr_fetch_partitions_roles_like_plain_fetch() {
        let clauses =
            relation_expr_fetch_clauses(&descriptor(), &[], &[], None, None, "$r").unwrap();
        let query = compile(&clauses);

        // min>=1 roles stay in the mandatory relation pattern.
        let main_pattern = query
            .lines()
            .find(|l| l.contains("$r isa $t (") && !l.trim_start().starts_with("try"))
            .expect("main relation pattern present");
        assert!(main_pattern.contains("sender: $rp0"));
        assert!(main_pattern.contains("receiver: $rp1"));

        // None-card and explicit (0,n) roles are matched optionally and
        // still contribute fetch items.
        assert!(query.contains("try { $r isa $t (participant: $rp2)"));
        assert!(query.contains("try { $r isa $t (observer: $rp3)"));
        assert!(query.contains("\"_role_2_iid\""));
        assert!(query.contains("\"_role_3_iid\""));
    }

    #[test]
    fn expr_count_does_not_require_unreferenced_optional_roles() {
        let clauses = relation_expr_count_clauses(&descriptor(), &[], "$r").unwrap();
        let query = compile(&clauses);

        // Counting must not demand players for roles the engine allows to be
        // unfilled; only explicit min>=1 roles stay in the pattern.
        assert!(query.contains("sender: $rp0"));
        assert!(query.contains("receiver: $rp1"));
        assert!(!query.contains("participant"));
        assert!(!query.contains("observer"));
    }
}

