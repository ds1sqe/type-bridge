#![allow(deprecated)] // Legacy lowering remains until 11.30 / 3.0.
//! Deprecated front-end lowering; official nodes retain all TypeQL meaning.
use super::*;
use crate::ast as old;
use typeql::{
    expression::Expression,
    pattern,
    query::{
        QueryStructure,
        stage::{self, fetch},
    },
    statement,
};

fn invalid<T>() -> Result<T, TqlError> {
    Err(TqlError::InvalidLegacyShape)
}

// Released Python callers also use Value::Variable for expression strings.
// Admit exactly one expression through the official parser, never another stage.
fn legacy_expression(source: &str) -> Result<Expr, TqlError> {
    let query = parse(&format!("match let $fragment = {source};"))?;
    if query.inner.has_explicit_end {
        return invalid();
    }
    let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
        return invalid();
    };
    if !pipeline.preambles.is_empty() || pipeline.stages.len() != 1 {
        return invalid();
    }
    let stage::Stage::Match(mut stage) = pipeline.stages.remove(0) else {
        return invalid();
    };
    if stage.patterns.len() != 1 {
        return invalid();
    }
    let pattern::Pattern::Statement(statement::Statement::Assignment(value)) =
        stage.patterns.remove(0)
    else {
        return invalid();
    };
    Ok(Expr { inner: value.rhs })
}

pub fn lower_legacy_value(value: &old::Value) -> Result<Expr, TqlError> {
    match value {
        old::Value::Literal(value) => {
            if ![
                "string",
                "long",
                "integer",
                "double",
                "decimal",
                "boolean",
                "date",
                "datetime",
                "datetime-tz",
                "datetime_tz",
                "duration",
            ]
            .contains(&value.value_type.as_str())
            {
                return Err(TqlError::InvalidLiteral);
            }
            let source = match &value.value {
                serde_json::Value::String(text) if value.value_type == "string" => {
                    serde_json::to_string(text).map_err(|_| TqlError::InvalidLiteral)?
                }
                serde_json::Value::String(text) if value.value_type == "decimal" => {
                    format!("{}dec", text.strip_suffix("dec").unwrap_or(text))
                }
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(number) if value.value_type == "decimal" => {
                    format!("{number}dec")
                }
                serde_json::Value::Number(number) if value.value_type == "double" => {
                    let mut source = number.to_string();
                    if !source.contains(['.', 'e', 'E']) {
                        source.push_str(".0");
                    }
                    source
                }
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(value) => value.to_string(),
                _ => return Err(TqlError::InvalidLiteral),
            };
            let inner = typeql::parse_value(&source).map_err(|_| TqlError::InvalidLiteral)?;
            use typeql::value::ValueLiteral as Literal;
            let matches_domain = matches!(
                (value.value_type.as_str(), &inner),
                ("string", Literal::String(_))
                    | ("long" | "integer", Literal::Integer(_))
                    | ("double", Literal::Double(_))
                    | ("decimal", Literal::Decimal(_))
                    | ("boolean", Literal::Boolean(_))
                    | ("date", Literal::Date(_))
                    | ("datetime", Literal::DateTime(_))
                    | ("datetime-tz" | "datetime_tz", Literal::DateTimeTz(_))
                    | ("duration", Literal::Duration(_))
            );
            if !matches_domain {
                return Err(TqlError::InvalidLiteral);
            }
            Ok(Expr {
                inner: Expression::Value(typeql::value::Literal { span: None, inner }),
            })
        }
        old::Value::Variable(name) => {
            if name.starts_with('$')
                && let Ok(value) = var(name)
            {
                return Ok(value);
            }
            legacy_expression(name)
        }
        old::Value::FunctionCall(value) => function(
            &value.function,
            value
                .args
                .iter()
                .map(lower_legacy_value)
                .collect::<Result<_, _>>()?,
        ),
        old::Value::Arithmetic(value) => arithmetic(
            lower_legacy_value(&value.left)?,
            &value.operator,
            lower_legacy_value(&value.right)?,
        ),
    }
}

pub fn lower_legacy_constraint(value: &old::Constraint) -> Result<Constraint, TqlError> {
    match value {
        old::Constraint::Isa { type_name, strict } => isa(type_name, *strict),
        old::Constraint::Iid(value) => iid(value),
        old::Constraint::Has { attr_name, value } => has(attr_name, lower_legacy_value(value)?),
    }
}

