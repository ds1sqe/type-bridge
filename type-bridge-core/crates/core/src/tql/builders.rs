#![allow(deprecated)] // Legacy node bridges are removed in 11.30 / 3.0.
use super::{Query, TqlError};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use typeql::{
    Variable,
    common::{identifier::Identifier, token},
    expression::{self, Expression},
    pattern,
    query::{
        self, QueryStructure,
        stage::{self, delete, fetch, modifier, reduce},
    },
    schema::definable::function::Argument,
    statement::{self, comparison, thing},
    type_::{self, NamedType, NamedTypeAny, TypeRef, TypeRefAny},
    value::{self, IntegerLiteral, StringLiteral},
};

macro_rules! node {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, Debug)]
        pub struct $name {
            pub(super) inner: $inner,
        }
    };
}
node!(Pattern, pattern::Pattern);
node!(Expr, Expression);
node!(Constraint, thing::Constraint);
node!(Stage, stage::Stage);
node!(FetchValue, fetch::FetchSome);
node!(Deletable, delete::Deletable);
node!(Reducer, reduce::Reducer);

pub(super) fn variable(name: &str) -> Result<Variable, TqlError> {
    let name = name.strip_prefix('$').unwrap_or(name);
    if name == "_" {
        return Ok(Variable::Anonymous {
            span: None,
            optional: None,
        });
    }
    if !typeql::common::identifier::is_valid_var_identifier(name) {
        return Err(TqlError::InvalidIdentifier);
    }
    Ok(Variable::Named {
        span: None,
        ident: Identifier::from(name),
        optional: None,
    })
}

pub(super) fn type_ref(name: &str) -> Result<TypeRef, TqlError> {
    if name.starts_with('$') {
        return variable(name).map(TypeRef::Variable);
    }
    if let Some((scope, name)) = name.split_once(':') {
        return Ok(TypeRef::Scoped(type_::ScopedLabel::new(
            None,
            typeql::parse_label(scope).map_err(|_| TqlError::InvalidIdentifier)?,
            typeql::parse_label(name).map_err(|_| TqlError::InvalidIdentifier)?,
        )));
    }
    typeql::parse_label(name)
        .map(TypeRef::Label)
        .map_err(|_| TqlError::InvalidIdentifier)
}

pub fn var(name: &str) -> Result<Expr, TqlError> {
    Ok(Expr {
        inner: Expression::Variable(variable(name)?),
    })
}

/// Parse a caller-owned literal slot without changing its domain or precision.
pub fn literal_source(source: &str) -> Result<Expr, TqlError> {
    let inner = typeql::parse_value(source).map_err(|_| TqlError::InvalidLiteral)?;
    Ok(Expr {
        inner: Expression::Value(value::Literal { span: None, inner }),
    })
}

/// Build a string literal without imposing a canonical-value payload limit.
pub fn string(value: &str) -> Result<Expr, TqlError> {
    literal_source(&serde_json::to_string(value).map_err(|_| TqlError::InvalidLiteral)?)
}

pub fn literal(value: &CanonicalValue) -> Result<Expr, TqlError> {
    let source = match value {
        CanonicalValue::String(value) => {
            serde_json::to_string(value.as_str()).map_err(|_| TqlError::InvalidLiteral)?
        }
        CanonicalValue::Long(value) => value.to_string(),
        CanonicalValue::Double(value) => {
            let mut source = value.get().to_string();
            if !source.contains(['.', 'e', 'E']) {
                source.push_str(".0");
            }
            source
        }
        CanonicalValue::Boolean(value) => value.to_string(),
        CanonicalValue::Date(value) => value.to_string(),
        CanonicalValue::DateTime(value) => value.to_string(),
        CanonicalValue::DateTimeTz(value) => match value.zone() {
            type_bridge_contract::temporal::TimeZoneDesignator::Named(name) => {
                format!("{} {name}", value.local())
            }
            _ => value.to_string(),
        },
        CanonicalValue::Decimal(value) => format!("{}dec", value.as_str()),
        CanonicalValue::Duration(value) => value.to_string(),
    };
    let inner = typeql::parse_value(&source).map_err(|_| TqlError::InvalidLiteral)?;
    Ok(Expr {
        inner: Expression::Value(value::Literal { span: None, inner }),
    })
}

