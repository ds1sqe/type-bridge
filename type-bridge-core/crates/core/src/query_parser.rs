//! Compatibility projection of the official TypeQL parser onto legacy clauses.
#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
use crate::ast::Clause;
use std::fmt;

/// Error returned by [`parse_typeql_query`].
#[derive(Debug, Clone)]
pub enum QueryParseError {
    /// The TypeQL query string could not be parsed.
    ParseError {
        /// Human-readable description of the parse failure.
        message: String,
        /// 1-based line number where the error occurred.
        line: usize,
        /// 1-based column number where the error occurred.
        column: usize,
    },
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryParseError::ParseError {
                message,
                line,
                column,
            } => write!(f, "parse error at {}:{}: {}", line, column, message),
        }
    }
}

impl std::error::Error for QueryParseError {}

/// Parse TypeQL using the official grammar and project supported legacy nodes.
/// Unsupported constructs return a typed adapter error without dropping data.
pub fn parse_typeql_query_checked(input: &str) -> Result<Vec<Clause>, crate::compiler::TqlError> {
    crate::tql::project_legacy(&crate::tql::parse(input)?)
}

/// Parse TypeQL into the deprecated clause vocabulary.
/// Use the checked entry point to distinguish unsupported constructs.
pub fn parse_typeql_query(input: &str) -> Result<Vec<Clause>, QueryParseError> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    parse_typeql_query_checked(input).map_err(|error| {
        let (line, column) = match error {
            crate::tql::TqlError::InvalidSyntax { line, column } => (line, column),
            _ => (1, 1),
        };
        QueryParseError::ParseError {
            message: error.to_string(),
            line,
            column,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    enum StmtContext {
        Insert,
        Delete,
    }
    fn parse_statement(
        input: &mut &str,
        context: StmtContext,
    ) -> Result<Statement, QueryParseError> {
        let source = match context {
            StmtContext::Insert => format!("insert {};", input.trim_end_matches(';')),
            StmtContext::Delete => format!(
                "match $x isa person; delete {};",
                input.trim_end_matches(';')
            ),
        };
        let clauses = parse_typeql_query(&source)?;
        let values = match clauses.into_iter().last().unwrap() {
            Clause::Insert(v) | Clause::Delete(v) => v,
            _ => panic!("write"),
        };
        assert_eq!(values.len(), 1);
        *input = "";
        Ok(values.into_iter().next().unwrap())
    }
    // Test fragments are enclosed in complete queries before official parsing.
    fn parse_pattern(input: &mut &str) -> Result<Pattern, QueryParseError> {
        let clauses = parse_typeql_query(&format!("match {};", input.trim_end_matches(';')))?;
        let Clause::Match(mut values) = clauses.into_iter().next().unwrap() else {
            panic!("match")
        };
        *input = "";
        Ok(values.remove(0))
    }
    fn parse_value(input: &mut &str) -> Result<Value, QueryParseError> {
        let clauses = parse_typeql_query(&format!("match let $value = {};", input))?;
        let Clause::MatchLet(mut values) = clauses.into_iter().next().unwrap() else {
            panic!("let")
        };
        *input = "";
        Ok(values.remove(0).expression)
    }
    fn variable(input: &mut &str) -> Result<String, QueryParseError> {
        let Pattern::Entity { variable, .. } =
            parse_pattern(&mut format!("{} isa person", input).as_str())?
        else {
            panic!("entity")
        };
        *input = "";
        Ok(variable)
    }
    fn quoted_string(input: &mut &str) -> Result<String, QueryParseError> {
        let Value::Literal(value) = parse_value(input)? else {
            panic!("literal")
        };
        Ok(value.value.as_str().unwrap().to_owned())
    }
    fn hex_id(input: &mut &str) -> Result<String, QueryParseError> {
        let Pattern::Iid { iid, .. } = parse_pattern(&mut format!("$x iid {}", input).as_str())?
        else {
            panic!("iid")
        };
        *input = "";
        Ok(iid)
    }
    fn parse_constraint(input: &mut &str) -> Result<Constraint, QueryParseError> {
        let Pattern::Entity {
            mut constraints, ..
        } = parse_pattern(&mut format!("$x isa person, {}", input).as_str())?
        else {
            panic!("entity")
        };
        *input = "";
        Ok(constraints.remove(0))
    }
    fn parse_fetch_item(input: &mut &str) -> Result<FetchItem, QueryParseError> {
        let mut clauses =
            parse_typeql_query(&format!("match $x isa person; fetch {{ {} }};", input))?;
        let Clause::Fetch(mut values) = clauses.remove(1) else {
            panic!("fetch")
        };
        *input = "";
        Ok(values.remove(0))
    }
    fn parse_comparison_op(input: &mut &str) -> Result<String, QueryParseError> {
        let Pattern::ValueComparison { operator, .. } =
            parse_pattern(&mut format!("$x {} \"value\"", input).as_str())?
        else {
            panic!("comparison")
        };
        *input = "";
        Ok(operator)
    }

    use crate::compiler::QueryCompiler;
    use serde_json::json;

    // === Token tests ===

    #[test]
    fn test_parse_variable() {
        let mut input = "$person";
        assert_eq!(variable(&mut input).unwrap(), "$person");
        assert!(input.is_empty());
    }

    #[test]
    fn test_parse_variable_with_hyphen() {
        let mut input = "$my-var";
        assert_eq!(variable(&mut input).unwrap(), "$my-var");
    }

    #[test]
    fn test_parse_quoted_string() {
        let mut input = r#""hello world""#;
        assert_eq!(quoted_string(&mut input).unwrap(), "hello world");
    }

    #[test]
    fn test_parse_quoted_string_with_escapes() {
        let mut input = r#""line1\nline2\ttab\\slash\"quote""#;
        assert_eq!(
            quoted_string(&mut input).unwrap(),
            "line1\nline2\ttab\\slash\"quote"
        );
    }

    #[test]
    fn test_parse_hex_id() {
        let mut input = "0x0000000000000001";
        assert_eq!(hex_id(&mut input).unwrap(), "0x0000000000000001");
    }

    // === Value tests ===

    #[test]
    fn test_parse_string_value() {
        let mut input = r#""Alice""#;
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("Alice"));
                assert_eq!(lit.value_type, "string");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_boolean_value() {
        let mut input = "true";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(true));
                assert_eq!(lit.value_type, "boolean");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_long_value() {
        let mut input = "42";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(42));
                assert_eq!(lit.value_type, "long");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_negative_long() {
        let mut input = "-100";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!(-100));
                assert_eq!(lit.value_type, "long");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_double_value() {
        let mut input = "3.15";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value_type, "double");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_decimal_value() {
        let mut input = "42dec";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("42"));
                assert_eq!(lit.value_type, "decimal");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_decimal_with_fraction() {
        let mut input = "123.45dec";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value_type, "decimal");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_date_value() {
        let mut input = "2024-01-15";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15"));
                assert_eq!(lit.value_type, "date");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_datetime_value() {
        let mut input = "2024-01-15T10:30:00";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15T10:30:00"));
                assert_eq!(lit.value_type, "datetime");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_datetime_tz_value() {
        let mut input = "2024-01-15T10:30:00+05:30";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("2024-01-15T10:30:00+05:30"));
                assert_eq!(lit.value_type, "datetime-tz");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_duration_value() {
        let mut input = "P1Y2M3D";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("P1Y2M3D"));
                assert_eq!(lit.value_type, "duration");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_duration_with_time() {
        let mut input = "PT5H30M";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Literal(lit) => {
                assert_eq!(lit.value, json!("PT5H30M"));
                assert_eq!(lit.value_type, "duration");
            }
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn test_parse_variable_value() {
        let mut input = "$person";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Variable(v) => assert_eq!(v, "$person"),
            _ => panic!("expected Variable"),
        }
    }

    #[test]
    fn test_parse_function_call() {
        let mut input = "count($p)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "count");
                assert_eq!(f.args.len(), 1);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_function_call_multiple_args() {
        let mut input = "max($x, $y)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "max");
                assert_eq!(f.args.len(), 2);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_function_call_no_args() {
        let mut input = "my_func()";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::FunctionCall(f) => {
                assert_eq!(f.function, "my_func");
                assert!(f.args.is_empty());
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_parse_arithmetic_value() {
        let mut input = "($x + 5)";
        let val = parse_value(&mut input).unwrap();
        match val {
            Value::Arithmetic(a) => {
                assert_eq!(a.operator, "+");
            }
            _ => panic!("expected Arithmetic"),
        }
    }

    // === Constraint tests ===

    #[test]
    fn test_parse_iid_constraint() {
        let mut input = "iid 0x123abc";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Iid(id) => assert_eq!(id, "0x123abc"),
            _ => panic!("expected Iid"),
        }
    }

    #[test]
    fn test_parse_has_constraint() {
        let mut input = r#"has name "Alice""#;
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Has { attr_name, value } => {
                assert_eq!(attr_name, "name");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Alice")),
                    _ => panic!("expected Literal value"),
                }
            }
            _ => panic!("expected Has"),
        }
    }

    #[test]
    fn test_parse_isa_constraint() {
        let mut input = "isa person";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Isa { type_name, strict } => {
                assert_eq!(type_name, "person");
                assert!(!strict);
            }
            _ => panic!("expected Isa"),
        }
    }

    #[test]
    fn test_parse_isa_strict_constraint() {
        let mut input = "isa! person";
        let c = parse_constraint(&mut input).unwrap();
        match c {
            Constraint::Isa { type_name, strict } => {
                assert_eq!(type_name, "person");
                assert!(strict);
            }
            _ => panic!("expected Isa"),
        }
    }

    // === Pattern tests ===

    #[test]
    fn test_parse_entity_pattern() {
        let mut input = r#"$p isa person, has name "Alice""#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity {
                variable,
                type_name,
                constraints,
                is_strict,
            } => {
                assert_eq!(variable, "$p");
                assert_eq!(type_name, "person");
                assert!(!is_strict);
                assert_eq!(constraints.len(), 1);
            }
            _ => panic!("expected Entity, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_entity_pattern_strict() {
        let mut input = "$p isa! person";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity { is_strict, .. } => assert!(is_strict),
            _ => panic!("expected Entity"),
        }
    }

    #[test]
    fn test_parse_entity_multiple_constraints() {
        let mut input = r#"$p isa person, iid 0x123, has name "Alice", has age 30"#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Entity { constraints, .. } => {
                assert_eq!(constraints.len(), 3);
            }
            _ => panic!("expected Entity"),
        }
    }

    #[test]
    fn test_parse_relation_pattern() {
        let mut input = "$r isa friendship (friend: $p, friend: $q)";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Relation {
                variable,
                type_name,
                role_players,
                constraints,
            } => {
                assert_eq!(variable, "$r");
                assert_eq!(type_name, "friendship");
                assert_eq!(role_players.len(), 2);
                assert_eq!(role_players[0].role, "friend");
                assert_eq!(role_players[0].player_var, "$p");
                assert!(constraints.is_empty());
            }
            _ => panic!("expected Relation, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_relation_with_constraints() {
        let mut input =
            r#"$r isa employment (employee: $p, employer: $c), has start-date 2024-01-15"#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Relation {
                role_players,
                constraints,
                ..
            } => {
                assert_eq!(role_players.len(), 2);
                assert_eq!(constraints.len(), 1);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_subtype_pattern() {
        let mut input = "$t sub entity";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::SubType {
                variable,
                parent_type,
            } => {
                assert_eq!(variable, "$t");
                assert_eq!(parent_type, "entity");
            }
            _ => panic!("expected SubType"),
        }
    }

    #[test]
    fn test_parse_has_pattern() {
        let mut input = "$p has name $n";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Has {
                thing_var,
                attr_type,
                attr_var,
            } => {
                assert_eq!(thing_var, "$p");
                assert_eq!(attr_type, "name");
                assert_eq!(attr_var, "$n");
            }
            _ => panic!("expected Has, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison() {
        let mut input = "$age > 18";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison {
                var,
                operator,
                value,
            } => {
                assert_eq!(var, "$age");
                assert_eq!(operator, ">");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!(18)),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected ValueComparison, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison_operators() {
        for (op_str, expected) in &[
            (">=", ">="),
            ("<=", "<="),
            ("!=", "!="),
            ("==", "=="),
            (">", ">"),
            ("<", "<"),
        ] {
            let input_str = format!("$x {} 10", op_str);
            let mut input = input_str.as_str();
            let p = parse_pattern(&mut input).unwrap();
            match p {
                Pattern::ValueComparison { operator, .. } => {
                    assert_eq!(operator, *expected);
                }
                _ => panic!("expected ValueComparison for {}", op_str),
            }
        }
    }

    #[test]
    fn test_parse_not_pattern() {
        let mut input = "not { $p isa person; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Not(inner) => {
                assert_eq!(inner.len(), 1);
            }
            _ => panic!("expected Not"),
        }
    }

    #[test]
    fn test_parse_or_pattern() {
        let mut input = "{ $p isa person; } or { $e isa employee; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Or(alternatives) => {
                assert_eq!(alternatives.len(), 2);
                assert_eq!(alternatives[0].len(), 1);
                assert_eq!(alternatives[1].len(), 1);
            }
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_parse_or_pattern_three_alternatives() {
        let mut input = "{ $a isa a; } or { $b isa b; } or { $c isa c; }";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Or(alternatives) => assert_eq!(alternatives.len(), 3),
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_parse_iid_pattern() {
        let mut input = "$p iid 0x0000000000000001";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Iid { variable, iid } => {
                assert_eq!(variable, "$p");
                assert_eq!(iid, "0x0000000000000001");
            }
            _ => panic!("expected Iid"),
        }
    }

    #[test]
    fn test_parse_attribute_pattern_with_value() {
        let mut input = r#"$a isa name "John""#;
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::Attribute {
                variable,
                type_name,
                value,
            } => {
                assert_eq!(variable, "$a");
                assert_eq!(type_name, "name");
                assert!(value.is_some());
                match value.unwrap() {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("John")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected Attribute, got {:?}", p),
        }
    }

    // === Statement tests ===

    #[test]
    fn test_parse_has_statement() {
        let mut input = r#"$p has name "Alice""#;
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Has {
                subject_var,
                attr_name,
                value,
            } => {
                assert_eq!(subject_var, "$p");
                assert_eq!(attr_name, "name");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Alice")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected Has"),
        }
    }

    #[test]
    fn test_parse_isa_statement() {
        let mut input = "$p isa person";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Isa {
                variable,
                type_name,
            } => {
                assert_eq!(variable, "$p");
                assert_eq!(type_name, "person");
            }
            _ => panic!("expected Isa"),
        }
    }

    #[test]
    fn test_parse_relation_stmt_with_var() {
        let mut input = "$r isa friendship, links (friend: $p, friend: $q)";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation {
                variable,
                type_name,
                role_players,
                include_variable,
                attributes,
            } => {
                assert_eq!(variable, "$r");
                assert_eq!(type_name, "friendship");
                assert!(include_variable);
                assert_eq!(role_players.len(), 2);
                assert!(attributes.is_empty());
            }
            _ => panic!("expected Relation, got {:?}", s),
        }
    }

    #[test]
    fn test_parse_relation_stmt_no_var() {
        let mut input = "(employee: $p, employer: $c) isa employment";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation {
                include_variable,
                role_players,
                ..
            } => {
                assert!(!include_variable);
                assert_eq!(role_players.len(), 2);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_relation_stmt_with_attrs() {
        let mut input = "(employee: $p, employer: $c) isa employment, has start-date 2024-01-15";
        let s = parse_statement(&mut input, StmtContext::Insert).unwrap();
        match s {
            Statement::Relation { attributes, .. } => {
                assert_eq!(attributes.len(), 1);
            }
            _ => panic!("expected Relation"),
        }
    }

    #[test]
    fn test_parse_delete_thing() {
        let mut input = "$p";
        let s = parse_statement(&mut input, StmtContext::Delete).unwrap();
        match s {
            Statement::DeleteThing(var) => assert_eq!(var, "$p"),
            _ => panic!("expected DeleteThing"),
        }
    }

    // === Fetch item tests ===

    #[test]
    fn test_parse_fetch_attribute() {
        let mut input = r#""name": $p.name"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Attribute {
                key,
                var,
                attr_name,
            } => {
                assert_eq!(key, "name");
                assert_eq!(var, "$p");
                assert_eq!(attr_name, "name");
            }
            _ => panic!("expected Attribute, got {:?}", item),
        }
    }

    #[test]
    fn test_parse_fetch_variable() {
        let mut input = r#""person": $p"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Variable { key, var } => {
                assert_eq!(key, "person");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Variable"),
        }
    }

    #[test]
    fn test_parse_fetch_attribute_list() {
        let mut input = r#""names": [$p.name]"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::AttributeList {
                key,
                var,
                attr_name,
            } => {
                assert_eq!(key, "names");
                assert_eq!(var, "$p");
                assert_eq!(attr_name, "name");
            }
            _ => panic!("expected AttributeList"),
        }
    }

    #[test]
    fn test_parse_fetch_function() {
        let mut input = r#""id": iid($p)"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::Function {
                key,
                func_name,
                var,
            } => {
                assert_eq!(key, "id");
                assert_eq!(func_name, "iid");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Function"),
        }
    }

    #[test]
    fn test_parse_fetch_wildcard() {
        let mut input = r#""all": {$p.*}"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::NestedWildcard { key, var } => {
                assert_eq!(key, "all");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected Wildcard"),
        }
    }

    #[test]
    fn test_parse_fetch_nested_wildcard() {
        let mut input = r#""nested": { $p.* }"#;
        let item = parse_fetch_item(&mut input).unwrap();
        match item {
            FetchItem::NestedWildcard { key, var } => {
                assert_eq!(key, "nested");
                assert_eq!(var, "$p");
            }
            _ => panic!("expected NestedWildcard"),
        }
    }

    // === Clause tests ===

    #[test]
    fn test_parse_match_clause() {
        let input = "match\n$p isa person;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Match(patterns) => {
                assert_eq!(patterns.len(), 1);
            }
            _ => panic!("expected Match"),
        }
    }

    #[test]
    fn test_parse_insert_clause() {
        let input = "insert\n$p isa person;\n$p has name \"Alice\";";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Insert(stmts) => {
                assert_eq!(stmts.len(), 2);
            }
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn test_parse_put_clause() {
        let input = "put\n$p isa person;\n$p has name \"Alice\";";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Put(stmts) => {
                assert_eq!(stmts.len(), 2);
            }
            _ => panic!("expected Put"),
        }
    }

    #[test]
    fn test_parse_match_put() {
        let input = "match\n$p isa person;\nput\n$p has age 30;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Put(_)));
    }

    #[test]
    fn test_parse_delete_clause() {
        let input = "delete\n$p;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Delete(stmts) => {
                assert_eq!(stmts.len(), 1);
                match &stmts[0] {
                    Statement::DeleteThing(v) => assert_eq!(v, "$p"),
                    _ => panic!("expected DeleteThing"),
                }
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_parse_update_clause() {
        let input = "update\n$p has age 31;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Update(stmts) => assert_eq!(stmts.len(), 1),
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_fetch_clause() {
        let input = "match $p isa person; fetch {\n  \"name\": $p.name,\n  \"age\": [$p.age]\n};";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        match &clauses[1] {
            Clause::Fetch(items) => assert_eq!(items.len(), 2),
            _ => panic!("expected Fetch"),
        }
    }

    #[test]
    fn test_parse_match_let_clause() {
        let input = "match\nlet $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].variables, vec!["$count"]);
                assert!(!assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet, got {:?}", clauses[0]),
        }
    }

    #[test]
    fn test_parse_match_let_stream() {
        let input = "match\nlet $item in get_items();";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert!(assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet"),
        }
    }

    #[test]
    fn test_parse_match_let_multi_vars() {
        let input = "match\nlet $type, $count in count_by_type();";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::MatchLet(assignments) => {
                assert_eq!(assignments[0].variables, vec!["$type", "$count"]);
                assert!(assignments[0].is_stream);
            }
            _ => panic!("expected MatchLet"),
        }
    }

    #[test]
    fn test_parse_reduce_clause() {
        let input = "reduce $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Reduce {
                assignments,
                group_by,
            } => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].variable, "$count");
                assert!(group_by.is_none());
            }
            _ => panic!("expected Reduce"),
        }
    }

    #[test]
    fn test_parse_reduce_with_groupby() {
        let input = "reduce $count = count($p) groupby $dept;";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Reduce {
                assignments,
                group_by,
            } => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(group_by, &Some("$dept".to_string()));
            }
            _ => panic!("expected Reduce"),
        }
    }

    #[test]
    fn test_parse_reduce_multiple_assignments() {
        let input = "reduce $count = count($p), $total = sum($salary);";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Reduce { assignments, .. } => {
                assert_eq!(assignments.len(), 2);
            }
            _ => panic!("expected Reduce"),
        }
    }

    // === Multi-clause tests ===

    #[test]
    fn test_parse_match_insert() {
        let input = "match\n$p isa person;\ninsert\n$p has age 30;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Insert(_)));
    }

    #[test]
    fn test_parse_match_delete() {
        let input = "match\n$p isa person;\ndelete\n$p;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Delete(_)));
    }

    #[test]
    fn test_parse_match_fetch() {
        let input = "match\n$p isa person;\nfetch {\n  \"name\": $p.name\n};";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Fetch(_)));
    }

    #[test]
    fn test_parse_match_reduce() {
        let input = "match\n$p isa person;\nreduce $count = count($p);";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 2);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Reduce { .. }));
    }

    // === Roundtrip tests ===

    fn assert_roundtrip(typeql: &str) {
        let compiler = QueryCompiler::new();
        let parsed = parse_typeql_query(typeql).unwrap();
        let recompiled = compiler.compile(&parsed);
        let reparsed = parse_typeql_query(&recompiled).unwrap();
        assert_eq!(parsed, reparsed, "{typeql}");
    }

    #[test]
    fn test_roundtrip_simple_match() {
        assert_roundtrip("match\n$p isa person;");
    }

    #[test]
    fn test_roundtrip_match_with_constraints() {
        assert_roundtrip("match\n$p isa person, has name \"Alice\", has age 30;");
    }

    #[test]
    fn test_roundtrip_match_strict() {
        assert_roundtrip("match\n$p isa! person;");
    }

    #[test]
    fn test_roundtrip_match_iid() {
        assert_roundtrip("match\n$p iid 0x0000000000000001;");
    }

    #[test]
    fn test_roundtrip_match_subtype() {
        assert_roundtrip("match\n$t sub entity;");
    }

    #[test]
    fn test_roundtrip_match_has_pattern() {
        assert_roundtrip("match\n$p has name $n;");
    }

    #[test]
    fn test_roundtrip_match_value_comparison() {
        assert_roundtrip("match\n$age > 18;");
    }

    #[test]
    fn test_roundtrip_match_not() {
        assert_roundtrip("match\nnot { $p isa person; };");
    }

    #[test]
    fn test_roundtrip_match_or() {
        assert_roundtrip("match\n{ $p isa person; } or { $e isa employee; };");
    }

    #[test]
    fn test_roundtrip_relation_match() {
        assert_roundtrip("match\n$r isa friendship (friend: $p, friend: $q);");
    }

    #[test]
    fn test_roundtrip_attribute_with_value() {
        assert_roundtrip("match\n$a isa name \"John\";");
    }

    #[test]
    fn test_roundtrip_insert() {
        assert_roundtrip("insert\n$p isa person;\n$p has name \"Alice\";");
    }

    #[test]
    fn test_roundtrip_relation_insert_with_var() {
        assert_roundtrip(
            "match\n$p isa person;\ninsert\n$r isa friendship, links (friend: $p, friend: $q);",
        );
    }

    #[test]
    fn test_roundtrip_relation_insert_no_var() {
        assert_roundtrip("insert\n(employee: $p, employer: $c) isa employment;");
    }

    #[test]
    fn test_roundtrip_relation_insert_with_attrs() {
        assert_roundtrip(
            "insert\n(employee: $p, employer: $c) isa employment, has start-date 2024-01-15;",
        );
    }

    #[test]
    fn test_roundtrip_delete() {
        assert_roundtrip("match\n$p isa person;\ndelete\n$p;");
    }

    #[test]
    fn test_roundtrip_update() {
        assert_roundtrip("update\n$p has age 31;");
    }

    #[test]
    fn test_roundtrip_fetch() {
        assert_roundtrip(
            "match $p isa person; fetch {\n  \"name\": $p.name,\n  \"age\": [$p.age]\n};",
        );
    }

    #[test]
    fn test_roundtrip_fetch_all_types() {
        assert_roundtrip(
            "match $p isa person; fetch {\n  \"a\": $p.name,\n  \"b\": $p,\n  \"c\": [$p.nick],\n  \"d\": iid($p),\n  \"e\": {$p.*},\n  \"f\": { $p.* }\n};",
        );
    }

    #[test]
    fn test_roundtrip_match_let() {
        assert_roundtrip("match\nlet $count = count($p);");
    }

    #[test]
    fn test_roundtrip_match_let_stream() {
        assert_roundtrip("match\nlet $item in get_items();");
    }

    #[test]
    fn test_roundtrip_match_let_multi_vars() {
        assert_roundtrip("match\nlet $type, $count in count_by_type();");
    }

    #[test]
    fn test_roundtrip_reduce() {
        assert_roundtrip("reduce $count = count($p);");
    }

    #[test]
    fn test_roundtrip_reduce_groupby() {
        assert_roundtrip("reduce $count = count($p) groupby $dept;");
    }

    #[test]
    fn test_roundtrip_reduce_multiple() {
        assert_roundtrip("reduce $count = count($p), $total = sum($salary);");
    }

    #[test]
    fn test_roundtrip_complex_match() {
        assert_roundtrip(
            "match\n$p isa person, has name \"Alice\", has age 30;\n$r isa friendship (friend: $p, friend: $q);\n$q isa person;\nnot { $q isa! employee; };",
        );
    }

    #[test]
    fn test_roundtrip_boolean_values() {
        assert_roundtrip("match\n$p isa person, has active true;");
    }

    #[test]
    fn test_roundtrip_decimal_value() {
        assert_roundtrip("match\n$p isa person, has salary 50000dec;");
    }

    #[test]
    fn test_roundtrip_date_value() {
        assert_roundtrip("match\n$p isa person, has birth-date 2024-01-15;");
    }

    #[test]
    fn test_roundtrip_datetime_value() {
        assert_roundtrip("match\n$p isa person, has created-at 2024-01-15T10:30:00;");
    }

    #[test]
    fn test_roundtrip_arithmetic() {
        assert_roundtrip("match\n$x > ($base + 100);");
    }

    #[test]
    fn test_roundtrip_function_call() {
        assert_roundtrip("match\nlet $count = count($p);");
    }

    // === Error tests ===

    #[test]
    fn test_error_empty_input() {
        let result = parse_typeql_query("").unwrap();
        assert!(result.is_empty());
        let result = parse_typeql_query("   \n  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_error_unrecognized_keyword() {
        assert!(parse_typeql_query("foobar $x;").is_err());
    }

    #[test]
    fn test_error_trailing_content() {
        assert!(parse_typeql_query("match\n$p isa person;\ngarbage").is_err());
    }

    // === Flexible whitespace tests ===

    #[test]
    fn test_flexible_whitespace() {
        // Extra spaces and different formatting should still parse
        let input = "match   \n  $p   isa   person  ,  has name   \"Alice\"  ;  ";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Match(_)));
    }

    #[test]
    fn test_comments_in_query() {
        let input = "match\n# find all persons\n$p isa person;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
    }

    #[test]
    fn test_multi_pattern_match() {
        let input = "match\n$p isa person;\n$p has name $n;\n$n > \"A\";";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Match(patterns) => assert_eq!(patterns.len(), 3),
            _ => panic!("expected Match"),
        }
    }

    // === Sort/Limit/Offset tests ===

    #[test]
    fn test_parse_sort_clause_single() {
        let input = "sort $age asc;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        match &clauses[0] {
            Clause::Sort(fields) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].variable, "$age");
                assert!(fields[0].ascending);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn test_parse_sort_clause_multiple() {
        let input = "sort $name asc, $age desc;";
        let clauses = parse_typeql_query(input).unwrap();
        match &clauses[0] {
            Clause::Sort(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].variable, "$name");
                assert!(fields[0].ascending);
                assert_eq!(fields[1].variable, "$age");
                assert!(!fields[1].ascending);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn test_parse_limit_clause() {
        let input = "limit 10;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Limit(10)));
    }

    #[test]
    fn test_parse_offset_clause() {
        let input = "offset 20;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(matches!(&clauses[0], Clause::Offset(20)));
    }

    #[test]
    fn test_parse_match_sort_limit_offset() {
        let input = "match\n$p isa person;\nsort $age asc;\nlimit 10;\noffset 5;";
        let clauses = parse_typeql_query(input).unwrap();
        assert_eq!(clauses.len(), 4);
        assert!(matches!(&clauses[0], Clause::Match(_)));
        assert!(matches!(&clauses[1], Clause::Sort(_)));
        assert!(matches!(&clauses[2], Clause::Limit(10)));
        assert!(matches!(&clauses[3], Clause::Offset(5)));
    }

    // === Contains/Like operator tests ===

    #[test]
    fn test_parse_contains_operator() {
        let mut input = "contains";
        let op = parse_comparison_op(&mut input).unwrap();
        assert_eq!(op, "contains");
    }

    #[test]
    fn test_parse_like_operator() {
        let mut input = "like";
        let op = parse_comparison_op(&mut input).unwrap();
        assert_eq!(op, "like");
    }

    #[test]
    fn test_parse_value_comparison_contains() {
        let mut input = "$name contains \"Ali\"";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison {
                var,
                operator,
                value,
            } => {
                assert_eq!(var, "$name");
                assert_eq!(operator, "contains");
                match value {
                    Value::Literal(lit) => assert_eq!(lit.value, json!("Ali")),
                    _ => panic!("expected Literal"),
                }
            }
            _ => panic!("expected ValueComparison, got {:?}", p),
        }
    }

    #[test]
    fn test_parse_value_comparison_like() {
        let mut input = "$name like \"^A.*\"";
        let p = parse_pattern(&mut input).unwrap();
        match p {
            Pattern::ValueComparison { var, operator, .. } => {
                assert_eq!(var, "$name");
                assert_eq!(operator, "like");
            }
            _ => panic!("expected ValueComparison"),
        }
    }

    // === Sort/Limit/Offset roundtrip tests ===

    #[test]
    fn test_roundtrip_sort_single() {
        assert_roundtrip("match\n$p isa person;\nsort $age asc;");
    }

    #[test]
    fn test_roundtrip_sort_multiple() {
        assert_roundtrip("match\n$p isa person;\nsort $name asc, $age desc;");
    }

    #[test]
    fn test_roundtrip_limit() {
        assert_roundtrip("match\n$p isa person;\nlimit 10;");
    }

    #[test]
    fn test_roundtrip_offset() {
        assert_roundtrip("match\n$p isa person;\noffset 20;");
    }

    #[test]
    fn test_roundtrip_sort_limit_offset() {
        assert_roundtrip("match\n$p isa person;\nsort $age asc;\nlimit 10;\noffset 5;");
    }

    #[test]
    fn test_roundtrip_contains() {
        assert_roundtrip("match\n$name contains \"Ali\";");
    }

    #[test]
    fn test_roundtrip_like() {
        assert_roundtrip("match\n$name like \"^A.*\";");
    }
}