fn players(values: &[old::RolePlayer]) -> Vec<(&str, &str)> {
    values
        .iter()
        .map(|value| (value.role.as_str(), value.player_var.as_str()))
        .collect()
}

pub fn lower_legacy_pattern(value: &old::Pattern) -> Result<Pattern, TqlError> {
    match value {
        old::Pattern::Entity {
            variable,
            type_name,
            constraints,
            is_strict,
        } => {
            let mut result = vec![isa(type_name, *is_strict)?];
            result.extend(
                constraints
                    .iter()
                    .map(lower_legacy_constraint)
                    .collect::<Result<Vec<_>, _>>()?,
            );
            thing(variable, result)
        }
        old::Pattern::Relation {
            variable,
            type_name,
            role_players,
            constraints,
        } => {
            let mut result = vec![if role_players.is_empty() {
                isa(type_name, false)?
            } else {
                isa_relation(type_name, players(role_players))?
            }];
            result.extend(
                constraints
                    .iter()
                    .map(lower_legacy_constraint)
                    .collect::<Result<Vec<_>, _>>()?,
            );
            thing(variable, result)
        }
        old::Pattern::Attribute {
            variable,
            type_name,
            value,
        } => thing(
            variable,
            vec![match value {
                Some(value) => isa_value(type_name, false, lower_legacy_value(value)?)?,
                None => isa(type_name, false)?,
            }],
        ),
        old::Pattern::SubType {
            variable,
            parent_type,
        } => subtype(variable, parent_type),
        old::Pattern::Has {
            thing_var,
            attr_type,
            attr_var,
        } => thing(thing_var, vec![has(attr_type, var(attr_var)?)?]),
        old::Pattern::ValueComparison {
            var: name,
            operator,
            value,
        } => compare(var(name)?, operator, lower_legacy_value(value)?),
        old::Pattern::Iid {
            variable,
            iid: value,
        } => thing(variable, vec![iid(value)?]),
        old::Pattern::Not(values) => Ok(not(lower_legacy_patterns(values)?)),
        old::Pattern::Try(values) => Ok(optional(lower_legacy_patterns(values)?)),
        old::Pattern::Or(branches) => Ok(or(branches
            .iter()
            .map(|branch| lower_legacy_patterns(branch))
            .collect::<Result<_, _>>()?)),
        old::Pattern::Raw(source) => {
            let mut values = parse_patterns(source)?;
            if values.len() == 1 {
                Ok(values.remove(0))
            } else {
                Ok(conjunction(values))
            }
        }
    }
}

pub fn lower_legacy_statement(value: &old::Statement) -> Result<Pattern, TqlError> {
    match value {
        old::Statement::Isa {
            variable,
            type_name,
        } => thing(variable, vec![isa(type_name, false)?]),
        old::Statement::Has {
            subject_var,
            attr_name,
            value,
        } => thing(
            subject_var,
            vec![has(attr_name, lower_legacy_value(value)?)?],
        ),
        old::Statement::Relation {
            variable,
            type_name,
            role_players,
            include_variable,
            attributes,
        } => {
            let mut constraints = vec![isa(type_name, false)?];
            for value in attributes {
                let old::Statement::Has {
                    attr_name, value, ..
                } = value
                else {
                    return invalid();
                };
                constraints.push(has(attr_name, lower_legacy_value(value)?)?);
            }
            if *include_variable {
                constraints.insert(1, links(players(role_players))?);
                thing(variable, constraints)
            } else {
                anonymous_relation(players(role_players), constraints)
            }
        }
        old::Statement::Raw(source) => {
            let mut values = parse_patterns(source)?;
            if values.len() == 1 {
                Ok(values.remove(0))
            } else {
                Ok(conjunction(values))
            }
        }
        old::Statement::DeleteThing(_) => invalid(),
    }
}

// Explicit conversion of Raw to AST nodes validates the enclosing match stage.
// Compatibility compilation preserves caller-owned Raw verbatim below.
pub fn parse_patterns(source: &str) -> Result<Vec<Pattern>, TqlError> {
    let source = source.trim().trim_end_matches(';');
    let query = parse(&format!("match {source};"))?;
    if query.inner.has_explicit_end {
        return invalid();
    }
    let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
        return invalid();
    };
    if !pipeline.preambles.is_empty() || pipeline.stages.len() != 1 {
        return invalid();
    }
    let stage::Stage::Match(value) = pipeline.stages.remove(0) else {
        return invalid();
    };
    Ok(value
        .patterns
        .into_iter()
        .map(|inner| Pattern { inner })
        .collect())
}

