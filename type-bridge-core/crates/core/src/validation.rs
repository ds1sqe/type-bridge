use crate::ast::{Clause, Constraint, FetchItem, Pattern, Statement, Value};
use crate::reserved_words::is_reserved_word;
use crate::schema::TypeSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use unicode_ident::{is_xid_continue, is_xid_start};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// The severity level of a validation diagnostic.
///
/// `Error` indicates a hard failure that makes the result invalid.
/// `Warning` indicates a potential issue that does not invalidate the result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// A hard validation error. Any result containing at least one `Error`
    /// is considered invalid (`is_valid == false`).
    Error,
    /// A soft warning. Warnings are reported but do not cause the overall
    /// validation result to be invalid.
    Warning,
}

/// The outcome of a validation pass.
///
/// Contains a boolean `is_valid` flag (true when no `Error`-severity
/// diagnostics are present) and the full list of diagnostics (errors and
/// warnings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the validated input is considered valid.
    ///
    /// This is `true` when there are no diagnostics with
    /// [`ValidationSeverity::Error`]; warnings alone do not invalidate.
    pub is_valid: bool,
    /// The list of validation diagnostics (errors and warnings).
    pub errors: Vec<ValidationError>,
}

/// A single validation diagnostic produced by the engine.
///
/// Each error carries a machine-readable `code`, a human-readable `message`,
/// a `path` indicating where in the query/data the issue was found, and a
/// `severity` level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    /// A machine-readable error code (e.g. `"UNKNOWN_TYPE"`,
    /// `"RULE_REQUIRED"`).
    pub code: String,
    /// A human-readable description of the issue.
    pub message: String,
    /// A dot-separated path indicating the location of the issue within the
    /// validated structure (e.g. `"clauses[0].patterns[1].constraints[0]"`).
    pub path: String,
    /// The severity of this diagnostic (`Error` or `Warning`).
    pub severity: ValidationSeverity,
}

fn error(code: &str, message: String, path: &str) -> ValidationError {
    ValidationError {
        code: code.to_string(),
        message,
        path: path.to_string(),
        severity: ValidationSeverity::Error,
    }
}

fn warning(code: &str, message: String, path: &str) -> ValidationError {
    ValidationError {
        code: code.to_string(),
        message,
        path: path.to_string(),
        severity: ValidationSeverity::Warning,
    }
}

fn make_result(errors: Vec<ValidationError>) -> ValidationResult {
    let is_valid = !errors
        .iter()
        .any(|e| e.severity == ValidationSeverity::Error);
    ValidationResult { is_valid, errors }
}

// ---------------------------------------------------------------------------
// Type environment (variable → type mapping)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TypeEnvironment {
    /// Maps variable name (e.g. "$p") to type name (e.g. "person").
    var_types: HashMap<String, String>,
}

impl TypeEnvironment {
    fn new() -> Self {
        TypeEnvironment {
            var_types: HashMap::new(),
        }
    }

    fn bind(&mut self, var: &str, type_name: &str) {
        self.var_types
            .insert(var.to_string(), type_name.to_string());
    }

    fn get_type(&self, var: &str) -> Option<&str> {
        self.var_types.get(var).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Value type compatibility
// ---------------------------------------------------------------------------

/// Check if a literal value type is compatible with a schema value type.
fn value_types_compatible(literal_type: &str, schema_type: &str) -> bool {
    if literal_type == schema_type {
        return true;
    }
    // long → double (implicit widening)
    if literal_type == "long" && schema_type == "double" {
        return true;
    }
    // long/double → decimal
    if (literal_type == "long" || literal_type == "double") && schema_type == "decimal" {
        return true;
    }
    // integer is an alias for long in some contexts
    if literal_type == "integer"
        && (schema_type == "long" || schema_type == "double" || schema_type == "decimal")
    {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Custom validation rules (JSON DSL)
// ---------------------------------------------------------------------------

/// The target that a custom validation rule applies to.
///
/// Rules can target either any instance of a particular attribute type, or
/// a specific entity-attribute combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum RuleTarget {
    /// Applies to any instance of this attribute type, regardless of which
    /// entity or relation owns it.
    Attribute {
        /// The name of the attribute type this rule targets.
        attribute: String,
    },
    /// Applies only when a specific entity type owns the attribute.
    EntityAttribute {
        /// The name of the entity type that must own the attribute for the
        /// rule to apply.
        entity: String,
        /// The name of the attribute type this rule targets.
        attribute: String,
    },
}

/// The kind of constraint that a custom validation rule enforces.
///
/// Each variant corresponds to a different check (presence, pattern matching,
/// numeric range, allowed values, cardinality, or string length).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum RuleType {
    /// Attribute must be present (non-null, non-missing).
    Required,
    /// String must match the given regex pattern.
    Regex {
        /// The regular expression pattern that the string value must match.
        pattern: String,
    },
    /// Numeric value must be within [min, max]. Either bound can be None.
    Range {
        /// The minimum allowed value (inclusive), or `None` for no lower bound.
        min: Option<f64>,
        /// The maximum allowed value (inclusive), or `None` for no upper bound.
        max: Option<f64>,
    },
    /// Value must be one of the allowed values.
    Values {
        /// The set of allowed values. Any value not in this list is rejected.
        allowed: Vec<serde_json::Value>,
    },
    /// Multi-value attribute count must be within [min, max].
    Cardinality {
        /// The minimum number of values required.
        min: u32,
        /// The maximum number of values allowed, or `None` for no upper bound.
        max: Option<u32>,
    },
    /// String length must be within [min, max].
    Length {
        /// The minimum string length (inclusive), or `None` for no lower bound.
        min: Option<u32>,
        /// The maximum string length (inclusive), or `None` for no upper bound.
        max: Option<u32>,
    },
}

/// A single custom validation rule.
///
/// Each rule has a unique `id`, a [`RuleTarget`] that determines which
/// attributes it applies to, a [`RuleType`] that specifies the constraint,
/// and an optional custom error message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// A unique identifier for this rule (e.g. `"email-regex"`).
    pub id: String,
    /// The target that this rule applies to (attribute or entity-attribute).
    pub target: RuleTarget,
    /// The type of constraint this rule enforces.
    pub rule_type: RuleType,
    /// An optional custom error message. When set, this message is used
    /// instead of the auto-generated default.
    #[serde(default)]
    pub error_message: Option<String>,
}

/// A named collection of custom validation rules.
///
/// This is the top-level structure used for JSON serialization/deserialization
/// of rule sets via [`ValidationEngine::load_rules`] and
/// [`ValidationEngine::export_rules`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRuleSet {
    /// The list of validation rules in this set.
    pub rules: Vec<ValidationRule>,
}

