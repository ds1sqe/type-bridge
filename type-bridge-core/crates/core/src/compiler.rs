//! TypeQL query compiler — converts AST nodes ([`QueryNode`](crate::ast::QueryNode),
//! usually [`Clause`](crate::ast::Clause)s) into TypeQL query strings.

#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
use crate::ast::{
    Clause, Constraint, Pattern, QueryNode, Statement, TypedComparisonOperator, TypedFetchRows,
    TypedFunctionArgument, TypedHydrateThings, TypedLiteral, TypedMatchPredicate, TypedMatchTarget,
    TypedMissingOrder, TypedPageRematch, TypedRootScan, TypedScalarOperand, TypedSortDirection,
    Value,
};
#[cfg(test)]
use crate::ast::{FetchItem, LetAssignment, LiteralValue, ReduceAssignment, SortField};

use crate::decimal::parse_decimal;
use crate::tql;
/// Opaque, non-exhaustive error returned by the checked compile and parse
/// entry points ([`QueryCompiler::try_compile`],
/// [`parse_typeql_query_checked`](crate::query_parser::parse_typeql_query_checked)).
pub use crate::tql::TqlError;
impl From<tql::TqlError> for TypedCompileError {
    fn from(value: tql::TqlError) -> Self {
        Self::new(value.to_string())
    }
}

use crate::reserved_words::is_reserved_word;
use std::sync::OnceLock;
use type_bridge_contract::id::is_canonical_thing_iid;
use type_bridge_contract::limits::{MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use unicode_ident::{is_xid_continue, is_xid_start};

/// Return whether a string is one safe, non-reserved TypeQL label.
///
/// This is the canonical guard for metadata interpolated by typed compilers
/// and descriptor registries. TypeQL labels start with an XID-start character
/// or underscore and continue with XID-continue characters or hyphens.
pub fn is_valid_typeql_label(value: &str) -> bool {
    if value.is_empty() || is_reserved_word(value) {
        return false;
    }
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| is_xid_start(first) || first == '_')
        && chars.all(|character| is_xid_continue(character) || character == '-')
}

/// A typed selected-row AST cannot be rendered without losing or inventing
/// canonical match semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedCompileError {
    message: String,
}

impl TypedCompileError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Return the stable compiler diagnostic.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for TypedCompileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TypedCompileError {}

/// One compiler-owned value bound through a TypeQL `given` stage.
///
/// Names are deterministic statement-local identifiers (`g0`, `g1`, ...)
/// without the `$` sigil. Values retain their canonical typed representation
/// until the provider adapter lowers them onto the TypeDB driver.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedQueryParameter {
    /// Deterministic variable name without the `$` sigil.
    pub name: String,
    /// Typed value supplied in the single prepared input row.
    pub value: TypedQueryParameterValue,
}

/// One compiler-owned `given` parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedQueryParameterValue {
    /// A released typed literal parameter.
    Literal(TypedLiteral),
    /// An exact generated projected canonical value.
    Canonical(CanonicalValue),
}

/// One prepared typed statement and its ordered `given`-row values.
///
/// `typeql` begins with a `given` stage whenever `parameters` is non-empty.
/// Temporal, decimal, and duration predicates remain safely inlined because
/// the current portable given-row adapter cannot preserve their complete
/// canonical TypeQL spelling surface.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedTypedStatement {
    /// Complete TypeQL statement, including its generated `given` header.
    pub typeql: String,
    /// One ordered input row whose names match the generated header.
    pub parameters: Vec<TypedQueryParameter>,
}

enum TypedParameterMode {
    Inline,
    Prepared(Vec<TypedQueryParameter>),
}

impl TypedParameterMode {
    fn comparison(
        &mut self,
        field: u16,
        operator: TypedComparisonOperator,
        value: &TypedLiteral,
    ) -> Result<tql::Pattern, TypedCompileError> {
        let operand = match self {
            Self::Prepared(parameters) if prepared_typed_value(operator, value).is_some() => {
                let name = format!("g{}", parameters.len());
                parameters.push(TypedQueryParameter {
                    name: name.clone(),
                    value: TypedQueryParameterValue::Literal(value.clone()),
                });
                tql::var(&name)?
            }
            _ => {
                let value = match (operator, value) {
                    (TypedComparisonOperator::StartsWith, TypedLiteral::String(v)) => {
                        TypedLiteral::String(format!("^{}.*", regex::escape(v)))
                    }
                    (TypedComparisonOperator::EndsWith, TypedLiteral::String(v)) => {
                        TypedLiteral::String(format!(".*{}$", regex::escape(v)))
                    }
                    _ => value.clone(),
                };
                tql::typed_literal(&value)?
            }
        };
        Ok(tql::compare(
            tql::var(&field_variable(field))?,
            comparison_token(operator),
            operand,
        )?)
    }
    fn canonical(&mut self, value: &CanonicalValue) -> Result<tql::Expr, TypedCompileError> {
        let Self::Prepared(parameters) = self else {
            return Err(TypedCompileError::new(
                "schema-function projected values require provider given-row support",
            ));
        };
        let name = format!("g{}", parameters.len());
        parameters.push(TypedQueryParameter {
            name: name.clone(),
            value: TypedQueryParameterValue::Canonical(value.clone()),
        });
        Ok(tql::var(&name)?)
    }
    fn render(&self, mut stages: Vec<tql::Stage>) -> Result<String, TypedCompileError> {
        if let Self::Prepared(parameters) = self
            && !parameters.is_empty()
        {
            let columns = parameters
                .iter()
                .map(|p| {
                    let tag = match &p.value {
                        TypedQueryParameterValue::Canonical(v) => v.value_type(),
                        TypedQueryParameterValue::Literal(v) => match v {
                            TypedLiteral::String(_) => ValueTypeTag::String,
                            TypedLiteral::Long(_) => ValueTypeTag::Long,
                            TypedLiteral::Double(_) => ValueTypeTag::Double,
                            TypedLiteral::Boolean(_) => ValueTypeTag::Boolean,
                            _ => unreachable!("unsupported given values stay inline"),
                        },
                    };
                    (p.name.as_str(), tag)
                })
                .collect::<Vec<_>>();
            stages.insert(0, tql::given(&columns)?);
        }
        Ok(tql::render(&tql::pipeline(stages))?)
    }
    fn finish(self, typeql: String) -> PreparedTypedStatement {
        let Self::Prepared(parameters) = self else {
            unreachable!()
        };
        PreparedTypedStatement { typeql, parameters }
    }
}

/// Compiles AST nodes into TypeQL. It is the only TypeQL renderer for the
/// untyped AST; bindings send their nodes here instead of rendering them.
pub struct QueryCompiler {}

impl Default for QueryCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryCompiler {
    /// Create a new compiler instance.
    pub fn new() -> Self {
        QueryCompiler {}
    }