/// Retain the released assertion emitter's compact spelling for extreme doubles.
pub fn compact_literal(value: &CanonicalValue) -> Result<Expr, TqlError> {
    if let CanonicalValue::Double(value) = value {
        let absolute = value.get().abs();
        if absolute >= 1e21 || absolute != 0.0 && absolute < 1e-6 {
            return literal_source(&format!("{:e}", value.get()));
        }
    }
    literal(value)
}

pub fn function(name: &str, args: Vec<Expr>) -> Result<Expr, TqlError> {
    let name = if token::Function::NAMES.contains(&name) {
        expression::FunctionName::Builtin(expression::BuiltinFunctionName::new(
            None,
            token::Function::from(name),
        ))
    } else if name.contains("::")
        && name.split("::").all(|part| {
            typeql::common::identifier::is_valid_label(part)
                && Identifier::from(part).as_str_unreserved().is_ok()
        })
    {
        expression::FunctionName::Namespaced(expression::NamespacedFunctionName::new(
            None,
            name.to_owned(),
        ))
    } else {
        let ident = Identifier::from(name);
        if !typeql::common::identifier::is_valid_label(name) || ident.as_str_unreserved().is_err() {
            return Err(TqlError::InvalidIdentifier);
        }
        expression::FunctionName::Identifier(ident)
    };
    Ok(Expr {
        inner: Expression::Function(expression::FunctionCall::new(
            None,
            name,
            args.into_iter().map(|v| v.inner).collect(),
        )),
    })
}

pub fn arithmetic(left: Expr, operator: &str, right: Expr) -> Result<Expr, TqlError> {
    if !token::ArithmeticOperator::NAMES.contains(&operator) {
        return Err(TqlError::InvalidOperator);
    }
    let operation = Expression::Operation(Box::new(expression::Operation::new(
        token::ArithmeticOperator::from(operator),
        left.inner,
        right.inner,
    )));
    Ok(Expr {
        inner: Expression::Paren(Box::new(expression::Paren {
            span: None,
            inner: operation,
        })),
    })
}

fn comparison(operator: &str, rhs: Expr) -> Result<comparison::Comparison, TqlError> {
    if !token::Comparator::NAMES.contains(&operator) {
        return Err(TqlError::InvalidOperator);
    }
    Ok(comparison::Comparison {
        span: None,
        comparator: token::Comparator::from(operator),
        rhs: rhs.inner,
    })
}

pub fn compare(left: Expr, operator: &str, right: Expr) -> Result<Pattern, TqlError> {
    Ok(Pattern {
        inner: pattern::Pattern::Statement(statement::Statement::Comparison(
            comparison::ComparisonStatement::new(None, left.inner, comparison(operator, right)?),
        )),
    })
}

pub fn isa(name: &str, exact: bool) -> Result<Constraint, TqlError> {
    Ok(Constraint {
        inner: thing::Constraint::Isa(thing::isa::Isa::new(
            None,
            if exact {
                thing::isa::IsaKind::Exact
            } else {
                thing::isa::IsaKind::Subtype
            },
            type_ref(name)?,
            None,
        )),
    })
}

pub fn iid(value: &str) -> Result<Constraint, TqlError> {
    if !value
        .strip_prefix("0x")
        .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(TqlError::InvalidLiteral);
    }
    Ok(Constraint {
        inner: thing::Constraint::Iid(thing::Iid {
            span: None,
            iid: value.to_owned(),
        }),
    })
}

pub fn has(name: &str, value: Expr) -> Result<Constraint, TqlError> {
    let value = match value.inner {
        Expression::Variable(v) => thing::HasValue::Variable(v),
        v => thing::HasValue::Expression(v),
    };
    Ok(Constraint {
        inner: thing::Constraint::Has(thing::Has::new(
            None,
            Some(TypeRefAny::Type(type_ref(name)?)),
            value,
        )),
    })
}

pub fn has_comparison(name: &str, operator: &str, value: Expr) -> Result<Constraint, TqlError> {
    Ok(Constraint {
        inner: thing::Constraint::Has(thing::Has::new(
            None,
            Some(TypeRefAny::Type(type_ref(name)?)),
            thing::HasValue::Comparison(comparison(operator, value)?),
        )),
    })
}

