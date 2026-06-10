//! TypeQL query compiler — converts AST [`Clause`](crate::ast::Clause)s into TypeQL query strings.

use crate::ast::{
    Clause, Constraint, FetchItem, LetAssignment, LiteralValue, Pattern, ReduceAssignment,
    SortField, Statement, Value,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ArithmeticValue, FunctionCallValue, RolePlayer};
    use serde_json::json;

    fn compiler() -> QueryCompiler {
        QueryCompiler::new()
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
}
