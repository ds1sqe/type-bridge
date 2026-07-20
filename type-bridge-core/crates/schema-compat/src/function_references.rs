use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::{
    id::{FunctionId, Label},
    schema::DeclaredSchema,
};
use typeql::{
    expression::{Expression, FunctionCall, FunctionName},
    pattern::Pattern,
    query::stage::{
        Stage,
        delete::DeletableKind,
        fetch::{FetchObjectBody, FetchSingle, FetchSome, FetchStream},
        modifier::Operator,
    },
    schema::definable::function::{FunctionBlock, ReturnStatement},
    statement::{
        Statement,
        thing::{Constraint as ThingConstraint, HasValue, Head as ThingHead, Relation, RolePlayer},
        type_::{ConstraintBase as TypeConstraintBase, LabelConstraint},
    },
    type_::{
        Label as TypeqlLabel, NamedType, NamedTypeAny, ScopedLabel as TypeqlScopedLabel, TypeRef,
        TypeRefAny,
    },
};

/// A declared schema plus references derived from each TypeQL function body.
///
/// The reference index is adapter metadata. It is deliberately excluded from
/// declared-schema canonical bytes and fingerprints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeqlDeclaredSchema {
    declared: DeclaredSchema,
    function_body_references: BTreeMap<FunctionId, FunctionBodyReferences>,
}

impl TypeqlDeclaredSchema {
    pub(crate) fn new(
        declared: DeclaredSchema,
        function_body_references: BTreeMap<FunctionId, FunctionBodyReferences>,
    ) -> Self {
        Self {
            declared,
            function_body_references,
        }
    }

    #[must_use]
    pub const fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }

    #[must_use]
    pub fn into_declared(self) -> DeclaredSchema {
        self.declared
    }

    #[must_use]
    pub const fn function_body_references(&self) -> &BTreeMap<FunctionId, FunctionBodyReferences> {
        &self.function_body_references
    }
}

/// Static schema references found in one function body.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionBodyReferences {
    references: BTreeSet<SchemaReference>,
    dynamic_type_reference: bool,
}

impl FunctionBodyReferences {
    #[must_use]
    pub const fn references(&self) -> &BTreeSet<SchemaReference> {
        &self.references
    }

    /// Returns true when a type position is supplied by a TypeQL variable.
    #[must_use]
    pub const fn has_dynamic_type_reference(&self) -> bool {
        self.dynamic_type_reference
    }
}

/// A neutral, static schema identity referenced by a TypeQL function body.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SchemaReference {
    Label(Label),
    Scoped { scope: Label, name: Label },
    Function(FunctionId),
}

pub(crate) fn collect_function_body_references(block: &FunctionBlock) -> FunctionBodyReferences {
    let mut collector = Collector::default();
    collector.visit_stages(&block.stages);
    collector.visit_return_statement(&block.return_stmt);
    FunctionBodyReferences {
        references: collector.references,
        dynamic_type_reference: collector.dynamic_type_reference,
    }
}

#[derive(Default)]
struct Collector {
    references: BTreeSet<SchemaReference>,
    dynamic_type_reference: bool,
}

impl Collector {
    fn visit_stages(&mut self, stages: &[Stage]) {
        for stage in stages {
            self.visit_stage(stage);
        }
    }

    fn visit_stage(&mut self, stage: &Stage) {
        match stage {
            Stage::Given(given) => {
                for argument in &given.variables {
                    self.visit_named_type_any(&argument.type_);
                }
            }
            Stage::Match(match_) => self.visit_patterns(&match_.patterns),
            Stage::Insert(insert) => self.visit_patterns(&insert.patterns),
            Stage::Put(put) => self.visit_patterns(&put.patterns),
            Stage::Update(update) => self.visit_patterns(&update.patterns),
            Stage::Fetch(fetch) => self.visit_fetch_object_body(&fetch.object.body),
            Stage::Delete(delete) => self.visit_deletables(&delete.deletables),
            Stage::Operator(operator) => self.visit_operator(operator),
        }
    }