fn lower_delete(value: &old::Statement) -> Result<Vec<Deletable>, TqlError> {
    match value {
        old::Statement::DeleteThing(name) => Ok(vec![delete_thing(name)?]),
        old::Statement::Raw(source) => {
            let query = parse(&format!("delete {};", source.trim().trim_end_matches(';')))?;
            if query.inner.has_explicit_end {
                return invalid();
            }
            let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
                return invalid();
            };
            if !pipeline.preambles.is_empty() || pipeline.stages.len() != 1 {
                return invalid();
            }
            let stage::Stage::Delete(value) = pipeline.stages.remove(0) else {
                return invalid();
            };
            Ok(value
                .deletables
                .into_iter()
                .map(|inner| Deletable { inner })
                .collect())
        }
        _ => invalid(),
    }
}

fn legacy_reducer(value: &old::Value) -> Result<Reducer, TqlError> {
    match value {
        old::Value::Variable(name) if name == "count" => reducer("count", None),
        old::Value::Variable(source) => {
            let query = parse(&format!("reduce $fragment = {source};"))?;
            if query.inner.has_explicit_end {
                return invalid();
            }
            let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
                return invalid();
            };
            if !pipeline.preambles.is_empty() || pipeline.stages.len() != 1 {
                return invalid();
            }
            let stage::Stage::Operator(stage::modifier::Operator::Reduce(mut stage)) =
                pipeline.stages.remove(0)
            else {
                return invalid();
            };
            if stage.groupby.is_some() || stage.reduce_assignments.len() != 1 {
                return invalid();
            }
            Ok(Reducer {
                inner: stage.reduce_assignments.remove(0).reducer,
            })
        }
        old::Value::FunctionCall(value) => match value.args.as_slice() {
            [] => reducer(&value.function, None),
            [old::Value::Variable(name)] => reducer(&value.function, Some(name)),
            _ => invalid(),
        },
        _ => invalid(),
    }
}

pub fn lower_legacy_fetch_item(value: &old::FetchItem) -> Result<(String, FetchValue), TqlError> {
    let (key, value) = match value {
        old::FetchItem::Attribute {
            key,
            var,
            attr_name,
        } => (key.clone(), fetch_attribute(var, attr_name, false)?),
        old::FetchItem::AttributeList {
            key,
            var,
            attr_name,
        } => (key.clone(), fetch_attribute(var, attr_name, true)?),
        old::FetchItem::Variable { key, var: name } => (key.clone(), fetch_expr(var(name)?)),
        old::FetchItem::Wildcard { key, var } | old::FetchItem::NestedWildcard { key, var } => {
            (key.clone(), fetch_attributes(var)?)
        }
        old::FetchItem::Function {
            key,
            func_name,
            var: name,
        } => (
            key.clone(),
            fetch_expr(function(func_name, vec![var(name)?])?),
        ),
        old::FetchItem::Raw(source) => {
            let query = parse(&format!("match let $fragment = 1; fetch {{ {source} }};"))?;
            if query.inner.has_explicit_end {
                return invalid();
            }
            let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
                return invalid();
            };
            if !pipeline.preambles.is_empty() || pipeline.stages.len() != 2 {
                return invalid();
            }
            let stage::Stage::Fetch(value) = pipeline.stages.remove(1) else {
                return invalid();
            };
            let fetch::FetchObjectBody::Entries(mut values) = value.object.body else {
                return invalid();
            };
            if values.len() != 1 {
                return invalid();
            }
            let value = values.remove(0);
            (
                value.key.unescape().map_err(|_| TqlError::InvalidLiteral)?,
                FetchValue { inner: value.value },
            )
        }
    };
    Ok((key, value))
}

fn lower_legacy_patterns(values: &[old::Pattern]) -> Result<Vec<Pattern>, TqlError> {
    let mut patterns = Vec::new();
    for value in values {
        if let old::Pattern::Raw(source) = value {
            patterns.extend(parse_patterns(source)?);
        } else {
            patterns.push(lower_legacy_pattern(value)?);
        }
    }
    Ok(patterns)
}

