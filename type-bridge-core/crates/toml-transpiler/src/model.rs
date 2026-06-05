//! Typed serde intermediate model for the TOML schema DSL.
//!
//! Attribute, entity-owns, relation/role, and entity-plays fields are present.
//! Sub, abstract, annotation, function, and struct fields are deliberately
//! omitted until a concrete emitter needs them — the model grows with the
//! feature surface, not ahead of it.

use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level TOML schema document.
///
/// Document order is preserved via [`IndexMap`] for `attributes`, `entities`,
/// and `relations`, so the emitted TypeQL declaration order matches the TOML
/// source order byte-for-byte.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlSchema {
    /// Attribute type declarations, keyed by attribute name.
    #[serde(default)]
    pub attributes: IndexMap<String, TomlAttribute>,
    /// Entity type declarations, keyed by entity name.
    #[serde(default)]
    pub entities: IndexMap<String, TomlEntity>,
    /// Relation type declarations, keyed by relation name.
    #[serde(default)]
    pub relations: IndexMap<String, TomlRelation>,
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
/// `entity <name>;` with no `owns` clauses.  `plays` lists the roles this
/// entity plays, in declaration order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlEntity {
    /// Attribute names this entity owns, in declaration order.
    #[serde(default)]
    pub owns: Vec<String>,
    /// Roles this entity plays, in declaration order.
    #[serde(default)]
    pub plays: Vec<TomlPlays>,
}

/// A single `plays` entry on an entity: `{ relation, role }`.
///
/// Emitted as `plays <relation>:<role>`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlPlays {
    /// The relation type name.
    pub relation: String,
    /// The role name within that relation.
    pub role: String,
}

/// A single relation type declaration.
///
/// `roles` is an ordered array of role descriptors; `owns` reuses the entity
/// owns convention (ordered string array, default empty).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlRelation {
    /// Roles this relation relates, in declaration order.
    #[serde(default)]
    pub roles: Vec<TomlRole>,
    /// Attribute names this relation owns, in declaration order.
    #[serde(default)]
    pub owns: Vec<String>,
}

/// A single role descriptor inside a `[relations.NAME]` table.
///
/// - `name` is the role name.
/// - `card` is an optional verbatim cardinality string (`"1..3"`, `"2.."`, …)
///   passed through as-is into `@card(...)`.
/// - `overrides` (TOML key `as`) is the optional parent-role override target;
///   emitted as `relates <name> as <overrides>`.  The field is renamed in
///   TOML because `as` is a Rust keyword.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlRole {
    /// Role name.
    pub name: String,
    /// Optional cardinality: verbatim `m..n` or `m..` string.
    #[serde(default)]
    pub card: Option<String>,
    /// Optional parent-role override (TOML key `as`).
    #[serde(default, rename = "as")]
    pub overrides: Option<String>,
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

    /// An unknown key inside a role inline table (e.g. `cart` instead of
    /// `card`) must produce a deserialisation error — `TomlRole` carries
    /// `#[serde(deny_unknown_fields)]`.
    #[test]
    fn test_malformed_toml_unknown_role_key() {
        let toml_text = r#"
[relations.review]
roles = [{ name = "reviewer", cart = "1..3" }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown role key `cart`, got Ok"
        );
    }
}
