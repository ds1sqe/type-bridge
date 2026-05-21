//! Expression system for building typed queries.
//!
//! Provides [`Expr`] for building filter expressions, [`Agg`] for
//! aggregation operations, [`SortDir`] for sort direction, and
//! [`AggResult`] for typed aggregation results.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use type_bridge_core_lib::ast::{FunctionCallValue, Pattern, ReduceAssignment, Value};

use crate::value::AttributeValue;

// ---------------------------------------------------------------------------
// Sort direction
// ---------------------------------------------------------------------------

/// Sort direction for query results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDir {
    /// Ascending order (smallest first).
    Asc,
    /// Descending order (largest first).
    Desc,
}

// ---------------------------------------------------------------------------
// Filter expressions
// ---------------------------------------------------------------------------

/// A filter expression that converts to AST [`Pattern`] nodes.
///
/// Supports comparison operators, string operators, and boolean
/// combinators. Multiple `Expr`s added via `filter()` are ANDed together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// Equality: `attr == value`.
    Eq {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// Greater than: `attr > value`.
    Gt {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// Less than: `attr < value`.
    Lt {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// Greater than or equal: `attr >= value`.
    Gte {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// Less than or equal: `attr <= value`.
    Lte {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// Not equal: `attr != value`.
    Neq {
        /// Attribute name.
        attr: String,
        /// Value to compare against.
        value: AttributeValue,
    },
    /// String contains: `attr contains substring`.
    Contains {
        /// Attribute name.
        attr: String,
        /// Substring to search for.
        substring: String,
    },
    /// String like (regex): `attr like pattern`.
    Like {
        /// Attribute name.
        attr: String,
        /// Regex pattern to match.
        pattern: String,
    },
    /// All child expressions must match.
    And(Vec<Expr>),
    /// At least one child expression must match.
    Or(Vec<Expr>),
    /// The inner expression must not match.
    Not(Box<Expr>),
    /// Filter on a role player's attribute within a relation query.
    ///
    /// Wraps an inner expression so it targets the role player variable
    /// (`$role_name`) instead of the relation variable.
    RolePlayer {
        /// The role name (e.g. `"employee"`).
        role: String,
        /// The inner expression applied to the role player.
        inner: Box<Expr>,
    },
}

impl Expr {
    // -- Convenience constructors --

    /// Create an equality filter.
    pub fn eq(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Eq {
            attr: attr.into(),
            value,
        }
    }

    /// Create a greater-than filter.
    pub fn gt(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Gt {
            attr: attr.into(),
            value,
        }
    }

    /// Create a less-than filter.
    pub fn lt(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Lt {
            attr: attr.into(),
            value,
        }
    }

    /// Create a greater-than-or-equal filter.
    pub fn gte(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Gte {
            attr: attr.into(),
            value,
        }
    }

    /// Create a less-than-or-equal filter.
    pub fn lte(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Lte {
            attr: attr.into(),
            value,
        }
    }

    /// Create a not-equal filter.
    pub fn neq(attr: impl Into<String>, value: AttributeValue) -> Self {
        Self::Neq {
            attr: attr.into(),
            value,
        }
    }

    /// Create a string-contains filter.
    pub fn contains(attr: impl Into<String>, substring: impl Into<String>) -> Self {
        Self::Contains {
            attr: attr.into(),
            substring: substring.into(),
        }
    }

    /// Create a string-like (regex) filter.
    pub fn like(attr: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::Like {
            attr: attr.into(),
            pattern: pattern.into(),
        }
    }

    /// AND multiple expressions together.
    pub fn and(exprs: Vec<Expr>) -> Self {
        Self::And(exprs)
    }

    /// OR multiple expressions together.
    pub fn or(exprs: Vec<Expr>) -> Self {
        Self::Or(exprs)
    }

    /// Negate an expression.
    #[allow(clippy::should_implement_trait)]
    pub fn not(expr: Expr) -> Self {
        Self::Not(Box::new(expr))
    }

    /// Range filter: `attr >= low AND attr <= high`.
    pub fn in_range(attr: impl Into<String>, low: AttributeValue, high: AttributeValue) -> Self {
        let a = attr.into();
        Self::And(vec![Self::gte(a.clone(), low), Self::lte(a, high)])
    }

    /// String starts-with filter using regex: `attr like "^prefix.*"`.
    pub fn startswith(attr: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self::Like {
            attr: attr.into(),
            pattern: format!("^{}.*", regex_escape(&prefix.into())),
        }
    }

    /// String ends-with filter using regex: `attr like ".*suffix$"`.
    pub fn endswith(attr: impl Into<String>, suffix: impl Into<String>) -> Self {
        Self::Like {
            attr: attr.into(),
            pattern: format!(".*{}$", regex_escape(&suffix.into())),
        }
    }

    /// Wrap an expression to target a role player's attribute.
    pub fn role_player(role: impl Into<String>, inner: Expr) -> Self {
        Self::RolePlayer {
            role: role.into(),
            inner: Box::new(inner),
        }
    }

    // -- AST conversion --

    /// Convert this expression to a list of AST [`Pattern`] nodes.
    ///
    /// `entity_var` is the variable bound to the entity (e.g. `"$e"`).
    /// `counter` is a mutable counter for generating unique attribute
    /// variable names (`$_attr0`, `$_attr1`, etc.).
    pub fn to_patterns(&self, entity_var: &str, counter: &mut usize) -> Vec<Pattern> {
        match self {
            Self::Eq { attr, value }
            | Self::Gt { attr, value }
            | Self::Lt { attr, value }
            | Self::Gte { attr, value }
            | Self::Lte { attr, value }
            | Self::Neq { attr, value } => {
                let op = match self {
                    Self::Eq { .. } => "==",
                    Self::Gt { .. } => ">",
                    Self::Lt { .. } => "<",
                    Self::Gte { .. } => ">=",
                    Self::Lte { .. } => "<=",
                    Self::Neq { .. } => "!=",
                    _ => unreachable!(),
                };
                let var_name = format!("$_attr{}", counter);
                *counter += 1;
                vec![
                    Pattern::Has {
                        thing_var: entity_var.to_string(),
                        attr_type: attr.clone(),
                        attr_var: var_name.clone(),
                    },
                    Pattern::ValueComparison {
                        var: var_name,
                        operator: op.to_string(),
                        value: value.to_ast_value(),
                    },
                ]
            }
            Self::Contains { attr, substring } => {
                let var_name = format!("$_attr{}", counter);
                *counter += 1;
                vec![
                    Pattern::Has {
                        thing_var: entity_var.to_string(),
                        attr_type: attr.clone(),
                        attr_var: var_name.clone(),
                    },
                    Pattern::ValueComparison {
                        var: var_name,
                        operator: "contains".to_string(),
                        value: AttributeValue::String(substring.clone()).to_ast_value(),
                    },
                ]
            }
            Self::Like { attr, pattern } => {
                let var_name = format!("$_attr{}", counter);
                *counter += 1;
                vec![
                    Pattern::Has {
                        thing_var: entity_var.to_string(),
                        attr_type: attr.clone(),
                        attr_var: var_name.clone(),
                    },
                    Pattern::ValueComparison {
                        var: var_name,
                        operator: "like".to_string(),
                        value: AttributeValue::String(pattern.clone()).to_ast_value(),
                    },
                ]
            }
            Self::And(children) => {
                let mut patterns = Vec::new();
                for child in children {
                    patterns.extend(child.to_patterns(entity_var, counter));
                }
                patterns
            }
            Self::Or(children) => {
                let branches: Vec<Vec<Pattern>> = children
                    .iter()
                    .map(|child| child.to_patterns(entity_var, counter))
                    .collect();
                vec![Pattern::Or(branches)]
            }
            Self::Not(inner) => {
                let inner_patterns = inner.to_patterns(entity_var, counter);
                vec![Pattern::Not(inner_patterns)]
            }
            Self::RolePlayer { role, inner } => {
                let role_var = format!("${}", role);
                inner.to_patterns(&role_var, counter)
            }
        }
    }

    /// Collect unique role names from role player expressions.
    pub fn collect_roles(
        &self,
        roles: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match self {
            Self::RolePlayer { role, .. } if seen.insert(role.clone()) => {
                roles.push(role.clone());
            }
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_roles(roles, seen);
                }
            }
            Self::Not(inner) => inner.collect_roles(roles, seen),
            _ => {}
        }
    }
}

/// Escape regex special characters for use in TypeQL `like` patterns.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '^' | '$' | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

/// An aggregation operation for reduce clauses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Agg {
    /// Count all matched results.
    Count,
    /// Sum values of a numeric attribute.
    Sum(String),
    /// Minimum value of an attribute.
    Min(String),
    /// Maximum value of an attribute.
    Max(String),
    /// Arithmetic mean of a numeric attribute.
    Mean(String),
    /// Median of a numeric attribute.
    Median(String),
    /// Standard deviation of a numeric attribute.
    Std(String),
}

impl Agg {
    /// Convert to a [`ReduceAssignment`] and optional `Has` binding pattern.
    ///
    /// Returns `(assignment, optional_has_pattern)`.
    /// For `Count`, no has pattern is needed. For attribute-based
    /// aggregations, a `Has` binding connects the entity variable
    /// to the attribute variable used in the function call.
    pub fn to_reduce_assignment(
        &self,
        entity_var: &str,
        counter: &mut usize,
    ) -> (ReduceAssignment, Option<Pattern>) {
        match self {
            Self::Count => {
                let assignment = ReduceAssignment {
                    variable: "$_count".to_string(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: "count".to_string(),
                        args: vec![Value::Variable(entity_var.to_string())],
                    }),
                };
                (assignment, None)
            }
            _ => {
                let (func_name, attr_name, result_var) = match self {
                    Self::Sum(a) => ("sum", a.as_str(), "$_sum"),
                    Self::Min(a) => ("min", a.as_str(), "$_min"),
                    Self::Max(a) => ("max", a.as_str(), "$_max"),
                    Self::Mean(a) => ("mean", a.as_str(), "$_mean"),
                    Self::Median(a) => ("median", a.as_str(), "$_median"),
                    Self::Std(a) => ("std", a.as_str(), "$_std"),
                    Self::Count => unreachable!(),
                };

                let attr_var = format!("$_agg{}", counter);
                *counter += 1;

                let has_pattern = Pattern::Has {
                    thing_var: entity_var.to_string(),
                    attr_type: attr_name.to_string(),
                    attr_var: attr_var.clone(),
                };

                let assignment = ReduceAssignment {
                    variable: result_var.to_string(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: func_name.to_string(),
                        args: vec![Value::Variable(attr_var)],
                    }),
                };

                (assignment, Some(has_pattern))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Aggregation result
// ---------------------------------------------------------------------------

/// Result of an aggregation query.
///
/// Wraps a map of result variable names to JSON values with typed
/// accessor methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggResult {
    values: HashMap<String, serde_json::Value>,
}

impl AggResult {
    /// Create from a map of variable names to JSON values.
    pub fn new(values: HashMap<String, serde_json::Value>) -> Self {
        Self { values }
    }

    /// Get the count result.
    pub fn count(&self) -> Option<u64> {
        self.values.get("$_count").and_then(|v| v.as_u64())
    }

    /// Get a 64-bit integer result by variable name.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.values.get(key).and_then(|v| v.as_i64())
    }

    /// Get a 64-bit float result by variable name.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.values.get(key).and_then(|v| v.as_f64())
    }

    /// Get the raw JSON value by variable name.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.values.get(key)
    }
}