fn lower_legacy_statements(values: &[old::Statement]) -> Result<Vec<Pattern>, TqlError> {
    let mut patterns = Vec::new();
    for value in values {
        if let old::Statement::Raw(source) = value {
            patterns.extend(parse_patterns(source)?);
        } else {
            patterns.push(lower_legacy_statement(value)?);
        }
    }
    Ok(patterns)
}

pub fn lower_legacy_clause(value: &old::Clause) -> Result<Stage, TqlError> {
    Ok(match value {
        old::Clause::Match(values) => match_stage(lower_legacy_patterns(values)?),
        old::Clause::MatchLet(values) => match_stage(
            values
                .iter()
                .map(|value| {
                    let_value(
                        &value
                            .variables
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        lower_legacy_value(&value.expression)?,
                        value.is_stream,
                    )
                })
                .collect::<Result<_, _>>()?,
        ),
        old::Clause::Insert(values) => insert(lower_legacy_statements(values)?),
        old::Clause::Put(values) => put(lower_legacy_statements(values)?),
        old::Clause::Update(values) => {
            let patterns = values
                .iter()
                .map(|value| match value {
                    old::Statement::Has {
                        subject_var,
                        attr_name,
                        value,
                    } => thing(
                        subject_var,
                        vec![has_comparison(attr_name, "==", lower_legacy_value(value)?)?],
                    ),
                    old::Statement::Raw(_) => lower_legacy_statement(value),
                    _ => invalid(),
                })
                .collect::<Result<_, _>>()?;
            update(patterns)
        }
        old::Clause::Delete(values) => delete(
            values
                .iter()
                .map(lower_delete)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect(),
        ),
        old::Clause::Fetch(values) => {
            let entries = values
                .iter()
                .map(lower_legacy_fetch_item)
                .collect::<Result<Vec<_>, _>>()?;
            fetch(
                entries
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.clone()))
                    .collect(),
            )?
        }
        old::Clause::Reduce {
            assignments,
            group_by,
        } => {
            let assignments = assignments
                .iter()
                .map(|value| Ok((value.variable.as_str(), legacy_reducer(&value.expression)?)))
                .collect::<Result<_, TqlError>>()?;
            let group = group_by
                .as_deref()
                .map(|value| value.split(',').map(str::trim).collect::<Vec<_>>())
                .unwrap_or_default();
            reduce_stage(assignments, &group)?
        }
        old::Clause::Sort(values) => sort(
            &values
                .iter()
                .map(|value| (value.variable.as_str(), !value.ascending))
                .collect::<Vec<_>>(),
        )?,
        old::Clause::Limit(value) => limit(*value),
        old::Clause::Offset(value) => offset(*value),
    })
}

pub fn compile_legacy(clauses: &[old::Clause]) -> Result<String, TqlError> {
    if clauses.iter().any(|value| match value {
        old::Clause::Match(v) => v.iter().any(raw_pattern),
        old::Clause::Insert(v)
        | old::Clause::Put(v)
        | old::Clause::Update(v)
        | old::Clause::Delete(v) => v.iter().any(|v| matches!(v, old::Statement::Raw(_))),
        old::Clause::Fetch(v) => v.iter().any(|v| matches!(v, old::FetchItem::Raw(_))),
        _ => false,
    }) {
        return clauses
            .iter()
            .map(render_legacy_clause)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(" "));
    }
    render(&pipeline(
        clauses
            .iter()
            .map(lower_legacy_clause)
            .collect::<Result<_, _>>()?,
    ))
}

fn render_pattern_fragment(mut value: Pattern) -> Result<String, TqlError> {
    if let Some(construct) = super::render_guard::prepare_pattern(&mut value.inner) {
        return Err(TqlError::Unsupported { construct });
    }
    Ok(value.inner.to_string())
}

