//! Name conversion utilities for TypeDB → Rust identifier mapping.
//!
//! TypeDB uses kebab-case for type names (e.g. `person-name`, `employee-id`).
//! Rust uses PascalCase for types and snake_case for fields/variables.

/// Convert a kebab-case TypeDB name to PascalCase for Rust struct names.
///
/// Examples:
/// - `"person"` → `"Person"`
/// - `"person-name"` → `"PersonName"`
/// - `"employment"` → `"Employment"`
/// - `"created-at"` → `"CreatedAt"`
pub fn to_pascal_case(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + chars.as_str()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// Convert a kebab-case TypeDB name to snake_case for Rust field names.
///
/// Examples:
/// - `"person"` → `"person"`
/// - `"person-name"` → `"person_name"`
/// - `"created-at"` → `"created_at"`
pub fn to_snake_case(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_single_word() {
        assert_eq!(to_pascal_case("person"), "Person");
    }

    #[test]
    fn pascal_case_multi_word() {
        assert_eq!(to_pascal_case("person-name"), "PersonName");
    }

    #[test]
    fn pascal_case_three_words() {
        assert_eq!(to_pascal_case("first-middle-last"), "FirstMiddleLast");
    }

    #[test]
    fn snake_case_single_word() {
        assert_eq!(to_snake_case("person"), "person");
    }

    #[test]
    fn snake_case_multi_word() {
        assert_eq!(to_snake_case("person-name"), "person_name");
    }

    #[test]
    fn snake_case_three_words() {
        assert_eq!(to_snake_case("created-at-time"), "created_at_time");
    }

    #[test]
    fn pascal_case_empty() {
        assert_eq!(to_pascal_case(""), "");
    }

    #[test]
    fn snake_case_no_hyphens() {
        assert_eq!(to_snake_case("name"), "name");
    }
}
