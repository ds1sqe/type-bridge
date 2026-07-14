//! TypeQL query compiler — converts AST [`Clause`](crate::ast::Clause)s into TypeQL query strings.

use crate::ast::{
    Clause, Constraint, FetchItem, LetAssignment, LiteralValue, Pattern, ReduceAssignment,
    SortField, Statement, TypedComparisonOperator, TypedFetchRows, TypedHydrateThings,
    TypedLiteral, TypedMatchPredicate, TypedMatchTarget, TypedMissingOrder, TypedPageRematch,
    TypedRootScan, TypedSortDirection, Value,
};
use crate::decimal::parse_decimal;
use crate::reserved_words::is_reserved_word;
use std::sync::OnceLock;
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
    pub value: TypedLiteral,
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
    ) -> String {
        let Self::Prepared(parameters) = self else {
            return compile_typed_value_comparison(field, operator, value);
        };
        let Some(value) = prepared_typed_value(operator, value) else {
            return compile_typed_value_comparison(field, operator, value);
        };
        let name = format!("g{}", parameters.len());
        let comparison = format!(
            "{} {} ${name}",
            field_variable(field),
            comparison_token(operator)
        );
        parameters.push(TypedQueryParameter { name, value });
        comparison
    }

    fn finish(self, typeql: String) -> PreparedTypedStatement {
        let Self::Prepared(parameters) = self else {
            unreachable!("only prepared compiler paths finish prepared statements")
        };
        let typeql = if parameters.is_empty() {
            typeql
        } else {
            let header = parameters
                .iter()
                .map(|parameter| {
                    format!(
                        "${}: {}",
                        parameter.name,
                        prepared_typed_value_type(&parameter.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("given {header};\n{typeql}")
        };
        PreparedTypedStatement { typeql, parameters }
    }
}

/// Compiles a sequence of [`Clause`] AST nodes into a TypeQL query string.
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

    /// Compile a slice of clauses into a complete TypeQL query string.
    pub fn compile(&self, clauses: &[Clause]) -> String {
        clauses
            .iter()
            .map(|c| self.compile_clause(c))
            .collect::<Vec<_>>()
            .join("\n")
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

    fn render_typed_fetch_rows(
        &self,
        query: &TypedFetchRows,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_fetch_rows(query)?;

        let mut patterns = query
            .targets
            .iter()
            .map(|target| {
                format!(
                    "{} {} {}",
                    binding_variable(target.binding),
                    if target.exact { "isa!" } else { "isa" },
                    target.type_name
                )
            })
            .collect::<Vec<_>>();

        let ordered_fields = query
            .order
            .iter()
            .map(|order| order.field)
            .collect::<std::collections::BTreeSet<_>>();
        for field_id in ordered_fields {
            let field = typed_field(&query.fields, field_id)?;
            patterns.push(format!(
                "{} has {} {}",
                binding_variable(field.owner),
                field.field_name,
                field_variable(field.id)
            ));
        }
        if let Some(predicate) = &query.predicate {
            patterns.push(self.render_typed_predicate(&query.fields, predicate, parameters)?);
        }

        let mut rendered = format!("match\n{};", patterns.join(";\n"));
        rendered.push_str("\nselect ");
        // Provider evidence must retain every positive binding. Canonical
        // selected-tuple projection/distinctness remains in the typed AST and
        // is applied by the Rust executor/result validator.
        let mut selected = query
            .targets
            .iter()
            .map(|target| binding_variable(target.binding))
            .collect::<Vec<_>>();
        selected.extend(query.order.iter().map(|order| field_variable(order.field)));
        selected.dedup();
        rendered.push_str(&selected.join(", "));
        rendered.push(';');
        if query.distinct {
            rendered.push_str("\ndistinct;");
        }
        if !query.order.is_empty() {
            rendered.push_str("\nsort ");
            rendered.push_str(
                &query
                    .order
                    .iter()
                    .map(|order| {
                        let direction = match order.direction {
                            TypedSortDirection::Ascending => "asc",
                            TypedSortDirection::Descending => "desc",
                        };
                        format!("{} {direction}", field_variable(order.field))
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            rendered.push(';');
        }
        if query.offset > 0 {
            rendered.push_str(&format!("\noffset {};", query.offset));
        }
        rendered.push_str(&format!("\nlimit {};", query.limit));
        Ok(rendered)
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
        let mut patterns = typed_target_patterns(&query.targets);
        let ordered_fields = query
            .order
            .iter()
            .map(|order| order.field)
            .collect::<std::collections::BTreeSet<_>>();
        for field_id in ordered_fields {
            let field = typed_field(&query.fields, field_id)?;
            patterns.push(format!(
                "{} has {} {}",
                binding_variable(field.owner),
                field.field_name,
                field_variable(field.id)
            ));
        }
        if let Some(predicate) = &query.predicate {
            patterns.push(self.render_typed_predicate(&query.fields, predicate, parameters)?);
        }

        let mut selected = vec![binding_variable(query.root)];
        selected.extend(query.order.iter().map(|order| field_variable(order.field)));
        selected.dedup();
        let mut rendered = format!(
            "match\n{};\nselect {};\ndistinct;",
            patterns.join(";\n"),
            selected.join(", ")
        );
        if !query.order.is_empty() {
            rendered.push_str("\nsort ");
            rendered.push_str(&compile_typed_order(&query.order));
            rendered.push(';');
        }
        if let Some(offset) = query.offset.filter(|offset| *offset > 0) {
            rendered.push_str(&format!("\noffset {offset};"));
        }
        if let Some(limit) = query.limit {
            rendered.push_str(&format!("\nlimit {limit};"));
        }
        Ok(rendered)
    }

    /// Compile one exact root-IID batch re-match with complete graph hydration.
    pub fn compile_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
    ) -> Result<String, TypedCompileError> {
        let mut parameters = TypedParameterMode::Inline;
        self.render_typed_page_rematch(query, &mut parameters)
    }

    /// Prepare one exact root-IID page re-match with deterministic `given`
    /// parameters for the original graph predicate.
    pub fn prepare_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
    ) -> Result<PreparedTypedStatement, TypedCompileError> {
        let mut parameters = TypedParameterMode::Prepared(Vec::new());
        let typeql = self.render_typed_page_rematch(query, &mut parameters)?;
        Ok(parameters.finish(typeql))
    }

    fn render_typed_page_rematch(
        &self,
        query: &TypedPageRematch,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        validate_typed_page_rematch(query)?;
        let mut patterns = typed_target_patterns(&query.targets);
        if let Some(predicate) = &query.predicate {
            patterns.push(self.render_typed_predicate(&query.fields, predicate, parameters)?);
        }
        patterns.push(
            query
                .root_concept_ids
                .iter()
                .map(|concept_id| {
                    format!("{{ {} iid {concept_id}; }}", binding_variable(query.root))
                })
                .collect::<Vec<_>>()
                .join(" or "),
        );
        patterns.extend(query.targets.iter().map(|target| {
            format!(
                "{} isa! {}",
                binding_variable(target.binding),
                type_variable(target.binding)
            )
        }));

        let bindings = query
            .targets
            .iter()
            .map(|target| {
                let binding = binding_variable(target.binding);
                let binding_key = format!("b{}", target.binding);
                let roles = if target.kind == crate::ast::TypedThingKind::Relation {
                    format!(
                        ", \"roles\": [ match {binding} links ($role: $player); $player isa! $player_type; fetch {{ \"role\": label($role), \"player_concept_id\": iid($player), \"player_concrete_type\": label($player_type), \"attributes\": {{ $player.* }} }}; ]"
                    )
                } else {
                    String::new()
                };
                format!(
                    "\"{binding_key}\": {{ \"concept_id\": iid({binding}), \"concrete_type\": label({}), \"attributes\": {{ {binding}.* }}{roles} }}",
                    type_variable(target.binding)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "match\n{};\nfetch {{ {bindings} }};",
            patterns.join(";\n")
        ))
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

        let branches = query
            .targets
            .iter()
            .flat_map(|target| {
                target.concept_ids.iter().map(move |concept_id| {
                    format!(
                        "{{ $thing iid {concept_id}; $thing isa {}; let $binding = {}; }}",
                        target.declared_type, target.binding
                    )
                })
            })
            .collect::<Vec<_>>()
            .join(" or ");
        let roles = match batch_kind {
            crate::ast::TypedThingKind::Entity => String::new(),
            crate::ast::TypedThingKind::Relation => {
                ", \"roles\": [ match $thing links ($role: $player); $player isa! $player_type; fetch { \"role\": label($role), \"player_concept_id\": iid($player), \"player_concrete_type\": label($player_type), \"attributes\": { $player.* } }; ]".to_owned()
            }
        };
        Ok(format!(
            "match\n{branches};\n$thing isa! $type;\nfetch {{ \"binding\": $binding, \"concept_id\": iid($thing), \"concrete_type\": label($type), \"attributes\": {{ $thing.* }}{roles} }};"
        ))
    }

    fn render_typed_predicate(
        &self,
        fields: &[crate::ast::TypedFieldBinding],
        predicate: &TypedMatchPredicate,
        parameters: &mut TypedParameterMode,
    ) -> Result<String, TypedCompileError> {
        match predicate {
            TypedMatchPredicate::FieldValue {
                field,
                operator,
                value,
            } => {
                let field = typed_field(fields, *field)?;
                let has = format!(
                    "{} has {} {}",
                    binding_variable(field.owner),
                    field.field_name,
                    field_variable(field.id)
                );
                let comparison = parameters.comparison(field.id, *operator, value);
                Ok(format!("{has}; {comparison}"))
            }
            TypedMatchPredicate::FieldComparison {
                left,
                operator,
                right,
            } => {
                let left = typed_field(fields, *left)?;
                let right = typed_field(fields, *right)?;
                Ok(format!(
                    "{} has {} {}; {} has {} {}; {} {} {}",
                    binding_variable(left.owner),
                    left.field_name,
                    field_variable(left.id),
                    binding_variable(right.owner),
                    right.field_name,
                    field_variable(right.id),
                    field_variable(left.id),
                    comparison_token(*operator),
                    field_variable(right.id)
                ))
            }
            TypedMatchPredicate::RoleEdge {
                relation,
                role_name,
                player,
                ..
            } => Ok(format!(
                "{} links ({}: {})",
                binding_variable(*relation),
                role_name,
                binding_variable(*player)
            )),
            TypedMatchPredicate::And { expressions } => expressions
                .iter()
                .map(|child| self.render_typed_predicate(fields, child, parameters))
                .collect::<Result<Vec<_>, _>>()
                .map(|children| children.join("; ")),
            TypedMatchPredicate::Or { expressions } => expressions
                .iter()
                .map(|child| {
                    self.render_typed_predicate(fields, child, parameters)
                        .map(|child| format!("{{ {child}; }}"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|children| children.join(" or ")),
            TypedMatchPredicate::Not { expression } => self
                .render_typed_predicate(fields, expression, parameters)
                .map(|child| format!("not {{ {child}; }}")),
        }
    }

    /// Compile a single clause into its TypeQL string representation.
    pub fn compile_clause(&self, clause: &Clause) -> String {
        match clause {
            Clause::Match(patterns) => {
                let p_str = patterns
                    .iter()
                    .map(|p| self.compile_pattern(p))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("match\n{};", p_str)
            }
            Clause::Insert(statements) => {
                let s_str = statements
                    .iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("insert\n{};", s_str)
            }
            Clause::Put(statements) => {
                let s_str = statements
                    .iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("put\n{};", s_str)
            }
            Clause::Delete(statements) => {
                let s_str = statements
                    .iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("delete\n{};", s_str)
            }
            Clause::Update(statements) => {
                let s_str = statements
                    .iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("update\n{};", s_str)
            }
            Clause::Fetch(items) => {
                let i_str = items
                    .iter()
                    .map(|i| self.compile_fetch_item(i))
                    .collect::<Vec<_>>()
                    .join(",\n  ");
                format!("fetch {{\n  {}\n}};", i_str)
            }
            Clause::MatchLet(assignments) => {
                let a_str = assignments
                    .iter()
                    .map(|a| self.compile_let_assignment(a))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("match\n{};", a_str)
            }
            Clause::Reduce {
                assignments,
                group_by,
            } => {
                let a_str = assignments
                    .iter()
                    .map(|a| self.compile_reduce_assignment(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut res = format!("reduce {}", a_str);
                if let Some(gb) = group_by {
                    res.push_str(&format!(" groupby {}", gb));
                }
                res.push(';');
                res
            }
            Clause::Sort(fields) => {
                let f_str = fields
                    .iter()
                    .map(|f| self.compile_sort_field(f))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("sort {};", f_str)
            }
            Clause::Limit(n) => format!("limit {};", n),
            Clause::Offset(n) => format!("offset {};", n),
        }
    }

    fn compile_pattern(&self, pattern: &Pattern) -> String {
        match pattern {
            Pattern::Entity {
                variable,
                type_name,
                constraints,
                is_strict,
            } => {
                let op = if *is_strict { "isa!" } else { "isa" };
                let mut parts = vec![format!("{} {} {}", variable, op, type_name)];
                for c in constraints {
                    parts.push(self.compile_constraint(c));
                }
                parts.join(", ")
            }
            Pattern::Relation {
                variable,
                type_name,
                role_players,
                constraints,
            } => {
                let roles_str = if !role_players.is_empty() {
                    let r_str = role_players
                        .iter()
                        .map(|rp| format!("{}: {}", rp.role, rp.player_var))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({}) ", r_str)
                } else {
                    "".to_string()
                };
                let mut parts = vec![
                    format!("{} isa {} {}", variable, type_name, roles_str)
                        .trim()
                        .to_string(),
                ];
                for c in constraints {
                    parts.push(self.compile_constraint(c));
                }
                parts.join(", ")
            }
            Pattern::SubType {
                variable,
                parent_type,
            } => format!("{} sub {}", variable, parent_type),
            Pattern::Has {
                thing_var,
                attr_type,
                attr_var,
            } => format!("{} has {} {}", thing_var, attr_type, attr_var),
            Pattern::ValueComparison {
                var,
                operator,
                value,
            } => format!("{} {} {}", var, operator, self.compile_value(value)),
            Pattern::Not(patterns) => {
                let inner = patterns
                    .iter()
                    .map(|p| self.compile_pattern(p))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("not {{ {}; }}", inner)
            }
            Pattern::Try(patterns) => {
                let inner = patterns
                    .iter()
                    .map(|p| self.compile_pattern(p))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("try {{ {}; }}", inner)
            }
            Pattern::Or(alternatives) => alternatives
                .iter()
                .map(|alt| {
                    let inner = alt
                        .iter()
                        .map(|p| self.compile_pattern(p))
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("{{ {}; }}", inner)
                })
                .collect::<Vec<_>>()
                .join(" or "),
            Pattern::Iid { variable, iid } => format!("{} iid {}", variable, iid),
            Pattern::Attribute {
                variable,
                type_name,
                value,
            } => {
                if let Some(v) = value {
                    format!(
                        "{} isa {}; {} {}",
                        variable,
                        type_name,
                        variable,
                        self.compile_value(v)
                    )
                } else {
                    format!("{} isa {}", variable, type_name)
                }
            }
            Pattern::Raw(content) => content.clone(),
        }
    }

    fn compile_statement(&self, stmt: &Statement) -> String {
        match stmt {
            Statement::Has {
                subject_var,
                attr_name,
                value,
            } => format!(
                "{} has {} {}",
                subject_var,
                attr_name,
                self.compile_value(value)
            ),
            Statement::Isa {
                variable,
                type_name,
            } => format!("{} isa {}", variable, type_name),
            Statement::Relation {
                variable,
                type_name,
                role_players,
                include_variable,
                attributes,
            } => {
                let r_str = role_players
                    .iter()
                    .map(|rp| format!("{}: {}", rp.role, rp.player_var))
                    .collect::<Vec<_>>()
                    .join(", ");
                let roles_str = format!("({})", r_str);
                let base = if *include_variable {
                    format!("{} isa {}, links {}", variable, type_name, roles_str)
                } else {
                    format!("{} isa {}", roles_str, type_name)
                };
                if attributes.is_empty() {
                    base
                } else {
                    let a_str = attributes
                        .iter()
                        .map(|a| {
                            if let Statement::Has {
                                attr_name, value, ..
                            } = a
                            {
                                format!("has {} {}", attr_name, self.compile_value(value))
                            } else {
                                "".to_string()
                            }
                        })
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}, {}", base, a_str)
                }
            }
            Statement::DeleteThing(variable) => variable.clone(),
            Statement::Raw(content) => content.clone(),
        }
    }

    fn compile_constraint(&self, constraint: &Constraint) -> String {
        match constraint {
            Constraint::Iid(iid) => format!("iid {}", iid),
            Constraint::Has { attr_name, value } => {
                format!("has {} {}", attr_name, self.compile_value(value))
            }
            Constraint::Isa { type_name, strict } => {
                format!("{} {}", if *strict { "isa!" } else { "isa" }, type_name)
            }
        }
    }

    fn compile_value(&self, value: &Value) -> String {
        match value {
            Value::Literal(lit) => self.format_literal(lit),
            Value::Variable(var) => var.clone(),
            Value::FunctionCall(func) => {
                let args = func
                    .args
                    .iter()
                    .map(|a| self.compile_value(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", func.function, args)
            }
            Value::Arithmetic(arith) => format!(
                "({} {} {})",
                self.compile_value(&arith.left),
                arith.operator,
                self.compile_value(&arith.right)
            ),
        }
    }

    fn format_literal(&self, lit: &LiteralValue) -> String {
        match &lit.value {
            serde_json::Value::String(s) => {
                if lit.value_type == "string" {
                    format!(
                        "\"{}\"",
                        s.replace("\\", "\\\\")
                            .replace("\"", "\\\"")
                            .replace("\n", "\\n")
                            .replace("\r", "\\r")
                            .replace("\t", "\\t")
                    )
                } else if lit.value_type == "decimal" {
                    s.strip_suffix("dec")
                        .map_or_else(|| format!("{s}dec"), |value| format!("{value}dec"))
                } else {
                    // Dates, datetimes, durations are passed as strings but not quoted in TypeQL
                    s.clone()
                }
            }
            serde_json::Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            serde_json::Value::Number(n) => {
                if lit.value_type == "decimal" {
                    format!("{}dec", n)
                } else {
                    n.to_string()
                }
            }
            _ => lit.value.to_string(),
        }
    }

    fn compile_let_assignment(&self, assign: &LetAssignment) -> String {
        let vars = assign.variables.join(", ");
        let op = if assign.is_stream { "in" } else { "=" };
        format!(
            "let {} {} {}",
            vars,
            op,
            self.compile_value(&assign.expression)
        )
    }

    fn compile_fetch_item(&self, item: &FetchItem) -> String {
        match item {
            FetchItem::Attribute {
                key,
                var,
                attr_name,
            } => format!("\"{}\": {}.{}", key, var, attr_name),
            FetchItem::Variable { key, var } => format!("\"{}\": {}", key, var),
            FetchItem::AttributeList {
                key,
                var,
                attr_name,
            } => format!("\"{}\": [{}.{}]", key, var, attr_name),
            FetchItem::Function {
                key,
                func_name,
                var,
            } => format!("\"{}\": {}({})", key, func_name, var),
            FetchItem::Wildcard { key, var } => format!("\"{}\": {}.*", key, var),
            FetchItem::NestedWildcard { key, var } => format!("\"{}\": {{ {}.* }}", key, var),
        }
    }

    fn compile_reduce_assignment(&self, assign: &ReduceAssignment) -> String {
        format!(
            "{} = {}",
            assign.variable,
            self.compile_value(&assign.expression)
        )
    }

    fn compile_sort_field(&self, field: &SortField) -> String {
        let dir = if field.ascending { "asc" } else { "desc" };
        format!("{} {}", field.variable, dir)
    }
}

fn binding_variable(binding: u16) -> String {
    format!("$b{binding}")
}

fn field_variable(field: u16) -> String {
    format!("$f{field}")
}

fn type_variable(binding: u16) -> String {
    format!("$t{binding}")
}

fn typed_target_patterns(targets: &[TypedMatchTarget]) -> Vec<String> {
    targets
        .iter()
        .map(|target| {
            format!(
                "{} {} {}",
                binding_variable(target.binding),
                if target.exact { "isa!" } else { "isa" },
                target.type_name
            )
        })
        .collect()
}

fn compile_typed_order(order: &[crate::ast::TypedMatchOrder]) -> String {
    order
        .iter()
        .map(|order| {
            let direction = match order.direction {
                TypedSortDirection::Ascending => "asc",
                TypedSortDirection::Descending => "desc",
            };
            format!("{} {direction}", field_variable(order.field))
        })
        .collect::<Vec<_>>()
        .join(", ")
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
        TypedMatchPredicate::And { expressions } | TypedMatchPredicate::Or { expressions } => {
            if expressions.is_empty() {
                return Err(TypedCompileError::new(
                    "typed boolean predicate cannot be empty",
                ));
            }
            for child in expressions {
                validate_typed_predicate(child, targets, fields, query)?;
            }
        }
        TypedMatchPredicate::Not { expression } => {
            validate_typed_predicate(expression, targets, fields, query)?
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

fn compile_typed_value_comparison(
    field: u16,
    operator: TypedComparisonOperator,
    value: &TypedLiteral,
) -> String {
    let field = field_variable(field);
    match (operator, value) {
        (TypedComparisonOperator::StartsWith, TypedLiteral::String(value)) => format!(
            "{field} like {}",
            compile_typed_literal(&TypedLiteral::String(format!(
                "^{}.*",
                regex::escape(value)
            )))
        ),
        (TypedComparisonOperator::EndsWith, TypedLiteral::String(value)) => format!(
            "{field} like {}",
            compile_typed_literal(&TypedLiteral::String(format!(
                ".*{}$",
                regex::escape(value)
            )))
        ),
        _ => format!(
            "{field} {} {}",
            comparison_token(operator),
            compile_typed_literal(value)
        ),
    }
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

fn prepared_typed_value_type(value: &TypedLiteral) -> &'static str {
    match value {
        TypedLiteral::String(_) => "string",
        TypedLiteral::Long(_) => "integer",
        TypedLiteral::Double(_) => "double",
        TypedLiteral::Boolean(_) => "boolean",
        TypedLiteral::Date(_)
        | TypedLiteral::DateTime(_)
        | TypedLiteral::DateTimeTz(_)
        | TypedLiteral::Decimal(_)
        | TypedLiteral::Duration(_) => {
            unreachable!("unsupported prepared values remain inline")
        }
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

fn compile_typed_literal(value: &TypedLiteral) -> String {
    match value {
        TypedLiteral::String(value) => format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        ),
        TypedLiteral::Long(value) => value.to_string(),
        TypedLiteral::Double(value) => value.to_string(),
        TypedLiteral::Boolean(value) => value.to_string(),
        TypedLiteral::Date(value)
        | TypedLiteral::DateTime(value)
        | TypedLiteral::DateTimeTz(value)
        | TypedLiteral::Duration(value) => value.clone(),
        TypedLiteral::Decimal(value) => value
            .strip_suffix("dec")
            .map_or_else(|| format!("{value}dec"), |value| format!("{value}dec")),
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
        assert_eq!(c.compile_pattern(&p), "$a isa name; $a \"John\"");
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
        assert_eq!(c.compile_pattern(&p), "not { $p isa person; }");
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
        assert_eq!(c.compile_clause(&clause), "match\n$p isa person;");
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
            "match\n$p isa person;\n$p has name $n;"
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
        assert_eq!(c.compile_clause(&clause), "insert\n$p has name \"Alice\";");
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
            "put\n$p isa person;\n$p has name \"Alice\";"
        );
    }

    #[test]
    fn test_clause_delete() {
        let c = compiler();
        let clause = Clause::Delete(vec![Statement::DeleteThing("$p".into())]);
        assert_eq!(c.compile_clause(&clause), "delete\n$p;");
    }

    #[test]
    fn test_clause_update() {
        let c = compiler();
        let clause = Clause::Update(vec![Statement::Has {
            subject_var: "$p".into(),
            attr_name: "age".into(),
            value: lit(json!(31), "long"),
        }]);
        assert_eq!(c.compile_clause(&clause), "update\n$p has age 31;");
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
            "fetch {\n  \"name\": $p.name,\n  \"person\": $p\n};"
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
        assert!(result.contains("\"emails\": [$p.email]"));
        assert!(result.contains("\"count\": count($p)"));
        assert!(result.contains("\"all\": $p.*"));
        assert!(result.contains("\"nested\": { $p.* }"));
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
        assert_eq!(c.compile_clause(&clause), "match\nlet $x = count($p);");
    }

    #[test]
    fn test_clause_match_let_stream() {
        let c = compiler();
        let clause = Clause::MatchLet(vec![LetAssignment {
            variables: vec!["$x".into()],
            expression: Value::FunctionCall(FunctionCallValue {
                function: "fetch".into(),
                args: vec![Value::Variable("$p".into())],
            }),
            is_stream: true,
        }]);
        assert_eq!(c.compile_clause(&clause), "match\nlet $x in fetch($p);");
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
        assert!(result.starts_with("match\n"));
        assert!(result.contains("insert\n"));
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
        assert!(result.starts_with("match\n"));
        assert!(result.contains("put\n"));
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
        assert_eq!(result, "match\n$p isa person;\ndelete\n$p;");
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
        assert!(result.starts_with("match\n"));
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
        assert!(result.starts_with("match\n"));
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
        roundtrip("insert\n$p isa person;\n$p has name \"Alice\";");
    }

    #[test]
    fn test_roundtrip_relation_insert() {
        roundtrip("insert\n$r isa employment, links (employee: $p, employer: $c);");
    }

    #[test]
    fn test_roundtrip_put() {
        roundtrip("put\n$p isa person;\n$p has name \"Alice\";");
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
        roundtrip("match $p isa person;\nfetch {\n  \"\": $p.*\n};");
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
        roundtrip("match $p isa person;\nsort $age asc;");
    }

    #[test]
    fn test_roundtrip_sort_multiple() {
        roundtrip("match $p isa person;\nsort $name asc, $age desc;");
    }

    #[test]
    fn test_roundtrip_limit() {
        roundtrip("match $p isa person;\nlimit 10;");
    }

    #[test]
    fn test_roundtrip_offset() {
        roundtrip("match $p isa person;\noffset 20;");
    }

    #[test]
    fn test_roundtrip_sort_limit_offset() {
        roundtrip("match $p isa person;\nsort $age asc;\nlimit 10;\noffset 5;");
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
                        "given $g0: {value_type};\n\
                         match\n\
                         $b0 isa person;\n\
                         $b0 has score $f0; $f0 == $g0;\n\
                         select $b0;\n\
                         distinct;\n\
                         limit 1;"
                    ),
                    parameters: vec![TypedQueryParameter {
                        name: "g0".into(),
                        value,
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
            "match\n\
             $b0 isa person;\n\
             $b0 has score $f0; $f0 == \"needle\";\n\
             select $b0;\n\
             distinct;\n\
             limit 1;"
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
            concat!(
                "given $g0: string;\n",
                "match\n",
                "$b0 isa person;\n",
                r#"$b0 has score $f0; $f0 like "^A\\.\\+%.*"; { $b0 has score $f0; $f0 contains $g0; } or { not { $b0 has score $f0; $f0 like ".*\\[x\\]\\$50%$"; }; }; $b0 has score $f0; $f0 like "^A[[:alpha:]]+%$";"#,
                "\nselect $b0;\n",
                "distinct;\n",
                "limit 1;",
            )
        );
        assert_eq!(
            prepared.parameters,
            vec![TypedQueryParameter {
                name: "g0".into(),
                value: TypedLiteral::String("50%_.*".into()),
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
            "given $g0: integer, $g1: boolean;\n\
             match\n\
             $b0 isa person;\n\
             $b0 has score $f0; $f0 == 1.25dec; $b0 has score $f0; $f0 == $g0; $b0 has score $f0; $f0 == P1D; $b0 has score $f0; $f0 == $g1;\n\
             select $b0;\n\
             distinct;\n\
             limit 1;"
        );
        assert_eq!(
            prepared.parameters,
            vec![
                TypedQueryParameter {
                    name: "g0".into(),
                    value: TypedLiteral::Long(42),
                },
                TypedQueryParameter {
                    name: "g1".into(),
                    value: TypedLiteral::Boolean(true),
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
            "given $g0: string;\n\
             match\n\
             $b0 isa person;\n\
             $b0 has score $f0; $f0 == $g0;\n\
             select $b0;\n\
             distinct;\n\
             limit 2;"
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
            "given $g0: string;\n\
             match\n\
             $b0 isa person;\n\
             $b0 has score $f0; $f0 == $g0;\n\
             { $b0 iid 0x01; };\n\
             $b0 isa! $t0;\n\
             fetch { \"b0\": { \"concept_id\": iid($b0), \"concrete_type\": label($t0), \"attributes\": { $b0.* } } };"
        );
        assert_eq!(prepared_root.parameters, prepared_rematch.parameters);
        assert_eq!(
            prepared_root.parameters,
            vec![TypedQueryParameter {
                name: "g0".into(),
                value: TypedLiteral::String("needle".into()),
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
        assert!(rendered.contains("select $b0, $b1, $b2, $f1, $f0;\ndistinct;"));
        assert!(rendered.contains("sort $f1 desc, $f0 asc;"));
        assert!(rendered.ends_with("offset 5;\nlimit 10;"));
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
        assert!(rendered.contains("select $b0, $f0;\ndistinct;"));
        assert!(rendered.contains("sort $f0 asc;\noffset 2;\nlimit 3;"));

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
        assert!(rendered.contains("$thing links ($role: $player)"));
        assert!(!rendered.contains("relation $type"));
        assert!(rendered.contains("$player isa! $player_type"));

        let entity = TypedHydrateThings {
            targets: vec![mixed.targets[1].clone()],
        };
        let rendered = compiler().compile_typed_hydrate_things(&entity).unwrap();
        assert!(rendered.contains("$thing iid 0x02; $thing isa person; let $binding = 4"));
        assert!(rendered.contains("$thing isa! $type"));
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