pub fn compile_legacy_node(node: &old::QueryNode) -> Result<String, TqlError> {
    match node {
        old::QueryNode::Pattern(value) if raw_pattern(value) => render_raw_pattern(value),
        old::QueryNode::Statement(old::Statement::Raw(value)) => Ok(value.clone()),
        old::QueryNode::Clause(value) => compile_legacy(std::slice::from_ref(value)),
        old::QueryNode::Pattern(value) => render_pattern_fragment(lower_legacy_pattern(value)?),
        old::QueryNode::Statement(old::Statement::DeleteThing(value)) => {
            Ok(delete_thing(value)?.inner.to_string())
        }
        old::QueryNode::Statement(value) => render_pattern_fragment(lower_legacy_statement(value)?),
        old::QueryNode::Constraint(value) => {
            let mut value = thing("fragment", vec![lower_legacy_constraint(value)?])?;
            super::render_guard::prepare_pattern(&mut value.inner);
            let pattern::Pattern::Statement(statement::Statement::Thing(value)) = value.inner
            else {
                unreachable!()
            };
            Ok(value.constraints[0].to_string())
        }
        old::QueryNode::Value(value) => {
            let mut value = lower_legacy_value(value)?;
            super::render_guard::prepare_expression(&mut value.inner);
            Ok(value.inner.to_string())
        }
    }
}