    fn visit_patterns(&mut self, patterns: &[Pattern]) {
        for pattern in patterns {
            self.visit_pattern(pattern);
        }
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Conjunction(conjunction) => self.visit_patterns(&conjunction.patterns),
            Pattern::Disjunction(disjunction) => {
                for branch in &disjunction.branches {
                    self.visit_patterns(branch);
                }
            }
            Pattern::Negation(negation) => self.visit_patterns(&negation.patterns),
            Pattern::Optional(optional) => self.visit_patterns(&optional.patterns),
            Pattern::Statement(statement) => self.visit_statement(statement),
        }
    }

    fn visit_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Is(_) => {}
            Statement::InIterable(iterable) => self.visit_expression(&iterable.rhs),
            Statement::Comparison(comparison) => {
                self.visit_expression(&comparison.lhs);
                self.visit_expression(&comparison.comparison.rhs);
            }
            Statement::Assignment(assignment) => self.visit_expression(&assignment.rhs),
            Statement::Thing(thing) => self.visit_thing(thing),
            Statement::Type(type_) => self.visit_type_statement(type_),
        }
    }

    fn visit_thing(&mut self, thing: &typeql::statement::thing::Thing) {
        match &thing.head {
            ThingHead::Variable(_) => {}
            ThingHead::Relation(type_, relation) => {
                if let Some(type_) = type_ {
                    self.visit_type_ref(type_);
                }
                self.visit_relation(relation);
            }
        }

        for constraint in &thing.constraints {
            match constraint {
                ThingConstraint::Isa(isa) => self.visit_type_ref(&isa.type_),
                ThingConstraint::Iid(_) => {}
                ThingConstraint::Has(has) => {
                    if let Some(type_) = &has.type_ {
                        self.visit_type_ref_any(type_);
                    }
                    self.visit_has_value(&has.value);
                }
                ThingConstraint::Links(links) => self.visit_relation(&links.relation),
            }
        }
    }

    fn visit_relation(&mut self, relation: &Relation) {
        for role_player in &relation.role_players {
            match role_player {
                RolePlayer::Typed(type_, _) => self.visit_type_ref_any(type_),
                RolePlayer::Untyped(_) => {}
            }
        }
    }

    fn visit_has_value(&mut self, value: &HasValue) {
        match value {
            HasValue::Variable(_) => {}
            HasValue::Expression(expression) => self.visit_expression(expression),
            HasValue::Comparison(comparison) => self.visit_expression(&comparison.rhs),
        }
    }

    fn visit_type_statement(&mut self, type_: &typeql::statement::type_::Type) {
        self.visit_type_ref(&type_.type_);
        for constraint in &type_.constraints {
            match &constraint.base {
                TypeConstraintBase::Sub(sub) => self.visit_type_ref(&sub.supertype),
                TypeConstraintBase::Label(label) => match label {
                    LabelConstraint::Name(label) => self.insert_label(label),
                    LabelConstraint::Scoped(scoped) => self.insert_scoped_label(scoped),
                },
                TypeConstraintBase::ValueType(value_type) => {
                    self.visit_named_type(&value_type.value_type);
                }
                TypeConstraintBase::Owns(owns) => self.visit_type_ref_any(&owns.owned),
                TypeConstraintBase::Relates(relates) => {
                    self.visit_type_ref_any(&relates.related);
                    if let Some(specialised) = &relates.specialised {
                        self.visit_type_ref_any(specialised);
                    }
                }
                TypeConstraintBase::Plays(plays) => self.visit_type_ref(&plays.role),
            }
        }
    }

    fn visit_expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Variable(_) | Expression::Value(_) => {}
            Expression::ListIndex(index) => self.visit_expression(&index.index),
            Expression::Function(function) => self.visit_function_call(function),
            Expression::Operation(operation) => {
                self.visit_expression(&operation.left);
                self.visit_expression(&operation.right);
            }
            Expression::Paren(paren) => self.visit_expression(&paren.inner),
            Expression::List(list) => {
                for item in &list.items {
                    self.visit_expression(item);
                }
            }
            Expression::ListIndexRange(range) => {
                self.visit_expression(&range.from);
                self.visit_expression(&range.to);
            }
            Expression::ScopedLabel(label) => self.insert_scoped_label(label),
            Expression::Label(label) => self.insert_label(label),
        }
    }

    fn visit_function_call(&mut self, function: &FunctionCall) {
        match &function.name {
            FunctionName::Builtin(_) => {}
            FunctionName::Identifier(identifier) => {
                self.references.insert(SchemaReference::Function(
                    FunctionId::new(identifier.as_str_unchecked())
                        .expect("TypeQL emitted a function identifier rejected by the contract"),
                ));
            }
        }
        for argument in &function.args {
            self.visit_expression(argument);
        }
    }

    fn visit_fetch_object_body(&mut self, body: &FetchObjectBody) {
        match body {
            FetchObjectBody::Entries(entries) => {
                for entry in entries {
                    self.visit_fetch_some(&entry.value);
                }
            }
            FetchObjectBody::AttributesAll(_) => {}
        }
    }

    fn visit_fetch_some(&mut self, fetch: &FetchSome) {
        match fetch {
            FetchSome::Object(object) => self.visit_fetch_object_body(&object.body),
            FetchSome::List(list) => self.visit_fetch_stream(&list.stream),
            FetchSome::Single(single) => self.visit_fetch_single(single),
        }
    }

    fn visit_fetch_single(&mut self, single: &FetchSingle) {
        match single {
            FetchSingle::Attribute(attribute) => self.visit_type_ref_any(&attribute.attribute),
            FetchSingle::Expression(expression) => self.visit_expression(expression),
            FetchSingle::FunctionBlock(block) => {
                self.visit_stages(&block.stages);
                self.visit_return_statement(&block.return_stmt);
            }
        }
    }

    fn visit_fetch_stream(&mut self, stream: &FetchStream) {
        match stream {
            FetchStream::Attribute(attribute) => self.visit_type_ref_any(&attribute.attribute),
            FetchStream::Function(function) => self.visit_function_call(function),
            FetchStream::SubQueryFetch(stages) => self.visit_stages(stages),
            FetchStream::SubQueryFunctionBlock(block) => {
                self.visit_stages(&block.stages);
                self.visit_return_statement(&block.return_stmt);
            }
        }
    }

    fn visit_deletables(&mut self, deletables: &[typeql::query::stage::delete::Deletable]) {
        for deletable in deletables {
            match &deletable.kind {
                DeletableKind::Has { .. } | DeletableKind::Concept { .. } => {}
                DeletableKind::Links { players, .. } => self.visit_relation(players),
                DeletableKind::Optional { deletables } => self.visit_deletables(deletables),
            }
        }
    }

    fn visit_operator(&mut self, operator: &Operator) {
        match operator {
            Operator::Select(_)
            | Operator::Sort(_)
            | Operator::Offset(_)
            | Operator::Limit(_)
            | Operator::Reduce(_)
            | Operator::Require(_)
            | Operator::Distinct(_) => {}
        }
    }

    fn visit_return_statement(&mut self, return_: &ReturnStatement) {
        match return_ {
            ReturnStatement::Stream(_)
            | ReturnStatement::Single(_)
            | ReturnStatement::Reduce(_) => {}
        }
    }

    fn visit_named_type_any(&mut self, type_: &NamedTypeAny) {
        match type_ {
            NamedTypeAny::Simple(type_) => self.visit_named_type(type_),
            NamedTypeAny::List(type_) => self.visit_named_type(&type_.inner),
            NamedTypeAny::Optional(type_) => self.visit_named_type(&type_.inner),
        }
    }

    fn visit_named_type(&mut self, type_: &NamedType) {
        match type_ {
            NamedType::Label(label) => self.insert_label(label),
            NamedType::BuiltinValueType(_) => {}
        }
    }

    fn visit_type_ref_any(&mut self, type_: &TypeRefAny) {
        match type_ {
            TypeRefAny::Type(type_) => self.visit_type_ref(type_),
            TypeRefAny::List(type_) => self.visit_type_ref(&type_.inner),
        }
    }

    fn visit_type_ref(&mut self, type_: &TypeRef) {
        match type_ {
            TypeRef::Label(label) => self.insert_label(label),
            TypeRef::Scoped(scoped) => self.insert_scoped_label(scoped),
            TypeRef::Variable(_) => self.dynamic_type_reference = true,
        }
    }

    fn insert_label(&mut self, label: &TypeqlLabel) {
        self.references.insert(SchemaReference::Label(
            Label::new(label.ident.as_str_unchecked())
                .expect("TypeQL emitted a label rejected by the contract"),
        ));
    }

    fn insert_scoped_label(&mut self, label: &TypeqlScopedLabel) {
        self.references.insert(SchemaReference::Scoped {
            scope: Label::new(label.scope.ident.as_str_unchecked())
                .expect("TypeQL emitted a scope rejected by the contract"),
            name: Label::new(label.name.ident.as_str_unchecked())
                .expect("TypeQL emitted a scoped name rejected by the contract"),
        });
    }
}
