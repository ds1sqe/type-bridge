use crate::core::ast::{Clause, Pattern, Statement, Constraint, Value, LiteralValue, LetAssignment, FetchItem, ReduceAssignment};

pub struct QueryCompiler {
}

impl Default for QueryCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryCompiler {
    pub fn new() -> Self {
        QueryCompiler {}
    }

    pub fn compile(&self, clauses: &[Clause]) -> String {
        clauses.iter()
            .map(|c| self.compile_clause(c))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn compile_clause(&self, clause: &Clause) -> String {
        match clause {
            Clause::Match(patterns) => {
                let p_str = patterns.iter()
                    .map(|p| self.compile_pattern(p))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("match\n{};", p_str)
            }
            Clause::Insert(statements) => {
                let s_str = statements.iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("insert\n{};", s_str)
            }
            Clause::Delete(statements) => {
                let s_str = statements.iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("delete\n{};", s_str)
            }
            Clause::Update(statements) => {
                let s_str = statements.iter()
                    .map(|s| self.compile_statement(s))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("update\n{};", s_str)
            }
            Clause::Fetch(items) => {
                let i_str = items.iter()
                    .map(|i| self.compile_fetch_item(i))
                    .collect::<Vec<_>>()
                    .join(",\n  ");
                format!("fetch {{\n  {}\n}};", i_str)
            }
            Clause::MatchLet(assignments) => {
                let a_str = assignments.iter()
                    .map(|a| self.compile_let_assignment(a))
                    .collect::<Vec<_>>()
                    .join(";\n");
                format!("match\n{};", a_str)
            }
            Clause::Reduce { assignments, group_by } => {
                let a_str = assignments.iter()
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
        }
    }

    fn compile_pattern(&self, pattern: &Pattern) -> String {
        match pattern {
            Pattern::Entity { variable, type_name, constraints, is_strict } => {
                let op = if *is_strict { "isa!" } else { "isa" };
                let mut parts = vec![format!("{} {} {}", variable, op, type_name)];
                for c in constraints {
                    parts.push(self.compile_constraint(c));
                }
                parts.join(", ")
            }
            Pattern::Relation { variable, type_name, role_players, constraints } => {
                let roles_str = if !role_players.is_empty() {
                    let r_str = role_players.iter()
                        .map(|rp| format!("{}: {}", rp.role, rp.player_var))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({}) ", r_str)
                } else {
                    "".to_string()
                };
                let mut parts = vec![format!("{} isa {} {}", variable, type_name, roles_str).trim().to_string()];
                for c in constraints {
                    parts.push(self.compile_constraint(c));
                }
                parts.join(", ")
            }
            Pattern::SubType { variable, parent_type } => format!("{} sub {}", variable, parent_type),
            Pattern::Has { thing_var, attr_type, attr_var } => format!("{} has {} {}", thing_var, attr_type, attr_var),
            Pattern::ValueComparison { var, operator, value } => format!("{} {} {}", var, operator, self.compile_value(value)),
            Pattern::Not(patterns) => {
                let inner = patterns.iter().map(|p| self.compile_pattern(p)).collect::<Vec<_>>().join("; ");
                format!("not {{ {}; }}", inner)
            }
            Pattern::Or(alternatives) => {
                alternatives.iter()
                    .map(|alt| {
                        let inner = alt.iter().map(|p| self.compile_pattern(p)).collect::<Vec<_>>().join("; ");
                        format!("{{ {}; }}", inner)
                    })
                    .collect::<Vec<_>>()
                    .join(" or ")
            }
            Pattern::Iid { variable, iid } => format!("{} iid {}", variable, iid),
            Pattern::Attribute { variable, type_name, value } => {
                if let Some(v) = value {
                    format!("{} isa {}; {} {}", variable, type_name, variable, self.compile_value(v))
                } else {
                    format!("{} isa {}", variable, type_name)
                }
            }
            Pattern::Raw(content) => content.clone(),
        }
    }

    fn compile_statement(&self, stmt: &Statement) -> String {
        match stmt {
            Statement::Has { subject_var, attr_name, value } => format!("{} has {} {}", subject_var, attr_name, self.compile_value(value)),
            Statement::Isa { variable, type_name } => format!("{} isa {}", variable, type_name),
            Statement::Relation { variable, type_name, role_players, include_variable, attributes } => {
                let r_str = role_players.iter()
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
                    let a_str = attributes.iter()
                        .map(|a| {
                            if let Statement::Has { attr_name, value, .. } = a {
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
            Constraint::Has { attr_name, value } => format!("has {} {}", attr_name, self.compile_value(value)),
            Constraint::Isa { type_name, strict } => format!("{} {}", if *strict { "isa!" } else { "isa" }, type_name),
        }
    }

    fn compile_value(&self, value: &Value) -> String {
        match value {
            Value::Literal(lit) => self.format_literal(lit),
            Value::Variable(var) => var.clone(),
            Value::FunctionCall(func) => {
                let args = func.args.iter().map(|a| self.compile_value(a)).collect::<Vec<_>>().join(", ");
                format!("{}({})", func.function, args)
            }
            Value::Arithmetic(arith) => format!("({} {} {})", self.compile_value(&arith.left), arith.operator, self.compile_value(&arith.right)),
        }
    }

    fn format_literal(&self, lit: &LiteralValue) -> String {
        match &lit.value {
            serde_json::Value::String(s) => {
                if lit.value_type == "string" {
                    format!("\"{}\"", s.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))
                } else {
                    // Dates, datetimes, durations are passed as strings but not quoted in TypeQL
                    s.clone()
                }
            }
            serde_json::Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
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
        format!("let {} {} {}", vars, op, self.compile_value(&assign.expression))
    }

    fn compile_fetch_item(&self, item: &FetchItem) -> String {
        match item {
            FetchItem::Attribute { key, var, attr_name } => format!("\"{}\": {}.{}", key, var, attr_name),
            FetchItem::Variable { key, var } => format!("\"{}\": {}", key, var),
            FetchItem::AttributeList { key, var, attr_name } => format!("\"{}\": [{}.{}]", key, var, attr_name),
            FetchItem::Function { key, func_name, var } => format!("\"{}\": {}({})", key, func_name, var),
            FetchItem::Wildcard { key, var } => format!("\"{}\": {}.*", key, var),
            FetchItem::NestedWildcard { key, var } => format!("\"{}\": {{ {}.* }}", key, var),
        }
    }

        fn compile_reduce_assignment(&self, assign: &ReduceAssignment) -> String {

            format!("{} = {}", assign.variable, self.compile_value(&assign.expression))

        }

    }

    

    #[cfg(test)]

    mod tests {

        use super::*;

        use serde_json::json;

    

        #[test]

        fn test_compile_literal() {

            let compiler = QueryCompiler::new();

            

            let s = LiteralValue { value: json!("hello"), value_type: "string".to_string() };

            assert_eq!(compiler.format_literal(&s), "\"hello\"");

            

            let b = LiteralValue { value: json!(true), value_type: "boolean".to_string() };

            assert_eq!(compiler.format_literal(&b), "true");

            

            let i = LiteralValue { value: json!(42), value_type: "long".to_string() };

            assert_eq!(compiler.format_literal(&i), "42");

        }

    

        #[test]

        fn test_compile_match() {

            let compiler = QueryCompiler::new();

            

            let patterns = vec![

                Pattern::Entity {

                    variable: "$p".to_string(),

                    type_name: "person".to_string(),

                    constraints: vec![

                        Constraint::Has {

                            attr_name: "name".to_string(),

                            value: Value::Literal(LiteralValue {

                                value: json!("Alice"),

                                value_type: "string".to_string(),

                            }),

                        }

                    ],

                    is_strict: false,

                }

            ];

            

            let clause = Clause::Match(patterns);

            assert_eq!(compiler.compile_clause(&clause), "match\n$p isa person, has name \"Alice\";");

        }

    }

    