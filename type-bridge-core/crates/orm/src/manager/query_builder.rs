//! Internal query builder wrapping the core [`QueryCompiler`].
//!
//! Constructs AST clauses from entity trait methods and compiles them
//! to TypeQL strings ready for execution.

use std::collections::HashSet;

use type_bridge_core_lib::ast::{
    Clause, Constraint, FetchItem, FunctionCallValue, Pattern, ReduceAssignment, RolePlayer,
    SortField, Statement, Value,
};
use type_bridge_core_lib::compiler::QueryCompiler;

use crate::entity::TypeBridgeEntity;
use crate::error::Result;
use crate::expr::{Agg, Expr, SortDir};
use crate::filter::Filter;
use crate::relation::TypeBridgeRelation;
use crate::{
    descriptor::{EntityDescriptor, RelationDescriptor},
    dynamic::{
        DynamicAggregate, DynamicAttributeMap, DynamicExpr, DynamicRolePlayerInput, DynamicSort,
    },
};

/// Build an insert + fetch-IID query for the given entity.
///
/// Produces TypeQL like:
/// ```text
/// insert $e isa person, has name "Alice", has age 30;
/// fetch { "iid": iid($e) };
/// ```
pub fn build_insert_with_iid<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let clauses = entity.to_insert_with_iid_fetch(var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query with optional filters.
///
/// Produces TypeQL like:
/// ```text
/// match $e isa! $t, has name "Alice"; $t sub person;
/// fetch { "_iid": iid($e), "_type": label($t), "attributes": { $e.* } };
/// ```
pub fn build_fetch<T: TypeBridgeEntity>(filters: &[Filter], var: &str) -> Result<String> {
    let clauses = T::build_polymorphic_fetch(var, T::TYPE_NAME, filters);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + delete query for a specific entity instance.
///
/// Uses IID or @key attributes for identification. Produces:
/// ```text
/// match $e isa person, has name "Alice";
/// delete $e;
/// ```
pub fn build_delete<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let clauses = vec![
        Clause::Match(vec![entity.to_match_pattern(var)]),
        Clause::Delete(vec![Statement::DeleteThing(var.to_string())]),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query with optional filters.
///
/// Produces TypeQL like:
/// ```text
/// match $e isa person, has name "Alice";
/// reduce $count = count($e);
/// ```
pub fn build_count<T: TypeBridgeEntity>(filters: &[Filter], var: &str) -> Result<String> {
    let constraints: Vec<Constraint> = filters
        .iter()
        .map(|f| Constraint::Has {
            attr_name: f.attr_name.clone(),
            value: f.value.to_ast_value(),
        })
        .collect();

    let clauses = vec![
        Clause::Match(vec![Pattern::Entity {
            variable: var.to_string(),
            type_name: T::TYPE_NAME.to_string(),
            constraints,
            is_strict: false,
        }]),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Dynamic entity query builders
// ------------------------------------------------------------------

/// Build an insert + fetch-IID query for a runtime entity descriptor.
pub fn build_dynamic_entity_insert_with_iid(
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_insert_clauses(descriptor, attributes, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a put + fetch-IID query for a runtime entity descriptor.
pub fn build_dynamic_entity_put(
    descriptor: &EntityDescriptor,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_put_clauses(descriptor, attributes, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + update query for a runtime entity descriptor.
pub fn build_dynamic_entity_update(
    descriptor: &EntityDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_update_clauses(descriptor, iid, attributes, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query for a runtime entity descriptor.
pub fn build_dynamic_entity_fetch(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_fetch_clauses(descriptor, filters, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware fetch query for a runtime entity descriptor.
pub fn build_dynamic_entity_expr_fetch(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    sorts: &[DynamicSort],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_expr_fetch_clauses(
        descriptor,
        expressions,
        sorts,
        limit,
        offset,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a cross-type or narrowed attribute-owner lookup query.
///
/// This backs the Python `TypeDBType.has(...)` surface without requiring
/// Python-side TypeQL construction. `kind` is used only for cross-type
/// lookups, where TypeDB needs an `entity $e` or `relation $r` type binding.
pub fn build_dynamic_has_lookup_query(
    kind: &str,
    attr_name: &str,
    expression: Option<&DynamicExpr>,
    type_name: Option<&str>,
) -> Result<String> {
    let (mut match_patterns, label_var) = if let Some(type_name) = type_name {
        (
            vec![Pattern::SubType {
                variable: "$t".to_string(),
                parent_type: type_name.to_string(),
            }],
            "$t".to_string(),
        )
    } else {
        let kind_var = match kind {
            "entity" => "$e",
            "relation" => "$r",
            other => {
                return Err(crate::error::OrmError::QueryExecution(format!(
                    "has lookup kind must be 'entity' or 'relation', got {other:?}"
                )));
            }
        };
        (
            vec![Pattern::Raw(format!("{kind} {kind_var}"))],
            kind_var.to_string(),
        )
    };

    let isa_op = if type_name.is_some() { "isa!" } else { "isa" };
    let isa_anchor = if type_name.is_some() {
        "$t"
    } else if kind == "entity" {
        "$e"
    } else {
        "$r"
    };

    if let Some(expression) = expression {
        match_patterns.push(Pattern::Raw(format!("$x {isa_op} {isa_anchor}")));
        let mut counter = 0;
        match_patterns.extend(expression.to_patterns("$x", &mut counter)?);
    } else {
        match_patterns.push(Pattern::Raw(format!(
            "$x {isa_op} {isa_anchor}, has {attr_name} $n"
        )));
    }

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Fetch(vec![
            FetchItem::Function {
                key: "_iid".to_string(),
                func_name: "iid".to_string(),
                var: "$x".to_string(),
            },
            FetchItem::Function {
                key: "_type".to_string(),
                func_name: "label".to_string(),
                var: label_var,
            },
            FetchItem::NestedWildcard {
                key: "attributes".to_string(),
                var: "$x".to_string(),
            },
        ]),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic IID fetch query for a runtime entity descriptor.
pub fn build_dynamic_entity_fetch_by_iid(
    descriptor: &EntityDescriptor,
    iid: &str,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_fetch_by_iid_clauses(descriptor, iid, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query for a runtime entity descriptor.
pub fn build_dynamic_entity_count(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_count_clauses(descriptor, filters, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware count query for a runtime entity descriptor.
pub fn build_dynamic_entity_expr_count(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_expr_count_clauses(descriptor, expressions, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware aggregate query for a runtime entity descriptor.
pub fn build_dynamic_entity_expr_aggregate(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses =
        crate::dynamic::entity_expr_aggregate_clauses(descriptor, expressions, aggregates, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware group-by aggregate query for a runtime entity descriptor.
pub fn build_dynamic_entity_expr_group_by_aggregate(
    descriptor: &EntityDescriptor,
    expressions: &[DynamicExpr],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_expr_group_by_aggregate_clauses(
        descriptor,
        expressions,
        group_fields,
        aggregates,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an aggregate query for a runtime entity descriptor.
pub fn build_dynamic_entity_aggregate(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_aggregate_clauses(descriptor, filters, aggregates, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a group-by aggregate query for a runtime entity descriptor.
pub fn build_dynamic_entity_group_by_aggregate(
    descriptor: &EntityDescriptor,
    filters: &[Filter],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_group_by_aggregate_clauses(
        descriptor,
        filters,
        group_fields,
        aggregates,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an IID-based delete query for a runtime entity descriptor.
pub fn build_dynamic_entity_delete_by_iid(
    descriptor: &EntityDescriptor,
    iid: &str,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::entity_delete_by_iid_clauses(descriptor, iid, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Entity update + put query builders
// ------------------------------------------------------------------

/// Build a match + update query for a specific entity instance.
///
/// Matches the entity by IID or @key attributes, then updates all
/// non-key attribute values. Produces TypeQL like:
/// ```text
/// match $e isa person, has name "Alice";
/// update $e has age 31;
/// ```
pub fn build_update<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let key_attrs: Vec<&'static str> = T::owned_attributes()
        .iter()
        .filter(|a| a.is_key())
        .map(|a| a.attr_name)
        .collect();

    let update_statements: Vec<Statement> = entity
        .to_attribute_values()
        .into_iter()
        .filter(|(name, _)| !key_attrs.contains(name))
        .map(|(attr_name, value)| Statement::Has {
            subject_var: var.to_string(),
            attr_name: attr_name.to_string(),
            value: value.to_ast_value(),
        })
        .collect();

    if update_statements.is_empty() {
        return Err(crate::error::OrmError::QueryExecution(
            "No non-key attributes to update".into(),
        ));
    }

    let clauses = vec![
        Clause::Match(vec![entity.to_match_pattern(var)]),
        Clause::Update(update_statements),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an idempotent put query (insert-or-update) for an entity.
///
/// Produces TypeQL using the `put` keyword instead of `insert`.
/// TypeDB will insert if no matching entity exists, or update if one does.
///
/// ```text
/// put $e isa person, has name "Alice";
/// update $e has age 30;
/// fetch { "iid": iid($e) };
/// ```
pub fn build_put<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let key_attrs: HashSet<&'static str> = T::owned_attributes()
        .iter()
        .filter(|a| a.is_key())
        .map(|a| a.attr_name)
        .collect();
    let attr_values = entity.to_attribute_values();

    let mut put_statements = vec![Statement::Isa {
        variable: var.to_string(),
        type_name: T::TYPE_NAME.to_string(),
    }];
    let mut update_statements = Vec::new();

    for (attr_name, value) in attr_values {
        let statement = Statement::Has {
            subject_var: var.to_string(),
            attr_name: attr_name.to_string(),
            value: value.to_ast_value(),
        };
        if key_attrs.is_empty() || key_attrs.contains(attr_name) {
            put_statements.push(statement);
        } else {
            update_statements.push(statement);
        }
    }

    let mut clauses = vec![Clause::Put(put_statements)];
    if !update_statements.is_empty() {
        clauses.push(Clause::Update(update_statements));
    }
    clauses.push(Clause::Fetch(vec![FetchItem::Function {
        key: "iid".to_string(),
        func_name: "iid".to_string(),
        var: var.to_string(),
    }]));

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Relation query builders
// ------------------------------------------------------------------

/// Build an insert + fetch-IID query for a relation.
///
/// Produces TypeQL like:
/// ```text
/// match $rp0 isa person, has name "Alice"; $rp1 isa company, has name "Acme";
/// insert $r isa employment, links (employee: $rp0, employer: $rp1), has position "Engineer";
/// fetch { "iid": iid($r) };
/// ```
pub fn build_relation_insert_with_iid<R: TypeBridgeRelation>(
    relation: &R,
    var: &str,
) -> Result<String> {
    let clauses = relation.to_insert_with_iid_fetch(var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query for relations with optional filters.
pub fn build_relation_fetch<R: TypeBridgeRelation>(
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = R::build_polymorphic_fetch(var, R::TYPE_NAME, filters);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + delete query for a specific relation instance.
pub fn build_relation_delete<R: TypeBridgeRelation>(relation: &R, var: &str) -> Result<String> {
    let match_patterns = relation.to_match_pattern(var);
    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Delete(vec![Statement::DeleteThing(var.to_string())]),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query for relations with optional filters.
pub fn build_relation_count<R: TypeBridgeRelation>(
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let constraints: Vec<Constraint> = filters
        .iter()
        .map(|f| Constraint::Has {
            attr_name: f.attr_name.clone(),
            value: f.value.to_ast_value(),
        })
        .collect();

    let clauses = vec![
        Clause::Match(vec![Pattern::Relation {
            variable: var.to_string(),
            type_name: R::TYPE_NAME.to_string(),
            role_players: vec![],
            constraints,
        }]),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Dynamic relation query builders
// ------------------------------------------------------------------

/// Build an insert + fetch-IID query for a runtime relation descriptor.
pub fn build_dynamic_relation_insert_with_iid(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<String> {
    let clauses =
        crate::dynamic::relation_insert_clauses(descriptor, attributes, role_players, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a put + fetch-IID query for a runtime relation descriptor.
pub fn build_dynamic_relation_put(
    descriptor: &RelationDescriptor,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_put_clauses(descriptor, attributes, role_players, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + update query for a runtime relation descriptor.
pub fn build_dynamic_relation_update(
    descriptor: &RelationDescriptor,
    iid: Option<&str>,
    attributes: &DynamicAttributeMap,
    role_players: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<String> {
    let clauses =
        crate::dynamic::relation_update_clauses(descriptor, iid, attributes, role_players, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query for a runtime relation descriptor.
pub fn build_dynamic_relation_fetch(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_fetch_clauses(descriptor, filters, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query for a runtime relation descriptor with role-player filters.
pub fn build_dynamic_relation_fetch_with_role_filters(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    role_filters: &[DynamicRolePlayerInput],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_fetch_with_role_filters_clauses(
        descriptor,
        filters,
        role_filters,
        var,
    );
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware fetch query for a runtime relation descriptor.
pub fn build_dynamic_relation_expr_fetch(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    sorts: &[DynamicSort],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_expr_fetch_clauses(
        descriptor,
        expressions,
        sorts,
        limit,
        offset,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic IID fetch query for a runtime relation descriptor.
pub fn build_dynamic_relation_fetch_by_iid(
    descriptor: &RelationDescriptor,
    iid: &str,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_fetch_by_iid_clauses(descriptor, iid, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query for a runtime relation descriptor.
pub fn build_dynamic_relation_count(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_count_clauses(descriptor, filters, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware count query for a runtime relation descriptor.
pub fn build_dynamic_relation_expr_count(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_expr_count_clauses(descriptor, expressions, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware aggregate query for a runtime relation descriptor.
pub fn build_dynamic_relation_expr_aggregate(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses =
        crate::dynamic::relation_expr_aggregate_clauses(descriptor, expressions, aggregates, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an expression-aware group-by aggregate query for a runtime relation descriptor.
pub fn build_dynamic_relation_expr_group_by_aggregate(
    descriptor: &RelationDescriptor,
    expressions: &[DynamicExpr],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_expr_group_by_aggregate_clauses(
        descriptor,
        expressions,
        group_fields,
        aggregates,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an aggregate query for a runtime relation descriptor.
pub fn build_dynamic_relation_aggregate(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_aggregate_clauses(descriptor, filters, aggregates, var)?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a group-by aggregate query for a runtime relation descriptor.
pub fn build_dynamic_relation_group_by_aggregate(
    descriptor: &RelationDescriptor,
    filters: &[Filter],
    group_fields: &[String],
    aggregates: &[DynamicAggregate],
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_group_by_aggregate_clauses(
        descriptor,
        filters,
        group_fields,
        aggregates,
        var,
    )?;
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an IID-based delete query for a runtime relation descriptor.
pub fn build_dynamic_relation_delete_by_iid(
    descriptor: &RelationDescriptor,
    iid: &str,
    var: &str,
) -> Result<String> {
    let clauses = crate::dynamic::relation_delete_by_iid_clauses(descriptor, iid, var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Expression-aware query builders
// ------------------------------------------------------------------

/// Build the common polymorphic match patterns for entity queries.
///
/// Returns `(base_patterns, expr_patterns)` where `base_patterns`
/// contains the `isa!` + `sub` patterns, and `expr_patterns` contains
/// the filter expression patterns.
fn build_entity_match_patterns<T: TypeBridgeEntity>(var: &str, filters: &[Expr]) -> Vec<Pattern> {
    let mut patterns = vec![
        Pattern::Entity {
            variable: var.to_string(),
            type_name: "$t".to_string(),
            constraints: vec![],
            is_strict: true,
        },
        Pattern::SubType {
            variable: "$t".to_string(),
            parent_type: T::TYPE_NAME.to_string(),
        },
    ];

    let mut counter = 0;
    for filter in filters {
        patterns.extend(filter.to_patterns(var, &mut counter));
    }

    patterns
}

/// Build a polymorphic fetch query with expression-based filters,
/// sorting, pagination.
pub fn build_expr_fetch<T: TypeBridgeEntity>(
    filters: &[Expr],
    sort_fields: &[(String, SortDir)],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_entity_match_patterns::<T>(var, filters);

    // Add Has bindings for sort attributes
    let mut sort_ast_fields = Vec::new();
    for (i, (attr, dir)) in sort_fields.iter().enumerate() {
        let sort_var = format!("$sort{}", i);
        match_patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: attr.clone(),
            attr_var: sort_var.clone(),
        });
        sort_ast_fields.push(SortField {
            variable: sort_var,
            ascending: *dir == SortDir::Asc,
        });
    }

    let fetch_items = vec![
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
    ];

    let mut clauses = vec![Clause::Match(match_patterns)];

    if !sort_ast_fields.is_empty() {
        clauses.push(Clause::Sort(sort_ast_fields));
    }
    if let Some(n) = limit {
        clauses.push(Clause::Limit(n));
    }
    if let Some(n) = offset {
        clauses.push(Clause::Offset(n));
    }
    clauses.push(Clause::Fetch(fetch_items));

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query with expression-based filters.
pub fn build_expr_count<T: TypeBridgeEntity>(filters: &[Expr], var: &str) -> Result<String> {
    let match_patterns = build_entity_match_patterns::<T>(var, filters);

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an aggregation query with expression-based filters.
pub fn build_expr_aggregate<T: TypeBridgeEntity>(
    filters: &[Expr],
    aggs: &[Agg],
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_entity_match_patterns::<T>(var, filters);

    let mut counter = 100; // offset to avoid collisions with filter variables
    let mut assignments = Vec::new();
    for agg in aggs {
        let (assign, has_pattern) = agg.to_reduce_assignment(var, &mut counter);
        if let Some(p) = has_pattern {
            match_patterns.push(p);
        }
        assignments.push(assign);
    }

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Expression-aware relation query builders
// ------------------------------------------------------------------

/// Collect role player bindings needed from filters.
fn collect_role_bindings(filters: &[Expr]) -> Vec<RolePlayer> {
    let mut roles = Vec::new();
    let mut seen = HashSet::new();
    for filter in filters {
        filter.collect_roles(&mut roles, &mut seen);
    }
    roles
        .into_iter()
        .map(|role| RolePlayer {
            role: role.clone(),
            player_var: format!("${}", role),
        })
        .collect()
}

/// Build the common polymorphic match patterns for relation queries.
fn build_relation_match_patterns<R: TypeBridgeRelation>(
    var: &str,
    filters: &[Expr],
) -> Vec<Pattern> {
    let role_players = collect_role_bindings(filters);

    let mut patterns = vec![Pattern::Relation {
        variable: var.to_string(),
        type_name: R::TYPE_NAME.to_string(),
        role_players,
        constraints: vec![],
    }];

    let mut counter = 0;
    for filter in filters {
        patterns.extend(filter.to_patterns(var, &mut counter));
    }

    patterns
}

/// Build a polymorphic fetch query for relations with expression-based
/// filters, sorting, and pagination.
pub fn build_relation_expr_fetch<R: TypeBridgeRelation>(
    filters: &[Expr],
    sort_fields: &[(String, SortDir)],
    limit: Option<u64>,
    offset: Option<u64>,
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_relation_match_patterns::<R>(var, filters);

    let mut sort_ast_fields = Vec::new();
    for (i, (attr, dir)) in sort_fields.iter().enumerate() {
        let sort_var = format!("$sort{}", i);
        match_patterns.push(Pattern::Has {
            thing_var: var.to_string(),
            attr_type: attr.clone(),
            attr_var: sort_var.clone(),
        });
        sort_ast_fields.push(SortField {
            variable: sort_var,
            ascending: *dir == SortDir::Asc,
        });
    }

    let fetch_items = vec![
        FetchItem::Function {
            key: "_iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        },
        FetchItem::NestedWildcard {
            key: "attributes".to_string(),
            var: var.to_string(),
        },
    ];

    let mut clauses = vec![Clause::Match(match_patterns)];

    if !sort_ast_fields.is_empty() {
        clauses.push(Clause::Sort(sort_ast_fields));
    }
    if let Some(n) = limit {
        clauses.push(Clause::Limit(n));
    }
    if let Some(n) = offset {
        clauses.push(Clause::Offset(n));
    }
    clauses.push(Clause::Fetch(fetch_items));

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query for relations with expression-based filters.
pub fn build_relation_expr_count<R: TypeBridgeRelation>(
    filters: &[Expr],
    var: &str,
) -> Result<String> {
    let match_patterns = build_relation_match_patterns::<R>(var, filters);

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an aggregation query for relations with expression-based filters.
pub fn build_relation_expr_aggregate<R: TypeBridgeRelation>(
    filters: &[Expr],
    aggs: &[Agg],
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_relation_match_patterns::<R>(var, filters);

    let mut counter = 100;
    let mut assignments = Vec::new();
    for agg in aggs {
        let (assign, has_pattern) = agg.to_reduce_assignment(var, &mut counter);
        if let Some(p) = has_pattern {
            match_patterns.push(p);
        }
        assignments.push(assign);
    }

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: None,
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Group-by aggregate query builders
// ------------------------------------------------------------------

/// Build a group-by aggregation query for entities.
pub fn build_expr_group_by_aggregate<T: TypeBridgeEntity>(
    filters: &[Expr],
    group_field: &str,
    aggs: &[Agg],
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_entity_match_patterns::<T>(var, filters);

    let group_var = "$group0".to_string();
    match_patterns.push(Pattern::Has {
        thing_var: var.to_string(),
        attr_type: group_field.to_string(),
        attr_var: group_var.clone(),
    });

    let mut counter = 100;
    let mut assignments = Vec::new();
    for agg in aggs {
        let (assign, has_pattern) = agg.to_reduce_assignment(var, &mut counter);
        if let Some(p) = has_pattern {
            match_patterns.push(p);
        }
        assignments.push(assign);
    }

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: Some(group_var),
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a group-by aggregation query for relations.
pub fn build_relation_group_by_aggregate<R: TypeBridgeRelation>(
    filters: &[Expr],
    group_field: &str,
    aggs: &[Agg],
    var: &str,
) -> Result<String> {
    let mut match_patterns = build_relation_match_patterns::<R>(var, filters);

    let group_var = "$group0".to_string();
    match_patterns.push(Pattern::Has {
        thing_var: var.to_string(),
        attr_type: group_field.to_string(),
        attr_var: group_var.clone(),
    });

    let mut counter = 100;
    let mut assignments = Vec::new();
    for agg in aggs {
        let (assign, has_pattern) = agg.to_reduce_assignment(var, &mut counter);
        if let Some(p) = has_pattern {
            match_patterns.push(p);
        }
        assignments.push(assign);
    }

    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Reduce {
            assignments,
            group_by: Some(group_var),
        },
    ];

    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::entity::{Annotation, OwnedAttributeInfo};
    use crate::value::AttributeValue;

    // Minimal test entity for query builder tests.
    struct TestPerson {
        iid: Option<String>,
        name: String,
        age: i64,
    }

    impl TypeBridgeEntity for TestPerson {
        const TYPE_NAME: &'static str = "person";

        fn owned_attributes() -> &'static [OwnedAttributeInfo] {
            &[
                OwnedAttributeInfo {
                    attr_name: "name",
                    value_type: ValueType::String,
                    annotations: &[Annotation::Key],
                },
                OwnedAttributeInfo {
                    attr_name: "age",
                    value_type: ValueType::Long,
                    annotations: &[],
                },
            ]
        }

        fn iid(&self) -> Option<&str> {
            self.iid.as_deref()
        }

        fn set_iid(&mut self, iid: String) {
            self.iid = Some(iid);
        }

        fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
            vec![
                ("name", AttributeValue::String(self.name.clone())),
                ("age", AttributeValue::Long(self.age)),
            ]
        }

        fn from_document(
            doc: &serde_json::Map<String, serde_json::Value>,
        ) -> crate::error::Result<Self> {
            let name = doc
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::OrmError::Hydration {
                    type_name: "person".into(),
                    message: "missing name".into(),
                })?
                .to_string();
            let age = doc.get("age").and_then(|v| v.as_i64()).ok_or_else(|| {
                crate::error::OrmError::Hydration {
                    type_name: "person".into(),
                    message: "missing age".into(),
                }
            })?;
            Ok(Self {
                iid: None,
                name,
                age,
            })
        }
    }

    #[test]
    fn insert_query_contains_isa_and_has() {
        let person = TestPerson {
            iid: None,
            name: "Alice".into(),
            age: 30,
        };
        let q = build_insert_with_iid::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("insert"));
        assert!(q.contains("isa person"));
        assert!(q.contains("has name"));
        assert!(q.contains("has age"));
        assert!(q.contains("fetch"));
        assert!(q.contains("iid"));
    }

    #[test]
    fn fetch_query_with_filters() {
        let filters = [Filter::string_eq("name", "Alice")];
        let q = build_fetch::<TestPerson>(&filters, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("isa!"));
        assert!(q.contains("has name"));
        assert!(q.contains("fetch"));
    }

    #[test]
    fn fetch_query_without_filters() {
        let q = build_fetch::<TestPerson>(&[], "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("sub person"));
        assert!(q.contains("fetch"));
    }

    #[test]
    fn delete_query_uses_match_and_delete() {
        let person = TestPerson {
            iid: Some("0xabc".into()),
            name: "Alice".into(),
            age: 30,
        };
        let q = build_delete::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("delete"));
        assert!(q.contains("isa person"));
    }

    #[test]
    fn count_query_uses_reduce() {
        let q = build_count::<TestPerson>(&[], "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("isa person"));
        assert!(q.contains("reduce"));
        assert!(q.contains("count"));
    }

    #[test]
    fn count_query_with_filters() {
        let filters = [Filter::long_eq("age", 30)];
        let q = build_count::<TestPerson>(&filters, "$e").unwrap();
        assert!(q.contains("has age"));
        assert!(q.contains("reduce"));
        assert!(q.contains("count"));
    }

    #[test]
    fn update_query_matches_and_updates_non_key() {
        let person = TestPerson {
            iid: Some("0xabc".into()),
            name: "Alice".into(),
            age: 31,
        };
        let q = build_update::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("update"));
        assert!(q.contains("has age"));
        // Key attributes should NOT appear in the update clause
        assert!(!q.contains("update\n$e has name"));
    }

    #[test]
    fn update_query_without_non_key_attrs_fails() {
        // TestPerson has only name (key) + age (non-key)
        // But a struct with only key attrs would fail
        struct KeyOnly {
            iid: Option<String>,
            name: String,
        }
        impl TypeBridgeEntity for KeyOnly {
            const TYPE_NAME: &'static str = "keyonly";
            fn owned_attributes() -> &'static [OwnedAttributeInfo] {
                &[OwnedAttributeInfo {
                    attr_name: "name",
                    value_type: ValueType::String,
                    annotations: &[Annotation::Key],
                }]
            }
            fn iid(&self) -> Option<&str> {
                self.iid.as_deref()
            }
            fn set_iid(&mut self, iid: String) {
                self.iid = Some(iid);
            }
            fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
                vec![("name", AttributeValue::String(self.name.clone()))]
            }
            fn from_document(
                _doc: &serde_json::Map<String, serde_json::Value>,
            ) -> crate::error::Result<Self> {
                unimplemented!()
            }
        }

        let entity = KeyOnly {
            iid: Some("0x1".into()),
            name: "Test".into(),
        };
        let result = build_update::<KeyOnly>(&entity, "$e");
        assert!(result.is_err());
    }

    #[test]
    fn put_query_uses_put_keyword() {
        let person = TestPerson {
            iid: None,
            name: "Alice".into(),
            age: 30,
        };
        let q = build_put::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("put"));
        assert!(!q.starts_with("insert"));
        assert!(q.contains("isa person"));
        assert!(q.contains("has name"));
        assert!(q.contains("fetch"));
    }

    // ── Expression-aware query builder tests ────────────────────────

    #[test]
    fn expr_fetch_with_gt_filter() {
        let filters = [Expr::gt("age", AttributeValue::Long(30))];
        let q = build_expr_fetch::<TestPerson>(&filters, &[], None, None, "$e").unwrap();
        assert!(q.contains("$e has age $attr0"));
        assert!(q.contains("$attr0 > 30"));
        assert!(q.contains("sub person"));
        assert!(q.contains("fetch"));
    }

    #[test]
    fn expr_fetch_with_sort() {
        let sort = [("age".to_string(), SortDir::Asc)];
        let q = build_expr_fetch::<TestPerson>(&[], &sort, None, None, "$e").unwrap();
        assert!(q.contains("$e has age $sort0"));
        assert!(q.contains("sort $sort0 asc;"));
    }

    #[test]
    fn expr_fetch_with_limit_offset() {
        let q = build_expr_fetch::<TestPerson>(&[], &[], Some(10), Some(5), "$e").unwrap();
        assert!(q.contains("limit 10;"));
        assert!(q.contains("offset 5;"));
    }

    #[test]
    fn expr_fetch_full_chain() {
        let filters = [Expr::gte("age", AttributeValue::Long(18))];
        let sort = [
            ("name".to_string(), SortDir::Asc),
            ("age".to_string(), SortDir::Desc),
        ];
        let q = build_expr_fetch::<TestPerson>(&filters, &sort, Some(10), Some(20), "$e").unwrap();
        assert!(q.contains("$e has age $attr0"));
        assert!(q.contains("$attr0 >= 18"));
        assert!(q.contains("$e has name $sort0"));
        assert!(q.contains("$e has age $sort1"));
        assert!(q.contains("sort $sort0 asc, $sort1 desc;"));
        assert!(q.contains("limit 10;"));
        assert!(q.contains("offset 20;"));
    }

    #[test]
    fn expr_count_with_filter() {
        let filters = [Expr::eq("name", AttributeValue::String("Alice".into()))];
        let q = build_expr_count::<TestPerson>(&filters, "$e").unwrap();
        assert!(q.contains("$e has name $attr0"));
        assert!(q.contains("$attr0 == \"Alice\""));
        assert!(q.contains("reduce"));
        assert!(q.contains("count"));
    }

    #[test]
    fn expr_aggregate_sum() {
        let aggs = [Agg::Sum("age".into())];
        let q = build_expr_aggregate::<TestPerson>(&[], &aggs, "$e").unwrap();
        assert!(q.contains("$e has age $agg100"));
        assert!(q.contains("$sum = sum($agg100)"));
        assert!(q.contains("reduce"));
    }

    #[test]
    fn expr_aggregate_count_and_sum() {
        let aggs = [Agg::Count, Agg::Sum("age".into())];
        let q = build_expr_aggregate::<TestPerson>(&[], &aggs, "$e").unwrap();
        assert!(q.contains("$count = count($e)"));
        assert!(q.contains("$sum = sum($agg100)"));
    }
}
