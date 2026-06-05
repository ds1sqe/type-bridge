//! Typed serde intermediate model for the TOML schema DSL.
//!
//! Only attribute and entity-owns fields are present. Relation, role, function,
//! struct, annotation, and sub fields are deliberately omitted until a concrete
//! emitter needs them — the model grows with the feature surface, not ahead of it.

use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level TOML schema document.
///
/// Document order is preserved via [`IndexMap`] for both `attributes` and
/// `entities`, so the emitted TypeQL declaration order matches the TOML source
/// order byte-for-byte.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlSchema {
    /// Attribute type declarations, keyed by attribute name.
    #[serde(default)]
    pub attributes: IndexMap<String, TomlAttribute>,
    /// Entity type declarations, keyed by entity name.
    #[serde(default)]
    pub entities: IndexMap<String, TomlEntity>,
}

/// A single attribute type declaration.
///
/// `value` is the TypeDB value type string (e.g. `"string"`, `"long"`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlAttribute {
    /// TypeDB value type, e.g. `"string"` or `"long"`.
    pub value: String,
}

/// A single entity type declaration.
///
/// `owns` preserves TOML declaration order; an empty list is valid and emits
/// `entity <name>;` with no `owns` clauses.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlEntity {
    /// Attribute names this entity owns, in declaration order.
    #[serde(default)]
    pub owns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unknown key in `[attributes.name]` must produce a deserialisation
    /// error because `#[serde(deny_unknown_fields)]` is active.
    #[test]
    fn test_malformed_toml_unknown_key() {
        let toml_text = r#"
[attributes.name]
valeu = "string"
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown key `valeu`, got Ok"
        );
    }

    /// A missing required field (`value`) in `[attributes.name]` must produce
    /// a deserialisation error.
    #[test]
    fn test_malformed_toml_missing_field() {
        let toml_text = r#"
[attributes.name]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for missing required field `value`, got Ok"
        );
    }
}