/// Extract attribute values from entity data, normalizing single values to a vec.
fn extract_values<'a>(
    data: &'a serde_json::Map<String, serde_json::Value>,
    attr_name: &str,
) -> Vec<&'a serde_json::Value> {
    match data.get(attr_name) {
        Some(serde_json::Value::Array(arr)) => arr.iter().collect(),
        Some(v) if !v.is_null() => vec![v],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// ValidationEngine
// ---------------------------------------------------------------------------

/// The main validation engine for TypeQL queries and entity data.
///
/// `ValidationEngine` provides two categories of validation:
///
/// 1. **Syntactic validation** -- checks type names, variable names, patterns,
///    and statements for well-formedness (e.g. reserved words, valid
///    identifiers, `$`-prefixed variables).
///
/// 2. **Schema-aware semantic validation** -- given a [`TypeSchema`], checks
///    that types exist, ownership is valid, role players match declared roles,
///    value types are compatible, abstract types are not instantiated, and
///    cardinality constraints are respected.
///
/// Additionally, the engine supports **custom validation rules** defined via a
/// portable JSON DSL. Rules can enforce required fields, regex patterns,
/// numeric ranges, allowed value sets, cardinality bounds, and string length
/// limits on entity data.
pub struct ValidationEngine {
    rules: Vec<ValidationRule>,
    /// Pre-compiled regex cache: rule_id → compiled Regex.
    regex_cache: HashMap<String, regex::Regex>,
}

impl Default for ValidationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationEngine {
    /// Create a new `ValidationEngine` with no custom rules loaded.
    pub fn new() -> Self {
        ValidationEngine {
            rules: Vec::new(),
            regex_cache: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Rule management
    // -----------------------------------------------------------------------

    /// Add a single custom validation rule to the engine.
    ///
    /// If the rule contains a [`RuleType::Regex`] variant, the pattern is
    /// pre-compiled and cached. Returns `Err` with a description if the regex
    /// pattern is invalid; in that case the rule is **not** added.
    pub fn add_rule(&mut self, rule: ValidationRule) -> Result<(), String> {
        if let RuleType::Regex { ref pattern } = rule.rule_type {
            let compiled = regex::Regex::new(pattern)
                .map_err(|e| format!("Invalid regex in rule '{}': {}", rule.id, e))?;
            self.regex_cache.insert(rule.id.clone(), compiled);
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Load a set of custom validation rules from a JSON string.
    ///
    /// The JSON must conform to the [`ValidationRuleSet`] schema. Rules with
    /// invalid regex patterns are skipped (not added) and their error messages
    /// are returned as warnings. Returns `Err` if the JSON itself cannot be
    /// parsed.
    pub fn load_rules(&mut self, json_str: &str) -> Result<Vec<String>, String> {
        let ruleset: ValidationRuleSet = serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse rules JSON: {}", e))?;
        let mut warnings = Vec::new();
        for rule in ruleset.rules {
            if let Err(e) = self.add_rule(rule) {
                warnings.push(e);
            }
        }
        Ok(warnings)
    }

    /// Export all currently loaded rules as a pretty-printed JSON string.
    ///
    /// The output conforms to the [`ValidationRuleSet`] schema and can be
    /// re-loaded with [`load_rules`](Self::load_rules).
    pub fn export_rules(&self) -> Result<String, String> {
        let ruleset = ValidationRuleSet {
            rules: self.rules.clone(),
        };
        serde_json::to_string_pretty(&ruleset).map_err(|e| e.to_string())
    }

    /// Remove all custom validation rules and clear the regex cache.
    pub fn clear_rules(&mut self) {
        self.rules.clear();
        self.regex_cache.clear();
    }

    /// Return the number of currently loaded custom validation rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    // -----------------------------------------------------------------------
    // Syntactic validation (unchanged API, now with severity field)
    // -----------------------------------------------------------------------

    /// Validate that a variable name is well-formed.
    ///
    /// A valid variable name must start with `$` and have at least one
    /// character after it. Returns a list of [`ValidationError`] diagnostics
    /// (empty if the name is valid).
    ///
    /// # Arguments
    ///
    /// * `name` -- The variable name to validate (e.g. `"$p"`).
    /// * `context` -- A human-readable context string for error messages
    ///   (e.g. `"Entity"`, `"Relation"`).
    /// * `path` -- The structural path for error reporting.
    pub fn validate_variable_name(
        &self,
        name: &str,
        context: &str,
        path: &str,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if !name.starts_with('$') {
            errors.push(error(
                "INVALID_VARIABLE",
                format!("Variable '{}' must start with '$' in {}", name, context),
                path,
            ));
        } else if name.len() == 1 {
            errors.push(error(
                "EMPTY_VARIABLE",
                format!("Variable name cannot be just '$' in {}", context),
                path,
            ));
        }
        errors
    }

    /// Validate that a type name is well-formed.
    ///
    /// Checks that the name is non-empty, is not a TypeQL reserved word,
    /// starts with a letter or underscore, and contains only valid identifier
    /// characters (Unicode XID_Continue plus hyphen `-`).
    ///
    /// # Arguments
    ///
    /// * `name` -- The type name to validate (e.g. `"person"`,
    ///   `"first-name"`).
    /// * `context` -- A human-readable context string for error messages
    ///   (e.g. `"Entity type"`, `"Attribute type"`).
    pub fn validate_type_name(&self, name: &str, context: &str) -> ValidationResult {
        let mut errors = Vec::new();

        if name.is_empty() {
            errors.push(error(
                "EMPTY_NAME",
                format!("Empty {} name is not allowed", context),
                "",
            ));
            return make_result(errors);
        }

        if is_reserved_word(name) {
            errors.push(error(
                "RESERVED_WORD",
                format!(
                    "Cannot use '{}' as {} name: it's a TypeQL reserved word!",
                    name, context
                ),
                "",
            ));
        }

        let mut chars = name.chars();
        if let Some(first) = chars.next()
            && !is_xid_start(first)
            && first != '_'
        {
            errors.push(error(
                "INVALID_START",
                format!(
                    "{} name '{}' must start with a letter or underscore",
                    context, name
                ),
                "",
            ));
        }

        for c in chars {
            if !is_xid_continue(c) && c != '-' {
                errors.push(error(
                    "INVALID_CHAR",
                    format!(
                        "{} name '{}' contains invalid character '{}'",
                        context, name, c
                    ),
                    "",
                ));
                break;
            }
        }

        make_result(errors)
    }

    fn check_type_name(&self, name: &str, context: &str, path: &str) -> Vec<ValidationError> {
        let res = self.validate_type_name(name, context);
        res.errors
            .into_iter()
            .map(|mut e| {
                e.path = path.to_string();
                e
            })
            .collect()
    }

    /// Validate the syntactic structure of a single pattern.
    ///
    /// Recursively checks all variable names, type names, constraints, and
    /// nested patterns (e.g. `Not`, `Or`) within the given [`Pattern`].
    /// Does **not** perform schema-aware checks; use [`validate_query`](Self::validate_query)
    /// for semantic validation.
    pub fn validate_pattern(&self, pattern: &Pattern) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_pattern_recursive(pattern, "pattern", &mut errors);
        make_result(errors)
    }

    fn validate_pattern_recursive(
        &self,
        pattern: &Pattern,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match pattern {
            Pattern::Entity {
                variable,
                type_name,
                constraints,
                ..
            } => {
                errors.extend(self.validate_variable_name(variable, "Entity", path));
                errors.extend(self.check_type_name(type_name, "Entity type", path));
                for (i, c) in constraints.iter().enumerate() {
                    self.validate_constraint_recursive(
                        c,
                        &format!("{}.constraints[{}]", path, i),
                        errors,
                    );
                }
            }
            Pattern::Relation {
                variable,
                type_name,
                role_players,
                constraints,
            } => {
                errors.extend(self.validate_variable_name(variable, "Relation", path));
                errors.extend(self.check_type_name(type_name, "Relation type", path));
                for (i, rp) in role_players.iter().enumerate() {
                    let rp_path = format!("{}.role_players[{}]", path, i);
                    errors.extend(self.check_type_name(&rp.role, "Role", &rp_path));
                    errors.extend(self.validate_variable_name(&rp.player_var, "Player", &rp_path));
                }
                for (i, c) in constraints.iter().enumerate() {
                    self.validate_constraint_recursive(
                        c,
                        &format!("{}.constraints[{}]", path, i),
                        errors,
                    );
                }
            }
            Pattern::SubType {
                variable,
                parent_type,
            } => {
                errors.extend(self.validate_variable_name(variable, "SubType variable", path));
                errors.extend(self.check_type_name(parent_type, "Parent type", path));
            }
            Pattern::Attribute {
                variable,
                type_name,
                value,
            } => {
                errors.extend(self.validate_variable_name(variable, "Attribute variable", path));
                errors.extend(self.check_type_name(type_name, "Attribute type", path));
                if let Some(v) = value {
                    self.validate_value_recursive(v, &format!("{}.value", path), errors);
                }
            }
            Pattern::Has {
                thing_var,
                attr_type,
                attr_var,
            } => {
                errors.extend(self.validate_variable_name(thing_var, "Subject", path));
                errors.extend(self.check_type_name(attr_type, "Attribute type", path));
                errors.extend(self.validate_variable_name(attr_var, "Attribute variable", path));
            }
            Pattern::ValueComparison { var, value, .. } => {
                errors.extend(self.validate_variable_name(var, "Variable", path));
                self.validate_value_recursive(value, &format!("{}.value", path), errors);
            }
            Pattern::Not(patterns) => {
                for (i, p) in patterns.iter().enumerate() {
                    self.validate_pattern_recursive(p, &format!("{}.not[{}]", path, i), errors);
                }
            }
            Pattern::Try(patterns) => {
                for (i, p) in patterns.iter().enumerate() {
                    self.validate_pattern_recursive(p, &format!("{}.try[{}]", path, i), errors);
                }
            }
            Pattern::Or(alternatives) => {
                for (i, alt) in alternatives.iter().enumerate() {
                    for (j, p) in alt.iter().enumerate() {
                        self.validate_pattern_recursive(
                            p,
                            &format!("{}.or[{}][{}]", path, i, j),
                            errors,
                        );
                    }
                }
            }
            Pattern::Iid { variable, .. } => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
            }
            Pattern::Raw(_) => {}
        }
    }

    /// Validate the syntactic structure of a single statement.
    ///
    /// Recursively checks variable names, type names, role players, and
    /// inline attributes within the given [`Statement`]. Does **not** perform
    /// schema-aware checks.
    pub fn validate_statement(&self, statement: &Statement) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_statement_recursive(statement, "statement", &mut errors);
        make_result(errors)
    }

    fn validate_statement_recursive(
        &self,
        statement: &Statement,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match statement {
            Statement::Has {
                subject_var,
                attr_name,
                value,
            } => {
                errors.extend(self.validate_variable_name(subject_var, "Subject", path));
                errors.extend(self.check_type_name(attr_name, "Attribute type", path));
                self.validate_value_recursive(value, &format!("{}.value", path), errors);
            }
            Statement::Isa {
                variable,
                type_name,
            } => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
                errors.extend(self.check_type_name(type_name, "Type", path));
            }
            Statement::Relation {
                variable,
                type_name,
                role_players,
                attributes,
                ..
            } => {
                errors.extend(self.validate_variable_name(variable, "Relation", path));
                errors.extend(self.check_type_name(type_name, "Relation type", path));
                for (i, rp) in role_players.iter().enumerate() {
                    let rp_path = format!("{}.role_players[{}]", path, i);
                    errors.extend(self.check_type_name(&rp.role, "Role", &rp_path));
                    errors.extend(self.validate_variable_name(&rp.player_var, "Player", &rp_path));
                }
                for (i, attr) in attributes.iter().enumerate() {
                    self.validate_statement_recursive(
                        attr,
                        &format!("{}.attributes[{}]", path, i),
                        errors,
                    );
                }
            }
            Statement::DeleteThing(variable) => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
            }
            Statement::Raw(_) => {}
        }
    }

    fn validate_constraint_recursive(
        &self,
        constraint: &Constraint,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match constraint {
            Constraint::Iid(_) => {}
            Constraint::Has { attr_name, value } => {
                errors.extend(self.check_type_name(attr_name, "Attribute type", path));
                self.validate_value_recursive(value, &format!("{}.value", path), errors);
            }
            Constraint::Isa { type_name, .. } => {
                errors.extend(self.check_type_name(type_name, "Type", path));
            }
        }
    }

    fn validate_value_recursive(
        &self,
        value: &Value,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match value {
            Value::Literal(_) => {}
            Value::Variable(v) => {
                errors.extend(self.validate_variable_name(v, "Value variable", path));
            }
            Value::FunctionCall(f) => {
                for (i, arg) in f.args.iter().enumerate() {
                    self.validate_value_recursive(arg, &format!("{}.args[{}]", path, i), errors);
                }
            }
            Value::Arithmetic(a) => {
                self.validate_value_recursive(&a.left, &format!("{}.left", path), errors);
                self.validate_value_recursive(&a.right, &format!("{}.right", path), errors);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Schema-aware query validation
    // -----------------------------------------------------------------------

    /// Validate a sequence of clauses against a schema.
    ///
    /// This performs both syntactic validation (names, variables) and semantic
    /// validation (ownership, roles, value types, abstract types, cardinality).
    ///
    /// The engine first builds a type environment by scanning all `Match`
    /// clauses to bind variables to their declared types, then validates each
    /// clause against the schema and the inferred environment.
    ///
    /// # Arguments
    ///
    /// * `clauses` -- The ordered list of query clauses (Match, Insert,
    ///   Delete, Update, Fetch, etc.).
    /// * `schema` -- The [`TypeSchema`] describing the database's type system.
    pub fn validate_query(&self, clauses: &[Clause], schema: &TypeSchema) -> ValidationResult {
        let mut errors = Vec::new();

        // Phase 1: Build type environment from all Match clauses.
        let mut env = TypeEnvironment::new();
        for clause in clauses {
            if let Clause::Match(patterns) = clause {
                self.build_type_env(patterns, schema, &mut env);
            }
        }

        // Phase 2: Validate each clause against schema + environment.
        for (i, clause) in clauses.iter().enumerate() {
            let path = format!("clauses[{}]", i);
            self.validate_clause_against_schema(clause, schema, &env, &path, &mut errors);
        }

        make_result(errors)
    }

    // -- Phase 1: build type environment -----------------------------------

    fn build_type_env(&self, patterns: &[Pattern], schema: &TypeSchema, env: &mut TypeEnvironment) {
        for pattern in patterns {
            match pattern {
                Pattern::Entity {
                    variable,
                    type_name,
                    ..
                } if schema.type_exists(type_name) => {
                    // The Entity pattern is used for any `$var isa type` match,
                    // so the type could be an entity, relation, or attribute.
                    env.bind(variable, type_name);
                }
                Pattern::Relation {
                    variable,
                    type_name,
                    ..
                } => {
                    env.bind(variable, type_name);
                }
                Pattern::Attribute {
                    variable,
                    type_name,
                    ..
                } => {
                    env.bind(variable, type_name);
                }
                Pattern::Has {
                    attr_type,
                    attr_var,
                    ..
                } if schema.attributes.contains_key(attr_type) => env.bind(attr_var, attr_type),
                Pattern::Or(alternatives) => {
                    // Conservative: only bind from first alternative.
                    if let Some(first) = alternatives.first() {
                        self.build_type_env(first, schema, env);
                    }
                }
                Pattern::Try(inner) => {
                    // Optional block: when matched it binds the same variables
                    // as a plain pattern, so its type info is usable downstream.
                    self.build_type_env(inner, schema, env);
                }
                // Not: don't bind (negation doesn't guarantee variable existence).
                // SubType, ValueComparison, Iid, Raw: no type info to extract.
                _ => {}
            }
        }
    }

    // -- Phase 2: per-clause validation ------------------------------------

    fn validate_clause_against_schema(
        &self,
        clause: &Clause,
        schema: &TypeSchema,
        env: &TypeEnvironment,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match clause {
            Clause::Match(patterns) => {
                for (i, p) in patterns.iter().enumerate() {
                    self.validate_pattern_against_schema(
                        p,
                        schema,
                        env,
                        &format!("{}.patterns[{}]", path, i),
                        errors,
                    );
                }
            }
            Clause::Insert(stmts) | Clause::Put(stmts) => {
                self.validate_insert_stmts(stmts, schema, env, path, errors);
            }
            Clause::Delete(stmts) | Clause::Update(stmts) => {
                for (i, s) in stmts.iter().enumerate() {
                    self.validate_statement_against_schema(
                        s,
                        schema,
                        env,
                        &format!("{}.stmts[{}]", path, i),
                        errors,
                    );
                }
            }
            Clause::Fetch(items) => {
                self.validate_fetch_items(items, schema, env, path, errors);
            }
            // MatchLet, Reduce: no schema validation needed.
            _ => {}
        }
    }

    // -- Pattern validation ------------------------------------------------

    fn validate_pattern_against_schema(
        &self,
        pattern: &Pattern,
        schema: &TypeSchema,
        env: &TypeEnvironment,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match pattern {
            Pattern::Entity {
                type_name,
                constraints,
                is_strict,
                ..
            } => {
                if !schema.type_exists(type_name) {
                    errors.push(error(
                        "UNKNOWN_TYPE",
                        format!("Type '{}' not found in schema", type_name),
                        path,
                    ));
                    return;
                }

                // Validate `has` constraints against ownership.
                for (i, c) in constraints.iter().enumerate() {
                    if let Constraint::Has { attr_name, value } = c {
                        self.validate_has_against_schema(
                            type_name,
                            attr_name,
                            value,
                            schema,
                            &format!("{}.constraints[{}]", path, i),
                            errors,
                        );
                    }
                }

                // Strict isa warning.
                if *is_strict {
                    self.check_strict_isa(type_name, schema, path, errors);
                }
            }

            Pattern::Relation {
                type_name,
                role_players,
                constraints,
                ..
            } => {
                if !schema.relations.contains_key(type_name) {
                    if schema.entities.contains_key(type_name)
                        || schema.attributes.contains_key(type_name)
                    {
                        errors.push(error(
                            "UNKNOWN_TYPE",
                            format!("'{}' is not a relation type", type_name),
                            path,
                        ));
                    } else {
                        errors.push(error(
                            "UNKNOWN_TYPE",
                            format!("Relation type '{}' not found in schema", type_name),
                            path,
                        ));
                    }
                    return;
                }

                // Validate role names.
                let all_roles = schema.get_all_relates(type_name);
                let role_names: HashSet<&str> = all_roles.iter().map(|r| r.name.as_str()).collect();

                for (i, rp) in role_players.iter().enumerate() {
                    let rp_path = format!("{}.role_players[{}]", path, i);

                    if !role_names.contains(rp.role.as_str()) {
                        let available: Vec<&str> = role_names.iter().copied().collect();
                        errors.push(error(
                            "UNKNOWN_ROLE",
                            format!(
                                "Role '{}' not found in relation '{}'. Available roles: {:?}",
                                rp.role, type_name, available
                            ),
                            &rp_path,
                        ));
                    }

                    // Role player type check.
                    if let Some(player_type) = env.get_type(&rp.player_var) {
                        self.validate_role_player_type(
                            player_type,
                            type_name,
                            &rp.role,
                            schema,
                            &rp_path,
                            errors,
                        );
                    }
                }

                // Validate `has` constraints on the relation.
                for (i, c) in constraints.iter().enumerate() {
                    if let Constraint::Has { attr_name, value } = c {
                        self.validate_has_against_schema(
                            type_name,
                            attr_name,
                            value,
                            schema,
                            &format!("{}.constraints[{}]", path, i),
                            errors,
                        );
                    }
                }
            }

            Pattern::Has {
                thing_var,
                attr_type,
                ..
            } => {
                // Validate attribute type exists.
                if !schema.attributes.contains_key(attr_type) {
                    errors.push(error(
                        "UNKNOWN_ATTRIBUTE_TYPE",
                        format!("Attribute type '{}' not found in schema", attr_type),
                        path,
                    ));
                }

                // If we know the owner's type, check ownership.
                if let Some(owner_type) = env.get_type(thing_var) {
                    self.validate_ownership(owner_type, attr_type, schema, path, errors);
                }
            }

            Pattern::Not(inner) => {
                for (i, p) in inner.iter().enumerate() {
                    self.validate_pattern_against_schema(
                        p,
                        schema,
                        env,
                        &format!("{}.not[{}]", path, i),
                        errors,
                    );
                }
            }

            Pattern::Or(alternatives) => {
                for (i, alt) in alternatives.iter().enumerate() {
                    for (j, p) in alt.iter().enumerate() {
                        self.validate_pattern_against_schema(
                            p,
                            schema,
                            env,
                            &format!("{}.or[{}][{}]", path, i, j),
                            errors,
                        );
                    }
                }
            }

            Pattern::Try(inner) => {
                for (i, p) in inner.iter().enumerate() {
                    self.validate_pattern_against_schema(
                        p,
                        schema,
                        env,
                        &format!("{}.try[{}]", path, i),
                        errors,
                    );
                }
            }

            // Attribute, SubType, ValueComparison, Iid, Raw: no extra schema checks.
            _ => {}
        }
    }

    // -- Statement validation ----------------------------------------------

    fn validate_statement_against_schema(
        &self,
        stmt: &Statement,
        schema: &TypeSchema,
        env: &TypeEnvironment,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        match stmt {
            Statement::Has {
                subject_var,
                attr_name,
                value,
            } => {
                if let Some(owner_type) = env.get_type(subject_var) {
                    self.validate_has_against_schema(
                        owner_type, attr_name, value, schema, path, errors,
                    );
                } else if !schema.attributes.contains_key(attr_name) {
                    errors.push(error(
                        "UNKNOWN_ATTRIBUTE_TYPE",
                        format!("Attribute type '{}' not found in schema", attr_name),
                        path,
                    ));
                }
            }
            Statement::Isa { type_name, .. } => {
                if !schema.type_exists(type_name) {
                    errors.push(error(
                        "UNKNOWN_TYPE",
                        format!("Type '{}' not found in schema", type_name),
                        path,
                    ));
                }
            }
            Statement::Relation {
                type_name,
                role_players,
                attributes,
                ..
            } => {
                if let Some(_rel) = schema.relations.get(type_name) {
                    // Validate roles.
                    let all_roles = schema.get_all_relates(type_name);
                    let role_names: HashSet<&str> =
                        all_roles.iter().map(|r| r.name.as_str()).collect();

                    for (j, rp) in role_players.iter().enumerate() {
                        if !role_names.contains(rp.role.as_str()) {
                            let available: Vec<&str> = role_names.iter().copied().collect();
                            errors.push(error(
                                "UNKNOWN_ROLE",
                                format!(
                                    "Role '{}' not found in relation '{}'. Available roles: {:?}",
                                    rp.role, type_name, available
                                ),
                                &format!("{}.role_players[{}]", path, j),
                            ));
                        }
                    }

                    // Validate inline attributes.
                    for (j, attr_stmt) in attributes.iter().enumerate() {
                        self.validate_statement_against_schema(
                            attr_stmt,
                            schema,
                            env,
                            &format!("{}.attrs[{}]", path, j),
                            errors,
                        );
                    }
                } else {
                    errors.push(error(
                        "UNKNOWN_TYPE",
                        format!("Relation type '{}' not found in schema", type_name),
                        path,
                    ));
                }
            }
            Statement::DeleteThing(_) | Statement::Raw(_) => {}
        }
    }

    // -- Insert-specific validation ----------------------------------------

    fn validate_insert_stmts(
        &self,
        stmts: &[Statement],
        schema: &TypeSchema,
        match_env: &TypeEnvironment,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        // Build a local env that includes match env + insert Isa/Relation bindings.
        let mut env = match_env.clone();
        for stmt in stmts {
            match stmt {
                Statement::Isa {
                    variable,
                    type_name,
                } if schema.type_exists(type_name) => env.bind(variable, type_name),
                Statement::Relation {
                    variable,
                    type_name,
                    ..
                } => {
                    env.bind(variable, type_name);
                }
                _ => {}
            }
        }

        // Track attribute value counts per (variable, attr_name) for cardinality.
        let mut attr_counts: HashMap<(String, String), usize> = HashMap::new();

        for (i, stmt) in stmts.iter().enumerate() {
            let stmt_path = format!("{}.stmts[{}]", path, i);
            match stmt {
                Statement::Isa { type_name, .. } => {
                    if !schema.type_exists(type_name) {
                        errors.push(error(
                            "UNKNOWN_TYPE",
                            format!("Type '{}' not found in schema", type_name),
                            &stmt_path,
                        ));
                    } else if schema.is_abstract(type_name) {
                        errors.push(error(
                            "ABSTRACT_TYPE_INSTANTIATION",
                            format!("Cannot instantiate abstract type '{}'", type_name),
                            &stmt_path,
                        ));
                    }
                }
                Statement::Has {
                    subject_var,
                    attr_name,
                    value,
                } => {
                    let key = (subject_var.clone(), attr_name.clone());
                    *attr_counts.entry(key).or_insert(0) += 1;

                    if let Some(owner_type) = env.get_type(subject_var) {
                        self.validate_has_against_schema(
                            owner_type, attr_name, value, schema, &stmt_path, errors,
                        );
                    } else if !schema.attributes.contains_key(attr_name) {
                        errors.push(error(
                            "UNKNOWN_ATTRIBUTE_TYPE",
                            format!("Attribute type '{}' not found in schema", attr_name),
                            &stmt_path,
                        ));
                    }
                }
                Statement::Relation {
                    type_name,
                    role_players,
                    attributes,
                    ..
                } => {
                    if schema.relations.contains_key(type_name) {
                        if schema.is_abstract(type_name) {
                            errors.push(error(
                                "ABSTRACT_TYPE_INSTANTIATION",
                                format!("Cannot instantiate abstract relation '{}'", type_name),
                                &stmt_path,
                            ));
                        }

                        let all_roles = schema.get_all_relates(type_name);
                        let role_names: HashSet<&str> =
                            all_roles.iter().map(|r| r.name.as_str()).collect();

                        for (j, rp) in role_players.iter().enumerate() {
                            if !role_names.contains(rp.role.as_str()) {
                                let available: Vec<&str> = role_names.iter().copied().collect();
                                errors.push(error(
                                    "UNKNOWN_ROLE",
                                    format!("Role '{}' not found in relation '{}'. Available roles: {:?}", rp.role, type_name, available),
                                    &format!("{}.role_players[{}]", stmt_path, j),
                                ));
                            }

                            if let Some(player_type) = env.get_type(&rp.player_var) {
                                self.validate_role_player_type(
                                    player_type,
                                    type_name,
                                    &rp.role,
                                    schema,
                                    &format!("{}.role_players[{}]", stmt_path, j),
                                    errors,
                                );
                            }
                        }

                        for (j, attr_stmt) in attributes.iter().enumerate() {
                            self.validate_statement_against_schema(
                                attr_stmt,
                                schema,
                                &env,
                                &format!("{}.attrs[{}]", stmt_path, j),
                                errors,
                            );
                        }
                    } else {
                        errors.push(error(
                            "UNKNOWN_TYPE",
                            format!("Relation type '{}' not found in schema", type_name),
                            &stmt_path,
                        ));
                    }
                }
                Statement::DeleteThing(_) | Statement::Raw(_) => {}
            }
        }

        // Cardinality warnings.
        for ((var, attr_name), count) in &attr_counts {
            if let Some(owner_type) = env.get_type(var)
                && let Some(attr) = schema
                    .get_all_owned_attributes(owner_type)
                    .iter()
                    .find(|a| a.name == *attr_name)
                && let Some(ref card) = attr.cardinality
                && let Some(max) = card.max
                && *count as u32 > max
            {
                errors.push(warning(
                    "CARDINALITY_EXCEEDED",
                    format!(
                        "Inserting {} values for '{}.{}' but @card allows max {}",
                        count, owner_type, attr_name, max
                    ),
                    path,
                ));
            }
        }
    }

    // -- Fetch validation --------------------------------------------------

    fn validate_fetch_items(
        &self,
        items: &[FetchItem],
        schema: &TypeSchema,
        env: &TypeEnvironment,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for (i, item) in items.iter().enumerate() {
            let item_path = format!("{}.fetch[{}]", path, i);
            match item {
                FetchItem::Attribute { var, attr_name, .. }
                | FetchItem::AttributeList { var, attr_name, .. } => {
                    if !schema.attributes.contains_key(attr_name) {
                        errors.push(error(
                            "UNKNOWN_ATTRIBUTE_TYPE",
                            format!("Attribute type '{}' not found in schema", attr_name),
                            &item_path,
                        ));
                    }
                    if let Some(owner_type) = env.get_type(var) {
                        self.validate_ownership(owner_type, attr_name, schema, &item_path, errors);
                    }
                }
                _ => {}
            }
        }
    }

    // -- Core helpers ------------------------------------------------------

    fn validate_has_against_schema(
        &self,
        owner_type: &str,
        attr_name: &str,
        value: &Value,
        schema: &TypeSchema,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        // Check attribute type exists at all.
        if !schema.attributes.contains_key(attr_name) {
            errors.push(error(
                "UNKNOWN_ATTRIBUTE_TYPE",
                format!("Attribute type '{}' not found in schema", attr_name),
                path,
            ));
            return;
        }

        // Check ownership.
        self.validate_ownership(owner_type, attr_name, schema, path, errors);

        // Check value type compatibility.
        if let Value::Literal(lit) = value
            && let Some(attr_type) = schema.attributes.get(attr_name)
            && !value_types_compatible(&lit.value_type, &attr_type.value_type)
        {
            errors.push(error(
                "VALUE_TYPE_MISMATCH",
                format!(
                    "Attribute '{}' expects value type '{}', but got '{}'",
                    attr_name, attr_type.value_type, lit.value_type
                ),
                path,
            ));
        }
    }

    fn validate_ownership(
        &self,
        owner_type: &str,
        attr_name: &str,
        schema: &TypeSchema,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let owned = schema.get_all_owned_attributes(owner_type);
        if !owned.iter().any(|a| a.name == attr_name) {
            let owned_names: Vec<&str> = owned.iter().map(|a| a.name.as_str()).collect();
            errors.push(error(
                "UNKNOWN_ATTRIBUTE_OWNERSHIP",
                format!(
                    "Type '{}' does not own attribute '{}'. Owned attributes: {:?}",
                    owner_type, attr_name, owned_names
                ),
                path,
            ));
        }
    }

    fn validate_role_player_type(
        &self,
        player_type: &str,
        relation_type: &str,
        role_name: &str,
        schema: &TypeSchema,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let role_ref = format!("{}:{}", relation_type, role_name);
        let plays = schema.get_all_plays_roles(player_type);

        if !plays.iter().any(|p| p.role_ref == role_ref) {
            let played: Vec<&str> = plays.iter().map(|p| p.role_ref.as_str()).collect();
            errors.push(error(
                "ROLE_PLAYER_TYPE_MISMATCH",
                format!(
                    "Type '{}' cannot play role '{}' in relation '{}'. It plays: {:?}",
                    player_type, role_name, relation_type, played
                ),
                path,
            ));
        }
    }

    fn check_strict_isa(
        &self,
        type_name: &str,
        schema: &TypeSchema,
        path: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let has_subtypes = schema
            .entities
            .values()
            .any(|e| e.parent.as_deref() == Some(type_name))
            || schema
                .relations
                .values()
                .any(|r| r.parent.as_deref() == Some(type_name))
            || schema
                .attributes
                .values()
                .any(|a| a.parent.as_deref() == Some(type_name));

        if !has_subtypes {
            errors.push(warning(
                "STRICT_ISA_NO_SUBTYPES",
                format!(
                    "'isa!' on type '{}' is redundant — it has no subtypes",
                    type_name
                ),
                path,
            ));
        }
    }

    // -----------------------------------------------------------------------
    // Entity data validation (custom rules)
    // -----------------------------------------------------------------------

    /// Validate entity data against loaded custom rules and optionally a schema.
    ///
    /// `entity_data` must be a JSON object containing a `__type__` field that
    /// identifies the entity type, along with attribute-name to value(s)
    /// mappings. Each loaded rule whose target matches the entity type and
    /// attribute is evaluated.
    ///
    /// # Arguments
    ///
    /// * `entity_data` -- A JSON object with `"__type__"` (entity type name)
    ///   and attribute_name to value(s) pairs.
    /// * `schema` -- An optional [`TypeSchema`] used to resolve ownership
    ///   when determining whether an `Attribute`-targeted rule applies to the
    ///   entity.
    pub fn validate_entity(
        &self,
        entity_data: &serde_json::Value,
        schema: Option<&TypeSchema>,
    ) -> ValidationResult {
        let mut errors = Vec::new();

        let obj = match entity_data.as_object() {
            Some(o) => o,
            None => {
                errors.push(error(
                    "INVALID_ENTITY_DATA",
                    "Entity data must be a JSON object".into(),
                    "",
                ));
                return make_result(errors);
            }
        };

        let entity_type = obj.get("__type__").and_then(|v| v.as_str()).unwrap_or("");

        for rule in &self.rules {
            errors.extend(self.apply_rule(rule, entity_type, obj, schema));
        }

        make_result(errors)
    }

    fn apply_rule(
        &self,
        rule: &ValidationRule,
        entity_type: &str,
        data: &serde_json::Map<String, serde_json::Value>,
        schema: Option<&TypeSchema>,
    ) -> Vec<ValidationError> {
        let (applies, attr_name) = match &rule.target {
            RuleTarget::Attribute { attribute } => {
                let owns = data.contains_key(attribute)
                    || schema.is_some_and(|s| {
                        s.get_all_owned_attributes(entity_type)
                            .iter()
                            .any(|a| a.name == *attribute)
                    });
                (owns, attribute.as_str())
            }
            RuleTarget::EntityAttribute { entity, attribute } => {
                (entity == entity_type, attribute.as_str())
            }
        };

        if !applies {
            return Vec::new();
        }

        let path = format!("{}.{}", entity_type, attr_name);
        let custom_msg = rule.error_message.as_deref();

        match &rule.rule_type {
            RuleType::Required => self.check_required(data, attr_name, &path, custom_msg),
            RuleType::Regex { .. } => {
                self.check_regex(data, attr_name, &rule.id, &path, custom_msg)
            }
            RuleType::Range { min, max } => {
                self.check_range_rule(data, attr_name, *min, *max, &path, custom_msg)
            }
            RuleType::Values { allowed } => {
                self.check_values(data, attr_name, allowed, &path, custom_msg)
            }
            RuleType::Cardinality { min, max } => {
                self.check_cardinality_rule(data, attr_name, *min, *max, &path, custom_msg)
            }
            RuleType::Length { min, max } => {
                self.check_length(data, attr_name, *min, *max, &path, custom_msg)
            }
        }
    }

    fn check_required(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        match data.get(attr_name) {
            None | Some(serde_json::Value::Null) => {
                let msg = custom_msg
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("'{}' is required", attr_name));
                vec![error("RULE_REQUIRED", msg, path)]
            }
            Some(serde_json::Value::Array(arr)) if arr.is_empty() => {
                let msg = custom_msg
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("'{}' is required (empty list)", attr_name));
                vec![error("RULE_REQUIRED", msg, path)]
            }
            _ => Vec::new(),
        }
    }

    fn check_regex(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        rule_id: &str,
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        let compiled = match self.regex_cache.get(rule_id) {
            Some(r) => r,
            None => return Vec::new(),
        };
        let values = extract_values(data, attr_name);
        let mut errors = Vec::new();
        for val in values {
            if let Some(s) = val.as_str()
                && !compiled.is_match(s)
            {
                let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                    format!(
                        "'{}' value '{}' does not match required pattern",
                        attr_name, s
                    )
                });
                errors.push(error("RULE_REGEX_MISMATCH", msg, path));
            }
        }
        errors
    }

    fn check_range_rule(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        min: Option<f64>,
        max: Option<f64>,
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        let values = extract_values(data, attr_name);
        let mut errors = Vec::new();
        for val in values {
            if let Some(n) = val.as_f64() {
                if let Some(lo) = min
                    && n < lo
                {
                    let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                        format!("'{}' value {} is below minimum {}", attr_name, n, lo)
                    });
                    errors.push(error("RULE_RANGE_VIOLATION", msg, path));
                }
                if let Some(hi) = max
                    && n > hi
                {
                    let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                        format!("'{}' value {} is above maximum {}", attr_name, n, hi)
                    });
                    errors.push(error("RULE_RANGE_VIOLATION", msg, path));
                }
            }
        }
        errors
    }

    fn check_values(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        allowed: &[serde_json::Value],
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        let values = extract_values(data, attr_name);
        let mut errors = Vec::new();
        for val in values {
            if !allowed.contains(val) {
                let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                    format!("'{}' value {} is not in allowed values", attr_name, val)
                });
                errors.push(error("RULE_VALUES_VIOLATION", msg, path));
            }
        }
        errors
    }

    fn check_cardinality_rule(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        min: u32,
        max: Option<u32>,
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        let count = match data.get(attr_name) {
            Some(serde_json::Value::Array(arr)) => arr.len() as u32,
            Some(serde_json::Value::Null) | None => 0,
            Some(_) => 1,
        };
        let mut errors = Vec::new();
        if count < min {
            let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                format!("'{}' has {} values, minimum is {}", attr_name, count, min)
            });
            errors.push(error("RULE_CARDINALITY_VIOLATION", msg, path));
        }
        if let Some(mx) = max
            && count > mx
        {
            let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                format!("'{}' has {} values, maximum is {}", attr_name, count, mx)
            });
            errors.push(error("RULE_CARDINALITY_VIOLATION", msg, path));
        }
        errors
    }

    fn check_length(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        attr_name: &str,
        min: Option<u32>,
        max: Option<u32>,
        path: &str,
        custom_msg: Option<&str>,
    ) -> Vec<ValidationError> {
        let values = extract_values(data, attr_name);
        let mut errors = Vec::new();
        for val in values {
            if let Some(s) = val.as_str() {
                let len = s.len() as u32;
                if let Some(lo) = min
                    && len < lo
                {
                    let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                        format!(
                            "'{}' value has length {}, minimum is {}",
                            attr_name, len, lo
                        )
                    });
                    errors.push(error("RULE_LENGTH_VIOLATION", msg, path));
                }
                if let Some(hi) = max
                    && len > hi
                {
                    let msg = custom_msg.map(|m| m.to_string()).unwrap_or_else(|| {
                        format!(
                            "'{}' value has length {}, maximum is {}",
                            attr_name, len, hi
                        )
                    });
                    errors.push(error("RULE_LENGTH_VIOLATION", msg, path));
                }
            }
        }
        errors
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Constraint, LiteralValue, Value};
    use serde_json::json;

    #[test]
    fn test_validate_type_name() {
        let engine = ValidationEngine::new();

        assert!(engine.validate_type_name("person", "entity").is_valid);
        assert!(
            engine
                .validate_type_name("first-name", "attribute")
                .is_valid
        );
        assert!(engine.validate_type_name("_internal", "role").is_valid);

        assert!(!engine.validate_type_name("define", "entity").is_valid);
        assert!(!engine.validate_type_name("", "entity").is_valid);
        assert!(!engine.validate_type_name("1st", "entity").is_valid);
        assert!(!engine.validate_type_name("person!", "entity").is_valid);
    }

    #[test]
    fn test_validate_pattern() {
        let engine = ValidationEngine::new();

        let valid_pattern = Pattern::Entity {
            variable: "$p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        };
        assert!(engine.validate_pattern(&valid_pattern).is_valid);

        let invalid_pattern = Pattern::Entity {
            variable: "p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "define".to_string(),
                value: Value::Variable("v".to_string()),
            }],
            is_strict: false,
        };
        let result = engine.validate_pattern(&invalid_pattern);
        assert!(!result.is_valid);
        assert_eq!(result.errors.len(), 3);
    }

    #[test]
    fn test_severity_field_present() {
        let engine = ValidationEngine::new();
        let result = engine.validate_type_name("define", "entity");
        assert!(!result.is_valid);
        assert_eq!(result.errors[0].severity, ValidationSeverity::Error);
    }

    #[test]
    fn test_warnings_dont_invalidate() {
        // A result with only warnings is still valid.
        let result = make_result(vec![warning("TEST_WARNING", "test".into(), "")]);
        assert!(result.is_valid);
    }
}

