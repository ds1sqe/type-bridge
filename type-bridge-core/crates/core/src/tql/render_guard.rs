use super::UnsupportedConstruct;
use typeql::{
    Annotation, Expression,
    pattern::Pattern,
    query::{
        QueryStructure,
        schema::SchemaQuery,
        stage::{
            Stage,
            fetch::{FetchObjectBody, FetchSingle, FetchSome, FetchStream},
        },
    },
    schema::definable::Definable,
    statement::{
        AssignmentPattern, Statement,
        thing::{Constraint, HasValue, isa::IsaInstanceConstraint},
    },
    value::{DateTimeTZLiteral, Literal, TimeZone, ValueLiteral},
};

/// Retain the lexical separator missing from typeql value.rs:311–315.
/// Remove this compatibility formatter when upstream IANA Display is fixed.
fn render_datetime_tz_iana_literal(value: &mut DateTimeTZLiteral) {
    if let TimeZone::IANA(zone) = &mut value.timezone
        && !zone.starts_with(' ')
    {
        zone.insert(0, ' ');
    }
}

pub(super) fn prepare(query: &mut typeql::Query) -> Option<UnsupportedConstruct> {
    let mut visitor = Prepare {
        unsupported: query
            .has_explicit_end
            .then_some(UnsupportedConstruct::ExplicitEnd),
    };
    match &mut query.structure {
        QueryStructure::Pipeline(pipeline) => {
            for preamble in &mut pipeline.preambles {
                visitor.annotations(&mut preamble.function.annotations);
                visitor.stages(&mut preamble.function.block.stages);
            }
            visitor.stages(&mut pipeline.stages);
        }
        QueryStructure::Schema(schema) => {
            let definables = match schema {
                SchemaQuery::Define(query) => &mut query.definables,
                SchemaQuery::Redefine(query) => &mut query.definables,
                SchemaQuery::Undefine(_) => return visitor.unsupported,
            };
            for definable in definables {
                match definable {
                    Definable::TypeRename(_) | Definable::RoleTypeRename(_) => {
                        visitor
                            .unsupported
                            .get_or_insert(UnsupportedConstruct::SchemaRename);
                    }
                    Definable::Function(function) => {
                        visitor.annotations(&mut function.annotations);
                        visitor.stages(&mut function.block.stages);
                    }
                    Definable::Struct(value) => {
                        visitor.annotations(&mut value.annotations);
                        for field in &mut value.fields {
                            visitor.annotations(&mut field.annotations);
                        }
                    }
                    Definable::TypeDeclaration(value) => {
                        visitor.annotations(&mut value.annotations);
                        for capability in &mut value.capabilities {
                            visitor.annotations(&mut capability.annotations);
                        }
                    }
                }
            }
        }
    }
    visitor.unsupported
}

struct Prepare {
    unsupported: Option<UnsupportedConstruct>,
}
impl Prepare {
    fn stages(&mut self, items: &mut [Stage]) {
        for stage in items {
            match stage {
                Stage::Match(value) => self.patterns(&mut value.patterns),
                Stage::Insert(value) => self.patterns(&mut value.patterns),
                Stage::Update(value) => self.patterns(&mut value.patterns),
                Stage::Put(value) => self.patterns(&mut value.patterns),
                Stage::Fetch(value) => self.object(&mut value.object.body),
                Stage::Given(_) | Stage::Delete(_) | Stage::Operator(_) => {}
            }
        }
    }

    fn patterns(&mut self, items: &mut [Pattern]) {
        for pattern in items {
            match pattern {
                Pattern::Conjunction(value) => self.patterns(&mut value.patterns),
                Pattern::Negation(value) => self.patterns(&mut value.patterns),
                Pattern::Optional(value) => self.patterns(&mut value.patterns),
                Pattern::Disjunction(value) => {
                    for branch in &mut value.branches {
                        self.patterns(branch);
                    }
                }
                Pattern::Statement(statement) => self.statement(statement),
            }
        }
    }

