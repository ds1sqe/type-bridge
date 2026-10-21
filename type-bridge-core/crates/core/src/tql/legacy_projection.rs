//! Lossless projections onto the deprecated node vocabulary (3.0).
#![allow(deprecated)]
use super::{Query, TqlError, UnsupportedConstruct};
use crate::ast as old;
use typeql::{
    Variable,
    expression::Expression,
    query::{
        QueryStructure,
        stage::{self, fetch, modifier, reduce},
    },
    statement::{self, thing},
    type_::{TypeRef, TypeRefAny},
    value::{Literal, ValueLiteral},
};

type Result<T> = std::result::Result<T, TqlError>;
fn unsupported<T>() -> Result<T> {
    Err(TqlError::Unsupported {
        construct: UnsupportedConstruct::LegacyProjection,
    })
}
fn variable(value: &Variable) -> Result<String> {
    match value {
        Variable::Named { optional: None, .. } | Variable::Anonymous { optional: None, .. } => {
            Ok(value.to_string())
        }
        _ => unsupported(),
    }
}
fn type_name(value: &TypeRef) -> Result<String> {
    match value {
        TypeRef::Variable(v) => variable(v),
        _ => Ok(value.to_string()),
    }
}
fn simple_type(value: &TypeRefAny) -> Result<String> {
    match value {
        TypeRefAny::Type(t) => type_name(t),
        _ => unsupported(),
    }
}
fn literal(value: &Literal) -> Result<old::Value> {
    let source = value.to_string();
    let (domain, data) = match &value.inner {
        ValueLiteral::Boolean(v) => ("boolean", serde_json::Value::Bool(v.value == "true")),
        ValueLiteral::Integer(_) => (
            "long",
            source
                .parse::<i64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| source.clone().into()),
        ),
        ValueLiteral::Double(_) => (
            "double",
            source
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or_else(|| source.clone().into()),
        ),
        ValueLiteral::Decimal(_) => (
            "decimal",
            source
                .strip_suffix("dec")
                .ok_or(TqlError::InvalidLiteral)?
                .into(),
        ),
        ValueLiteral::String(v) => (
            "string",
            v.unescape().map_err(|_| TqlError::InvalidLiteral)?.into(),
        ),
        ValueLiteral::Date(_) => ("date", source.into()),
        ValueLiteral::DateTime(_) => ("datetime", source.into()),
        ValueLiteral::DateTimeTz(_) => ("datetime-tz", source.into()),
        ValueLiteral::Duration(_) => ("duration", source.into()),
        ValueLiteral::Struct(_) => return unsupported(),
    };
    Ok(old::Value::Literal(old::LiteralValue {
        value: data,
        value_type: domain.into(),
    }))
}
fn expression(value: &Expression) -> Result<old::Value> {
    Ok(match value {
        Expression::Variable(v) => old::Value::Variable(variable(v)?),
        Expression::Label(v) => old::Value::Variable(v.to_string()),
        Expression::ScopedLabel(v) => old::Value::Variable(v.to_string()),
        Expression::Value(v) => return literal(v),
        Expression::Function(v) => old::Value::FunctionCall(old::FunctionCallValue {
            function: v.name.to_string(),
            args: v.args.iter().map(expression).collect::<Result<_>>()?,
        }),
        Expression::Operation(v) => old::Value::Arithmetic(old::ArithmeticValue {
            left: Box::new(expression(&v.left)?),
            operator: v.op.to_string(),
            right: Box::new(expression(&v.right)?),
        }),
        Expression::Paren(v) => return expression(&v.inner),
        _ => return unsupported(),
    })
}
fn has_value(value: &thing::HasValue, update: bool) -> Result<old::Value> {
    match value {
        thing::HasValue::Variable(v) => Ok(old::Value::Variable(variable(v)?)),
        thing::HasValue::Expression(v) => expression(v),
        thing::HasValue::Comparison(v) if update && v.comparator.to_string() == "==" => {
            expression(&v.rhs)
        }
        _ => unsupported(),
    }
}
fn constraint(value: &thing::Constraint) -> Result<old::Constraint> {
    Ok(match value {
        thing::Constraint::Isa(v) if v.constraint.is_none() => old::Constraint::Isa {
            type_name: type_name(&v.type_)?,
            strict: matches!(v.kind, thing::isa::IsaKind::Exact),
        },
        thing::Constraint::Iid(v) => old::Constraint::Iid(v.iid.clone()),
        thing::Constraint::Has(v) => old::Constraint::Has {
            attr_name: simple_type(v.type_.as_ref().ok_or(TqlError::Unsupported {
                construct: UnsupportedConstruct::LegacyProjection,
            })?)?,
            value: has_value(&v.value, false)?,
        },
        _ => return unsupported(),
    })
}
fn players(value: &thing::Relation) -> Result<Vec<old::RolePlayer>> {
    value
        .role_players
        .iter()
        .map(|v| match v {
            thing::RolePlayer::Typed(t, v) => Ok(old::RolePlayer {
                role: simple_type(t)?,
                player_var: variable(v)?,
            }),
            thing::RolePlayer::Untyped(_) => unsupported(),
        })
        .collect()
}
fn patterns(values: &[typeql::Pattern]) -> Result<Vec<old::Pattern>> {
    values.iter().map(pattern).collect()
}
fn pattern(value: &typeql::Pattern) -> Result<old::Pattern> {
    use typeql::Pattern as P;
    Ok(match value {
        P::Negation(v) => old::Pattern::Not(patterns(&v.patterns)?),
        P::Optional(v) => old::Pattern::Try(patterns(&v.patterns)?),
        P::Disjunction(v) => old::Pattern::Or(
            v.branches
                .iter()
                .map(|b| patterns(b))
                .collect::<Result<_>>()?,
        ),
        P::Statement(statement::Statement::Comparison(v)) => {
            let Expression::Variable(lhs) = &v.lhs else {
                return unsupported();
            };
            old::Pattern::ValueComparison {
                var: variable(lhs)?,
                operator: v.comparison.comparator.to_string(),
                value: expression(&v.comparison.rhs)?,
            }
        }
        P::Statement(statement::Statement::Type(v))
            if v.kind.is_none()
                && v.constraints.len() == 1
                && v.constraints[0].annotations.is_empty() =>
        {
            let statement::type_::ConstraintBase::Sub(sub) = &v.constraints[0].base else {
                return unsupported();
            };
            if !matches!(sub.kind, statement::type_::SubKind::Transitive) {
                return unsupported();
            }
            old::Pattern::SubType {
                variable: type_name(&v.type_)?,
                parent_type: type_name(&sub.supertype)?,
            }
        }
        P::Statement(statement::Statement::Thing(v)) => {
            let thing::Head::Variable(name) = &v.head else {
                return unsupported();
            };
            let name = variable(name)?;
            let Some((first, rest)) = v.constraints.split_first() else {
                return unsupported();
            };
            match first {
                thing::Constraint::Isa(isa) => {
                    let type_name = type_name(&isa.type_)?;
                    match &isa.constraint {
                        None => old::Pattern::Entity {
                            variable: name,
                            type_name,
                            constraints: rest.iter().map(constraint).collect::<Result<_>>()?,
                            is_strict: matches!(isa.kind, thing::isa::IsaKind::Exact),
                        },
                        Some(thing::isa::IsaInstanceConstraint::Relation(r))
                            if matches!(isa.kind, thing::isa::IsaKind::Subtype) =>
                        {
                            old::Pattern::Relation {
                                variable: name,
                                type_name,
                                role_players: players(r)?,
                                constraints: rest.iter().map(constraint).collect::<Result<_>>()?,
                            }
                        }
                        Some(thing::isa::IsaInstanceConstraint::Value(v))
                            if rest.is_empty()
                                && matches!(isa.kind, thing::isa::IsaKind::Subtype) =>
                        {
                            old::Pattern::Attribute {
                                variable: name,
                                type_name,
                                value: Some(literal(v)?),
                            }
                        }
                        Some(thing::isa::IsaInstanceConstraint::Expression(v))
                            if rest.is_empty()
                                && matches!(isa.kind, thing::isa::IsaKind::Subtype) =>
                        {
                            old::Pattern::Attribute {
                                variable: name,
                                type_name,
                                value: Some(expression(v)?),
                            }
                        }
                        _ => return unsupported(),
                    }
                }
                thing::Constraint::Iid(iid) if rest.is_empty() => old::Pattern::Iid {
                    variable: name,
                    iid: iid.iid.clone(),
                },
                thing::Constraint::Has(has) if rest.is_empty() => {
                    let Some(t) = &has.type_ else {
                        return unsupported();
                    };
                    let thing::HasValue::Variable(attr) = &has.value else {
                        return unsupported();
                    };
                    old::Pattern::Has {
                        thing_var: name,
                        attr_type: simple_type(t)?,
                        attr_var: variable(attr)?,
                    }
                }
                _ => return unsupported(),
            }
        }
        _ => return unsupported(),
    })
}
fn assignment(value: &typeql::Pattern) -> Result<old::LetAssignment> {
    match value {
        typeql::Pattern::Statement(statement::Statement::Assignment(v)) => {
            let statement::AssignmentPattern::Variables(names) = &v.lhs else {
                return unsupported();
            };
            Ok(old::LetAssignment {
                variables: names.iter().map(variable).collect::<Result<_>>()?,
                expression: expression(&v.rhs)?,
                is_stream: false,
            })
        }
        typeql::Pattern::Statement(statement::Statement::InIterable(v)) => Ok(old::LetAssignment {
            variables: v.lhs.iter().map(variable).collect::<Result<_>>()?,
            expression: expression(&v.rhs)?,
            is_stream: true,
        }),
        _ => unsupported(),
    }
}
fn statements(values: &[typeql::Pattern], update: bool) -> Result<Vec<old::Statement>> {
    let mut output = Vec::new();
    for value in values {
        let typeql::Pattern::Statement(statement::Statement::Thing(v)) = value else {
            return unsupported();
        };
        if v.constraints.is_empty() {
            return unsupported();
        }
        let (name, anonymous_relation) = match &v.head {
            thing::Head::Variable(v) => (variable(v)?, None),
            thing::Head::Relation(None, relation) => (String::new(), Some(relation)),
            _ => return unsupported(),
        };
        let mut isa = None;
        let mut relation = anonymous_relation;
        let mut attrs = Vec::new();
        for c in &v.constraints {
            match c {
                thing::Constraint::Isa(v)
                    if isa.is_none()
                        && v.constraint.is_none()
                        && matches!(v.kind, thing::isa::IsaKind::Subtype)
                        && !update =>
                {
                    isa = Some(type_name(&v.type_)?)
                }
                thing::Constraint::Links(v) if relation.is_none() && !update => {
                    relation = Some(&v.relation)
                }
                thing::Constraint::Has(v) => {
                    let Some(t) = &v.type_ else {
                        return unsupported();
                    };
                    attrs.push(old::Statement::Has {
                        subject_var: name.clone(),
                        attr_name: simple_type(t)?,
                        value: has_value(&v.value, update)?,
                    });
                }
                _ => return unsupported(),
            }
        }
        if let Some(relation) = relation {
            output.push(old::Statement::Relation {
                variable: name,
                type_name: isa.ok_or(TqlError::Unsupported {
                    construct: UnsupportedConstruct::LegacyProjection,
                })?,
                role_players: players(relation)?,
                include_variable: anonymous_relation.is_none(),
                attributes: attrs,
            });
        } else {
            if let Some(type_name) = isa {
                output.push(old::Statement::Isa {
                    variable: name,
                    type_name,
                });
            }
            output.extend(attrs);
        }
    }
    Ok(output)
}
fn fetch_item(value: &fetch::FetchObjectEntry) -> Result<old::FetchItem> {
    let key = value.key.unescape().map_err(|_| TqlError::InvalidLiteral)?;
    Ok(match &value.value {
        fetch::FetchSome::Single(fetch::FetchSingle::Attribute(v)) => old::FetchItem::Attribute {
            key,
            var: variable(&v.owner)?,
            attr_name: simple_type(&v.attribute)?,
        },
        fetch::FetchSome::List(fetch::FetchList {
            stream: fetch::FetchStream::Attribute(v),
            ..
        }) => old::FetchItem::AttributeList {
            key,
            var: variable(&v.owner)?,
            attr_name: simple_type(&v.attribute)?,
        },
        fetch::FetchSome::Single(fetch::FetchSingle::Expression(Expression::Variable(v))) => {
            old::FetchItem::Variable {
                key,
                var: variable(v)?,
            }
        }
        fetch::FetchSome::Single(fetch::FetchSingle::Expression(Expression::Function(v)))
            if v.args.len() == 1 =>
        {
            let Expression::Variable(arg) = &v.args[0] else {
                return unsupported();
            };
            old::FetchItem::Function {
                key,
                func_name: v.name.to_string(),
                var: variable(arg)?,
            }
        }
        fetch::FetchSome::Object(fetch::FetchObject {
            body: fetch::FetchObjectBody::AttributesAll(v),
            ..
        }) => old::FetchItem::NestedWildcard {
            key,
            var: variable(v)?,
        },
        _ => return unsupported(),
    })
}
fn reduce(value: &reduce::Reduce) -> Result<old::Clause> {
    let assignments = value
        .reduce_assignments
        .iter()
        .map(|a| {
            let (name, input) = match &a.reducer {
                reduce::Reducer::Count(v) => ("count".into(), v.variable.as_ref()),
                reduce::Reducer::Stat(v) => (v.reduce_operator.to_string(), Some(&v.variable)),
                reduce::Reducer::Collect(v) => (v.reduce_operator.to_string(), Some(&v.variable)),
            };
            Ok(old::ReduceAssignment {
                variable: variable(&a.variable)?,
                expression: old::Value::FunctionCall(old::FunctionCallValue {
                    function: name,
                    args: input
                        .map(|v| variable(v).map(old::Value::Variable))
                        .transpose()?
                        .into_iter()
                        .collect(),
                }),
            })
        })
        .collect::<Result<_>>()?;
    Ok(old::Clause::Reduce {
        assignments,
        group_by: value
            .groupby
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(variable)
                    .collect::<Result<Vec<_>>>()
                    .map(|v| v.join(", "))
            })
            .transpose()?,
    })
}
/// Project only constructs expressible by the legacy vocabulary.
pub fn project_legacy(query: &Query) -> Result<Vec<old::Clause>> {
    if query.inner.has_explicit_end {
        return unsupported();
    }
    let QueryStructure::Pipeline(pipeline) = &query.inner.structure else {
        return unsupported();
    };
    if !pipeline.preambles.is_empty() {
        return unsupported();
    }
    pipeline
        .stages
        .iter()
        .map(|s| {
            Ok(match s {
                stage::Stage::Match(v) => {
                    if v.patterns.iter().all(|p| {
                        matches!(
                            p,
                            typeql::Pattern::Statement(
                                statement::Statement::Assignment(_)
                                    | statement::Statement::InIterable(_)
                            )
                        )
                    }) {
                        old::Clause::MatchLet(
                            v.patterns.iter().map(assignment).collect::<Result<_>>()?,
                        )
                    } else {
                        old::Clause::Match(patterns(&v.patterns)?)
                    }
                }
                stage::Stage::Insert(v) => old::Clause::Insert(statements(&v.patterns, false)?),
                stage::Stage::Put(v) => old::Clause::Put(statements(&v.patterns, false)?),
                stage::Stage::Update(v) => old::Clause::Update(statements(&v.patterns, true)?),
                stage::Stage::Delete(v) => old::Clause::Delete(
                    v.deletables
                        .iter()
                        .map(|d| match &d.kind {
                            stage::delete::DeletableKind::Concept { variable: v } => {
                                Ok(old::Statement::DeleteThing(variable(v)?))
                            }
                            _ => unsupported(),
                        })
                        .collect::<Result<_>>()?,
                ),
                stage::Stage::Fetch(v) => {
                    let fetch::FetchObjectBody::Entries(entries) = &v.object.body else {
                        return unsupported();
                    };
                    old::Clause::Fetch(entries.iter().map(fetch_item).collect::<Result<_>>()?)
                }
                stage::Stage::Operator(v) => match v {
                    modifier::Operator::Sort(v) => old::Clause::Sort(
                        v.ordered_variables
                            .iter()
                            .map(|v| {
                                Ok(old::SortField {
                                    variable: variable(&v.variable)?,
                                    ascending: v
                                        .ordering
                                        .as_ref()
                                        .map(|o| o.to_string() != "desc")
                                        .unwrap_or(true),
                                })
                            })
                            .collect::<Result<_>>()?,
                    ),
                    modifier::Operator::Limit(v) => old::Clause::Limit(
                        v.limit
                            .value
                            .parse()
                            .map_err(|_| TqlError::InvalidLiteral)?,
                    ),
                    modifier::Operator::Offset(v) => old::Clause::Offset(
                        v.offset
                            .value
                            .parse()
                            .map_err(|_| TqlError::InvalidLiteral)?,
                    ),
                    modifier::Operator::Reduce(v) => return reduce(v),
                    _ => return unsupported(),
                },
                _ => return unsupported(),
            })
        })
        .collect()
}