#[cfg(test)]
mod schema_validation_tests {
    use super::*;
    use crate::ast::*;
    use crate::schema::*;
    use serde_json::json;

    /// Build a test schema:
    ///   attribute name, value string;
    ///   attribute age, value long;
    ///   attribute email, value string;
    ///   attribute salary, value double;
    ///   entity person, owns name @key, owns age, owns email, plays employment:employee;
    ///   entity employee sub person, owns salary;
    ///   entity company, owns name @key, plays employment:employer;
    ///   relation employment, relates employee, relates employer;
    ///   relation friendship sub relation, relates friend; (abstract)
    fn build_test_schema() -> TypeSchema {
        let mut schema = TypeSchema::new();

        schema.attributes.insert(
            "name".into(),
            AttributeType {
                name: "name".into(),
                value_type: "string".into(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );
        schema.attributes.insert(
            "age".into(),
            AttributeType {
                name: "age".into(),
                value_type: "long".into(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );
        schema.attributes.insert(
            "email".into(),
            AttributeType {
                name: "email".into(),
                value_type: "string".into(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );
        schema.attributes.insert(
            "salary".into(),
            AttributeType {
                name: "salary".into(),
                value_type: "double".into(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );
        schema.attributes.insert(
            "score".into(),
            AttributeType {
                name: "score".into(),
                value_type: "long".into(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );

        schema.entities.insert(
            "person".into(),
            EntityType {
                name: "person".into(),
                parent: None,
                is_abstract: false,
                owns: vec![
                    OwnedAttribute {
                        name: "name".into(),
                        is_key: true,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "age".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "email".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                ],
                owns_order: vec!["name".into(), "age".into(), "email".into()],
                plays: vec![PlayedRole {
                    role_ref: "employment:employee".into(),
                    cardinality: None,
                }],
            },
        );
        schema.entities.insert(
            "employee".into(),
            EntityType {
                name: "employee".into(),
                parent: Some("person".into()),
                is_abstract: false,
                owns: vec![
                    // Inherited from person after resolution:
                    OwnedAttribute {
                        name: "name".into(),
                        is_key: true,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "age".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "email".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "salary".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: None,
                        ordered: false,
                        distinct: false,
                    },
                ],
                owns_order: vec!["name".into(), "age".into(), "email".into(), "salary".into()],
                plays: vec![PlayedRole {
                    role_ref: "employment:employee".into(),
                    cardinality: None,
                }],
            },
        );
        schema.entities.insert(
            "company".into(),
            EntityType {
                name: "company".into(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".into(),
                    is_key: true,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: None,
                    cardinality: None,
                    ordered: false,
                    distinct: false,
                }],
                owns_order: vec!["name".into()],
                plays: vec![PlayedRole {
                    role_ref: "employment:employer".into(),
                    cardinality: None,
                }],
            },
        );
        schema.entities.insert(
            "animal".into(),
            EntityType {
                name: "animal".into(),
                parent: None,
                is_abstract: true,
                owns: vec![OwnedAttribute {
                    name: "name".into(),
                    is_key: false,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: None,
                    cardinality: None,
                    ordered: false,
                    distinct: false,
                }],
                owns_order: vec!["name".into()],
                plays: vec![],
            },
        );

        schema.relations.insert(
            "employment".into(),
            RelationType {
                name: "employment".into(),
                parent: None,
                is_abstract: false,
                roles: vec![
                    RoleSpec {
                        name: "employee".into(),
                        overrides: None,
                        cardinality: None,
                        distinct: false,
                        ordered: false,
                        is_abstract: false,
                    },
                    RoleSpec {
                        name: "employer".into(),
                        overrides: None,
                        cardinality: None,
                        distinct: false,
                        ordered: false,
                        is_abstract: false,
                    },
                ],
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );
        schema.relations.insert(
            "friendship".into(),
            RelationType {
                name: "friendship".into(),
                parent: None,
                is_abstract: true,
                roles: vec![RoleSpec {
                    name: "friend".into(),
                    overrides: None,
                    cardinality: None,
                    distinct: false,
                    ordered: false,
                    is_abstract: false,
                }],
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );

        // Entity with cardinality constraint for testing.
        schema.entities.insert(
            "student".into(),
            EntityType {
                name: "student".into(),
                parent: None,
                is_abstract: false,
                owns: vec![
                    OwnedAttribute {
                        name: "name".into(),
                        is_key: true,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: Some(Cardinality {
                            min: 1,
                            max: Some(1),
                        }),
                        ordered: false,
                        distinct: false,
                    },
                    OwnedAttribute {
                        name: "score".into(),
                        is_key: false,
                        is_unique: false,
                        is_cascade: false,
                        subkey_group: None,
                        cardinality: Some(Cardinality {
                            min: 0,
                            max: Some(3),
                        }),
                        ordered: false,
                        distinct: false,
                    },
                ],
                owns_order: vec!["name".into(), "score".into()],
                plays: vec![],
            },
        );

        schema
    }

    // -- Unknown type tests ------------------------------------------------

    #[test]
    fn test_unknown_entity_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$x".into(),
            type_name: "spaceship".into(),
            constraints: vec![],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_unknown_relation_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Relation {
            variable: "$r".into(),
            type_name: "nonexistent".into(),
            role_players: vec![],
            constraints: vec![],
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_entity_used_as_relation() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Relation {
            variable: "$r".into(),
            type_name: "person".into(),
            role_players: vec![],
            constraints: vec![],
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.message.contains("not a relation type"))
        );
    }

    // -- Ownership tests ---------------------------------------------------

    #[test]
    fn test_valid_has_constraint() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_invalid_has_constraint() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // person doesn't own salary (only employee does).
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "salary".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(50000.0),
                    value_type: "double".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_OWNERSHIP")
        );
    }

    #[test]
    fn test_inherited_attribute_valid() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // employee inherits name from person.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$e".into(),
            type_name: "employee".into(),
            constraints: vec![Constraint::Has {
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("Bob"),
                    value_type: "string".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_unknown_attribute_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "nonexistent".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("x"),
                    value_type: "string".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_TYPE")
        );
    }

    #[test]
    fn test_has_pattern_ownership_check() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // $p isa person; $p has salary $s; -> person doesn't own salary
        let clauses = vec![Clause::Match(vec![
            Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Has {
                thing_var: "$p".into(),
                attr_type: "salary".into(),
                attr_var: "$s".into(),
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_OWNERSHIP")
        );
    }

    // -- Role validation tests ---------------------------------------------

    #[test]
    fn test_valid_role_names() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Relation {
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
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_unknown_role_name() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![RolePlayer {
                role: "manager".into(),
                player_var: "$p".into(),
            }],
            constraints: vec![],
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_ROLE"));
    }

    // -- Role player type tests --------------------------------------------

    #[test]
    fn test_valid_role_player_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // $p isa person; $c isa company; $r (employee: $p, employer: $c) isa employment;
        let clauses = vec![Clause::Match(vec![
            Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Entity {
                variable: "$c".into(),
                type_name: "company".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Relation {
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
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_invalid_role_player_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // company can't play employee role.
        let clauses = vec![Clause::Match(vec![
            Pattern::Entity {
                variable: "$c".into(),
                type_name: "company".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Relation {
                variable: "$r".into(),
                type_name: "employment".into(),
                role_players: vec![RolePlayer {
                    role: "employee".into(),
                    player_var: "$c".into(),
                }],
                constraints: vec![],
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "ROLE_PLAYER_TYPE_MISMATCH")
        );
    }

    // -- Value type tests --------------------------------------------------

    #[test]
    fn test_valid_value_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "age".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(30),
                    value_type: "long".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_invalid_value_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // age is long, but we're passing a string.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "age".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("thirty"),
                    value_type: "string".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "VALUE_TYPE_MISMATCH")
        );
    }

    #[test]
    fn test_long_to_double_widening() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // salary is double, using a long literal is OK.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$e".into(),
            type_name: "employee".into(),
            constraints: vec![Constraint::Has {
                attr_name: "salary".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(50000),
                    value_type: "long".into(),
                }),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    // -- Abstract type tests -----------------------------------------------

    #[test]
    fn test_abstract_type_in_match_ok() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // Matching on abstract types is fine.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$a".into(),
            type_name: "animal".into(),
            constraints: vec![],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_abstract_entity_in_insert() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Insert(vec![Statement::Isa {
            variable: "$a".into(),
            type_name: "animal".into(),
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "ABSTRACT_TYPE_INSTANTIATION")
        );
    }

    #[test]
    fn test_abstract_relation_in_insert() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Insert(vec![Statement::Relation {
            variable: "$r".into(),
            type_name: "friendship".into(),
            role_players: vec![],
            include_variable: true,
            attributes: vec![],
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "ABSTRACT_TYPE_INSTANTIATION")
        );
    }

    // -- Cardinality tests -------------------------------------------------

    #[test]
    fn test_cardinality_exceeded_warning() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // student owns name @card(1..1), inserting 2 names.
        let clauses = vec![Clause::Insert(vec![
            Statement::Isa {
                variable: "$s".into(),
                type_name: "student".into(),
            },
            Statement::Has {
                subject_var: "$s".into(),
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("A"),
                    value_type: "string".into(),
                }),
            },
            Statement::Has {
                subject_var: "$s".into(),
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("B"),
                    value_type: "string".into(),
                }),
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        // Should be valid (warning only), but have a CARDINALITY_EXCEEDED warning.
        assert!(result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "CARDINALITY_EXCEEDED"
                    && e.severity == ValidationSeverity::Warning)
        );
    }

    #[test]
    fn test_cardinality_within_limit() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // student owns score @card(0..3), inserting 2 scores is OK.
        let clauses = vec![Clause::Insert(vec![
            Statement::Isa {
                variable: "$s".into(),
                type_name: "student".into(),
            },
            Statement::Has {
                subject_var: "$s".into(),
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("A"),
                    value_type: "string".into(),
                }),
            },
            Statement::Has {
                subject_var: "$s".into(),
                attr_name: "score".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(90),
                    value_type: "long".into(),
                }),
            },
            Statement::Has {
                subject_var: "$s".into(),
                attr_name: "score".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(85),
                    value_type: "long".into(),
                }),
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == "CARDINALITY_EXCEEDED")
        );
    }

    // -- Strict isa tests --------------------------------------------------

    #[test]
    fn test_strict_isa_with_subtypes_no_warning() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // person has subtype employee, so isa! is meaningful.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![],
            is_strict: true,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == "STRICT_ISA_NO_SUBTYPES")
        );
    }

    #[test]
    fn test_strict_isa_no_subtypes_warning() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // company has no subtypes.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$c".into(),
            type_name: "company".into(),
            constraints: vec![],
            is_strict: true,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid); // Warnings don't invalidate.
        assert!(result.errors.iter().any(
            |e| e.code == "STRICT_ISA_NO_SUBTYPES" && e.severity == ValidationSeverity::Warning
        ));
    }

    // -- Unknown variable type (graceful skip) -----------------------------

    #[test]
    fn test_unknown_variable_type_skipped() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // $p has no isa, so ownership can't be checked — should not error.
        let clauses = vec![Clause::Match(vec![Pattern::Has {
            thing_var: "$p".into(),
            attr_type: "name".into(),
            attr_var: "$n".into(),
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    // -- Multi-clause validation -------------------------------------------

    #[test]
    fn test_match_insert_validates_both() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // Match binds $p as person, then insert tries to add salary (person doesn't own salary).
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Insert(vec![Statement::Has {
                subject_var: "$p".into(),
                attr_name: "salary".into(),
                value: Value::Literal(LiteralValue {
                    value: json!(50000.0),
                    value_type: "double".into(),
                }),
            }]),
        ];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_OWNERSHIP")
        );
    }

    // -- Edge cases --------------------------------------------------------

    #[test]
    fn test_empty_query_passes() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let result = engine.validate_query(&[], &schema);
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_valid_complex_query() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // A fully valid match + fetch query.
        let clauses = vec![
            Clause::Match(vec![
                Pattern::Entity {
                    variable: "$p".into(),
                    type_name: "person".into(),
                    constraints: vec![Constraint::Has {
                        attr_name: "name".into(),
                        value: Value::Literal(LiteralValue {
                            value: json!("Alice"),
                            value_type: "string".into(),
                        }),
                    }],
                    is_strict: false,
                },
                Pattern::Entity {
                    variable: "$c".into(),
                    type_name: "company".into(),
                    constraints: vec![],
                    is_strict: false,
                },
                Pattern::Relation {
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
                },
            ]),
            Clause::Fetch(vec![
                FetchItem::Attribute {
                    key: "name".into(),
                    var: "$p".into(),
                    attr_name: "name".into(),
                },
                FetchItem::Attribute {
                    key: "company".into(),
                    var: "$c".into(),
                    attr_name: "name".into(),
                },
            ]),
        ];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_fetch_unknown_attribute() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![FetchItem::Attribute {
                key: "x".into(),
                var: "$p".into(),
                attr_name: "nonexistent".into(),
            }]),
        ];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_TYPE")
        );
    }