    fn statement(&mut self, statement: &mut Statement) {
        match statement {
            Statement::Assignment(value) => {
                if matches!(value.lhs, AssignmentPattern::Deconstruct(_)) {
                    // Upstream statement/mod.rs:151 calls todo!() for this variant.
                    self.unsupported
                        .get_or_insert(UnsupportedConstruct::StructDestructuring);
                }
                self.expression(&mut value.rhs);
            }
            Statement::InIterable(value) => self.expression(&mut value.rhs),
            Statement::Comparison(value) => {
                self.expression(&mut value.lhs);
                self.expression(&mut value.comparison.rhs);
            }
            Statement::Thing(value) => {
                for constraint in &mut value.constraints {
                    match constraint {
                        Constraint::Isa(value) => match &mut value.constraint {
                            Some(
                                IsaInstanceConstraint::Value(value)
                                | IsaInstanceConstraint::Struct(value),
                            ) => self.literal(value),
                            Some(IsaInstanceConstraint::Expression(value)) => {
                                self.expression(value)
                            }
                            Some(IsaInstanceConstraint::Comparison(value)) => {
                                self.expression(&mut value.rhs)
                            }
                            Some(IsaInstanceConstraint::Relation(_)) | None => {}
                        },
                        Constraint::Has(value) => match &mut value.value {
                            HasValue::Expression(value) => self.expression(value),
                            HasValue::Comparison(value) => self.expression(&mut value.rhs),
                            HasValue::Variable(_) => {}
                        },
                        Constraint::Links(_) | Constraint::Iid(_) => {}
                    }
                }
            }
            Statement::Type(value) => {
                for constraint in &mut value.constraints {
                    self.annotations(&mut constraint.annotations);
                }
            }
            Statement::Is(_) => {}
        }
    }

    fn expression(&mut self, value: &mut Expression) {
        match value {
            Expression::Value(value) => self.literal(value),
            Expression::Function(value) => {
                for argument in &mut value.args {
                    self.expression(argument);
                }
            }
            Expression::Operation(value) => {
                self.expression(&mut value.left);
                self.expression(&mut value.right);
            }
            Expression::Paren(value) => self.expression(&mut value.inner),
            Expression::List(value) => {
                for item in &mut value.items {
                    self.expression(item);
                }
            }
            Expression::ListIndex(value) => self.expression(&mut value.index),
            Expression::ListIndexRange(value) => {
                self.expression(&mut value.from);
                self.expression(&mut value.to);
            }
            Expression::Variable(_) | Expression::Label(_) | Expression::ScopedLabel(_) => {}
        }
    }

    fn literal(&mut self, literal: &mut Literal) {
        if let ValueLiteral::DateTimeTz(value) = &mut literal.inner {
            render_datetime_tz_iana_literal(value);
        }
    }

    fn annotations(&mut self, annotations: &mut [Annotation]) {
        for annotation in annotations {
            match annotation {
                Annotation::Range(value) => {
                    if let Some(value) = &mut value.min {
                        self.literal(value);
                    }
                    if let Some(value) = &mut value.max {
                        self.literal(value);
                    }
                }
                Annotation::Values(value) => {
                    for value in &mut value.values {
                        self.literal(value);
                    }
                }
                _ => {}
            }
        }
    }

    fn object(&mut self, body: &mut FetchObjectBody) {
        if let FetchObjectBody::Entries(entries) = body {
            for entry in entries {
                match &mut entry.value {
                    FetchSome::Object(value) => self.object(&mut value.body),
                    FetchSome::Single(FetchSingle::FunctionBlock(value)) => {
                        self.stages(&mut value.stages)
                    }
                    FetchSome::Single(FetchSingle::Expression(value)) => self.expression(value),
                    FetchSome::Single(FetchSingle::Attribute(_)) => {}
                    FetchSome::List(value) => match &mut value.stream {
                        FetchStream::SubQueryFetch(value) => self.stages(value),
                        FetchStream::SubQueryFunctionBlock(value) => self.stages(&mut value.stages),
                        FetchStream::Function(value) => {
                            for argument in &mut value.args {
                                self.expression(argument);
                            }
                        }
                        FetchStream::Attribute(_) => {}
                    },
                }
            }
        }
    }
}

pub(super) fn prepare_pattern(pattern: &mut Pattern) -> Option<UnsupportedConstruct> {
    let mut visitor = Prepare { unsupported: None };
    visitor.patterns(std::slice::from_mut(pattern));
    visitor.unsupported
}

pub(super) fn prepare_expression(value: &mut Expression) {
    Prepare { unsupported: None }.expression(value);
}