    /// Compile legacy clauses or stage fragments using released compatibility behavior.
    ///
    /// Non-`Has` relation attributes are discarded, as in the released renderer.
    /// Other unrenderable structured inputs return an empty string. Caller-owned
    /// `Raw` text is emitted verbatim. Use [`Self::try_compile`] to receive errors.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile(&self, clauses: &[Clause]) -> String {
        tql::compile_legacy_released(clauses)
    }

    /// Compile one canonical typed selected-row statement.
    ///
    /// All variables are derived from numeric binding/field IDs at this final
    /// render boundary. Invalid references fail instead of falling back to raw
    /// TypeQL fragments.
    pub fn compile_typed_fetch_rows(
        &self,
        query: &TypedFetchRows,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_fetch_rows(query, &mut parameters)
    }

    /// Prepare one canonical typed selected-row statement for TypeDB 3.12's
    /// `given` transport.
    ///
    /// Supported literal operands become one deterministic input row. Temporal,
    /// decimal, and duration operands remain inlined until the portable driver
    /// contract represents their complete canonical TypeQL surface.
    pub fn prepare_typed_fetch_rows(
        &self,
        query: &TypedFetchRows,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_fetch_rows(query, &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    /// Compile a provider-bounded scan over distinct public tuple identities.
    ///
    /// Unlike [`Self::compile_typed_fetch_rows`], this intentionally projects
    /// only `query.projection`. It is a cardinality/window proof, not complete
    /// provider evidence; callers must run a separate complete-evidence query
    /// before materializing a successful result.
    pub fn compile_typed_tuple_scan(
        &self,
        query: &TypedFetchRows,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_tuple_scan(query, &mut parameters)
    }

    /// Prepare a distinct public-tuple scan for TypeDB 3.12 `given` transport.
    pub fn prepare_typed_tuple_scan(
        &self,
        query: &TypedFetchRows,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_tuple_scan(query, &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    fn render_typed_tuple_scan(
        &self,
        query: &TypedFetchRows,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_fetch_rows(query)?;
        if !query.distinct || !query.order.is_empty() || query.offset != 0 || query.limit != 2 {
            return Err(TypedCompileError::new(
                "typed tuple scan requires distinct unordered identities at offset zero and limit two",
            ));
        }
        let mut patterns = typed_target_patterns(&query.targets)?;
        if let Some(predicate) = &query.predicate {
            patterns.extend(self.render_typed_predicate(
                &query.targets,
                &query.fields,
                predicate,
                parameters,
                &mut 0,
            )?);
        }
        let mut stages = vec![tql::match_stage(patterns)];
        if query.predicate.as_ref().is_some_and(contains_reachable) {
            append_reachability_reduce(
                &mut stages,
                query.targets.iter().map(|t| binding_variable(t.binding)),
            )?;
        }
        let names = query
            .projection
            .iter()
            .map(|b| binding_variable(*b))
            .collect::<Vec<_>>();
        stages.push(tql::select(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )?);
        stages.push(tql::distinct());
        stages.push(tql::limit(query.limit));
        parameters.render(stages)
    }

    fn render_typed_fetch_rows(
        &self,
        query: &TypedFetchRows,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_fetch_rows(query)?;
        let mut patterns = typed_target_patterns(&query.targets)?;
        for id in query
            .order
            .iter()
            .map(|o| o.field)
            .collect::<std::collections::BTreeSet<_>>()
        {
            patterns.push(typed_has(typed_field(&query.fields, id)?)?);
        }
        if let Some(predicate) = &query.predicate {
            patterns.extend(self.render_typed_predicate(
                &query.targets,
                &query.fields,
                predicate,
                parameters,
                &mut 0,
            )?);
        }
        let mut stages = vec![tql::match_stage(patterns)];
        let mut names = query
            .targets
            .iter()
            .map(|t| binding_variable(t.binding))
            .collect::<Vec<_>>();
        names.extend(query.order.iter().map(|o| field_variable(o.field)));
        names.dedup();
        if query.predicate.as_ref().is_some_and(contains_reachable) {
            append_reachability_reduce(&mut stages, names.iter().cloned())?;
        }
        stages.push(tql::select(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )?);
        if query.distinct {
            stages.push(tql::distinct());
        }
        append_typed_order(&mut stages, &query.order)?;
        if query.offset > 0 {
            stages.push(tql::offset(query.offset));
        }
        stages.push(tql::limit(query.limit));
        parameters.render(stages)
    }

    /// Compile one typed distinct-root stream.
    pub fn compile_typed_root_scan(
        &self,
        query: &TypedRootScan,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_root_scan(query, &mut parameters)
    }

    /// Prepare one typed distinct-root stream with deterministic `given`
    /// parameters for every supported predicate literal.
    pub fn prepare_typed_root_scan(
        &self,
        query: &TypedRootScan,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_root_scan(query, &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    fn render_typed_root_scan(
        &self,
        query: &TypedRootScan,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_root_scan(query)?;
        let mut patterns = typed_target_patterns(&query.targets)?;
        for id in query
            .order
            .iter()
            .map(|o| o.field)
            .collect::<std::collections::BTreeSet<_>>()
        {
            patterns.push(typed_has(typed_field(&query.fields, id)?)?);
        }
        if let Some(predicate) = &query.predicate {
            patterns.extend(self.render_typed_predicate(
                &query.targets,
                &query.fields,
                predicate,
                parameters,
                &mut 0,
            )?);
        }
        let mut names = vec![binding_variable(query.root)];
        names.extend(query.order.iter().map(|o| field_variable(o.field)));
        names.dedup();
        let mut stages = vec![tql::match_stage(patterns)];
        if query.predicate.as_ref().is_some_and(contains_reachable) {
            let mut environment = query
                .targets
                .iter()
                .map(|t| binding_variable(t.binding))
                .collect::<Vec<_>>();
            environment.extend(query.order.iter().map(|o| field_variable(o.field)));
            append_reachability_reduce(&mut stages, environment)?;
        }
        stages.push(tql::select(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )?);
        stages.push(tql::distinct());
        append_typed_order(&mut stages, &query.order)?;
        if let Some(n) = query.offset.filter(|n| *n > 0) {
            stages.push(tql::offset(n));
        }
        if let Some(n) = query.limit {
            stages.push(tql::limit(n));
        }
        parameters.render(stages)
    }

    /// Compile one exact root-IID batch re-match with complete graph hydration.
    pub fn compile_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_page_rematch(query, None, &mut parameters)
    }

    /// Compile one exact root-IID page re-match with a provider-owned answer
    /// ceiling before its document fetch.
    pub fn compile_typed_page_rematch_bounded(
        &self,
        query: &TypedPageRematch,
        limit: u64,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_page_rematch(query, Some(limit), &mut parameters)
    }

    /// Prepare one exact root-IID page re-match with deterministic `given`
    /// parameters for the original graph predicate.
    pub fn prepare_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_page_rematch(query, None, &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    /// Prepare one exact root-IID page re-match with a provider-owned answer
    /// ceiling before its document fetch.
    pub fn prepare_typed_page_rematch_bounded(
        &self,
        query: &TypedPageRematch,
        limit: u64,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_page_rematch(query, Some(limit), &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    fn render_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
        limit: Option<u64>,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_page_rematch(query)?;
        let mut patterns = typed_target_patterns(&query.targets)?;
        if let Some(predicate) = &query.predicate {
            patterns.extend(self.render_typed_predicate(
                &query.targets,
                &query.fields,
                predicate,
                parameters,
                &mut 0,
            )?);
        }
        patterns.push(tql::or(
            query
                .root_concept_ids
                .iter()
                .map(|id| {
                    Ok(vec![tql::thing(
                        &binding_variable(query.root),
                        vec![tql::iid(id)?],
                    )?])
                })
                .collect::<Result<_, tql::TqlError>>()?,
        ));
        for t in &query.targets {
            patterns.push(tql::thing(
                &binding_variable(t.binding),
                vec![tql::isa(&type_variable(t.binding), true)?],
            )?);
        }
        let mut stages = vec![tql::match_stage(patterns)];
        if query.predicate.as_ref().is_some_and(contains_reachable) {
            let mut names = query
                .targets
                .iter()
                .map(|t| binding_variable(t.binding))
                .collect::<Vec<_>>();
            names.extend(query.targets.iter().map(|t| type_variable(t.binding)));
            append_reachability_reduce(&mut stages, names)?;
        }
        if let Some(n) = limit {
            stages.push(tql::limit(n));
        }
        let bindings = query
            .targets
            .iter()
            .map(|t| {
                Ok((
                    format!("b{}", t.binding),
                    typed_thing_fetch(
                        &binding_variable(t.binding),
                        &type_variable(t.binding),
                        t.kind,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>, TypedCompileError>>()?;
        stages.push(tql::fetch(
            bindings
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect(),
        )?);
        parameters.render(stages)
    }

    /// Compile one canonical batched IID hydration statement.
    ///
    /// This is one kind-homogeneous provider statement for a complete batch.
    /// Entity batches never contain a `links` pattern; relation batches bind
    /// every IID through its declared relation type before role hydration.
    /// IID values are grammar-checked before the final render boundary.
    pub fn compile_typed_hydrate_things(
        &self,
        query: &TypedHydrateThings,
    ) -> Result<String, TypedCompileError> {
        if query.targets.is_empty() {
            return Err(TypedCompileError::new(
                "typed hydration requires at least one identity",
            ));
        }
        let batch_kind = query.targets[0].kind;
        if query.targets.iter().any(|target| target.kind != batch_kind) {
            return Err(TypedCompileError::new(
                "typed hydration batch must contain exactly one thing kind",
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        let mut bindings = std::collections::BTreeSet::new();
        for target in &query.targets {
            if !is_valid_typeql_label(&target.declared_type) {
                return Err(TypedCompileError::new(
                    "typed hydration contains an invalid declared type label",
                ));
            }
            if !bindings.insert(target.binding)
                || target.concept_ids.is_empty()
                || target.concrete_descriptors.is_empty()
            {
                return Err(TypedCompileError::new(
                    "typed hydration targets must be unique and complete",
                ));
            }
            for descriptor in &target.concrete_descriptors {
                if !is_valid_typeql_label(&descriptor.type_name)
                    || descriptor
                        .fields
                        .iter()
                        .any(|field| !is_valid_typeql_label(&field.attribute_type))
                    || descriptor.roles.iter().any(|role| {
                        !is_valid_typeql_label(&role.role_name)
                            || role
                                .player_types
                                .iter()
                                .any(|player| !is_valid_typeql_label(player))
                    })
                {
                    return Err(TypedCompileError::new(
                        "typed hydration contains invalid descriptor labels",
                    ));
                }
                if descriptor.kind != target.kind
                    || (descriptor.kind == crate::ast::TypedThingKind::Entity
                        && !descriptor.roles.is_empty())
                {
                    return Err(TypedCompileError::new(
                        "typed hydration descriptor metadata has an incompatible kind",
                    ));
                }
            }
            for concept_id in &target.concept_ids {
                if !identities.insert((target.binding, concept_id.as_str())) {
                    return Err(TypedCompileError::new(
                        "typed hydration contains a duplicate binding/IID pair",
                    ));
                }
                if !valid_typed_iid(concept_id) {
                    return Err(TypedCompileError::new(
                        "typed hydration contains an invalid provider IID",
                    ));
                }
            }
        }

        let answer_limit = u64::try_from(identities.len()).map_err(|_| {
            TypedCompileError::new(
                "typed hydration identity count exceeds the provider limit range",
            )
        })?;
        let mut branches = Vec::new();
        for target in &query.targets {
            for id in &target.concept_ids {
                branches.push(vec![
                    tql::thing("thing", vec![tql::iid(id)?])?,
                    tql::thing("thing", vec![tql::isa(&target.declared_type, false)?])?,
                    tql::let_value(
                        &["binding"],
                        tql::typed_literal(&TypedLiteral::Long(i64::from(target.binding)))?,
                        false,
                    )?,
                ]);
            }
        }
        let patterns = vec![
            tql::or(branches),
            tql::thing("thing", vec![tql::isa("$type", true)?])?,
        ];
        let mut entries = typed_thing_entries("thing", "type", batch_kind)?;
        entries.insert(0, ("binding".into(), tql::fetch_expr(tql::var("binding")?)));
        let stages = vec![
            tql::match_stage(patterns),
            tql::limit(answer_limit),
            tql::fetch(
                entries
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect(),
            )?,
        ];
        TypedParameterMode::Inline.render(stages)
    }

    fn render_typed_predicate(
        &self,
        targets: &[TypedMatchTarget],
        fields: &[crate::ast::TypedFieldBinding],
        predicate: &TypedMatchPredicate,
        parameters: &mut TypedParameterMode,
        next_reachability: &mut usize,
    ) -> Result<Vec<tql::Pattern>, TypedCompileError> {
        Ok(match predicate {
            TypedMatchPredicate::FieldValue {
                field,
                operator,
                value,
            } => {
                let field = typed_field(fields, *field)?;
                vec![
                    typed_has(field)?,
                    parameters.comparison(field.id, *operator, value)?,
                ]
            }
            TypedMatchPredicate::FieldComparison {
                left,
                operator,
                right,
            } => {
                let left = typed_field(fields, *left)?;
                let right = typed_field(fields, *right)?;
                vec![
                    typed_has(left)?,
                    typed_has(right)?,
                    tql::compare(
                        tql::var(&field_variable(left.id))?,
                        comparison_token(*operator),
                        tql::var(&field_variable(right.id))?,
                    )?,
                ]
            }
            TypedMatchPredicate::FunctionComparison {
                calls,
                left,
                operator,
                right,
            } => {
                let mut patterns = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for operand in [left, right] {
                    if let TypedScalarOperand::Field { field } = operand
                        && seen.insert(*field)
                    {
                        patterns.push(typed_has(typed_field(fields, *field)?)?);
                    }
                }
                for call in calls {
                    let arguments = call
                        .arguments
                        .iter()
                        .map(|a| match a {
                            TypedFunctionArgument::Binding { binding } => {
                                Ok(tql::var(&binding_variable(*binding))?)
                            }
                            TypedFunctionArgument::Value { value } => parameters.canonical(value),
                            TypedFunctionArgument::CallResult { call } => {
                                Ok(tql::var(&function_variable(*call))?)
                            }
                        })
                        .collect::<Result<Vec<_>, TypedCompileError>>()?;
                    patterns.push(tql::let_value(
                        &[&function_variable(call.result)],
                        tql::function(&call.function, arguments)?,
                        false,
                    )?);
                }
                patterns.push(tql::compare(
                    render_typed_scalar_operand(left, parameters)?,
                    comparison_token(*operator),
                    render_typed_scalar_operand(right, parameters)?,
                )?);
                patterns
            }
            TypedMatchPredicate::FieldPresence { field, present } => {
                let absent = tql::not(vec![typed_has(typed_field(fields, *field)?)?]);
                // Double negation keeps presence existential without multiplying owners.
                vec![if *present {
                    tql::not(vec![absent])
                } else {
                    absent
                }]
            }
            TypedMatchPredicate::BindingIid { binding, iid } => vec![tql::thing(
                &binding_variable(*binding),
                vec![tql::iid(iid)?],
            )?],
            TypedMatchPredicate::RoleEdge {
                relation,
                role_name,
                player,
                ..
            } => vec![tql::thing(
                &binding_variable(*relation),
                vec![tql::links(vec![(role_name, &binding_variable(*player))])?],
            )?],
            TypedMatchPredicate::Reachable {
                relation_type,
                role_from,
                role_to,
                source,
                target,
                min_depth,
                max_depth,
            } => {
                let id = *next_reachability;
                *next_reachability = next_reachability.saturating_add(1);
                let source_target = typed_target(targets, *source)?;
                let witness = format!("$R{id}z");
                let mut branches = Vec::new();
                for length in usize::from(*min_depth)..=usize::from(*max_depth) {
                    if length == 0 {
                        // Anchor the identity branch for TypeDB's disjunction planner.
                        branches.push(vec![
                            tql::thing(&witness, vec![tql::isa(&source_target.type_name, false)?])?,
                            tql::identity(&binding_variable(*source), &witness)?,
                            tql::identity(&binding_variable(*target), &witness)?,
                        ]);
                        continue;
                    }
                    let mut hops = Vec::new();
                    for hop in 1..=length {
                        // Fresh names keep intermediates local to each path length.
                        let from = if hop == 1 {
                            binding_variable(*source)
                        } else {
                            format!("$R{id}l{length}h{}", hop - 1)
                        };
                        let to = if hop == length {
                            binding_variable(*target)
                        } else {
                            format!("$R{id}l{length}h{hop}")
                        };
                        hops.push(tql::anonymous_relation(
                            vec![(role_from, &from), (role_to, &to)],
                            vec![tql::isa(relation_type, true)?],
                        )?);
                    }
                    branches.push(hops);
                }
                vec![tql::or(branches)]
            }
            TypedMatchPredicate::And { expressions } => {
                let mut values = Vec::new();
                for child in expressions {
                    values.extend(self.render_typed_predicate(
                        targets,
                        fields,
                        child,
                        parameters,
                        next_reachability,
                    )?);
                }
                values
            }
            TypedMatchPredicate::Or { expressions } => vec![tql::or(
                expressions
                    .iter()
                    .map(|child| {
                        self.render_typed_predicate(
                            targets,
                            fields,
                            child,
                            parameters,
                            next_reachability,
                        )
                    })
                    .collect::<Result<_, _>>()?,
            )],
            TypedMatchPredicate::Not { expression } => {
                vec![tql::not(self.render_typed_predicate(
                    targets,
                    fields,
                    expression,
                    parameters,
                    next_reachability,
                )?)]
            }
        })
    }

    /// Compile legacy clauses or stage fragments, rejecting malformed structured nodes.
    ///
    /// Deprecated caller-owned `Raw` text remains a verbatim escape hatch.
    pub fn try_compile(&self, clauses: &[Clause]) -> Result<String, TqlError> {
        crate::tql::compile_legacy(clauses)
    }

    /// Compile one legacy node, rejecting invalid or unrepresentable nodes.
    pub fn try_compile_node(&self, node: &QueryNode) -> Result<String, TqlError> {
        crate::tql::compile_legacy_node(node)
    }

    /// Compile one legacy node with released compatibility behavior.
    /// Non-Has relation attributes are dropped; other invalid shapes return empty output.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_node(&self, node: &QueryNode) -> String {
        tql::compile_legacy_node_released(node)
    }

    /// Compile one legacy clause. Invalid shapes use the released compatibility behavior.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_clause(&self, clause: &Clause) -> String {
        self.compile(std::slice::from_ref(clause))
    }

    /// Compile one legacy pattern. Invalid shapes use the released compatibility behavior.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_pattern(&self, pattern: &Pattern) -> String {
        self.compile_node(&QueryNode::Pattern(pattern.clone()))
    }

    /// Compile one legacy statement. Invalid shapes use the released compatibility behavior.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_statement(&self, statement: &Statement) -> String {
        self.compile_node(&QueryNode::Statement(statement.clone()))
    }

    /// Compile one legacy constraint. Invalid shapes use the released compatibility behavior.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_constraint(&self, constraint: &Constraint) -> String {
        self.compile_node(&QueryNode::Constraint(constraint.clone()))
    }

    /// Compile one legacy value. Invalid shapes use the released compatibility behavior.
    #[deprecated(
        since = "2.3.0",
        note = "Use typed query plans or try_compile; removed in 3.0."
    )]
    pub fn compile_value(&self, value: &Value) -> String {
        self.compile_node(&QueryNode::Value(value.clone()))
    }

    #[cfg(test)]
    fn format_literal(&self, literal: &LiteralValue) -> String {
        self.compile_value(&Value::Literal(literal.clone()))
    }
}

fn binding_variable(binding: u16) -> String {
    format!("$b{binding}")
}

fn field_variable(field: u16) -> String {
    format!("$f{field}")
}

fn function_variable(call: u16) -> String {
    format!("$c{call}")
}

fn render_typed_scalar_operand(
    operand: &TypedScalarOperand,
    parameters: &mut TypedParameterMode,
) -> Result<tql::Expr, TypedCompileError> {
    match operand {
        TypedScalarOperand::Field { field } => Ok(tql::var(&field_variable(*field))?),
        TypedScalarOperand::Value { value } => parameters.canonical(value),
        TypedScalarOperand::CallResult { call } => Ok(tql::var(&function_variable(*call))?),
    }
}

fn type_variable(binding: u16) -> String {
    format!("$t{binding}")
}

fn contains_reachable(predicate: &TypedMatchPredicate) -> bool {
    match predicate {
        TypedMatchPredicate::Reachable { .. } => true,
        TypedMatchPredicate::And { expressions } | TypedMatchPredicate::Or { expressions } => {
            expressions.iter().any(contains_reachable)
        }
        TypedMatchPredicate::Not { expression } => contains_reachable(expression),
        TypedMatchPredicate::FieldValue { .. }
        | TypedMatchPredicate::FieldComparison { .. }
        | TypedMatchPredicate::FunctionComparison { .. }
        | TypedMatchPredicate::FieldPresence { .. }
        | TypedMatchPredicate::BindingIid { .. }
        | TypedMatchPredicate::RoleEdge { .. } => false,
    }
}

fn append_reachability_reduce(
    stages: &mut Vec<tql::Stage>,
    variables: impl IntoIterator<Item = String>,
) -> Result<(), TypedCompileError> {
    let mut seen = std::collections::BTreeSet::new();
    let names = variables
        .into_iter()
        .filter(|v| seen.insert(v.clone()))
        .collect::<Vec<_>>();
    debug_assert!(!names.is_empty());
    stages.push(tql::count(
        "RreachableProofCount",
        None,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
    )?);
    Ok(())
}
fn typed_target_patterns(
    targets: &[TypedMatchTarget],
) -> Result<Vec<tql::Pattern>, TypedCompileError> {
    targets
        .iter()
        .map(|t| {
            Ok(tql::thing(
                &binding_variable(t.binding),
                vec![tql::isa(&t.type_name, t.exact)?],
            )?)
        })
        .collect()
}
fn typed_has(field: &crate::ast::TypedFieldBinding) -> Result<tql::Pattern, TypedCompileError> {
    Ok(tql::thing(
        &binding_variable(field.owner),
        vec![tql::has(
            &field.field_name,
            tql::var(&field_variable(field.id))?,
        )?],
    )?)
}
fn append_typed_order(
    stages: &mut Vec<tql::Stage>,
    order: &[crate::ast::TypedMatchOrder],
) -> Result<(), TypedCompileError> {
    if !order.is_empty() {
        let fields = order
            .iter()
            .map(|o| {
                (
                    field_variable(o.field),
                    matches!(o.direction, TypedSortDirection::Descending),
                )
            })
            .collect::<Vec<_>>();
        stages.push(tql::sort(
            &fields
                .iter()
                .map(|(n, d)| (n.as_str(), *d))
                .collect::<Vec<_>>(),
        )?);
    }
    Ok(())
}
fn typed_thing_entries(
    binding: &str,
    type_name: &str,
    kind: crate::ast::TypedThingKind,
) -> Result<Vec<(String, tql::FetchValue)>, TypedCompileError> {
    let mut entries = vec![
        (
            "concept_id".into(),
            tql::fetch_expr(tql::function("iid", vec![tql::var(binding)?])?),
        ),
        (
            "concrete_type".into(),
            tql::fetch_expr(tql::function("label", vec![tql::var(type_name)?])?),
        ),
        ("attributes".into(), tql::fetch_attributes(binding)?),
    ];
    if kind == crate::ast::TypedThingKind::Relation {
        let patterns = vec![
            tql::thing(binding, vec![tql::links(vec![("$role", "$player")])?])?,
            tql::thing("player", vec![tql::isa("$player_type", true)?])?,
        ];
        let fetch = tql::fetch(vec![
            (
                "role",
                tql::fetch_expr(tql::function("label", vec![tql::var("role")?])?),
            ),
            (
                "player_concept_id",
                tql::fetch_expr(tql::function("iid", vec![tql::var("player")?])?),
            ),
            (
                "player_concrete_type",
                tql::fetch_expr(tql::function("label", vec![tql::var("player_type")?])?),
            ),
            ("attributes", tql::fetch_attributes("player")?),
        ])?;
        entries.push((
            "roles".into(),
            tql::fetch_subquery(vec![tql::match_stage(patterns), fetch]),
        ));
    }
    Ok(entries)
}
fn typed_thing_fetch(
    binding: &str,
    type_name: &str,
    kind: crate::ast::TypedThingKind,
) -> Result<tql::FetchValue, TypedCompileError> {
    let entries = typed_thing_entries(binding, type_name, kind)?;
    Ok(tql::fetch_object(
        entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect(),
    )?)
}

fn valid_typed_iid(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn typed_field(
    fields: &[crate::ast::TypedFieldBinding],
    field: u16,
) -> Result<&crate::ast::TypedFieldBinding, TypedCompileError> {
    fields
        .iter()
        .find(|candidate| candidate.id == field)
        .ok_or_else(|| TypedCompileError::new(format!("unknown typed field ID {field}")))
}

fn typed_target(
    targets: &[TypedMatchTarget],
    binding: u16,
) -> Result<&TypedMatchTarget, TypedCompileError> {
    targets
        .iter()
        .find(|candidate| candidate.binding == binding)
        .ok_or_else(|| TypedCompileError::new(format!("unknown typed target binding {binding}")))
}

fn validate_typed_fetch_rows(query: &TypedFetchRows) -> Result<(), TypedCompileError> {
    if query.targets.is_empty() || query.projection.is_empty() {
        return Err(TypedCompileError::new(
            "typed selected-row query requires targets and projection",
        ));
    }
    if query.limit == 0 {
        return Err(TypedCompileError::new(
            "typed selected-row query requires a positive limit",
        ));
    }
    if query
        .targets
        .iter()
        .any(|target| !is_valid_typeql_label(&target.type_name))
    {
        return Err(TypedCompileError::new(
            "typed match target contains an invalid TypeQL label",
        ));
    }
    if query
        .fields
        .iter()
        .any(|field| !is_valid_typeql_label(&field.field_name))
    {
        return Err(TypedCompileError::new(
            "typed match field contains an invalid TypeQL label",
        ));
    }
    let targets = query
        .targets
        .iter()
        .map(|target| target.binding)
        .collect::<std::collections::BTreeSet<_>>();
    if targets.len() != query.targets.len() {
        return Err(TypedCompileError::new(
            "typed selected-row targets contain duplicate binding IDs",
        ));
    }
    if query
        .projection
        .iter()
        .any(|binding| !targets.contains(binding))
    {
        return Err(TypedCompileError::new(
            "typed selected-row projection references an unknown binding",
        ));
    }
    if query
        .projection
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != query.projection.len()
    {
        return Err(TypedCompileError::new(
            "typed selected-row projection contains duplicate bindings",
        ));
    }
    let fields = query
        .fields
        .iter()
        .map(|field| field.id)
        .collect::<std::collections::BTreeSet<_>>();
    if fields.len() != query.fields.len()
        || query
            .fields
            .iter()
            .any(|field| !targets.contains(&field.owner))
    {
        return Err(TypedCompileError::new(
            "typed selected-row fields contain duplicate IDs or unknown owners",
        ));
    }
    if query
        .order
        .iter()
        .any(|order| !fields.contains(&order.field))
    {
        return Err(TypedCompileError::new(
            "typed selected-row order references an unknown field",
        ));
    }
    if query
        .order
        .iter()
        .any(|order| order.missing != TypedMissingOrder::Reject)
    {
        return Err(TypedCompileError::new(
            "typed selected-row compiler cannot preserve missing-value ordering",
        ));
    }
    if let Some(predicate) = &query.predicate {
        validate_typed_predicate(predicate, &targets, &fields, query)?;
    }
    Ok(())
}

fn validate_typed_root_scan(query: &TypedRootScan) -> Result<(), TypedCompileError> {
    if query.limit == Some(0) {
        return Err(TypedCompileError::new(
            "typed distinct-root scan requires a positive limit when bounded",
        ));
    }
    if query.offset.is_some() && query.limit.is_none() {
        return Err(TypedCompileError::new(
            "typed distinct-root offset requires a bounded limit",
        ));
    }
    let graph = TypedFetchRows {
        targets: query.targets.clone(),
        fields: query.fields.clone(),
        predicate: query.predicate.clone(),
        projection: vec![query.root],
        distinct: true,
        order: query.order.clone(),
        offset: query.offset.unwrap_or_default(),
        limit: query.limit.unwrap_or(1),
    };
    validate_typed_fetch_rows(&graph)
}

fn validate_typed_page_rematch(query: &TypedPageRematch) -> Result<(), TypedCompileError> {
    if query.root_concept_ids.is_empty() {
        return Err(TypedCompileError::new(
            "typed page re-match requires at least one root IID",
        ));
    }
    if query
        .root_concept_ids
        .iter()
        .any(|iid| !valid_typed_iid(iid))
    {
        return Err(TypedCompileError::new(
            "typed page re-match contains an invalid root IID",
        ));
    }
    if query
        .root_concept_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != query.root_concept_ids.len()
    {
        return Err(TypedCompileError::new(
            "typed page re-match contains duplicate root IIDs",
        ));
    }
    let graph = TypedFetchRows {
        targets: query.targets.clone(),
        fields: query.fields.clone(),
        predicate: query.predicate.clone(),
        projection: vec![query.root],
        distinct: true,
        order: Vec::new(),
        offset: 0,
        limit: 1,
    };
    validate_typed_fetch_rows(&graph)?;
    let targets = query
        .targets
        .iter()
        .map(|target| target.binding)
        .collect::<std::collections::BTreeSet<_>>();
    let fields = query
        .fields
        .iter()
        .map(|field| field.id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut collections = std::collections::BTreeSet::new();
    for collection in &query.collection_orders {
        if !targets.contains(&collection.binding)
            || !collections.insert(collection.binding)
            || collection.order.is_empty()
            || collection
                .order
                .iter()
                .any(|order| !fields.contains(&order.field))
        {
            return Err(TypedCompileError::new(
                "typed page re-match contains an invalid collection order",
            ));
        }
    }
    Ok(())
}

fn validate_typed_predicate(
    predicate: &TypedMatchPredicate,
    targets: &std::collections::BTreeSet<u16>,
    fields: &std::collections::BTreeSet<u16>,
    query: &TypedFetchRows,
) -> Result<(), TypedCompileError> {
    let mut nodes = 0_usize;
    inspect_typed_predicate_structure(predicate, 1, true, &mut nodes)?;
    validate_typed_predicate_semantics(predicate, targets, fields, query)
}

fn inspect_typed_predicate_structure(
    predicate: &TypedMatchPredicate,
    depth: usize,
    reachable_allowed: bool,
    nodes: &mut usize,
) -> Result<(), TypedCompileError> {
    *nodes = (*nodes).saturating_add(1);
    if depth > MAX_PREDICATE_DEPTH || *nodes > MAX_PREDICATE_NODES {
        return Err(TypedCompileError::new(
            "typed predicate exceeds canonical size or depth limits",
        ));
    }
    match predicate {
        TypedMatchPredicate::Reachable {
            min_depth,
            max_depth,
            ..
        } => {
            if !reachable_allowed {
                return Err(TypedCompileError::new(
                    "typed reachability is admitted only in the root conjunction",
                ));
            }
            if min_depth > max_depth {
                return Err(TypedCompileError::new(
                    "typed reachability minimum depth exceeds its maximum depth",
                ));
            }
            if usize::from(*max_depth) > MAX_PREDICATE_DEPTH {
                return Err(TypedCompileError::new(
                    "typed reachability exceeds the canonical depth ceiling",
                ));
            }
            let first_positive = usize::from((*min_depth).max(1));
            let maximum = usize::from(*max_depth);
            let expanded_hops = if first_positive <= maximum {
                (first_positive..=maximum).fold(0_usize, usize::saturating_add)
            } else {
                0
            };
            let expanded = expanded_hops.saturating_add(usize::from(*min_depth == 0));
            *nodes = (*nodes).saturating_add(expanded.saturating_sub(1));
            if *nodes > MAX_PREDICATE_NODES {
                return Err(TypedCompileError::new(
                    "typed reachability expansion exceeds the canonical predicate-node ceiling",
                ));
            }
        }
        TypedMatchPredicate::And { expressions } => {
            for child in expressions {
                inspect_typed_predicate_structure(
                    child,
                    depth.saturating_add(1),
                    reachable_allowed,
                    nodes,
                )?;
            }
        }
        TypedMatchPredicate::Or { expressions } => {
            for child in expressions {
                inspect_typed_predicate_structure(child, depth.saturating_add(1), false, nodes)?;
            }
        }
        TypedMatchPredicate::Not { expression } => {
            inspect_typed_predicate_structure(expression, depth.saturating_add(1), false, nodes)?
        }
        TypedMatchPredicate::FieldValue { .. }
        | TypedMatchPredicate::FieldComparison { .. }
        | TypedMatchPredicate::FunctionComparison { .. }
        | TypedMatchPredicate::FieldPresence { .. }
        | TypedMatchPredicate::BindingIid { .. }
        | TypedMatchPredicate::RoleEdge { .. } => {}
    }
    Ok(())
}

fn validate_typed_predicate_semantics(
    predicate: &TypedMatchPredicate,
    targets: &std::collections::BTreeSet<u16>,
    fields: &std::collections::BTreeSet<u16>,
    query: &TypedFetchRows,
) -> Result<(), TypedCompileError> {
    match predicate {
        TypedMatchPredicate::FieldValue { field, value, .. } => {
            if !fields.contains(field) {
                return Err(TypedCompileError::new(format!(
                    "typed predicate references unknown field ID {field}"
                )));
            }
            validate_typed_literal(value)?;
        }
        TypedMatchPredicate::FieldComparison { left, right, .. } => {
            if !fields.contains(left) || !fields.contains(right) {
                return Err(TypedCompileError::new(
                    "typed field comparison references an unknown field",
                ));
            }
        }
        TypedMatchPredicate::FunctionComparison {
            calls, left, right, ..
        } => {
            let mut prior = std::collections::BTreeSet::new();
            for (index, call) in calls.iter().enumerate() {
                let expected = u16::try_from(index).map_err(|_| {
                    TypedCompileError::new("typed function call ordinal exceeds u16")
                })?;
                if call.result != expected || !is_valid_typeql_label(&call.function) {
                    return Err(TypedCompileError::new(
                        "typed function calls require dense results and valid schema labels",
                    ));
                }
                for argument in &call.arguments {
                    match argument {
                        TypedFunctionArgument::Binding { binding } => {
                            if !targets.contains(binding) {
                                return Err(TypedCompileError::new(
                                    "typed function argument references an unknown binding",
                                ));
                            }
                        }
                        TypedFunctionArgument::Value { .. } => {}
                        TypedFunctionArgument::CallResult { call } => {
                            if !prior.contains(call) {
                                return Err(TypedCompileError::new(
                                    "typed function call dependency is not prior and acyclic",
                                ));
                            }
                        }
                    }
                }
                prior.insert(call.result);
            }
            let validate_operand = |operand: &TypedScalarOperand| match operand {
                TypedScalarOperand::Field { field } if fields.contains(field) => Ok(()),
                TypedScalarOperand::Value { .. } => Ok(()),
                TypedScalarOperand::CallResult { call } if prior.contains(call) => Ok(()),
                _ => Err(TypedCompileError::new(
                    "typed function comparison references an unknown scalar operand",
                )),
            };
            validate_operand(left)?;
            validate_operand(right)?;
            if !matches!(left, TypedScalarOperand::CallResult { .. })
                && !matches!(right, TypedScalarOperand::CallResult { .. })
            {
                return Err(TypedCompileError::new(
                    "typed function comparison must consume a function call result",
                ));
            }
        }
        TypedMatchPredicate::FieldPresence { field, .. } => {
            if !fields.contains(field) {
                return Err(TypedCompileError::new(format!(
                    "typed field-presence predicate references unknown field ID {field}"
                )));
            }
        }
        TypedMatchPredicate::BindingIid { binding, iid } => {
            if !targets.contains(binding) {
                return Err(TypedCompileError::new(format!(
                    "typed IID predicate references unknown binding ID {binding}"
                )));
            }
            if !is_canonical_thing_iid(iid) {
                return Err(TypedCompileError::new(
                    "typed IID predicate contains a non-canonical thing IID",
                ));
            }
        }
        TypedMatchPredicate::RoleEdge {
            relation,
            role_name,
            player,
            ..
        } => {
            if !is_valid_typeql_label(role_name) {
                return Err(TypedCompileError::new(
                    "typed role edge contains an invalid TypeQL label",
                ));
            }
            if !targets.contains(relation) || !targets.contains(player) {
                return Err(TypedCompileError::new(
                    "typed role edge references an unknown binding",
                ));
            }
            if !query.targets.iter().any(|target| {
                target.binding == *relation && target.kind == crate::ast::TypedThingKind::Relation
            }) {
                return Err(TypedCompileError::new(
                    "typed role edge source is not a relation target",
                ));
            }
        }
        TypedMatchPredicate::Reachable {
            relation_type,
            role_from,
            role_to,
            source,
            target,
            ..
        } => {
            if !is_valid_typeql_label(relation_type)
                || !is_valid_typeql_label(role_from)
                || !is_valid_typeql_label(role_to)
            {
                return Err(TypedCompileError::new(
                    "typed reachability contains an invalid TypeQL label",
                ));
            }
            if !targets.contains(source) || !targets.contains(target) {
                return Err(TypedCompileError::new(
                    "typed reachability references an unknown endpoint binding",
                ));
            }
        }
        TypedMatchPredicate::And { expressions } | TypedMatchPredicate::Or { expressions } => {
            if expressions.is_empty() {
                return Err(TypedCompileError::new(
                    "typed boolean predicate cannot be empty",
                ));
            }
            for child in expressions {
                validate_typed_predicate_semantics(child, targets, fields, query)?;
            }
        }
        TypedMatchPredicate::Not { expression } => {
            validate_typed_predicate_semantics(expression, targets, fields, query)?
        }
    }
    Ok(())
}

fn validate_typed_literal(value: &TypedLiteral) -> Result<(), TypedCompileError> {
    let (kind, valid) = match value {
        TypedLiteral::String(_) | TypedLiteral::Long(_) | TypedLiteral::Boolean(_) => {
            return Ok(());
        }
        TypedLiteral::Double(value) => ("double", value.is_finite()),
        TypedLiteral::Date(value) => ("date", valid_typeql_date(value)),
        TypedLiteral::DateTime(value) => ("datetime", valid_typeql_datetime(value)),
        TypedLiteral::DateTimeTz(value) => ("datetime-tz", valid_typeql_datetime_tz(value)),
        TypedLiteral::Decimal(value) => ("decimal", parse_decimal(value).is_some()),
        TypedLiteral::Duration(value) => ("duration", valid_typeql_duration(value)),
    };
    if valid {
        Ok(())
    } else {
        Err(TypedCompileError::new(format!(
            "typed predicate contains an invalid canonical {kind} literal"
        )))
    }
}

fn valid_typeql_date(value: &str) -> bool {
    static DATE: OnceLock<regex::Regex> = OnceLock::new();
    let captures = DATE
        .get_or_init(|| {
            regex::Regex::new(
                r"^(?P<year>(?:[0-9]{4}|[+-][0-9]{1,6}))-(?P<month>[0-9]{2})-(?P<day>[0-9]{2})$",
            )
            .expect("typed date regex is valid")
        })
        .captures(value);
    let Some(captures) = captures else {
        return false;
    };
    let Some(year) = captures
        .name("year")
        .and_then(|year| year.as_str().parse::<i32>().ok())
    else {
        return false;
    };
    // TypeDB's documented date range is 262144 BCE through 262142 CE.
    if !(-262_144..=262_142).contains(&year) {
        return false;
    }
    let Some(month) = captures
        .name("month")
        .and_then(|month| month.as_str().parse::<u32>().ok())
    else {
        return false;
    };
    let Some(day) = captures
        .name("day")
        .and_then(|day| day.as_str().parse::<u32>().ok())
    else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day >= 1 && day <= days[(month - 1) as usize]
}

fn valid_typeql_datetime(value: &str) -> bool {
    let Some((date, clock)) = value.split_once('T') else {
        return false;
    };
    valid_typeql_date(date) && valid_typeql_clock(clock)
}

fn valid_typeql_datetime_tz(value: &str) -> bool {
    let Some((date, zoned_clock)) = value.split_once('T') else {
        return false;
    };
    if !valid_typeql_date(date) {
        return false;
    }

    if let Some(clock) = zoned_clock.strip_suffix('Z') {
        return valid_typeql_clock(clock);
    }
    if let Some((clock, zone)) = zoned_clock.split_once(' ') {
        return valid_typeql_clock(clock) && valid_typeql_iana_zone(zone);
    }
    let Some(offset_index) = zoned_clock
        .char_indices()
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))
    else {
        return false;
    };
    let (clock, offset) = zoned_clock.split_at(offset_index);
    valid_typeql_clock(clock) && valid_typeql_offset(offset)
}

fn valid_typeql_clock(value: &str) -> bool {
    static CLOCK: OnceLock<regex::Regex> = OnceLock::new();
    let captures = CLOCK
        .get_or_init(|| {
            regex::Regex::new(
                r"^(?P<hour>[0-9]{2}):(?P<minute>[0-9]{2})(?::(?P<second>[0-9]{2})(?:\.(?P<fraction>[0-9]{1,9}))?)?$",
            )
            .expect("typed clock regex is valid")
        })
        .captures(value);
    let Some(captures) = captures else {
        return false;
    };
    let Some(hour) = captures
        .name("hour")
        .and_then(|hour| hour.as_str().parse::<u32>().ok())
    else {
        return false;
    };
    let Some(minute) = captures
        .name("minute")
        .and_then(|minute| minute.as_str().parse::<u32>().ok())
    else {
        return false;
    };
    let second = captures
        .name("second")
        .and_then(|second| second.as_str().parse::<u32>().ok())
        .unwrap_or_default();
    if hour > 24 || minute > 59 || second > 59 {
        return false;
    }
    hour != 24
        || (minute == 0
            && second == 0
            && captures
                .name("fraction")
                .is_none_or(|fraction| fraction.as_str().bytes().all(|digit| digit == b'0')))
}

fn valid_typeql_offset(value: &str) -> bool {
    static OFFSET: OnceLock<regex::Regex> = OnceLock::new();
    let captures = OFFSET
        .get_or_init(|| {
            regex::Regex::new(r"^[+-](?P<hour>[0-9]{2})(?::?(?P<minute>[0-9]{2}))?$")
                .expect("typed timezone offset regex is valid")
        })
        .captures(value);
    let Some(captures) = captures else {
        return false;
    };
    let Some(hour) = captures
        .name("hour")
        .and_then(|hour| hour.as_str().parse::<u32>().ok())
    else {
        return false;
    };
    let minute = captures
        .name("minute")
        .and_then(|minute| minute.as_str().parse::<u32>().ok())
        .unwrap_or_default();
    hour <= 23 && minute <= 59
}

fn valid_typeql_iana_zone(value: &str) -> bool {
    static IANA_ZONE: OnceLock<regex::Regex> = OnceLock::new();
    IANA_ZONE
        .get_or_init(|| {
            regex::Regex::new(r"^[A-Za-z][A-Za-z0-9._+-]*(?:/[A-Za-z0-9._+-]+)*$")
                .expect("typed IANA timezone regex is valid")
        })
        .is_match(value)
}

fn valid_typeql_duration(value: &str) -> bool {
    static DURATION: OnceLock<regex::Regex> = OnceLock::new();
    if value == "P" || value == "PT" || value.ends_with('T') {
        return false;
    }
    DURATION
        .get_or_init(|| {
            regex::Regex::new(
                r"^P(?:[0-9]+W|(?:[0-9]+Y)?(?:[0-9]+M)?(?:[0-9]+D)?(?:T(?:[0-9]+H)?(?:[0-9]+M)?(?:[0-9]+(?:\.[0-9]{1,9})?S)?)?)$",
            )
            .expect("typed duration regex is valid")
        })
        .is_match(value)
}

fn prepared_typed_value(
    operator: TypedComparisonOperator,
    value: &TypedLiteral,
) -> Option<TypedLiteral> {
    match (operator, value) {
        (
            TypedComparisonOperator::StartsWith
            | TypedComparisonOperator::EndsWith
            | TypedComparisonOperator::Regex,
            _,
        ) => None,
        (
            _,
            TypedLiteral::Date(_)
            | TypedLiteral::DateTime(_)
            | TypedLiteral::DateTimeTz(_)
            | TypedLiteral::Decimal(_)
            | TypedLiteral::Duration(_),
        ) => None,
        _ => Some(value.clone()),
    }
}

fn comparison_token(operator: TypedComparisonOperator) -> &'static str {
    match operator {
        TypedComparisonOperator::Equal => "==",
        TypedComparisonOperator::NotEqual => "!=",
        TypedComparisonOperator::LessThan => "<",
        TypedComparisonOperator::LessThanOrEqual => "<=",
        TypedComparisonOperator::GreaterThan => ">",
        TypedComparisonOperator::GreaterThanOrEqual => ">=",
        TypedComparisonOperator::Contains => "contains",
        TypedComparisonOperator::StartsWith
        | TypedComparisonOperator::EndsWith
        | TypedComparisonOperator::Regex => "like",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        ArithmeticValue, FunctionCallValue, RolePlayer, TypedFieldBinding, TypedHydrateThings,
        TypedHydrationDescriptor, TypedHydrationField, TypedHydrationRole, TypedHydrationTarget,
        TypedMatchOrder, TypedMatchTarget, TypedMissingOrder, TypedThingKind,
    };
    use serde_json::json;
    fn compile_typed_value_comparison(
        field: u16,
        operator: TypedComparisonOperator,
        value: &TypedLiteral,
    ) -> String {
        let pattern = TypedParameterMode::Inline
            .comparison(field, operator, value)
            .unwrap();
        crate::tql::render_pattern(&pattern).unwrap()
    }

    #[test]
    fn typed_typeql_labels_reject_injection_and_reserved_words() {
        for valid in ["person", "first-name", "employee_name", "Ångström"] {
            assert!(
                is_valid_typeql_label(valid),
                "expected valid label: {valid}"
            );
        }
        for invalid in [
            "",
            "123person",
            "two words",
            "person; match $x isa thing",
            "person\"",
            "person.role",
            "match",
            "ENTITY",
        ] {
            assert!(
                !is_valid_typeql_label(invalid),
                "expected invalid label: {invalid}"
            );
        }
    }

    fn compiler() -> QueryCompiler {
        QueryCompiler::new()
    }

    fn typed_literal_query(value: TypedLiteral) -> TypedFetchRows {
        typed_predicate_query(TypedMatchPredicate::FieldValue {
            field: 0,
            operator: TypedComparisonOperator::Equal,
            value,
        })
    }

    fn typed_predicate_query(predicate: TypedMatchPredicate) -> TypedFetchRows {
        TypedFetchRows {
            targets: vec![TypedMatchTarget {
                binding: 0,
                kind: TypedThingKind::Entity,
                type_name: "person".into(),
                exact: false,
            }],
            fields: vec![TypedFieldBinding {
                id: 0,
                owner: 0,
                field_name: "score".into(),
            }],
            predicate: Some(predicate),
            projection: vec![0],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 1,
        }
    }

    #[test]
    fn schema_function_calls_require_prepared_canonical_inputs_and_lower_in_dependency_order() {
        let query = typed_predicate_query(TypedMatchPredicate::FunctionComparison {
            calls: vec![
                crate::ast::TypedFunctionCall {
                    result: 0,
                    function: "qualifying-score".into(),
                    arguments: vec![
                        TypedFunctionArgument::Binding { binding: 0 },
                        TypedFunctionArgument::Value {
                            value: CanonicalValue::Long(40),
                        },
                    ],
                },
                crate::ast::TypedFunctionCall {
                    result: 1,
                    function: "identity-score".into(),
                    arguments: vec![TypedFunctionArgument::CallResult { call: 0 }],
                },
            ],
            left: TypedScalarOperand::CallResult { call: 1 },
            operator: TypedComparisonOperator::Equal,
            right: TypedScalarOperand::Field { field: 0 },
        });

        assert!(
            compiler().compile_typed_fetch_rows(&query).is_err(),
            "projected values never fall back to inline TypeQL"
        );
        let prepared = compiler().prepare_typed_fetch_rows(&query).unwrap();
        assert_eq!(
            prepared.parameters,
            vec![TypedQueryParameter {
                name: "g0".into(),
                value: TypedQueryParameterValue::Canonical(CanonicalValue::Long(40)),
            }]
        );
        assert!(prepared.typeql.starts_with("given $g0: integer; match "));
        assert!(prepared.typeql.contains("$b0 has score $f0"));
        assert!(prepared.typeql.contains(
            "let $c0 = qualifying-score($b0, $g0); let $c1 = identity-score($c0); $c1 == $f0"
        ));
    }

    #[test]
    fn typed_presence_and_iid_predicates_compile_without_raw_fragments() {
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(
                    TypedMatchPredicate::FieldPresence {
                        field: 0,
                        present: true,
                    },
                ))
                .unwrap(),
            "match $b0 isa person; not { not { $b0 has score $f0; \n}; \n}; select $b0; distinct; limit 1;"
        );
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(
                    TypedMatchPredicate::FieldPresence {
                        field: 0,
                        present: false,
                    },
                ))
                .unwrap(),
            "match $b0 isa person; not { $b0 has score $f0; \n}; select $b0; distinct; limit 1;"
        );
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(TypedMatchPredicate::BindingIid {
                    binding: 0,
                    iid: "0xAb12".into(),
                },))
                .unwrap(),
            "match $b0 isa person; $b0 iid 0xAb12; select $b0; distinct; limit 1;"
        );

        let error = compiler()
            .compile_typed_fetch_rows(&typed_predicate_query(TypedMatchPredicate::BindingIid {
                binding: 0,
                iid: "not-an-iid".into(),
            }))
            .unwrap_err();
        assert_eq!(
            error.message(),
            "typed IID predicate contains a non-canonical thing IID"
        );
    }

    fn lit(value: serde_json::Value, value_type: &str) -> Value {
        Value::Literal(LiteralValue {
            value,
            value_type: value_type.to_string(),
        })
    }

    // ── Literal formatting ──────────────────────────────────────────────

    #[test]
    fn test_literal_string() {
        let c = compiler();
        let l = LiteralValue {
            value: json!("hello"),
            value_type: "string".into(),
        };
        assert_eq!(c.format_literal(&l), "\"hello\"");
    }

    #[test]
    fn test_literal_string_escapes() {
        let c = compiler();
        let l = LiteralValue {
            value: json!("a\"b\\c\nd"),
            value_type: "string".into(),
        };
        assert_eq!(c.format_literal(&l), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn test_literal_boolean() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(true),
                value_type: "boolean".into()
            }),
            "true"
        );
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(false),
                value_type: "boolean".into()
            }),
            "false"
        );
    }

    #[test]
    fn test_literal_long() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(42),
                value_type: "long".into()
            }),
            "42"
        );
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(-7),
                value_type: "long".into()
            }),
            "-7"
        );
    }

    #[test]
    fn test_literal_double() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(3.15),
                value_type: "double".into()
            }),
            "3.15"
        );
    }

    #[test]
    fn test_literal_decimal() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(42),
                value_type: "decimal".into()
            }),
            "42dec"
        );
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!(3.15),
                value_type: "decimal".into()
            }),
            "3.15dec"
        );
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("10.25"),
                value_type: "decimal".into()
            }),
            "10.25dec"
        );
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("10.25dec"),
                value_type: "decimal".into()
            }),
            "10.25dec"
        );
    }

    #[test]
    fn test_literal_date() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("2024-01-15"),
                value_type: "date".into()
            }),
            "2024-01-15"
        );
    }

    #[test]
    fn test_literal_datetime() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("2024-01-15T10:30:00"),
                value_type: "datetime".into()
            }),
            "2024-01-15T10:30:00"
        );
    }

    #[test]
    fn test_literal_datetime_tz() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("2024-01-15T10:30:00+09:00"),
                value_type: "datetime-tz".into()
            }),
            "2024-01-15T10:30:00+09:00"
        );
    }

    #[test]
    fn test_literal_duration() {
        let c = compiler();
        assert_eq!(
            c.format_literal(&LiteralValue {
                value: json!("P1Y2M3D"),
                value_type: "duration".into()
            }),
            "P1Y2M3D"
        );
    }

    // ── Values ──────────────────────────────────────────────────────────

    #[test]
    fn test_value_variable() {
        let c = compiler();
        assert_eq!(c.compile_value(&Value::Variable("$x".into())), "$x");
    }

    #[test]
    fn test_value_function_call() {
        let c = compiler();
        let v = Value::FunctionCall(FunctionCallValue {
            function: "count".into(),
            args: vec![Value::Variable("$p".into())],
        });
        assert_eq!(c.compile_value(&v), "count($p)");
    }

    #[test]
    fn test_value_function_call_multiple_args() {
        let c = compiler();
        let v = Value::FunctionCall(FunctionCallValue {
            function: "max".into(),
            args: vec![Value::Variable("$a".into()), Value::Variable("$b".into())],
        });
        assert_eq!(c.compile_value(&v), "max($a, $b)");
    }

    #[test]
    fn test_value_arithmetic() {
        let c = compiler();
        let v = Value::Arithmetic(ArithmeticValue {
            left: Box::new(Value::Variable("$x".into())),
            operator: "+".into(),
            right: Box::new(lit(json!(1), "long")),
        });
        assert_eq!(c.compile_value(&v), "($x + 1)");
    }

    #[test]
    fn test_value_nested_arithmetic() {
        let c = compiler();
        let v = Value::Arithmetic(ArithmeticValue {
            left: Box::new(Value::Arithmetic(ArithmeticValue {
                left: Box::new(Value::Variable("$a".into())),
                operator: "+".into(),
                right: Box::new(Value::Variable("$b".into())),
            })),
            operator: "*".into(),
            right: Box::new(lit(json!(2), "long")),
        });
        assert_eq!(c.compile_value(&v), "(($a + $b) * 2)");
    }

    // ── Constraints ─────────────────────────────────────────────────────

    #[test]
    fn test_constraint_iid() {
        let c = compiler();
        assert_eq!(
            c.compile_constraint(&Constraint::Iid("0xabc".into())),
            "iid 0xabc"
        );
    }

    #[test]
    fn test_constraint_has() {
        let c = compiler();
        let con = Constraint::Has {
            attr_name: "age".into(),
            value: lit(json!(30), "long"),
        };
        assert_eq!(c.compile_constraint(&con), "has age 30");
    }

    #[test]
    fn test_constraint_isa() {
        let c = compiler();
        assert_eq!(
            c.compile_constraint(&Constraint::Isa {
                type_name: "person".into(),
                strict: false
            }),
            "isa person"
        );
        assert_eq!(
            c.compile_constraint(&Constraint::Isa {
                type_name: "person".into(),
                strict: true
            }),
            "isa! person"
        );
    }

    // ── Patterns ────────────────────────────────────────────────────────

    #[test]
    fn test_pattern_entity_no_constraints() {
        let c = compiler();
        let p = Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![],
            is_strict: false,
        };
        assert_eq!(c.compile_pattern(&p), "$p isa person");
    }

    #[test]
    fn test_pattern_entity_strict() {
        let c = compiler();
        let p = Pattern::Entity {
            variable: "$e".into(),
            type_name: "employee".into(),
            constraints: vec![],
            is_strict: true,
        };
        assert_eq!(c.compile_pattern(&p), "$e isa! employee");
    }

    #[test]
    fn test_pattern_entity_multiple_constraints() {
        let c = compiler();
        let p = Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![
                Constraint::Has {
                    attr_name: "name".into(),
                    value: lit(json!("Alice"), "string"),
                },
                Constraint::Has {
                    attr_name: "age".into(),
                    value: lit(json!(30), "long"),
                },
            ],
            is_strict: false,
        };
        assert_eq!(
            c.compile_pattern(&p),
            "$p isa person, has name \"Alice\", has age 30"
        );
    }

    #[test]
    fn test_pattern_relation() {
        let c = compiler();
        let p = Pattern::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![
                RolePlayer {
                    role: "employee".into(),
                    player_var: "$p".into(),
                },
                RolePlayer {
                    role: "employer".into(),
                    player_var: "$c".into(),
                },
            ],
            constraints: vec![],
        };
        assert_eq!(
            c.compile_pattern(&p),
            "$r isa employment (employee: $p, employer: $c)"
        );
    }

    #[test]
    fn test_pattern_relation_with_constraint() {
        let c = compiler();
        let p = Pattern::Relation {
            variable: "$r".into(),
            type_name: "friendship".into(),
            role_players: vec![RolePlayer {
                role: "friend".into(),
                player_var: "$a".into(),
            }],
            constraints: vec![Constraint::Has {
                attr_name: "since".into(),
                value: lit(json!("2024-01-01"), "date"),
            }],
        };
        assert_eq!(
            c.compile_pattern(&p),
            "$r isa friendship (friend: $a), has since 2024-01-01"
        );
    }

    #[test]
    fn test_pattern_subtype() {
        let c = compiler();
        let p = Pattern::SubType {
            variable: "$t".into(),
            parent_type: "entity".into(),
        };
        assert_eq!(c.compile_pattern(&p), "$t sub entity");
    }

    #[test]
    fn test_pattern_attribute_without_value() {
        let c = compiler();
        let p = Pattern::Attribute {
            variable: "$a".into(),
            type_name: "name".into(),
            value: None,
        };
        assert_eq!(c.compile_pattern(&p), "$a isa name");
    }

    #[test]
    fn test_pattern_attribute_with_value() {
        let c = compiler();
        let p = Pattern::Attribute {
            variable: "$a".into(),
            type_name: "name".into(),
            value: Some(lit(json!("John"), "string")),
        };
        assert_eq!(c.compile_pattern(&p), "$a isa name \"John\"");
    }

    #[test]
    fn test_pattern_has() {
        let c = compiler();
        let p = Pattern::Has {
            thing_var: "$p".into(),
            attr_type: "name".into(),
            attr_var: "$n".into(),
        };
        assert_eq!(c.compile_pattern(&p), "$p has name $n");
    }

    #[test]
    fn test_pattern_value_comparison_all_operators() {
        let c = compiler();
        for (op, expected) in [
            (">", "$x > 5"),
            ("<", "$x < 5"),
            (">=", "$x >= 5"),
            ("<=", "$x <= 5"),
            ("==", "$x == 5"),
            ("!=", "$x != 5"),
        ] {
            let p = Pattern::ValueComparison {
                var: "$x".into(),
                operator: op.into(),
                value: lit(json!(5), "long"),
            };
            assert_eq!(c.compile_pattern(&p), expected);
        }
    }

    #[test]
    fn test_pattern_not() {
        let c = compiler();
        let p = Pattern::Not(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![],
            is_strict: false,
        }]);
        assert_eq!(c.compile_pattern(&p), "not { $p isa person; \n}");
    }

    #[test]
    fn test_pattern_or() {
        let c = compiler();
        let p = Pattern::Or(vec![
            vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "a".into(),
                constraints: vec![],
                is_strict: false,
            }],
            vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "b".into(),
                constraints: vec![],
                is_strict: false,
            }],
        ]);
        assert_eq!(c.compile_pattern(&p), "{ $x isa a; } or { $x isa b; }");
    }

    #[test]
    fn test_pattern_or_three_alternatives() {
        let c = compiler();
        let p = Pattern::Or(vec![
            vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "a".into(),
                constraints: vec![],
                is_strict: false,
            }],
            vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "b".into(),
                constraints: vec![],
                is_strict: false,
            }],
            vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "c".into(),
                constraints: vec![],
                is_strict: false,
            }],
        ]);
        assert_eq!(
            c.compile_pattern(&p),
            "{ $x isa a; } or { $x isa b; } or { $x isa c; }"
        );
    }

    #[test]
    fn test_pattern_iid() {
        let c = compiler();
        let p = Pattern::Iid {
            variable: "$p".into(),
            iid: "0x1234".into(),
        };
        assert_eq!(c.compile_pattern(&p), "$p iid 0x1234");
    }

    #[test]
    fn test_pattern_raw() {
        let c = compiler();
        let p = Pattern::Raw("some raw content".into());
        assert_eq!(c.compile_pattern(&p), "some raw content");
    }

    // ── Statements ──────────────────────────────────────────────────────

    #[test]
    fn test_statement_has() {
        let c = compiler();
        let s = Statement::Has {
            subject_var: "$p".into(),
            attr_name: "name".into(),
            value: lit(json!("Alice"), "string"),
        };
        assert_eq!(c.compile_statement(&s), "$p has name \"Alice\"");
    }

    #[test]
    fn test_statement_isa() {
        let c = compiler();
        let s = Statement::Isa {
            variable: "$p".into(),
            type_name: "person".into(),
        };
        assert_eq!(c.compile_statement(&s), "$p isa person");
    }

    #[test]
    fn test_statement_relation_with_variable() {
        let c = compiler();
        let s = Statement::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![
                RolePlayer {
                    role: "employee".into(),
                    player_var: "$p".into(),
                },
                RolePlayer {
                    role: "employer".into(),
                    player_var: "$c".into(),
                },
            ],
            include_variable: true,
            attributes: vec![],
        };
        assert_eq!(
            c.compile_statement(&s),
            "$r isa employment, links (employee: $p, employer: $c)"
        );
    }

    #[test]
    fn test_statement_relation_without_variable() {
        let c = compiler();
        let s = Statement::Relation {
            variable: "".into(),
            type_name: "employment".into(),
            role_players: vec![RolePlayer {
                role: "employee".into(),
                player_var: "$p".into(),
            }],
            include_variable: false,
            attributes: vec![],
        };
        assert_eq!(c.compile_statement(&s), "(employee: $p) isa employment");
    }

    #[test]
    fn test_statement_relation_with_attributes() {
        let c = compiler();
        let s = Statement::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![RolePlayer {
                role: "employee".into(),
                player_var: "$p".into(),
            }],
            include_variable: true,
            attributes: vec![Statement::Has {
                subject_var: "$r".into(),
                attr_name: "start-date".into(),
                value: lit(json!("2024-01-01"), "date"),
            }],
        };
        assert_eq!(
            c.compile_statement(&s),
            "$r isa employment, links (employee: $p), has start-date 2024-01-01"
        );
    }

    #[test]
    fn test_statement_delete_thing() {
        let c = compiler();
        let s = Statement::DeleteThing("$p".into());
        assert_eq!(c.compile_statement(&s), "$p");
    }

    #[test]
    fn test_statement_raw() {
        let c = compiler();
        let s = Statement::Raw("raw statement".into());
        assert_eq!(c.compile_statement(&s), "raw statement");
    }

    // ── Clauses ─────────────────────────────────────────────────────────

    #[test]
    fn test_clause_match() {
        let c = compiler();
        let clause = Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![],
            is_strict: false,
        }]);
        assert_eq!(c.compile_clause(&clause), "match $p isa person;");
    }

    #[test]
    fn test_clause_match_multiple_patterns() {
        let c = compiler();
        let clause = Clause::Match(vec![
            Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Has {
                thing_var: "$p".into(),
                attr_type: "name".into(),
                attr_var: "$n".into(),
            },
        ]);
        assert_eq!(
            c.compile_clause(&clause),
            "match $p isa person; $p has name $n;"
        );
    }

    #[test]
    fn test_clause_insert() {
        let c = compiler();
        let clause = Clause::Insert(vec![Statement::Has {
            subject_var: "$p".into(),
            attr_name: "name".into(),
            value: lit(json!("Alice"), "string"),
        }]);
        assert_eq!(c.compile_clause(&clause), "insert $p has name \"Alice\";");
    }

    #[test]
    fn test_clause_put() {
        let c = compiler();
        let clause = Clause::Put(vec![
            Statement::Isa {
                variable: "$p".into(),
                type_name: "person".into(),
            },
            Statement::Has {
                subject_var: "$p".into(),
                attr_name: "name".into(),
                value: lit(json!("Alice"), "string"),
            },
        ]);
        assert_eq!(
            c.compile_clause(&clause),
            "put $p isa person; $p has name \"Alice\";"
        );
    }

    #[test]
    fn test_clause_delete() {
        let c = compiler();
        let clause = Clause::Delete(vec![Statement::DeleteThing("$p".into())]);
        assert_eq!(c.compile_clause(&clause), "delete $p;");
    }

    #[test]
    fn test_clause_update() {
        let c = compiler();
        let clause = Clause::Update(vec![Statement::Has {
            subject_var: "$p".into(),
            attr_name: "age".into(),
            value: lit(json!(31), "long"),
        }]);
        assert_eq!(c.compile_clause(&clause), "update $p has age == 31;");
    }

    #[test]
    fn test_clause_fetch() {
        let c = compiler();
        let clause = Clause::Fetch(vec![
            FetchItem::Attribute {
                key: "name".into(),
                var: "$p".into(),
                attr_name: "name".into(),
            },
            FetchItem::Variable {
                key: "person".into(),
                var: "$p".into(),
            },
        ]);
        assert_eq!(
            c.compile_clause(&clause),
            "fetch { \"name\": $p.name, \"person\": $p};"
        );
    }

    #[test]
    fn test_clause_fetch_all_item_types() {
        let c = compiler();
        let clause = Clause::Fetch(vec![
            FetchItem::Attribute {
                key: "name".into(),
                var: "$p".into(),
                attr_name: "name".into(),
            },
            FetchItem::Variable {
                key: "person".into(),
                var: "$p".into(),
            },
            FetchItem::AttributeList {
                key: "emails".into(),
                var: "$p".into(),
                attr_name: "email".into(),
            },
            FetchItem::Function {
                key: "count".into(),
                func_name: "count".into(),
                var: "$p".into(),
            },
            FetchItem::Wildcard {
                key: "all".into(),
                var: "$p".into(),
            },
            FetchItem::NestedWildcard {
                key: "nested".into(),
                var: "$p".into(),
            },
        ]);
        let result = c.compile_clause(&clause);
        assert!(result.contains("\"name\": $p.name"));
        assert!(result.contains("\"person\": $p"));
        assert!(result.contains("\"emails\": [ $p.email]"));
        assert!(result.contains("\"count\": count($p)"));
        assert!(result.contains("\"all\": {  $p.*}"));
        assert!(result.contains("\"nested\": {  $p.*}"));
    }

    #[test]
    fn test_fetch_raw_item_passes_through() {
        let c = compiler();
        let clause = Clause::Fetch(vec![
            FetchItem::Raw("\"raw_name\": $p.person-name".into()),
            FetchItem::Variable {
                key: "person".into(),
                var: "$p".into(),
            },
        ]);
        assert_eq!(
            c.compile_clause(&clause),
            "fetch { \"raw_name\": $p.person-name, \"person\": $p };"
        );
        let wire: FetchItem = serde_json::from_value(json!({"Raw": "\"k\": $p.*"})).unwrap();
        assert_eq!(wire, FetchItem::Raw("\"k\": $p.*".into()));
    }

    /// Decode a node from the tagged form bindings send, then compile it.
    fn compile_wire_node(node: serde_json::Value) -> String {
        let node: QueryNode = serde_json::from_value(node).expect("query node wire form");
        compiler().compile_node(&node)
    }

    #[test]
    fn test_node_clause() {
        let rendered = compile_wire_node(json!({"Clause": {"Match": [
            {"type": "Entity", "data": {
                "variable": "$p", "type_name": "person", "constraints": [], "is_strict": false
            }}
        ]}}));
        assert_eq!(rendered, "match $p isa person;");
    }

    #[test]
    fn test_node_pattern() {
        let rendered = compile_wire_node(json!({"Pattern": {"type": "Entity", "data": {
            "variable": "$p",
            "type_name": "person",
            "constraints": [{"type": "Has", "data": {
                "attr_name": "name",
                "value": {"type": "Literal", "data": {"value": "Alice", "value_type": "string"}}
            }}],
            "is_strict": true
        }}}));
        assert_eq!(rendered, "$p isa! person, has name \"Alice\"");
    }

    #[test]
    fn test_node_statement() {
        let rendered = compile_wire_node(json!({"Statement": {"type": "Relation", "data": {
            "variable": "$r",
            "type_name": "employment",
            "role_players": [{"role": "employee", "player_var": "$p"}],
            "include_variable": false,
            "attributes": [{"type": "Has", "data": {
                "subject_var": "$r",
                "attr_name": "since",
                "value": {"type": "Literal", "data": {"value": "2024-01-02", "value_type": "date"}}
            }}]
        }}}));
        assert_eq!(
            rendered,
            "(employee: $p) isa employment, has since 2024-01-02"
        );
    }

    #[test]
    fn test_node_constraint() {
        let rendered = compile_wire_node(json!({"Constraint": {"type": "Has", "data": {
            "attr_name": "score",
            "value": {"type": "Literal", "data": {"value": "1.50", "value_type": "decimal"}}
        }}}));
        assert_eq!(rendered, "has score 1.50dec");
    }

    #[test]
    fn test_node_value() {
        let rendered = compile_wire_node(json!({"Value": {"type": "Arithmetic", "data": {
            "left": {"type": "Variable", "data": "$a"},
            "operator": "+",
            "right": {"type": "FunctionCall", "data": {
                "function": "abs",
                "args": [{"type": "Literal", "data": {"value": -2, "value_type": "long"}}]
            }}
        }}}));
        assert_eq!(rendered, "($a + abs(-2))");
    }

    #[test]
    fn test_clause_match_let() {
        let c = compiler();
        let clause = Clause::MatchLet(vec![LetAssignment {
            variables: vec!["$x".into()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "count".into(),
                args: vec![Value::Variable("$p".into())],
            }),
            is_stream: false,
        }]);
        assert_eq!(c.compile_clause(&clause), "match let $x = count($p);");
    }

    #[test]
    fn test_clause_match_let_stream() {
        let c = compiler();
        let clause = Clause::MatchLet(vec![LetAssignment {
            variables: vec!["$x".into()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "fetch_items".into(),
                args: vec![Value::Variable("$p".into())],
            }),
            is_stream: true,
        }]);
        assert_eq!(
            c.compile_clause(&clause),
            "match let $x in fetch_items($p);"
        );
    }

    #[test]
    fn test_clause_reduce() {
        let c = compiler();
        let clause = Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".into(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable("$p".into())],
                }),
            }],
            group_by: None,
        };
        assert_eq!(c.compile_clause(&clause), "reduce $count = count($p);");
    }

    #[test]
    fn test_clause_reduce_with_groupby() {
        let c = compiler();
        let clause = Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".into(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable("$p".into())],
                }),
            }],
            group_by: Some("$city".into()),
        };
        assert_eq!(
            c.compile_clause(&clause),
            "reduce $count = count($p) groupby $city;"
        );
    }

    #[test]
    fn test_clause_reduce_multiple_assignments() {
        let c = compiler();
        let clause = Clause::Reduce {
            assignments: vec![
                ReduceAssignment {
                    variable: "$count".into(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: "count".into(),
                        args: vec![Value::Variable("$p".into())],
                    }),
                },
                ReduceAssignment {
                    variable: "$sum".into(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: "sum".into(),
                        args: vec![Value::Variable("$a".into())],
                    }),
                },
            ],
            group_by: None,
        };
        assert_eq!(
            c.compile_clause(&clause),
            "reduce $count = count($p), $sum = sum($a);"
        );
    }

    #[test]
    fn test_clause_sort_single() {
        let c = compiler();
        let clause = Clause::Sort(vec![SortField {
            variable: "$age".into(),
            ascending: true,
        }]);
        assert_eq!(c.compile_clause(&clause), "sort $age asc;");
    }

    #[test]
    fn test_clause_sort_multiple() {
        let c = compiler();
        let clause = Clause::Sort(vec![
            SortField {
                variable: "$name".into(),
                ascending: true,
            },
            SortField {
                variable: "$age".into(),
                ascending: false,
            },
        ]);
        assert_eq!(c.compile_clause(&clause), "sort $name asc, $age desc;");
    }

    #[test]
    fn test_clause_limit() {
        let c = compiler();
        assert_eq!(c.compile_clause(&Clause::Limit(10)), "limit 10;");
    }

    #[test]
    fn test_clause_offset() {
        let c = compiler();
        assert_eq!(c.compile_clause(&Clause::Offset(20)), "offset 20;");
    }

    #[test]
    fn test_value_comparison_contains() {
        let c = compiler();
        let p = Pattern::ValueComparison {
            var: "$name".into(),
            operator: "contains".into(),
            value: lit(json!("Ali"), "string"),
        };
        assert_eq!(c.compile_pattern(&p), "$name contains \"Ali\"");
    }

    #[test]
    fn test_value_comparison_like() {
        let c = compiler();
        let p = Pattern::ValueComparison {
            var: "$name".into(),
            operator: "like".into(),
            value: lit(json!("^A.*"), "string"),
        };
        assert_eq!(c.compile_pattern(&p), "$name like \"^A.*\"");
    }

    // ── Multi-clause ────────────────────────────────────────────────────

    #[test]
    fn test_multi_clause_match_sort_limit_offset() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![FetchItem::Wildcard {
                key: "".into(),
                var: "$p".into(),
            }]),
            Clause::Sort(vec![SortField {
                variable: "$age".into(),
                ascending: true,
            }]),
            Clause::Limit(10),
            Clause::Offset(5),
        ];
        let result = c.compile(&clauses);
        assert!(result.contains("sort $age asc;"));
        assert!(result.contains("limit 10;"));
        assert!(result.contains("offset 5;"));
    }

    #[test]
    fn test_multi_clause_match_insert() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Insert(vec![Statement::Has {
                subject_var: "$p".into(),
                attr_name: "age".into(),
                value: lit(json!(30), "long"),
            }]),
        ];
        let result = c.compile(&clauses);
        assert!(result.starts_with("match "));
        assert!(result.contains("insert "));
    }

    #[test]
    fn test_multi_clause_match_put() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Put(vec![Statement::Has {
                subject_var: "$p".into(),
                attr_name: "age".into(),
                value: lit(json!(30), "long"),
            }]),
        ];
        let result = c.compile(&clauses);
        assert!(result.starts_with("match "));
        assert!(result.contains("put "));
    }

    #[test]
    fn test_multi_clause_match_delete() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Delete(vec![Statement::DeleteThing("$p".into())]),
        ];
        let result = c.compile(&clauses);
        assert_eq!(result, "match $p isa person; delete $p;");
    }

    #[test]
    fn test_multi_clause_match_fetch() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![FetchItem::Wildcard {
                key: "".into(),
                var: "$p".into(),
            }]),
        ];
        let result = c.compile(&clauses);
        assert!(result.starts_with("match "));
        assert!(result.contains("fetch {"));
    }

    #[test]
    fn test_multi_clause_match_reduce() {
        let c = compiler();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Reduce {
                assignments: vec![ReduceAssignment {
                    variable: "$c".into(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: "count".into(),
                        args: vec![Value::Variable("$p".into())],
                    }),
                }],
                group_by: None,
            },
        ];
        let result = c.compile(&clauses);
        assert!(result.starts_with("match "));
        assert!(result.contains("reduce $c = count($p);"));
    }

    // ── Roundtrip tests (parse → compile → parse → compare ASTs) ──────

    fn roundtrip(tql: &str) {
        let ast1 = crate::query_parser::parse_typeql_query(tql)
            .unwrap_or_else(|e| panic!("First parse failed for: {tql}\n{e}"));
        let compiled = compiler().compile(&ast1);
        let ast2 = crate::query_parser::parse_typeql_query(&compiled)
            .unwrap_or_else(|e| panic!("Second parse failed for compiled: {compiled}\n{e}"));
        assert_eq!(
            ast1, ast2,
            "AST mismatch for roundtrip.\nOriginal: {tql}\nCompiled: {compiled}"
        );
    }

    #[test]
    fn test_roundtrip_simple_match() {
        roundtrip("match $p isa person;");
    }

    #[test]
    fn test_roundtrip_match_with_constraints() {
        roundtrip("match $p isa person, has name \"Alice\", has age 30;");
    }

    #[test]
    fn test_roundtrip_match_strict() {
        roundtrip("match $e isa! employee;");
    }

    #[test]
    fn test_roundtrip_match_iid() {
        roundtrip("match $p iid 0x1234abcd;");
    }

    #[test]
    fn test_roundtrip_match_subtype() {
        roundtrip("match $t sub entity;");
    }

    #[test]
    fn test_roundtrip_match_has_variable() {
        roundtrip("match $p has name $n;");
    }

    #[test]
    fn test_roundtrip_match_value_comparison() {
        roundtrip("match $x > 5;");
        roundtrip("match $x <= 100;");
        roundtrip("match $x == 42;");
        roundtrip("match $x != 0;");
    }

    #[test]
    fn test_roundtrip_match_not() {
        roundtrip("match not { $p isa person; };");
    }

    #[test]
    fn test_roundtrip_match_or() {
        roundtrip("match { $x isa a; } or { $x isa b; };");
    }

    #[test]
    fn test_roundtrip_relation_match() {
        roundtrip("match $r isa employment (employee: $p, employer: $c);");
    }

    #[test]
    fn test_roundtrip_insert() {
        roundtrip("insert $p isa person; $p has name \"Alice\";");
    }

    #[test]
    fn test_roundtrip_relation_insert() {
        roundtrip("insert\n$r isa employment, links (employee: $p, employer: $c);");
    }

    #[test]
    fn test_roundtrip_put() {
        roundtrip("put $p isa person; $p has name \"Alice\";");
    }

    #[test]
    fn test_roundtrip_match_put() {
        roundtrip("match $p isa person, has name \"Alice\";\nput\n$p has age 30;");
    }

    #[test]
    fn test_roundtrip_match_delete() {
        roundtrip("match $p isa person, has name \"Alice\";\ndelete $p;");
    }

    #[test]
    fn test_roundtrip_match_fetch_attribute() {
        roundtrip("match $p isa person;\nfetch {\n  \"name\": $p.name\n};");
    }

    #[test]
    fn test_roundtrip_match_fetch_wildcard() {
        roundtrip("match $p isa person;\nfetch {\n  \"\": {$p.*}\n};");
    }

    #[test]
    fn test_roundtrip_match_fetch_nested_wildcard() {
        roundtrip("match $p isa person;\nfetch {\n  \"\": { $p.* }\n};");
    }

    #[test]
    fn test_roundtrip_reduce() {
        roundtrip("match $p isa person;\nreduce $count = count($p);");
    }

    #[test]
    fn test_roundtrip_reduce_groupby() {
        roundtrip("match $p isa person;\nreduce $count = count($p) groupby $city;");
    }

    #[test]
    fn test_roundtrip_boolean_values() {
        roundtrip("match $p isa person, has active true;");
        roundtrip("match $p isa person, has active false;");
    }

    #[test]
    fn test_roundtrip_decimal_value() {
        roundtrip("match $p isa person, has score 42dec;");
    }

    #[test]
    fn test_roundtrip_date_value() {
        roundtrip("match $p isa person, has birthday 2024-01-15;");
    }

    #[test]
    fn test_roundtrip_datetime_value() {
        roundtrip("match $p isa event, has start-time 2024-01-15T10:30:00;");
    }

    #[test]
    fn test_roundtrip_string_with_escapes() {
        roundtrip("match $p isa person, has bio \"line1\\nline2\";");
    }

    #[test]
    fn test_roundtrip_sort_single() {
        roundtrip("match $p isa person; sort $age asc;");
    }

    #[test]
    fn test_roundtrip_sort_multiple() {
        roundtrip("match $p isa person; sort $name asc, $age desc;");
    }

    #[test]
    fn test_roundtrip_limit() {
        roundtrip("match $p isa person; limit 10;");
    }

    #[test]
    fn test_roundtrip_offset() {
        roundtrip("match $p isa person; offset 20;");
    }

    #[test]
    fn test_roundtrip_sort_limit_offset() {
        roundtrip("match $p isa person; sort $age asc; limit 10; offset 5;");
    }

    #[test]
    fn test_roundtrip_contains() {
        roundtrip("match $name contains \"Ali\";");
    }

    #[test]
    fn test_roundtrip_like() {
        roundtrip("match $name like \"^A.*\";");
    }

    #[test]
    fn typed_string_comparisons_distinguish_literals_from_regex() {
        assert_eq!(
            compile_typed_value_comparison(
                7,
                TypedComparisonOperator::StartsWith,
                &TypedLiteral::String("A.+%".into()),
            ),
            r#"$f7 like "^A\\.\\+%.*""#,
        );
        assert_eq!(
            compile_typed_value_comparison(
                7,
                TypedComparisonOperator::EndsWith,
                &TypedLiteral::String("[x]$50%".into()),
            ),
            r#"$f7 like ".*\\[x\\]\\$50%$""#,
        );
        assert_eq!(
            compile_typed_value_comparison(
                7,
                TypedComparisonOperator::Contains,
                &TypedLiteral::String("50%_.*".into()),
            ),
            r#"$f7 contains "50%_.*""#,
        );
        assert_eq!(
            compile_typed_value_comparison(
                7,
                TypedComparisonOperator::Regex,
                &TypedLiteral::String("^A[[:alpha:]]+%$".into()),
            ),
            r#"$f7 like "^A[[:alpha:]]+%$""#,
        );
    }

    #[test]
    fn prepared_fetch_rows_emits_one_given_parameter_for_every_supported_value_kind() {
        let cases = [
            (TypedLiteral::String("needle".into()), "string"),
            (TypedLiteral::Long(42), "integer"),
            (TypedLiteral::Double(1.25), "double"),
            (TypedLiteral::Boolean(true), "boolean"),
        ];

        for (value, value_type) in cases {
            let prepared = compiler()
                .prepare_typed_fetch_rows(&typed_literal_query(value.clone()))
                .unwrap();
            assert_eq!(
                prepared,
                PreparedTypedStatement {
                    typeql: format!(
                        "given $g0: {value_type}; match $b0 isa person; $b0 has score $f0; $f0 == $g0; select $b0; distinct; limit 1;"
                    ),
                    parameters: vec![TypedQueryParameter {
                        name: "g0".into(),
                        value: TypedQueryParameterValue::Literal(value),
                    }],
                }
            );
        }

        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&typed_literal_query(TypedLiteral::String(
                    "needle".into(),
                )))
                .unwrap(),
            "match $b0 isa person; $b0 has score $f0; $f0 == \"needle\"; select $b0; distinct; limit 1;"
        );
    }

    #[test]
    fn prepared_string_parameters_preserve_structural_order_and_provider_regex_rules() {
        let query = typed_predicate_query(TypedMatchPredicate::And {
            expressions: vec![
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::StartsWith,
                    value: TypedLiteral::String("A.+%".into()),
                },
                TypedMatchPredicate::Or {
                    expressions: vec![
                        TypedMatchPredicate::FieldValue {
                            field: 0,
                            operator: TypedComparisonOperator::Contains,
                            value: TypedLiteral::String("50%_.*".into()),
                        },
                        TypedMatchPredicate::Not {
                            expression: Box::new(TypedMatchPredicate::FieldValue {
                                field: 0,
                                operator: TypedComparisonOperator::EndsWith,
                                value: TypedLiteral::String("[x]$50%".into()),
                            }),
                        },
                    ],
                },
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::Regex,
                    value: TypedLiteral::String("^A[[:alpha:]]+%$".into()),
                },
            ],
        });

        let prepared = compiler().prepare_typed_fetch_rows(&query).unwrap();
        assert_eq!(
            prepared.typeql,
            "given $g0: string; match $b0 isa person; $b0 has score $f0; $f0 like \"^A\\\\.\\\\+%.*\"; { $b0 has score $f0; $f0 contains $g0; } or { not { $b0 has score $f0; \n $f0 like \".*\\\\[x\\\\]\\\\$50%$\"; \n}; }; $b0 has score $f0; $f0 like \"^A[[:alpha:]]+%$\"; select $b0; distinct; limit 1;"
        );
        assert_eq!(
            prepared.parameters,
            vec![TypedQueryParameter {
                name: "g0".into(),
                value: TypedQueryParameterValue::Literal(TypedLiteral::String("50%_.*".into())),
            }]
        );
    }

    #[test]
    fn prepared_predicates_inline_lossy_value_kinds_without_renumbering_supported_values() {
        let query = typed_predicate_query(TypedMatchPredicate::And {
            expressions: vec![
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::Equal,
                    value: TypedLiteral::Decimal("1.25".into()),
                },
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::Equal,
                    value: TypedLiteral::Long(42),
                },
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::Equal,
                    value: TypedLiteral::Duration("P1D".into()),
                },
                TypedMatchPredicate::FieldValue {
                    field: 0,
                    operator: TypedComparisonOperator::Equal,
                    value: TypedLiteral::Boolean(true),
                },
            ],
        });

        let prepared = compiler().prepare_typed_fetch_rows(&query).unwrap();
        assert_eq!(
            prepared.typeql,
            "given $g0: integer, $g1: boolean; match $b0 isa person; $b0 has score $f0; $f0 == 1.25dec; $b0 has score $f0; $f0 == $g0; $b0 has score $f0; $f0 == P1D; $b0 has score $f0; $f0 == $g1; select $b0; distinct; limit 1;"
        );
        assert_eq!(
            prepared.parameters,
            vec![
                TypedQueryParameter {
                    name: "g0".into(),
                    value: TypedQueryParameterValue::Literal(TypedLiteral::Long(42)),
                },
                TypedQueryParameter {
                    name: "g1".into(),
                    value: TypedQueryParameterValue::Literal(TypedLiteral::Boolean(true)),
                },
            ]
        );

        for value in [
            TypedLiteral::Date("+20000-01-01".into()),
            TypedLiteral::DateTime("2024-03-30T24:00:00.000000000".into()),
            TypedLiteral::DateTimeTz("1987-12-22T17:29 Asia/Kolkata".into()),
            TypedLiteral::Decimal("1.25".into()),
            TypedLiteral::Duration("P1D".into()),
        ] {
            let query = typed_literal_query(value);
            let prepared = compiler().prepare_typed_fetch_rows(&query).unwrap();
            assert!(prepared.parameters.is_empty());
            assert_eq!(
                prepared.typeql,
                compiler().compile_typed_fetch_rows(&query).unwrap()
            );
        }
    }

    #[test]
    fn prepared_root_scan_and_page_rematch_share_the_given_contract() {
        let fetch = typed_literal_query(TypedLiteral::String("needle".into()));
        let root = TypedRootScan {
            targets: fetch.targets.clone(),
            fields: fetch.fields.clone(),
            predicate: fetch.predicate.clone(),
            root: 0,
            order: vec![],
            offset: None,
            limit: Some(2),
        };
        let prepared_root = compiler().prepare_typed_root_scan(&root).unwrap();
        assert_eq!(
            prepared_root.typeql,
            "given $g0: string; match $b0 isa person; $b0 has score $f0; $f0 == $g0; select $b0; distinct; limit 2;"
        );

        let rematch = TypedPageRematch {
            targets: fetch.targets,
            fields: fetch.fields,
            predicate: fetch.predicate,
            root: 0,
            root_concept_ids: vec!["0x01".into()],
            collection_orders: vec![],
        };
        let prepared_rematch = compiler().prepare_typed_page_rematch(&rematch).unwrap();
        assert_eq!(
            prepared_rematch.typeql,
            "given $g0: string; match $b0 isa person; $b0 has score $f0; $f0 == $g0; { $b0 iid 0x01; }; $b0 isa! $t0; fetch { \"b0\": { \"concept_id\": iid($b0), \"concrete_type\": label($t0), \"attributes\": {  $b0.*}}};"
        );
        assert_eq!(prepared_root.parameters, prepared_rematch.parameters);
        assert_eq!(
            prepared_root.parameters,
            vec![TypedQueryParameter {
                name: "g0".into(),
                value: TypedQueryParameterValue::Literal(TypedLiteral::String("needle".into())),
            }]
        );

        let without_predicate = TypedRootScan {
            targets: vec![TypedMatchTarget {
                binding: 0,
                kind: TypedThingKind::Entity,
                type_name: "person".into(),
                exact: false,
            }],
            fields: vec![],
            predicate: None,
            root: 0,
            order: vec![],
            offset: None,
            limit: None,
        };
        let prepared = compiler()
            .prepare_typed_root_scan(&without_predicate)
            .unwrap();
        assert!(prepared.parameters.is_empty());
        assert_eq!(
            prepared.typeql,
            compiler()
                .compile_typed_root_scan(&without_predicate)
                .unwrap()
        );
    }

    #[test]
    fn typed_fetch_rows_compiles_strict_targets_graph_boolean_distinct_and_window() {
        let query = TypedFetchRows {
            targets: vec![
                TypedMatchTarget {
                    binding: 0,
                    kind: TypedThingKind::Entity,
                    type_name: "person".into(),
                    exact: true,
                },
                TypedMatchTarget {
                    binding: 1,
                    kind: TypedThingKind::Relation,
                    type_name: "employment".into(),
                    exact: false,
                },
                TypedMatchTarget {
                    binding: 2,
                    kind: TypedThingKind::Entity,
                    type_name: "company".into(),
                    exact: true,
                },
            ],
            fields: vec![
                TypedFieldBinding {
                    id: 0,
                    owner: 0,
                    field_name: "name".into(),
                },
                TypedFieldBinding {
                    id: 1,
                    owner: 2,
                    field_name: "name".into(),
                },
            ],
            predicate: Some(TypedMatchPredicate::And {
                expressions: vec![
                    TypedMatchPredicate::FieldValue {
                        field: 0,
                        operator: TypedComparisonOperator::StartsWith,
                        value: TypedLiteral::String("Al".into()),
                    },
                    TypedMatchPredicate::FieldComparison {
                        left: 0,
                        operator: TypedComparisonOperator::NotEqual,
                        right: 1,
                    },
                    TypedMatchPredicate::RoleEdge {
                        edge: 0,
                        relation: 1,
                        role_name: "employee".into(),
                        player: 0,
                    },
                    TypedMatchPredicate::Or {
                        expressions: vec![
                            TypedMatchPredicate::RoleEdge {
                                edge: 1,
                                relation: 1,
                                role_name: "employer".into(),
                                player: 2,
                            },
                            TypedMatchPredicate::Not {
                                expression: Box::new(TypedMatchPredicate::FieldValue {
                                    field: 1,
                                    operator: TypedComparisonOperator::Equal,
                                    value: TypedLiteral::String("Retired".into()),
                                }),
                            },
                        ],
                    },
                ],
            }),
            projection: vec![2, 0, 1],
            distinct: true,
            order: vec![
                TypedMatchOrder {
                    field: 1,
                    direction: TypedSortDirection::Descending,
                    missing: TypedMissingOrder::Reject,
                },
                TypedMatchOrder {
                    field: 0,
                    direction: TypedSortDirection::Ascending,
                    missing: TypedMissingOrder::Reject,
                },
            ],
            offset: 5,
            limit: 10,
        };

        let rendered = compiler().compile_typed_fetch_rows(&query).unwrap();
        assert!(rendered.contains("$b0 isa! person"));
        assert!(rendered.contains("$b1 isa employment"));
        assert!(rendered.contains("$b1 links (employee: $b0)"));
        assert!(rendered.contains("$f0 like \"^Al.*\""));
        assert!(rendered.contains("$f0 != $f1"));
        assert!(rendered.contains("not {"));
        assert!(rendered.contains("select $b0, $b1, $b2, $f1, $f0; distinct;"));
        assert!(rendered.contains("sort $f1 desc, $f0 asc;"));
        assert!(rendered.ends_with("offset 5; limit 10;"));
    }

    #[test]
    fn typed_bounded_reachability_unrolls_stably_and_collapses_proof_paths() {
        let query = TypedFetchRows {
            targets: vec![
                TypedMatchTarget {
                    binding: 0,
                    kind: TypedThingKind::Entity,
                    type_name: "node".into(),
                    exact: false,
                },
                TypedMatchTarget {
                    binding: 1,
                    kind: TypedThingKind::Entity,
                    type_name: "node".into(),
                    exact: false,
                },
            ],
            fields: vec![],
            predicate: Some(TypedMatchPredicate::Reachable {
                relation_type: "edge".into(),
                role_from: "origin".into(),
                role_to: "destination".into(),
                source: 0,
                target: 1,
                min_depth: 0,
                max_depth: 3,
            }),
            projection: vec![0, 1],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 5,
        };

        assert_eq!(
            compiler().compile_typed_fetch_rows(&query).unwrap(),
            "match $b0 isa node; $b1 isa node; { $R0z isa node; $b0 is $R0z; $b1 is $R0z; } or { (origin: $b0, destination: $b1) isa! edge; } or { (origin: $b0, destination: $R0l2h1) isa! edge; (origin: $R0l2h1, destination: $b1) isa! edge; } or { (origin: $b0, destination: $R0l3h1) isa! edge; (origin: $R0l3h1, destination: $R0l3h2) isa! edge; (origin: $R0l3h2, destination: $b1) isa! edge; }; reduce $RreachableProofCount = count groupby $b0, $b1; select $b0, $b1; distinct; limit 5;"
        );
    }

    #[test]
    fn typed_zero_hop_reachability_uses_source_descriptor_and_retains_target_constraints() {
        let query = TypedFetchRows {
            targets: vec![
                TypedMatchTarget {
                    binding: 0,
                    kind: TypedThingKind::Entity,
                    type_name: "base-node".into(),
                    exact: false,
                },
                TypedMatchTarget {
                    binding: 1,
                    kind: TypedThingKind::Entity,
                    type_name: "different-node".into(),
                    exact: true,
                },
            ],
            fields: vec![],
            predicate: Some(TypedMatchPredicate::Reachable {
                relation_type: "edge".into(),
                role_from: "origin".into(),
                role_to: "destination".into(),
                source: 0,
                target: 1,
                min_depth: 0,
                max_depth: 0,
            }),
            projection: vec![0, 1],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 1,
        };

        // The source remains subtype-aware (`isa`), while the independently
        // declared exact target still makes identity impossible when these
        // schema types are disjoint.
        assert_eq!(
            compiler().compile_typed_fetch_rows(&query).unwrap(),
            "match $b0 isa base-node; $b1 isa! different-node; { $R0z isa base-node; $b0 is $R0z; $b1 is $R0z; }; reduce $RreachableProofCount = count groupby $b0, $b1; select $b0, $b1; distinct; limit 1;"
        );
    }

    #[test]
    fn typed_bounded_reachability_rejects_bounds_nesting_and_label_injection() {
        let predicate =
            |min_depth, max_depth, relation_type: &str| TypedMatchPredicate::Reachable {
                relation_type: relation_type.into(),
                role_from: "origin".into(),
                role_to: "destination".into(),
                source: 0,
                target: 0,
                min_depth,
                max_depth,
            };
        assert!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(predicate(2, 1, "edge")))
                .unwrap_err()
                .message()
                .contains("minimum depth")
        );
        assert!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(predicate(
                    0,
                    1,
                    "edge; match $x isa thing"
                )))
                .unwrap_err()
                .message()
                .contains("invalid TypeQL label")
        );
        let nested = TypedMatchPredicate::Not {
            expression: Box::new(predicate(0, 1, "edge")),
        };
        assert!(
            compiler()
                .compile_typed_fetch_rows(&typed_predicate_query(nested))
                .unwrap_err()
                .message()
                .contains("root conjunction")
        );
    }

    #[test]
    fn typed_tuple_scan_selects_the_complete_public_tuple_and_keeps_hidden_witnesses() {
        let query = TypedFetchRows {
            targets: vec![
                TypedMatchTarget {
                    binding: 0,
                    kind: TypedThingKind::Entity,
                    type_name: "person".into(),
                    exact: true,
                },
                TypedMatchTarget {
                    binding: 1,
                    kind: TypedThingKind::Relation,
                    type_name: "employment".into(),
                    exact: false,
                },
                TypedMatchTarget {
                    binding: 2,
                    kind: TypedThingKind::Entity,
                    type_name: "company".into(),
                    exact: true,
                },
            ],
            fields: vec![],
            predicate: Some(TypedMatchPredicate::RoleEdge {
                edge: 0,
                relation: 1,
                role_name: "employee".into(),
                player: 0,
            }),
            projection: vec![2, 0],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 2,
        };

        let expected = "match $b0 isa! person; $b1 isa employment; $b2 isa! company; $b1 links (employee: $b0); select $b2, $b0; distinct; limit 2;";
        assert_eq!(
            compiler().compile_typed_tuple_scan(&query).unwrap(),
            expected
        );
        let prepared = compiler().prepare_typed_tuple_scan(&query).unwrap();
        assert!(prepared.parameters.is_empty());
        assert_eq!(prepared.typeql, expected);

        let mut wrong_limit = query;
        wrong_limit.limit = 3;
        assert_eq!(
            compiler()
                .compile_typed_tuple_scan(&wrong_limit)
                .unwrap_err()
                .message(),
            "typed tuple scan requires distinct unordered identities at offset zero and limit two"
        );
    }

    #[test]
    fn typed_fetch_rows_rejects_lossy_unknown_references() {
        let query = TypedFetchRows {
            targets: vec![TypedMatchTarget {
                binding: 0,
                kind: TypedThingKind::Entity,
                type_name: "person".into(),
                exact: false,
            }],
            fields: vec![],
            predicate: None,
            projection: vec![1],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 1,
        };

        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&query)
                .unwrap_err()
                .message(),
            "typed selected-row projection references an unknown binding"
        );
    }

    #[test]
    fn typed_fetch_rows_rejects_hostile_metadata_before_rendering() {
        let valid = TypedFetchRows {
            targets: vec![
                TypedMatchTarget {
                    binding: 0,
                    kind: TypedThingKind::Relation,
                    type_name: "employment-record".into(),
                    exact: false,
                },
                TypedMatchTarget {
                    binding: 1,
                    kind: TypedThingKind::Entity,
                    type_name: "person".into(),
                    exact: false,
                },
            ],
            fields: vec![TypedFieldBinding {
                id: 0,
                owner: 1,
                field_name: "first-name".into(),
            }],
            predicate: Some(TypedMatchPredicate::RoleEdge {
                edge: 0,
                relation: 0,
                role_name: "employee-role".into(),
                player: 1,
            }),
            projection: vec![1],
            distinct: true,
            order: vec![TypedMatchOrder {
                field: 0,
                direction: TypedSortDirection::Ascending,
                missing: TypedMissingOrder::Reject,
            }],
            offset: 0,
            limit: 1,
        };
        compiler().compile_typed_fetch_rows(&valid).unwrap();

        let mut hostile = valid.clone();
        hostile.targets[0].type_name = "employment; delete $x".into();
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&hostile)
                .unwrap_err()
                .message(),
            "typed match target contains an invalid TypeQL label"
        );

        let mut hostile = valid.clone();
        hostile.fields[0].field_name = "match".into();
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&hostile)
                .unwrap_err()
                .message(),
            "typed match field contains an invalid TypeQL label"
        );

        let mut hostile = valid;
        let Some(TypedMatchPredicate::RoleEdge { role_name, .. }) = &mut hostile.predicate else {
            unreachable!()
        };
        *role_name = "employee role".into();
        assert_eq!(
            compiler()
                .compile_typed_fetch_rows(&hostile)
                .unwrap_err()
                .message(),
            "typed role edge contains an invalid TypeQL label"
        );
    }

    #[test]
    fn typed_public_compilers_reject_unsafe_literal_text_before_rendering() {
        let hostile = [
            (
                TypedLiteral::Double(f64::INFINITY),
                "typed predicate contains an invalid canonical double literal",
            ),
            (
                TypedLiteral::Date("2024-01-01; delete $x".into()),
                "typed predicate contains an invalid canonical date literal",
            ),
            (
                TypedLiteral::DateTime("2024-01-01T00:00; select $x".into()),
                "typed predicate contains an invalid canonical datetime literal",
            ),
            (
                TypedLiteral::DateTimeTz("2024-01-01T00:00:00Z; fetch {}".into()),
                "typed predicate contains an invalid canonical datetime-tz literal",
            ),
            (
                TypedLiteral::Decimal("0; delete $x".into()),
                "typed predicate contains an invalid canonical decimal literal",
            ),
            (
                TypedLiteral::Duration("P1D; delete $x".into()),
                "typed predicate contains an invalid canonical duration literal",
            ),
        ];

        for (literal, expected) in hostile {
            let query = typed_literal_query(literal);
            assert_eq!(
                compiler()
                    .compile_typed_fetch_rows(&query)
                    .unwrap_err()
                    .message(),
                expected
            );
        }

        let query = typed_literal_query(TypedLiteral::Date("2024-01-01; fetch {}".into()));
        let root = TypedRootScan {
            targets: query.targets.clone(),
            fields: query.fields.clone(),
            predicate: query.predicate.clone(),
            root: 0,
            order: vec![],
            offset: None,
            limit: None,
        };
        assert_eq!(
            compiler()
                .compile_typed_root_scan(&root)
                .unwrap_err()
                .message(),
            "typed predicate contains an invalid canonical date literal"
        );

        let rematch = TypedPageRematch {
            targets: query.targets,
            fields: query.fields,
            predicate: query.predicate,
            root: 0,
            root_concept_ids: vec!["0x01".into()],
            collection_orders: vec![],
        };
        assert_eq!(
            compiler()
                .compile_typed_page_rematch(&rematch)
                .unwrap_err()
                .message(),
            "typed predicate contains an invalid canonical date literal"
        );
    }

    #[test]
    fn typed_literal_validation_preserves_documented_canonical_forms() {
        let canonical = [
            TypedLiteral::Date("+20000-01-01".into()),
            TypedLiteral::DateTime("2025-01-10T16:13".into()),
            TypedLiteral::DateTime("2024-03-30T24:00:00.000000000".into()),
            TypedLiteral::DateTimeTz("2024-03-30T12:00:00Z".into()),
            TypedLiteral::DateTimeTz("1920-04-26T16:30-0930".into()),
            TypedLiteral::DateTimeTz("1987-12-22T17:29 Asia/Kolkata".into()),
            TypedLiteral::Decimal("-0.0200000000000000000dec".into()),
            TypedLiteral::Decimal("-9223372036854775808.0000000000000000000".into()),
            TypedLiteral::Decimal("9223372036854775807.9999999999999999999".into()),
            TypedLiteral::Duration("P12W".into()),
            TypedLiteral::Duration("P1Y2M3DT4H5M6.789S".into()),
        ];

        for literal in canonical {
            compiler()
                .compile_typed_fetch_rows(&typed_literal_query(literal))
                .unwrap();
        }

        for literal in [
            TypedLiteral::Date("2023-02-29".into()),
            TypedLiteral::DateTime("2024-01-01T24:01".into()),
            TypedLiteral::DateTimeTz("2024-01-01T12:00:00+2460".into()),
            TypedLiteral::Decimal("1.00000000000000000000".into()),
            TypedLiteral::Decimal("9223372036854775808".into()),
            TypedLiteral::Decimal("-9223372036854775808.0000000000000000001".into()),
            TypedLiteral::Decimal("-9223372036854775809".into()),
            TypedLiteral::Decimal("999999999999999999999999999999999999999".into()),
            TypedLiteral::Duration("P1W1D".into()),
            TypedLiteral::Duration("PT1.0000000000S".into()),
        ] {
            assert!(
                compiler()
                    .compile_typed_fetch_rows(&typed_literal_query(literal))
                    .is_err()
            );
        }
    }

    #[test]
    fn typed_root_scan_and_page_rematch_preserve_distinct_window_and_original_graph() {
        let targets = vec![
            TypedMatchTarget {
                binding: 0,
                kind: TypedThingKind::Entity,
                type_name: "person".into(),
                exact: false,
            },
            TypedMatchTarget {
                binding: 1,
                kind: TypedThingKind::Relation,
                type_name: "employment".into(),
                exact: false,
            },
        ];
        let fields = vec![TypedFieldBinding {
            id: 0,
            owner: 0,
            field_name: "person-identity".into(),
        }];
        let predicate = Some(TypedMatchPredicate::RoleEdge {
            edge: 0,
            relation: 1,
            role_name: "employee".into(),
            player: 0,
        });
        let root_scan = TypedRootScan {
            targets: targets.clone(),
            fields: fields.clone(),
            predicate: predicate.clone(),
            root: 0,
            order: vec![TypedMatchOrder {
                field: 0,
                direction: TypedSortDirection::Ascending,
                missing: TypedMissingOrder::Reject,
            }],
            offset: Some(2),
            limit: Some(3),
        };
        let rendered = compiler().compile_typed_root_scan(&root_scan).unwrap();
        assert!(rendered.contains("$b1 links (employee: $b0)"));
        assert!(rendered.contains("select $b0, $f0; distinct;"));
        assert!(rendered.contains("sort $f0 asc; offset 2; limit 3;"));

        let rematch = TypedPageRematch {
            targets,
            fields,
            predicate,
            root: 0,
            root_concept_ids: vec!["0x01".into(), "0x02".into()],
            collection_orders: vec![crate::ast::TypedCollectionOrder {
                binding: 0,
                order: vec![TypedMatchOrder {
                    field: 0,
                    direction: TypedSortDirection::Ascending,
                    missing: TypedMissingOrder::Reject,
                }],
            }],
        };
        let rendered = compiler().compile_typed_page_rematch(&rematch).unwrap();
        assert!(rendered.contains("{ $b0 iid 0x01; } or { $b0 iid 0x02; }"));
        assert!(rendered.contains("$b0 isa person"));
        assert!(rendered.contains("$b1 links (employee: $b0)"));
        assert!(rendered.contains("$b0 isa! $t0"));
        assert!(rendered.contains("$b1 isa! $t1"));
        assert!(rendered.contains("$player isa! $player_type"));
        assert_eq!(rendered.matches("fetch {").count(), 2);
        assert!(rendered.contains("\"b0\""));
        assert!(rendered.contains("\"b1\""));
        let bounded = compiler()
            .compile_typed_page_rematch_bounded(&rematch, 7)
            .unwrap();
        assert!(bounded.contains(" limit 7; fetch {"));
        let prepared = compiler()
            .prepare_typed_page_rematch_bounded(&rematch, 7)
            .unwrap();
        assert!(prepared.typeql.contains(" limit 7; fetch {"));

        let mut invalid = rematch;
        invalid.root_concept_ids = vec!["0x01; match $evil isa thing".into()];
        assert_eq!(
            compiler()
                .compile_typed_page_rematch(&invalid)
                .unwrap_err()
                .message(),
            "typed page re-match contains an invalid root IID"
        );
    }

    #[test]
    fn typed_hydration_partitions_entity_and_relation_links_safely() {
        let mixed = TypedHydrateThings {
            targets: vec![
                TypedHydrationTarget {
                    binding: 3,
                    declared_type: "employment".into(),
                    kind: TypedThingKind::Relation,
                    concept_ids: vec!["0x01".into(), "0x0a".into()],
                    concrete_descriptors: vec![TypedHydrationDescriptor {
                        type_name: "employment".into(),
                        kind: TypedThingKind::Relation,
                        fields: vec![TypedHydrationField {
                            field_name: "code".into(),
                            attribute_type: "employment-code".into(),
                            value_type: "string".into(),
                        }],
                        roles: vec![TypedHydrationRole {
                            role_name: "employee".into(),
                            player_types: vec!["person".into()],
                        }],
                    }],
                },
                TypedHydrationTarget {
                    binding: 4,
                    declared_type: "person".into(),
                    kind: TypedThingKind::Entity,
                    concept_ids: vec!["0x02".into()],
                    concrete_descriptors: vec![TypedHydrationDescriptor {
                        type_name: "person".into(),
                        kind: TypedThingKind::Entity,
                        fields: Vec::new(),
                        roles: Vec::new(),
                    }],
                },
            ],
        };

        assert_eq!(
            compiler()
                .compile_typed_hydrate_things(&mixed)
                .unwrap_err()
                .message(),
            "typed hydration batch must contain exactly one thing kind"
        );

        let relation = TypedHydrateThings {
            targets: vec![mixed.targets[0].clone()],
        };
        let rendered = compiler().compile_typed_hydrate_things(&relation).unwrap();
        assert!(rendered.contains("$thing iid 0x01; $thing isa employment; let $binding = 3"));
        assert!(rendered.contains("$thing iid 0x0a; $thing isa employment; let $binding = 3"));
        assert!(rendered.contains("$thing isa! $type"));
        assert!(rendered.contains(" limit 2; fetch {"));
        assert!(rendered.contains("$thing links ($role: $player)"));
        assert!(!rendered.contains("relation $type"));
        assert!(rendered.contains("$player isa! $player_type"));

        let entity = TypedHydrateThings {
            targets: vec![mixed.targets[1].clone()],
        };
        let rendered = compiler().compile_typed_hydrate_things(&entity).unwrap();
        assert!(rendered.contains("$thing iid 0x02; $thing isa person; let $binding = 4"));
        assert!(rendered.contains("$thing isa! $type"));
        assert!(rendered.contains(" limit 1; fetch {"));
        assert!(!rendered.contains("links"));
        assert!(!rendered.contains("\"roles\""));

        let mut invalid = relation.clone();
        invalid.targets[0].concept_ids = vec!["0x01; match $evil isa thing".into()];
        assert_eq!(
            compiler()
                .compile_typed_hydrate_things(&invalid)
                .unwrap_err()
                .message(),
            "typed hydration contains an invalid provider IID"
        );

        let mut hostile = relation;
        hostile.targets[0].concrete_descriptors[0].roles[0].player_types =
            vec!["person; delete $x".into()];
        assert_eq!(
            compiler()
                .compile_typed_hydrate_things(&hostile)
                .unwrap_err()
                .message(),
            "typed hydration contains invalid descriptor labels"
        );
    }
}
