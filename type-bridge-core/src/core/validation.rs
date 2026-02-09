use serde::{Deserialize, Serialize};
use crate::core::ast::{Pattern, Statement};
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

        

            pub fn validate_pattern(&self, _pattern: &Pattern) -> ValidationResult {

                // Placeholder for recursive pattern validation

                ValidationResult {

                    is_valid: true,

                    errors: Vec::new(),

                }

            }

        

            pub fn validate_statement(&self, _statement: &Statement) -> ValidationResult {

                // Placeholder for recursive statement validation

                ValidationResult {

                    is_valid: true,

                    errors: Vec::new(),

                }

            }

        }

        

        #[cfg(test)]

        mod tests {

            use super::*;

        

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

        }

        