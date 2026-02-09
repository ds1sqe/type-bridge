use serde::{Deserialize, Serialize};
use crate::core::ast::{Pattern, Statement, Constraint, Value, RolePlayer};
use crate::core::reserved_words::is_reserved_word;
use unicode_ident::{is_xid_start, is_xid_continue};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub path: String,
}

pub struct ValidationEngine {
    // Placeholder for schema and rules
}

impl ValidationEngine {
    pub fn new() -> Self {
        ValidationEngine {}
    }

    pub fn validate_variable_name(&self, name: &str, context: &str, path: &str) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if !name.starts_with('$') {
            errors.push(ValidationError {
                code: "INVALID_VARIABLE".to_string(),
                message: format!("Variable '{}' must start with '$' in {}", name, context),
                path: path.to_string(),
            });
        } else if name.len() == 1 {
            errors.push(ValidationError {
                code: "EMPTY_VARIABLE".to_string(),
                message: format!("Variable name cannot be just '$' in {}", context),
                path: path.to_string(),
            });
        }
        errors
    }

    pub fn validate_type_name(&self, name: &str, context: &str) -> ValidationResult {
        let mut errors = Vec::new();

        if name.is_empty() {
            errors.push(ValidationError {
                code: "EMPTY_NAME".to_string(),
                message: format!("Empty {} name is not allowed", context),
                path: "".to_string(),
            });
            return ValidationResult { is_valid: false, errors };
        }

        if is_reserved_word(name) {
            errors.push(ValidationError {
                code: "RESERVED_WORD".to_string(),
                message: format!("Cannot use '{}' as {} name: it's a TypeQL reserved word!", name, context),
                path: "".to_string(),
            });
        }

        let mut chars = name.chars();
        if let Some(first) = chars.next() {
            if !is_xid_start(first) && first != '_' {
                errors.push(ValidationError {
                    code: "INVALID_START".to_string(),
                    message: format!("{} name '{}' must start with a letter or underscore", context, name),
                    path: "".to_string(),
                });
            }
        }

        for c in chars {
            if !is_xid_continue(c) && c != '-' {
                errors.push(ValidationError {
                    code: "INVALID_CHAR".to_string(),
                    message: format!("{} name '{}' contains invalid character '{}'", context, name, c),
                    path: "".to_string(),
                });
                break;
            }
        }

        ValidationResult {
            is_valid: errors.is_empty(),
            errors,
        }
    }

    fn check_type_name(&self, name: &str, context: &str, path: &str) -> Vec<ValidationError> {
        let res = self.validate_type_name(name, context);
        res.errors.into_iter().map(|mut e| { e.path = path.to_string(); e }).collect()
    }

    pub fn validate_pattern(&self, pattern: &Pattern) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_pattern_recursive(pattern, "pattern", &mut errors);
        ValidationResult {
            is_valid: errors.is_empty(),
            errors,
        }
    }

    fn validate_pattern_recursive(&self, pattern: &Pattern, path: &str, errors: &mut Vec<ValidationError>) {
        match pattern {
            Pattern::Entity { variable, type_name, constraints, .. } => {
                errors.extend(self.validate_variable_name(variable, "Entity", path));
                errors.extend(self.check_type_name(type_name, "Entity type", path));
                for (i, c) in constraints.iter().enumerate() {
                    self.validate_constraint_recursive(c, &format!("{}.constraints[{}]", path, i), errors);
                }
            }
            Pattern::Relation { variable, type_name, role_players, constraints } => {
                errors.extend(self.validate_variable_name(variable, "Relation", path));
                errors.extend(self.check_type_name(type_name, "Relation type", path));
                for (i, rp) in role_players.iter().enumerate() {
                    let rp_path = format!("{}.role_players[{}]", path, i);
                    errors.extend(self.check_type_name(&rp.role, "Role", &rp_path));
                    errors.extend(self.validate_variable_name(&rp.player_var, "Player", &rp_path));
                }
                for (i, c) in constraints.iter().enumerate() {
                    self.validate_constraint_recursive(c, &format!("{}.constraints[{}]", path, i), errors);
                }
            }
            Pattern::SubType { variable, parent_type } => {
                errors.extend(self.validate_variable_name(variable, "SubType variable", path));
                errors.extend(self.check_type_name(parent_type, "Parent type", path));
            }
            Pattern::Attribute { variable, type_name, value } => {
                errors.extend(self.validate_variable_name(variable, "Attribute variable", path));
                errors.extend(self.check_type_name(type_name, "Attribute type", path));
                if let Some(v) = value {
                    self.validate_value_recursive(v, &format!("{}.value", path), errors);
                }
            }
            Pattern::Has { thing_var, attr_type, attr_var } => {
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
            Pattern::Or(alternatives) => {
                for (i, alt) in alternatives.iter().enumerate() {
                    for (j, p) in alt.iter().enumerate() {
                        self.validate_pattern_recursive(p, &format!("{}.or[{}][{}]", path, i, j), errors);
                    }
                }
            }
            Pattern::Iid { variable, .. } => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
            }
            Pattern::Raw(_) => {}
        }
    }

    pub fn validate_statement(&self, statement: &Statement) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_statement_recursive(statement, "statement", &mut errors);
        ValidationResult {
            is_valid: errors.is_empty(),
            errors,
        }
    }

    fn validate_statement_recursive(&self, statement: &Statement, path: &str, errors: &mut Vec<ValidationError>) {
        match statement {
            Statement::Has { subject_var, attr_name, value } => {
                errors.extend(self.validate_variable_name(subject_var, "Subject", path));
                errors.extend(self.check_type_name(attr_name, "Attribute type", path));
                self.validate_value_recursive(value, &format!("{}.value", path), errors);
            }
            Statement::Isa { variable, type_name } => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
                errors.extend(self.check_type_name(type_name, "Type", path));
            }
            Statement::Relation { variable, type_name, role_players, attributes, .. } => {
                errors.extend(self.validate_variable_name(variable, "Relation", path));
                errors.extend(self.check_type_name(type_name, "Relation type", path));
                for (i, rp) in role_players.iter().enumerate() {
                    let rp_path = format!("{}.role_players[{}]", path, i);
                    errors.extend(self.check_type_name(&rp.role, "Role", &rp_path));
                    errors.extend(self.validate_variable_name(&rp.player_var, "Player", &rp_path));
                }
                for (i, attr) in attributes.iter().enumerate() {
                    self.validate_statement_recursive(attr, &format!("{}.attributes[{}]", path, i), errors);
                }
            }
            Statement::DeleteThing(variable) => {
                errors.extend(self.validate_variable_name(variable, "Variable", path));
            }
            Statement::Raw(_) => {}
        }
    }

    fn validate_constraint_recursive(&self, constraint: &Constraint, path: &str, errors: &mut Vec<ValidationError>) {
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

    fn validate_value_recursive(&self, value: &Value, path: &str, errors: &mut Vec<ValidationError>) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ast::{Constraint, Value, LiteralValue};
    use serde_json::json;

    #[test]
    fn test_validate_type_name() {
        let engine = ValidationEngine::new();
        
        assert!(engine.validate_type_name("person", "entity").is_valid);
        assert!(engine.validate_type_name("first-name", "attribute").is_valid);
        assert!(engine.validate_type_name("_internal", "role").is_valid);
        
        assert!(!engine.validate_type_name("define", "entity").is_valid); // Reserved
        assert!(!engine.validate_type_name("", "entity").is_valid); // Empty
        assert!(!engine.validate_type_name("1st", "entity").is_valid); // Invalid start
        assert!(!engine.validate_type_name("person!", "entity").is_valid); // Invalid char
    }

    #[test]
    fn test_validate_pattern() {
        let engine = ValidationEngine::new();

        let valid_pattern = Pattern::Entity {
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
        };
        assert!(engine.validate_pattern(&valid_pattern).is_valid);

        let invalid_pattern = Pattern::Entity {
            variable: "p".to_string(), // Missing $
            type_name: "person".to_string(),
            constraints: vec![
                Constraint::Has {
                    attr_name: "define".to_string(), // Reserved word
                    value: Value::Variable("v".to_string()), // Missing $
                }
            ],
            is_strict: false,
        };
        let result = engine.validate_pattern(&invalid_pattern);
        assert!(!result.is_valid);
        assert_eq!(result.errors.len(), 3);
    }
}


        