pub(super) fn relation(players: Vec<(&str, &str)>) -> Result<thing::Relation, TqlError> {
    let role_players = players
        .into_iter()
        .map(|(role, player)| {
            Ok(thing::RolePlayer::Typed(
                TypeRefAny::Type(type_ref(role)?),
                variable(player)?,
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(thing::Relation {
        span: None,
        role_players,
    })
}

pub fn links(players: Vec<(&str, &str)>) -> Result<Constraint, TqlError> {
    Ok(Constraint {
        inner: thing::Constraint::Links(thing::Links::new(None, relation(players)?)),
    })
}

pub fn thing(name: &str, constraints: Vec<Constraint>) -> Result<Pattern, TqlError> {
    Ok(Pattern {
        inner: pattern::Pattern::from(thing::Thing::new(
            None,
            thing::Head::Variable(variable(name)?),
            constraints.into_iter().map(|v| v.inner).collect(),
        )),
    })
}

pub fn let_value(names: &[&str], value: Expr, stream: bool) -> Result<Pattern, TqlError> {
    let lhs = names
        .iter()
        .map(|name| variable(name))
        .collect::<Result<_, _>>()?;
    let stmt = if stream {
        statement::Statement::InIterable(statement::InIterable {
            span: None,
            lhs,
            rhs: value.inner,
        })
    } else {
        statement::Statement::Assignment(statement::Assignment::new(
            None,
            statement::AssignmentPattern::Variables(lhs),
            value.inner,
        ))
    };
    Ok(Pattern {
        inner: pattern::Pattern::Statement(stmt),
    })
}

pub fn optional(patterns: Vec<Pattern>) -> Pattern {
    Pattern {
        inner: pattern::Pattern::Optional(pattern::Optional {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn not(patterns: Vec<Pattern>) -> Pattern {
    Pattern {
        inner: pattern::Pattern::Negation(pattern::Negation {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn or(branches: Vec<Vec<Pattern>>) -> Pattern {
    Pattern {
        inner: pattern::Pattern::Disjunction(pattern::Disjunction {
            span: None,
            branches: branches
                .into_iter()
                .map(|b| b.into_iter().map(|v| v.inner).collect())
                .collect(),
        }),
    }
}

pub fn match_stage(patterns: Vec<Pattern>) -> Stage {
    Stage {
        inner: stage::Stage::Match(stage::Match {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn insert(patterns: Vec<Pattern>) -> Stage {
    Stage {
        inner: stage::Stage::Insert(stage::Insert {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn put(patterns: Vec<Pattern>) -> Stage {
    Stage {
        inner: stage::Stage::Put(stage::Put {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn update(patterns: Vec<Pattern>) -> Stage {
    Stage {
        inner: stage::Stage::Update(stage::Update {
            span: None,
            patterns: patterns.into_iter().map(|v| v.inner).collect(),
        }),
    }
}
pub fn delete_thing(name: &str) -> Result<Deletable, TqlError> {
    Ok(Deletable {
        inner: delete::Deletable::new(
            None,
            delete::DeletableKind::Concept {
                variable: variable(name)?,
            },
        ),
    })
}
pub fn delete_has(owner: &str, attribute: &str) -> Result<Deletable, TqlError> {
    Ok(Deletable {
        inner: delete::Deletable::new(
            None,
            delete::DeletableKind::Has {
                owner: variable(owner)?,
                attribute: variable(attribute)?,
            },
        ),
    })
}

/// Build `links ($role: $player, ...) of $relation`, which removes only the
/// listed role players from the relation bound to `relation`; the relation
/// and the players remain. `players` pairs a role label with a player
/// variable name, both without sigils.
pub fn delete_links(relation: &str, players: Vec<(&str, &str)>) -> Result<Deletable, TqlError> {
    Ok(Deletable {
        inner: delete::Deletable::new(
            None,
            delete::DeletableKind::Links {
                players: self::relation(players)?,
                relation: variable(relation)?,
            },
        ),
    })
}

pub fn delete_optional(items: Vec<Deletable>) -> Deletable {
    Deletable {
        inner: delete::Deletable::new(
            None,
            delete::DeletableKind::Optional {
                deletables: items.into_iter().map(|value| value.inner).collect(),
            },
        ),
    }
}

pub fn delete(items: Vec<Deletable>) -> Stage {
    Stage {
        inner: stage::Stage::Delete(stage::Delete::new(
            None,
            items.into_iter().map(|v| v.inner).collect(),
        )),
    }
}

pub fn fetch_expr(value: Expr) -> FetchValue {
    FetchValue {
        inner: fetch::FetchSome::Single(fetch::FetchSingle::Expression(value.inner)),
    }
}
pub fn fetch_attributes(name: &str) -> Result<FetchValue, TqlError> {
    Ok(FetchValue {
        inner: fetch::FetchSome::Object(fetch::FetchObject::new(
            None,
            fetch::FetchObjectBody::AttributesAll(variable(name)?),
        )),
    })
}
pub fn fetch(entries: Vec<(&str, FetchValue)>) -> Result<Stage, TqlError> {
    let entries = entries
        .into_iter()
        .map(|(key, value)| {
            Ok(fetch::FetchObjectEntry::new(
                None,
                StringLiteral {
                    value: serde_json::to_string(key).map_err(|_| TqlError::InvalidLiteral)?,
                },
                value.inner,
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(Stage {
        inner: stage::Stage::Fetch(stage::Fetch::new(
            None,
            fetch::FetchObject::new(None, fetch::FetchObjectBody::Entries(entries)),
        )),
    })
}

pub fn select(names: &[&str]) -> Result<Stage, TqlError> {
    Ok(Stage {
        inner: stage::Stage::Operator(modifier::Operator::Select(modifier::Select::new(
            None,
            names
                .iter()
                .map(|name| variable(name))
                .collect::<Result<_, _>>()?,
        ))),
    })
}
pub fn sort(fields: &[(&str, bool)]) -> Result<Stage, TqlError> {
    let fields = fields
        .iter()
        .map(|(name, descending)| {
            Ok(modifier::OrderedVariable::new(
                None,
                variable(name)?,
                Some(if *descending {
                    token::Order::Desc
                } else {
                    token::Order::Asc
                }),
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(Stage {
        inner: stage::Stage::Operator(modifier::Operator::Sort(modifier::Sort::new(None, fields))),
    })
}
/// Sort using the grammar's implicit ascending order.
pub fn sort_default(names: &[&str]) -> Result<Stage, TqlError> {
    let fields = names
        .iter()
        .map(|name| Ok(modifier::OrderedVariable::new(None, variable(name)?, None)))
        .collect::<Result<_, TqlError>>()?;
    Ok(Stage {
        inner: stage::Stage::Operator(modifier::Operator::Sort(modifier::Sort::new(None, fields))),
    })
}

pub fn offset(value: u64) -> Stage {
    Stage {
        inner: stage::Stage::Operator(modifier::Operator::Offset(modifier::Offset::new(
            None,
            IntegerLiteral {
                value: value.to_string(),
            },
        ))),
    }
}
pub fn limit(value: u64) -> Stage {
    Stage {
        inner: stage::Stage::Operator(modifier::Operator::Limit(modifier::Limit::new(
            None,
            IntegerLiteral {
                value: value.to_string(),
            },
        ))),
    }
}
pub fn count(target: &str, input: Option<&str>, group: &[&str]) -> Result<Stage, TqlError> {
    reduce_stage(vec![(target, reducer("count", input)?)], group)
}

pub fn reducer(name: &str, input: Option<&str>) -> Result<Reducer, TqlError> {
    let input = input.map(variable).transpose()?;
    let inner = match (name, input) {
        ("count", input) => reduce::Reducer::Count(reduce::Count::new(None, input)),
        ("list", Some(input)) => reduce::Reducer::Collect(reduce::Collect::new(
            None,
            token::ReduceOperatorCollect::List,
            input,
        )),
        (name, Some(input)) if token::ReduceOperatorStat::NAMES.contains(&name) => {
            reduce::Reducer::Stat(reduce::Stat::new(
                None,
                token::ReduceOperatorStat::from(name),
                input,
            ))
        }
        _ => return Err(TqlError::InvalidOperator),
    };
    Ok(Reducer { inner })
}

pub fn reduce_stage(assignments: Vec<(&str, Reducer)>, group: &[&str]) -> Result<Stage, TqlError> {
    let assignments = assignments
        .into_iter()
        .map(|(name, reducer)| {
            Ok(reduce::ReduceAssign {
                variable: variable(name)?,
                reducer: reducer.inner,
            })
        })
        .collect::<Result<_, TqlError>>()?;
    let groupby = if group.is_empty() {
        None
    } else {
        Some(
            group
                .iter()
                .map(|name| variable(name))
                .collect::<Result<_, _>>()?,
        )
    };
    Ok(Stage {
        inner: stage::Stage::Operator(modifier::Operator::Reduce(stage::Reduce::new(
            None,
            assignments,
            groupby,
        ))),
    })
}
pub fn given(columns: &[(&str, ValueTypeTag)]) -> Result<Stage, TqlError> {
    let arguments = columns
        .iter()
        .map(|(name, domain)| {
            Ok(Argument::new(
                None,
                variable(name)?,
                builtin_value_type(*domain),
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(Stage {
        inner: stage::Stage::Given(stage::Given::new(None, arguments)),
    })
}
pub fn pipeline(stages: Vec<Stage>) -> Query {
    pipeline_with_functions(Vec::new(), stages)
}

pub fn conjunction(patterns: Vec<Pattern>) -> Pattern {
    Pattern {
        inner: pattern::Pattern::Conjunction(pattern::Conjunction {
            span: None,
            patterns: patterns.into_iter().map(|value| value.inner).collect(),
        }),
    }
}

pub fn distinct() -> Stage {
    Stage {
        inner: stage::Stage::Operator(modifier::Operator::Distinct(modifier::Distinct::new(None))),
    }
}

pub fn identity(left: &str, right: &str) -> Result<Pattern, TqlError> {
    Ok(Pattern {
        inner: pattern::Pattern::Statement(statement::Statement::Is(statement::Is {
            span: None,
            lhs: variable(left)?,
            rhs: variable(right)?,
        })),
    })
}

pub fn subtype(name: &str, parent: &str) -> Result<Pattern, TqlError> {
    use statement::type_::{Constraint, ConstraintBase, Sub, SubKind, Type};
    Ok(Pattern {
        inner: pattern::Pattern::Statement(statement::Statement::Type(Type::new(
            None,
            None,
            type_ref(name)?,
            vec![Constraint::new(
                None,
                ConstraintBase::Sub(Sub::new(None, SubKind::Transitive, type_ref(parent)?)),
                vec![],
            )],
        ))),
    })
}

pub fn isa_value(name: &str, exact: bool, value: Expr) -> Result<Constraint, TqlError> {
    let mut result = isa(name, exact)?;
    let thing::Constraint::Isa(isa) = &mut result.inner else {
        unreachable!()
    };
    isa.constraint = Some(match value.inner {
        Expression::Value(value) => thing::isa::IsaInstanceConstraint::Value(value),
        value => thing::isa::IsaInstanceConstraint::Expression(value),
    });
    Ok(result)
}

pub fn isa_relation(name: &str, players: Vec<(&str, &str)>) -> Result<Constraint, TqlError> {
    let mut result = isa(name, false)?;
    let thing::Constraint::Isa(isa) = &mut result.inner else {
        unreachable!()
    };
    isa.constraint = Some(thing::isa::IsaInstanceConstraint::Relation(relation(
        players,
    )?));
    Ok(result)
}

pub fn anonymous_relation(
    players: Vec<(&str, &str)>,
    constraints: Vec<Constraint>,
) -> Result<Pattern, TqlError> {
    Ok(Pattern {
        inner: pattern::Pattern::from(thing::Thing::new(
            None,
            thing::Head::Relation(None, relation(players)?),
            constraints.into_iter().map(|value| value.inner).collect(),
        )),
    })
}

pub fn fetch_attribute(owner: &str, name: &str, list: bool) -> Result<FetchValue, TqlError> {
    let attribute = fetch::FetchAttribute {
        span: None,
        owner: variable(owner)?,
        attribute: TypeRefAny::Type(type_ref(name)?),
    };
    Ok(FetchValue {
        inner: if list {
            fetch::FetchSome::List(fetch::FetchList::new(
                None,
                fetch::FetchStream::Attribute(attribute),
            ))
        } else {
            fetch::FetchSome::Single(fetch::FetchSingle::Attribute(attribute))
        },
    })
}

pub fn typed_literal(value: &crate::ast::TypedLiteral) -> Result<Expr, TqlError> {
    use crate::ast::TypedLiteral as L;
    let source = match value {
        L::String(value) => serde_json::to_string(value).map_err(|_| TqlError::InvalidLiteral)?,
        L::Long(value) => value.to_string(),
        L::Double(value) if value.is_finite() => {
            let s = value.to_string();
            if s.contains(['.', 'e', 'E']) {
                s
            } else {
                format!("{s}.0")
            }
        }
        L::Double(_) => return Err(TqlError::InvalidLiteral),
        L::Boolean(value) => value.to_string(),
        L::Decimal(value) => format!("{}dec", value.strip_suffix("dec").unwrap_or(value)),
        L::Date(value) | L::DateTime(value) | L::DateTimeTz(value) | L::Duration(value) => {
            value.clone()
        }
    };
    Ok(Expr {
        inner: Expression::Value(value::Literal {
            span: None,
            inner: typeql::parse_value(&source).map_err(|_| TqlError::InvalidLiteral)?,
        }),
    })
}

pub fn fetch_object(entries: Vec<(&str, FetchValue)>) -> Result<FetchValue, TqlError> {
    let Stage {
        inner: stage::Stage::Fetch(value),
    } = fetch(entries)?
    else {
        unreachable!()
    };
    Ok(FetchValue {
        inner: fetch::FetchSome::Object(value.object),
    })
}
pub fn fetch_subquery(stages: Vec<Stage>) -> FetchValue {
    FetchValue {
        inner: fetch::FetchSome::List(fetch::FetchList {
            span: None,
            stream: fetch::FetchStream::SubQueryFetch(
                stages.into_iter().map(|v| v.inner).collect(),
            ),
        }),
    }
}

/// Render a validated adapter pattern fragment through official Display.
pub fn render_pattern(value: &Pattern) -> Result<String, TqlError> {
    let mut value = value.clone();
    if let Some(construct) = super::render_guard::prepare_pattern(&mut value.inner) {
        return Err(TqlError::Unsupported { construct });
    }
    Ok(value.inner.to_string())
}

node!(
    FunctionDefinition,
    typeql::schema::definable::function::Function
);

pub fn require(names: &[&str]) -> Result<Stage, TqlError> {
    Ok(Stage {
        inner: stage::Stage::Operator(modifier::Operator::Require(modifier::Require::new(
            None,
            names
                .iter()
                .map(|name| variable(name))
                .collect::<Result<_, _>>()?,
        ))),
    })
}

pub fn function_definition(
    name: &str,
    arguments: &[(&str, &str)],
    output: ValueTypeTag,
    stages: Vec<Stage>,
    result: Reducer,
) -> Result<FunctionDefinition, TqlError> {
    use typeql::schema::definable::function as f;
    let ident = Identifier::from(name);
    if !typeql::common::identifier::is_valid_label(name) || ident.as_str_unreserved().is_err() {
        return Err(TqlError::InvalidIdentifier);
    }
    let arguments = arguments
        .iter()
        .map(|(name, label)| {
            Ok(Argument::new(
                None,
                variable(name)?,
                NamedTypeAny::Simple(NamedType::Label(
                    typeql::parse_label(label).map_err(|_| TqlError::InvalidIdentifier)?,
                )),
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    let output = builtin_value_type(output);
    Ok(FunctionDefinition {
        inner: f::Function::new(
            None,
            f::Signature {
                span: None,
                ident,
                args: arguments,
                output: f::Output::Single(f::Single::new(None, vec![output])),
            },
            vec![],
            f::FunctionBlock::new(
                None,
                stages.into_iter().map(|stage| stage.inner).collect(),
                f::ReturnStatement::Reduce(f::ReturnReduction::Value(vec![result.inner], None)),
            ),
            String::new(),
        ),
    })
}

pub fn pipeline_with_functions(functions: Vec<FunctionDefinition>, stages: Vec<Stage>) -> Query {
    Query::from_ast(typeql::Query {
        span: None,
        structure: QueryStructure::Pipeline(query::Pipeline::new(
            None,
            functions
                .into_iter()
                .map(|function| query::pipeline::Preamble {
                    span: None,
                    function: function.inner,
                })
                .collect(),
            stages.into_iter().map(|stage| stage.inner).collect(),
        )),
        has_explicit_end: false,
    })
}

pub fn fetch_stream(stages: Vec<Stage>, names: &[&str]) -> Result<FetchValue, TqlError> {
    use typeql::schema::definable::function as f;
    if names.is_empty() {
        return Err(TqlError::InvalidShape);
    }
    Ok(FetchValue {
        inner: fetch::FetchSome::List(fetch::FetchList::new(
            None,
            fetch::FetchStream::SubQueryFunctionBlock(f::FunctionBlock::new(
                None,
                stages.into_iter().map(|stage| stage.inner).collect(),
                f::ReturnStatement::Stream(f::ReturnStream {
                    span: None,
                    vars: names
                        .iter()
                        .map(|name| variable(name))
                        .collect::<Result<_, _>>()?,
                }),
            )),
        )),
    })
}

fn builtin_value_type(domain: ValueTypeTag) -> NamedTypeAny {
    NamedTypeAny::Simple(builtin_named_type(domain))
}

pub(super) fn builtin_named_type(domain: ValueTypeTag) -> NamedType {
    let spelling = match domain {
        ValueTypeTag::Long => "integer",
        ValueTypeTag::DateTimeTz => "datetime-tz",
        other => other.as_str(),
    };
    NamedType::BuiltinValueType(type_::BuiltinValueType::new(
        None,
        token::ValueType::from(spelling),
    ))
}

/// Derive read-only count views without modifying a carried copy statement.
pub fn backfill_count_views(source: &str) -> Result<(Query, Query), TqlError> {
    let query = super::parse(source)?;
    if query.inner.has_explicit_end {
        return Err(TqlError::InvalidShape);
    }
    let QueryStructure::Pipeline(mut pipeline) = query.inner.structure else {
        return Err(TqlError::InvalidShape);
    };
    if !pipeline.preambles.is_empty() || pipeline.stages.len() != 2 {
        return Err(TqlError::InvalidShape);
    }
    let stage::Stage::Insert(insert) = pipeline.stages.pop().unwrap() else {
        return Err(TqlError::InvalidShape);
    };
    let stage::Stage::Match(matched) = pipeline.stages.pop().unwrap() else {
        return Err(TqlError::InvalidShape);
    };
    let [pattern::Pattern::Statement(statement::Statement::Thing(destination))] =
        insert.patterns.as_slice()
    else {
        return Err(TqlError::InvalidShape);
    };
    let thing::Head::Variable(owner) = &destination.head else {
        return Err(TqlError::InvalidShape);
    };
    let [thing::Constraint::Has(destination)] = destination.constraints.as_slice() else {
        return Err(TqlError::InvalidShape);
    };
    let Some(destination_type) = &destination.type_ else {
        return Err(TqlError::InvalidShape);
    };
    let mut guards = 0;
    let mut total = Vec::new();
    for candidate in &matched.patterns {
        let is_guard = match candidate {
            pattern::Pattern::Negation(negation) => match negation.patterns.as_slice() {
                [pattern::Pattern::Statement(statement::Statement::Thing(guard))] => {
                    match (&guard.head, guard.constraints.as_slice()) {
                        (thing::Head::Variable(guard_owner), [thing::Constraint::Has(has)]) => {
                            guard_owner.name() == owner.name()
                                && owner.name().is_some()
                                && matches!(has.value, thing::HasValue::Variable(_))
                                && has.type_.as_ref().is_some_and(|name| {
                                    name.to_string() == destination_type.to_string()
                                })
                        }
                        _ => false,
                    }
                }
                _ => false,
            },
            _ => false,
        };
        if is_guard {
            guards += 1;
        } else {
            total.push(Pattern {
                inner: candidate.clone(),
            });
        }
    }
    if guards > 1 {
        return Err(TqlError::InvalidShape);
    }
    let guarded = matched
        .patterns
        .into_iter()
        .map(|inner| Pattern { inner })
        .collect();
    Ok((
        self::pipeline(vec![match_stage(guarded), count("c", None, &[])?]),
        self::pipeline(vec![match_stage(total), count("c", None, &[])?]),
    ))
}

/// Render an opaque expression using the official AST display form.
pub fn render_expression(value: &Expr) -> String {
    let mut expression = value.inner.clone();
    super::render_guard::prepare_expression(&mut expression);
    expression.to_string()
}

/// Build an instance pattern with a concrete type and ownership fields.
pub fn entity(name: &str, kind: &str, fields: Vec<(&str, Expr)>) -> Result<Pattern, TqlError> {
    let mut constraints = vec![isa(kind, false)?];
    for (name, value) in fields {
        constraints.push(has(name, value)?);
    }
    thing(name, constraints)
}

/// Render an internal probe tag without allowing another TypeQL statement.
pub fn render_commented(query: &Query, comment: &str) -> Result<String, TqlError> {
    if comment.contains(['\n', '\r']) {
        return Err(TqlError::InvalidShape);
    }
    Ok(format!("# {comment}\n{}", super::render(query)?))
}

/// Join already-rendered complete queries for retained document-returning APIs.
pub fn render_query_document(queries: &[String]) -> String {
    queries.join("\n\n")
}
