use crate::_schema::TypeSchema;
use crate::reserved_words::is_reserved_word;
use crate::validation_rule_wire::ReleasedF64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

#[cfg(test)]
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
#[derive(Debug, Clone, Serialize)]
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

#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
enum RuleTypeWire {
    Required,
    Regex {
        pattern: String,
    },
    Range {
        min: Option<ReleasedF64>,
        max: Option<ReleasedF64>,
    },
    Values {
        allowed: Vec<serde_json::Value>,
    },
    Cardinality {
        min: u32,
        max: Option<u32>,
    },
    Length {
        min: Option<u32>,
        max: Option<u32>,
    },
}

impl<'de> Deserialize<'de> for RuleType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match RuleTypeWire::deserialize(deserializer)? {
            RuleTypeWire::Required => Ok(Self::Required),
            RuleTypeWire::Regex { pattern } => Ok(Self::Regex { pattern }),
            RuleTypeWire::Range { min, max } => Ok(Self::Range {
                min: min.map(ReleasedF64::into_inner),
                max: max.map(ReleasedF64::into_inner),
            }),
            RuleTypeWire::Values { allowed } => Ok(Self::Values { allowed }),
            RuleTypeWire::Cardinality { min, max } => Ok(Self::Cardinality { min, max }),
            RuleTypeWire::Length { min, max } => Ok(Self::Length { min, max }),
        }
    }
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
    fn range_wire_accepts_feature_unified_numbers_and_content_first_order() {
        let range: RuleType =
            serde_json::from_str(r#"{"data":{"min":-1.25,"max":2.5},"type":"Range"}"#)
                .expect("content-first range numbers");
        match range {
            RuleType::Range { min, max } => {
                assert_eq!(min, Some(-1.25));
                assert_eq!(max, Some(2.5));
            }
            _ => panic!("range fixture decoded as another rule type"),
        }

        let without_bounds: RuleType =
            serde_json::from_str(r#"{"data":{},"type":"Range"}"#).expect("missing optional bounds");
        match without_bounds {
            RuleType::Range { min, max } => {
                assert_eq!(min, None);
                assert_eq!(max, None);
            }
            _ => panic!("range fixture decoded as another rule type"),
        }
    }

    #[test]
    fn range_wire_preserves_released_malformed_error_snapshots() {
        let string_error =
            serde_json::from_str::<RuleType>(r#"{"type":"Range","data":{"min":"zero","max":1}}"#)
                .expect_err("string range bound")
                .to_string();
        assert_eq!(
            string_error,
            "invalid type: string \"zero\", expected f64 at line 1 column 36",
        );

        let overflow_error =
            serde_json::from_str::<RuleType>(r#"{"type":"Range","data":{"min":1e400,"max":1}}"#)
                .expect_err("overflowing range bound")
                .to_string();
        assert_eq!(overflow_error, "number out of range at line 1 column 35",);

        let content_first =
            serde_json::from_str::<RuleType>(r#"{"data":{"min":"zero","max":1},"type":"Range"}"#)
                .expect_err("content-first string range bound")
                .to_string();
        assert_eq!(
            content_first,
            "invalid type: string \"zero\", expected f64 at line 1 column 46"
        );
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