// Deprecated caller-owned Raw escape hatches are the explicit 11.30 / 3.0
// text-building exception. Structured nodes still use official AST Display.
fn raw_pattern(value: &old::Pattern) -> bool {
    match value {
        old::Pattern::Raw(_) => true,
        old::Pattern::Not(v) | old::Pattern::Try(v) => v.iter().any(raw_pattern),
        old::Pattern::Or(v) => v.iter().flatten().any(raw_pattern),
        _ => false,
    }
}
fn render_raw_pattern(value: &old::Pattern) -> Result<String, TqlError> {
    if !raw_pattern(value) {
        return render_pattern_fragment(lower_legacy_pattern(value)?);
    }
    Ok(match value {
        old::Pattern::Raw(value) => value.clone(),
        old::Pattern::Not(values) | old::Pattern::Try(values) => {
            let keyword = if matches!(value, old::Pattern::Not(_)) {
                "not"
            } else {
                "try"
            };
            format!(
                "{keyword} {{ {}; }}",
                values
                    .iter()
                    .map(render_raw_pattern)
                    .collect::<Result<Vec<_>, _>>()?
                    .join("; ")
            )
        }
        old::Pattern::Or(branches) => branches
            .iter()
            .map(|values| {
                Ok(format!(
                    "{{ {}; }}",
                    values
                        .iter()
                        .map(render_raw_pattern)
                        .collect::<Result<Vec<_>, TqlError>>()?
                        .join("; ")
                ))
            })
            .collect::<Result<Vec<_>, TqlError>>()?
            .join(" or "),
        _ => unreachable!(),
    })
}
fn terminated_fragments(values: Vec<String>) -> String {
    values
        .into_iter()
        .map(|value| {
            if value.trim_end().ends_with(';') {
                value
            } else {
                format!("{value};")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn render_legacy_clause(value: &old::Clause) -> Result<String, TqlError> {
    match value {
        old::Clause::Match(values) if values.iter().any(raw_pattern) => Ok(format!(
            "match {}",
            terminated_fragments(
                values
                    .iter()
                    .map(render_raw_pattern)
                    .collect::<Result<Vec<_>, _>>()?
            )
        )),
        old::Clause::Insert(values)
        | old::Clause::Put(values)
        | old::Clause::Update(values)
        | old::Clause::Delete(values)
            if values.iter().any(|v| matches!(v, old::Statement::Raw(_))) =>
        {
            let keyword = match value {
                old::Clause::Insert(_) => "insert",
                old::Clause::Put(_) => "put",
                old::Clause::Update(_) => "update",
                _ => "delete",
            };
            let fragments = values
                .iter()
                .map(|v| match v {
                    old::Statement::Raw(v) => Ok(v.clone()),
                    _ => compile_legacy_node(&old::QueryNode::Statement(v.clone())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{keyword} {}", terminated_fragments(fragments)))
        }
        old::Clause::Fetch(values)
            if values.iter().any(|v| matches!(v, old::FetchItem::Raw(_))) =>
        {
            let entries = values
                .iter()
                .map(|v| match v {
                    old::FetchItem::Raw(v) => Ok(v.clone()),
                    _ => {
                        let (key, value) = lower_legacy_fetch_item(v)?;
                        Ok(format!(
                            "{}: {}",
                            serde_json::to_string(&key).map_err(|_| TqlError::InvalidLiteral)?,
                            value.inner
                        ))
                    }
                })
                .collect::<Result<Vec<_>, TqlError>>()?;
            Ok(format!("fetch {{ {} }};", entries.join(", ")))
        }
        _ => render(&pipeline(vec![lower_legacy_clause(value)?])),
    }
}

fn released_statement(value: &mut old::Statement) {
    if let old::Statement::Relation { attributes, .. } = value {
        // Released String wrappers dropped non-Has attributes. Checked APIs reject them.
        attributes.retain(|v| matches!(v, old::Statement::Has { .. }));
    }
}
fn released_clause(value: &mut old::Clause) {
    match value {
        old::Clause::Insert(v)
        | old::Clause::Put(v)
        | old::Clause::Update(v)
        | old::Clause::Delete(v) => v.iter_mut().for_each(released_statement),
        _ => {}
    }
}
pub fn compile_legacy_released(values: &[old::Clause]) -> String {
    let mut values = values.to_vec();
    values.iter_mut().for_each(released_clause);
    compile_legacy(&values).unwrap_or_default()
}
pub fn compile_legacy_node_released(value: &old::QueryNode) -> String {
    let mut value = value.clone();
    match &mut value {
        old::QueryNode::Statement(v) => released_statement(v),
        old::QueryNode::Clause(v) => released_clause(v),
        _ => {}
    }
    compile_legacy_node(&value).unwrap_or_default()
}

/// Render the statement body of a legacy clause without slicing rendered text.
pub fn legacy_clause_body(value: &old::Clause) -> Result<String, TqlError> {
    if let old::Clause::Match(values) = value
        && values.iter().any(raw_pattern)
    {
        return values
            .iter()
            .map(render_raw_pattern)
            .collect::<Result<Vec<_>, _>>()
            .map(|v| v.join("; "));
    }
    let stage = lower_legacy_clause(value)?;
    let values = match stage.inner {
        stage::Stage::Match(v) => v.patterns,
        stage::Stage::Insert(v) => v.patterns,
        stage::Stage::Put(v) => v.patterns,
        stage::Stage::Update(v) => v.patterns,
        _ => return invalid(),
    };
    values
        .into_iter()
        .map(|inner| render_pattern_fragment(Pattern { inner }))
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join("; "))
}

/// Compile retained batch helpers through typed clauses and the same renderer.
pub fn compile_legacy_batch(nodes: &[old::QueryNode], operation: &str) -> Result<String, TqlError> {
    if operation.is_empty() {
        return nodes
            .iter()
            .map(compile_legacy_node)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join("\n"));
    }
    let clause = match operation {
        "match" => old::Clause::Match(
            nodes
                .iter()
                .map(|node| match node {
                    old::QueryNode::Pattern(value) => Ok(value.clone()),
                    _ => invalid(),
                })
                .collect::<Result<_, _>>()?,
        ),
        "insert" | "delete" | "update" | "put" => {
            let statements = nodes
                .iter()
                .map(|node| match node {
                    old::QueryNode::Statement(value) => Ok(value.clone()),
                    _ => invalid(),
                })
                .collect::<Result<_, _>>()?;
            match operation {
                "insert" => old::Clause::Insert(statements),
                "delete" => old::Clause::Delete(statements),
                "update" => old::Clause::Update(statements),
                _ => old::Clause::Put(statements),
            }
        }
        _ => return invalid(),
    };
    compile_legacy(&[clause])
}

/// Render typed legacy fetch entries without building JSON fragments in a host.
pub fn render_legacy_fetch_items(values: &[old::FetchItem]) -> Result<Vec<String>, TqlError> {
    let query = pipeline(vec![lower_legacy_clause(&old::Clause::Fetch(
        values.to_vec(),
    ))?]);
    if let Some(construct) = query.render_problem {
        return Err(TqlError::Unsupported { construct });
    }
    let QueryStructure::Pipeline(mut query) = query.inner.structure else {
        return invalid();
    };
    let Some(stage::Stage::Fetch(fetch)) = query.stages.pop() else {
        return invalid();
    };
    let typeql::query::stage::fetch::FetchObjectBody::Entries(entries) = fetch.object.body else {
        return invalid();
    };
    Ok(entries.into_iter().map(|entry| entry.to_string()).collect())
}
