//! Dynamic rows and inputs used by runtime descriptor managers.

use serde::{Deserialize, Serialize};
use type_bridge_core_lib::ast::{
    Clause, Constraint, FetchItem, FunctionCallValue, Pattern, ReduceAssignment, RolePlayer,
    Statement, Value,
};

use crate::descriptor::{EntityDescriptor, RelationDescriptor};
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

pub(crate) fn entity_insert_clauses(
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Vec<Clause> {
    let mut statements = vec![Statement::Isa {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
    }];
    statements.extend(attribute_statements(var, attributes));
    vec![
        Clause::Insert(statements),
        Clause::Fetch(vec![FetchItem::Function {
            key: "iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        }]),
    ]
}

pub(crate) fn entity_fetch_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    let constraints = filter_constraints(filters);
    vec![
        Clause::Match(vec![
            Pattern::Entity {
                variable: var.to_string(),
                type_name: "$t".to_string(),
                constraints,
                is_strict: true,
            },
            Pattern::SubType {
                variable: "$t".to_string(),
                parent_type: descriptor.type_name.clone(),
            },
        ]),
        polymorphic_fetch_items(var),
    ]
}

pub(crate) fn entity_count_clauses(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Entity {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            constraints: filter_constraints(filters),
            is_strict: false,
        }]),
        count_clause(var),
    ]
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
        Clause::Delete(vec![Statement::Isa {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
        }]),
    ]
}

pub(crate) fn relation_insert_clauses(
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

    let role_players: Vec<_> = role_players
        .iter()
        .enumerate()
        .map(|(index, player)| RolePlayer {
            role: player.role_name.clone(),
            player_var: format!("$rp{index}"),
        })
        .collect();

    let mut clauses = Vec::new();
    if !match_patterns.is_empty() {
        clauses.push(Clause::Match(match_patterns));
    }
    clauses.push(Clause::Insert(vec![Statement::Relation {
        variable: var.to_string(),
        type_name: descriptor.type_name.clone(),
        role_players,
        include_variable: true,
        attributes: attribute_statements(var, attributes),
    }]));
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
    vec![
        Clause::Match(vec![
            Pattern::Relation {
                variable: var.to_string(),
                type_name: "$t".to_string(),
                role_players: vec![],
                constraints: filter_constraints(filters),
            },
            Pattern::SubType {
                variable: "$t".to_string(),
                parent_type: descriptor.type_name.clone(),
            },
        ]),
        polymorphic_fetch_items(var),
    ]
}

pub(crate) fn relation_count_clauses(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    var: &str,
) -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Relation {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
            role_players: vec![],
            constraints: filter_constraints(filters),
        }]),
        count_clause(var),
    ]
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
        Clause::Delete(vec![Statement::Isa {
            variable: var.to_string(),
            type_name: descriptor.type_name.clone(),
        }]),
    ]
}

fn attribute_statements(var: &str, attributes: &DynamicAttributeMap) -> Vec<Statement> {
    attributes
        .iter()
        .map(|(attr_name, value)| Statement::Has {
            subject_var: var.to_string(),
            attr_name: attr_name.clone(),
            value: value.to_ast_value(),
        })
        .collect()
}

fn filter_constraints(filters: &[Filter]) -> Vec<Constraint> {
    filters
        .iter()
        .map(|filter| Constraint::Has {
            attr_name: filter.attr_name.clone(),
            value: filter.value.to_ast_value(),
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