    #[test]
    fn test_fetch_ownership_check() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![FetchItem::Attribute {
                key: "s".into(),
                var: "$p".into(),
                attr_name: "salary".into(),
            }]),
        ];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_OWNERSHIP")
        );
    }

    #[test]
    fn test_insert_unknown_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Insert(vec![Statement::Isa {
            variable: "$x".into(),
            type_name: "spaceship".into(),
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_insert_unknown_relation() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Insert(vec![Statement::Relation {
            variable: "$r".into(),
            type_name: "nonexistent".into(),
            role_players: vec![],
            include_variable: true,
            attributes: vec![],
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_insert_role_validation() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![
            Clause::Match(vec![
                Pattern::Entity {
                    variable: "$p".into(),
                    type_name: "person".into(),
                    constraints: vec![],
                    is_strict: false,
                },
                Pattern::Entity {
                    variable: "$c".into(),
                    type_name: "company".into(),
                    constraints: vec![],
                    is_strict: false,
                },
            ]),
            Clause::Insert(vec![Statement::Relation {
                variable: "$r".into(),
                type_name: "employment".into(),
                role_players: vec![RolePlayer {
                    role: "boss".into(),
                    player_var: "$p".into(),
                }],
                include_variable: true,
                attributes: vec![],
            }]),
        ];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_ROLE"));
    }

    #[test]
    fn test_variable_value_skips_type_check() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        // Using a variable as value shouldn't trigger VALUE_TYPE_MISMATCH.
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Has {
                attr_name: "name".into(),
                value: Value::Variable("$n".into()),
            }],
            is_strict: false,
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_or_pattern_validation() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![Pattern::Or(vec![
            vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }],
            vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "nonexistent".into(),
                constraints: vec![],
                is_strict: false,
            }],
        ])])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_not_pattern_validation() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Match(vec![
            Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Not(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![Constraint::Has {
                    attr_name: "salary".into(), // person doesn't own salary
                    value: Value::Literal(LiteralValue {
                        value: json!(0.0),
                        value_type: "double".into(),
                    }),
                }],
                is_strict: false,
            }]),
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "UNKNOWN_ATTRIBUTE_OWNERSHIP")
        );
    }

    // -- Put clause validation (same semantics as Insert) --------------------

    #[test]
    fn test_put_valid() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Put(vec![
            Statement::Isa {
                variable: "$p".into(),
                type_name: "person".into(),
            },
            Statement::Has {
                subject_var: "$p".into(),
                attr_name: "name".into(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".into(),
                }),
            },
        ])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(result.is_valid);
    }

    #[test]
    fn test_put_unknown_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Put(vec![Statement::Isa {
            variable: "$x".into(),
            type_name: "spaceship".into(),
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "UNKNOWN_TYPE"));
    }

    #[test]
    fn test_put_abstract_type() {
        let engine = ValidationEngine::new();
        let schema = build_test_schema();
        let clauses = vec![Clause::Put(vec![Statement::Isa {
            variable: "$a".into(),
            type_name: "animal".into(),
        }])];
        let result = engine.validate_query(&clauses, &schema);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "ABSTRACT_TYPE_INSTANTIATION")
        );
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;
    use serde_json::json;

    fn sample_rules_json() -> String {
        json!({
            "rules": [
                {
                    "id": "name-required",
                    "target": { "type": "EntityAttribute", "data": { "entity": "person", "attribute": "name" } },
                    "rule_type": { "type": "Required" },
                    "error_message": "Person must have a name"
                },
                {
                    "id": "email-regex",
                    "target": { "type": "Attribute", "data": { "attribute": "email" } },
                    "rule_type": { "type": "Regex", "data": { "pattern": "^.+@.+\\..+$" } }
                },
                {
                    "id": "age-range",
                    "target": { "type": "Attribute", "data": { "attribute": "age" } },
                    "rule_type": { "type": "Range", "data": { "min": 0.0, "max": 150.0 } }
                },
                {
                    "id": "name-length",
                    "target": { "type": "Attribute", "data": { "attribute": "name" } },
                    "rule_type": { "type": "Length", "data": { "min": 1, "max": 100 } }
                }
            ]
        }).to_string()
    }

    #[test]
    fn test_load_rules() {
        let mut engine = ValidationEngine::new();
        let warnings = engine.load_rules(&sample_rules_json()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(engine.rule_count(), 4);
    }

    #[test]
    fn test_export_roundtrip() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let exported = engine.export_rules().unwrap();
        let mut engine2 = ValidationEngine::new();
        engine2.load_rules(&exported).unwrap();
        assert_eq!(engine2.rule_count(), 4);
    }

    #[test]
    fn test_required_passes() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "age": 30 });
        let result = engine.validate_entity(&data, None);
        assert!(result.is_valid);
    }

    #[test]
    fn test_required_fails_missing() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "age": 30 });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "RULE_REQUIRED"));
    }

    #[test]
    fn test_required_fails_null() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": null, "age": 30 });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.code == "RULE_REQUIRED"));
    }

    #[test]
    fn test_required_custom_message() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "age": 30 });
        let result = engine.validate_entity(&data, None);
        let req_error = result
            .errors
            .iter()
            .find(|e| e.code == "RULE_REQUIRED")
            .unwrap();
        assert_eq!(req_error.message, "Person must have a name");
    }

    #[test]
    fn test_regex_passes() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "email": "alice@example.com" });
        let result = engine.validate_entity(&data, None);
        assert!(result.is_valid);
    }

    #[test]
    fn test_regex_fails() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "email": "not-an-email" });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "RULE_REGEX_MISMATCH")
        );
    }

    #[test]
    fn test_range_passes() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "age": 30 });
        let result = engine.validate_entity(&data, None);
        assert!(result.is_valid);
    }

    #[test]
    fn test_range_fails_below() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "age": -1 });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "RULE_RANGE_VIOLATION")
        );
    }

    #[test]
    fn test_range_fails_above() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "Alice", "age": 200 });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "RULE_RANGE_VIOLATION")
        );
    }

    #[test]
    fn test_length_fails_too_short() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        let data = json!({ "__type__": "person", "name": "" });
        let result = engine.validate_entity(&data, None);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "RULE_LENGTH_VIOLATION")
        );
    }

    #[test]
    fn test_values_rule() {
        let rules = json!({
            "rules": [{
                "id": "status-values",
                "target": { "type": "Attribute", "data": { "attribute": "status" } },
                "rule_type": { "type": "Values", "data": { "allowed": ["active", "inactive"] } }
            }]
        })
        .to_string();
        let mut engine = ValidationEngine::new();
        engine.load_rules(&rules).unwrap();

        let valid = json!({ "__type__": "person", "status": "active" });
        assert!(engine.validate_entity(&valid, None).is_valid);

        let invalid = json!({ "__type__": "person", "status": "deleted" });
        let result = engine.validate_entity(&invalid, None);
        assert!(!result.is_valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == "RULE_VALUES_VIOLATION")
        );
    }

    #[test]
    fn test_cardinality_rule() {
        let rules = json!({
            "rules": [{
                "id": "tags-card",
                "target": { "type": "EntityAttribute", "data": { "entity": "person", "attribute": "tags" } },
                "rule_type": { "type": "Cardinality", "data": { "min": 1, "max": 3 } }
            }]
        }).to_string();
        let mut engine = ValidationEngine::new();
        engine.load_rules(&rules).unwrap();

        let valid = json!({ "__type__": "person", "tags": ["a", "b"] });
        assert!(engine.validate_entity(&valid, None).is_valid);

        let too_few = json!({ "__type__": "person", "tags": [] });
        assert!(!engine.validate_entity(&too_few, None).is_valid);

        let too_many = json!({ "__type__": "person", "tags": ["a", "b", "c", "d"] });
        assert!(!engine.validate_entity(&too_many, None).is_valid);
    }

    #[test]
    fn test_entity_attribute_scoping() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        // name-required targets person, should not fire on company
        let data = json!({ "__type__": "company", "age": 30 });
        let result = engine.validate_entity(&data, None);
        assert!(result.is_valid);
    }

    #[test]
    fn test_clear_rules() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        assert_eq!(engine.rule_count(), 4);
        engine.clear_rules();
        assert_eq!(engine.rule_count(), 0);
        // No rules → always valid
        let data = json!({ "__type__": "person" });
        assert!(engine.validate_entity(&data, None).is_valid);
    }

    #[test]
    fn test_invalid_regex_rejected() {
        let rules = json!({
            "rules": [{
                "id": "bad-regex",
                "target": { "type": "Attribute", "data": { "attribute": "name" } },
                "rule_type": { "type": "Regex", "data": { "pattern": "[invalid(" } }
            }]
        })
        .to_string();
        let mut engine = ValidationEngine::new();
        let warnings = engine.load_rules(&rules).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(engine.rule_count(), 0);
    }

    #[test]
    fn test_existing_methods_still_work() {
        let mut engine = ValidationEngine::new();
        engine.load_rules(&sample_rules_json()).unwrap();
        assert!(engine.validate_type_name("person", "entity").is_valid);
        assert!(!engine.validate_type_name("define", "entity").is_valid);
    }
}