// ---------------------------------------------------------------------------
// Group-by result
// ---------------------------------------------------------------------------

/// Result of a group-by aggregation query.
///
/// Each entry maps a group key value to its [`AggResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupByResult {
    groups: Vec<(serde_json::Value, AggResult)>,
}

impl GroupByResult {
    /// Create from a list of (group_key, agg_result) pairs.
    pub fn new(groups: Vec<(serde_json::Value, AggResult)>) -> Self {
        Self { groups }
    }

    /// Iterate over (group_key, agg_result) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&serde_json::Value, &AggResult)> {
        self.groups.iter().map(|(k, v)| (k, v))
    }

    /// Number of groups.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Whether there are no groups.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Get the aggregation result for a specific group key.
    pub fn get(&self, key: &serde_json::Value) -> Option<&AggResult> {
        self.groups.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Get aggregation result by string key.
    pub fn get_by_str(&self, key: &str) -> Option<&AggResult> {
        self.get(&serde_json::Value::String(key.to_string()))
    }

    /// Get aggregation result by integer key.
    pub fn get_by_i64(&self, key: i64) -> Option<&AggResult> {
        self.get(&serde_json::json!(key))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_expr_eq_patterns() {
        let expr = Expr::eq("age", AttributeValue::Long(30));
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 2);
        assert_eq!(counter, 1);
        match &patterns[0] {
            Pattern::Has {
                thing_var,
                attr_type,
                attr_var,
            } => {
                assert_eq!(thing_var, "$e");
                assert_eq!(attr_type, "age");
                assert_eq!(attr_var, "$_attr0");
            }
            _ => panic!("expected Has"),
        }
        match &patterns[1] {
            Pattern::ValueComparison { var, operator, .. } => {
                assert_eq!(var, "$_attr0");
                assert_eq!(operator, "==");
            }
            _ => panic!("expected ValueComparison"),
        }
    }

    #[test]
    fn test_expr_gt_patterns() {
        let expr = Expr::gt("salary", AttributeValue::Long(50000));
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 2);
        match &patterns[1] {
            Pattern::ValueComparison { operator, .. } => assert_eq!(operator, ">"),
            _ => panic!("expected ValueComparison"),
        }
    }

    #[test]
    fn test_expr_lt_patterns() {
        let expr = Expr::lt("age", AttributeValue::Long(18));
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        match &patterns[1] {
            Pattern::ValueComparison { operator, .. } => assert_eq!(operator, "<"),
            _ => panic!("expected ValueComparison"),
        }
    }

    #[test]
    fn test_expr_gte_lte_neq() {
        for (expr, expected_op) in [
            (Expr::gte("x", AttributeValue::Long(1)), ">="),
            (Expr::lte("x", AttributeValue::Long(1)), "<="),
            (Expr::neq("x", AttributeValue::Long(1)), "!="),
        ] {
            let mut counter = 0;
            let patterns = expr.to_patterns("$e", &mut counter);
            match &patterns[1] {
                Pattern::ValueComparison { operator, .. } => assert_eq!(operator, expected_op),
                _ => panic!("expected ValueComparison"),
            }
        }
    }

    #[test]
    fn test_expr_contains_patterns() {
        let expr = Expr::contains("name", "Ali");
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 2);
        match &patterns[1] {
            Pattern::ValueComparison {
                operator, value, ..
            } => {
                assert_eq!(operator, "contains");
                if let Value::Literal(lit) = value {
                    assert_eq!(lit.value, json!("Ali"));
                } else {
                    panic!("expected Literal");
                }
            }
            _ => panic!("expected ValueComparison"),
        }
    }

    #[test]
    fn test_expr_like_patterns() {
        let expr = Expr::like("email", "^.*@example\\.com$");
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 2);
        match &patterns[1] {
            Pattern::ValueComparison { operator, .. } => assert_eq!(operator, "like"),
            _ => panic!("expected ValueComparison"),
        }
    }

    #[test]
    fn test_expr_and_flattens() {
        let expr = Expr::and(vec![
            Expr::gt("age", AttributeValue::Long(18)),
            Expr::lt("age", AttributeValue::Long(65)),
        ]);
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        // 2 patterns per comparison × 2 comparisons = 4
        assert_eq!(patterns.len(), 4);
        assert_eq!(counter, 2);
    }

    #[test]
    fn test_expr_or_generates_or_pattern() {
        let expr = Expr::or(vec![
            Expr::eq("dept", AttributeValue::String("HR".into())),
            Expr::eq("dept", AttributeValue::String("Eng".into())),
        ]);
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 1);
        match &patterns[0] {
            Pattern::Or(branches) => {
                assert_eq!(branches.len(), 2);
                assert_eq!(branches[0].len(), 2); // Has + ValueComparison
                assert_eq!(branches[1].len(), 2);
            }
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_expr_not_generates_not_pattern() {
        let expr = Expr::not(Expr::eq(
            "status",
            AttributeValue::String("inactive".into()),
        ));
        let mut counter = 0;
        let patterns = expr.to_patterns("$e", &mut counter);
        assert_eq!(patterns.len(), 1);
        match &patterns[0] {
            Pattern::Not(inner) => assert_eq!(inner.len(), 2),
            _ => panic!("expected Not"),
        }
    }

    #[test]
    fn test_counter_increments_across_expressions() {
        let mut counter = 0;
        Expr::eq("a", AttributeValue::Long(1)).to_patterns("$e", &mut counter);
        Expr::gt("b", AttributeValue::Long(2)).to_patterns("$e", &mut counter);
        Expr::contains("c", "x").to_patterns("$e", &mut counter);
        assert_eq!(counter, 3);
    }

    #[test]
    fn test_agg_count() {
        let mut counter = 0;
        let (assign, pattern) = Agg::Count.to_reduce_assignment("$e", &mut counter);
        assert_eq!(assign.variable, "$_count");
        assert!(pattern.is_none());
        assert_eq!(counter, 0); // Count doesn't use counter
    }

    #[test]
    fn test_agg_sum() {
        let mut counter = 0;
        let (assign, pattern) = Agg::Sum("salary".into()).to_reduce_assignment("$e", &mut counter);
        assert_eq!(assign.variable, "$_sum");
        assert!(pattern.is_some());
        match pattern.unwrap() {
            Pattern::Has {
                attr_type,
                attr_var,
                ..
            } => {
                assert_eq!(attr_type, "salary");
                assert_eq!(attr_var, "$_agg0");
            }
            _ => panic!("expected Has"),
        }
        assert_eq!(counter, 1);
    }

    #[test]
    fn test_agg_result_count() {
        let mut values = HashMap::new();
        values.insert("$_count".into(), json!(42));
        let result = AggResult::new(values);
        assert_eq!(result.count(), Some(42));
    }

    #[test]
    fn test_agg_result_get_f64() {
        let mut values = HashMap::new();
        values.insert("$_mean".into(), json!(2.78));
        let result = AggResult::new(values);
        assert_eq!(result.get_f64("$_mean"), Some(2.78));
    }

    #[test]
    fn test_agg_result_get_i64() {
        let mut values = HashMap::new();
        values.insert("$_sum".into(), json!(100));
        let result = AggResult::new(values);
        assert_eq!(result.get_i64("$_sum"), Some(100));
    }

    #[test]
    fn test_agg_result_missing_key() {
        let result = AggResult::new(HashMap::new());
        assert_eq!(result.count(), None);
        assert_eq!(result.get_f64("$_sum"), None);
    }

    #[test]
    fn test_expr_in_range() {
        let expr = Expr::in_range("age", AttributeValue::Long(20), AttributeValue::Long(30));
        match expr {
            Expr::And(children) => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Expr::Gte { attr, .. } if attr == "age"));
                assert!(matches!(&children[1], Expr::Lte { attr, .. } if attr == "age"));
            }
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn test_expr_startswith() {
        let expr = Expr::startswith("name", "Ali");
        match expr {
            Expr::Like { attr, pattern } => {
                assert_eq!(attr, "name");
                assert_eq!(pattern, "^Ali.*");
            }
            _ => panic!("expected Like"),
        }
    }

    #[test]
    fn test_expr_endswith() {
        let expr = Expr::endswith("name", "ice");
        match expr {
            Expr::Like { attr, pattern } => {
                assert_eq!(attr, "name");
                assert_eq!(pattern, ".*ice$");
            }
            _ => panic!("expected Like"),
        }
    }

    #[test]
    fn test_expr_startswith_escapes_special_chars() {
        let expr = Expr::startswith("email", "foo.bar");
        match expr {
            Expr::Like { pattern, .. } => {
                assert_eq!(pattern, "^foo\\.bar.*");
            }
            _ => panic!("expected Like"),
        }
    }

    #[test]
    fn test_expr_role_player_patterns() {
        let expr = Expr::role_player("employee", Expr::gt("age", AttributeValue::Long(30)));
        let mut counter = 0;
        let patterns = expr.to_patterns("$r", &mut counter);
        // Should generate patterns against $employee, not $r
        assert_eq!(patterns.len(), 2);
        match &patterns[0] {
            Pattern::Has {
                thing_var,
                attr_type,
                ..
            } => {
                assert_eq!(thing_var, "$employee");
                assert_eq!(attr_type, "age");
            }
            _ => panic!("expected Has"),
        }
    }

    #[test]
    fn test_collect_roles() {
        let expr = Expr::and(vec![
            Expr::role_player("employee", Expr::gt("age", AttributeValue::Long(30))),
            Expr::role_player(
                "employer",
                Expr::eq("name", AttributeValue::String("Corp".into())),
            ),
            Expr::role_player("employee", Expr::lt("age", AttributeValue::Long(65))),
        ]);
        let mut roles = Vec::new();
        let mut seen = std::collections::HashSet::new();
        expr.collect_roles(&mut roles, &mut seen);
        assert_eq!(roles, vec!["employee", "employer"]);
    }

    #[test]
    fn test_group_by_result() {
        let mut values1 = HashMap::new();
        values1.insert("$_mean".into(), json!(35.5));
        let mut values2 = HashMap::new();
        values2.insert("$_mean".into(), json!(28.3));

        let result = GroupByResult::new(vec![
            (json!("Engineering"), AggResult::new(values1)),
            (json!("Sales"), AggResult::new(values2)),
        ]);

        assert_eq!(result.len(), 2);
        assert!(!result.is_empty());
        assert_eq!(
            result.get_by_str("Engineering").unwrap().get_f64("$_mean"),
            Some(35.5)
        );
        assert_eq!(
            result.get_by_str("Sales").unwrap().get_f64("$_mean"),
            Some(28.3)
        );
        assert!(result.get_by_str("HR").is_none());
    }

    #[test]
    fn expr_serde_roundtrip() {
        let expr = Expr::and(vec![
            Expr::eq("name", AttributeValue::String("Alice".into())),
            Expr::or(vec![
                Expr::gt("age", AttributeValue::Long(18)),
                Expr::lt("age", AttributeValue::Long(65)),
            ]),
            Expr::not(Expr::eq(
                "status",
                AttributeValue::String("inactive".into()),
            )),
        ]);
        let json = serde_json::to_string(&expr).unwrap();
        let parsed: Expr = serde_json::from_str(&json).unwrap();
        // Verify structure is preserved
        match parsed {
            Expr::And(children) => {
                assert_eq!(children.len(), 3);
                assert!(matches!(&children[0], Expr::Eq { .. }));
                assert!(matches!(&children[1], Expr::Or(_)));
                assert!(matches!(&children[2], Expr::Not(_)));
            }
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn agg_serde_roundtrip() {
        for agg in [
            Agg::Count,
            Agg::Sum("salary".into()),
            Agg::Min("age".into()),
            Agg::Max("age".into()),
            Agg::Mean("score".into()),
        ] {
            let json = serde_json::to_string(&agg).unwrap();
            let _parsed: Agg = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn role_player_serde_roundtrip() {
        let expr = Expr::role_player("employee", Expr::gt("age", AttributeValue::Long(30)));
        let json = serde_json::to_string(&expr).unwrap();
        let parsed: Expr = serde_json::from_str(&json).unwrap();
        match parsed {
            Expr::RolePlayer { role, inner } => {
                assert_eq!(role, "employee");
                assert!(matches!(*inner, Expr::Gt { .. }));
            }
            _ => panic!("expected RolePlayer"),
        }
    }

    #[test]
    fn sort_dir_serde_roundtrip() {
        let json = serde_json::to_string(&SortDir::Asc).unwrap();
        let parsed: SortDir = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SortDir::Asc);
    }
}
