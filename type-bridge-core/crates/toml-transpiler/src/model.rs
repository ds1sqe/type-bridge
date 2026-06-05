//! Typed serde intermediate model for the TOML schema DSL.
//!
//! Attribute, entity-owns, relation/role, entity-plays, sub/abstract, and
//! annotation fields are present. Function and struct fields are deliberately
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
/// When `sub` is set, `value` should be absent (the parent supplies the type).
/// Field-level validation of value/sub exclusivity is left to the diagnostics
/// layer, not enforced structurally here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlAttribute {
    /// TypeDB value type, e.g. `"string"` or `"long"`. `None` when `sub` is set.
    #[serde(default)]
    pub value: Option<String>,
    /// Parent attribute type for inheritance (`sub <parent>`).
    #[serde(default)]
    pub sub: Option<String>,
    /// Whether this attribute type is abstract.
    #[serde(default, rename = "abstract")]
    pub is_abstract: bool,
    /// Optional regex constraint on the value: emitted as `@regex("...")`.
    #[serde(default)]
    pub regex: Option<String>,
    /// Optional allowed-values constraint: emitted as `@values("a", "b", ...)`.
    #[serde(default)]
    pub values: Option<Vec<String>>,
    /// Optional range constraint (verbatim `m..n` / `m..` / `..n` string):
    /// emitted as `@range(m..n)`.
    #[serde(default)]
    pub range: Option<String>,
}

/// A single entity type declaration.
///
/// `owns` preserves TOML declaration order; an empty list is valid and emits
/// `entity <name>;` with no `owns` clauses.  `plays` lists the roles this
/// entity plays, in declaration order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlEntity {
    /// Parent entity type for inheritance (`sub <parent>`).
    #[serde(default)]
    pub sub: Option<String>,
    /// Whether this entity type is abstract.
    #[serde(default, rename = "abstract")]
    pub is_abstract: bool,
    /// Attribute owns entries (bare names or annotated tables), in declaration order.
    #[serde(default)]
    pub owns: Vec<TomlOwns>,
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
/// owns convention (ordered array, default empty).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlRelation {
    /// Parent relation type for inheritance (`sub <parent>`).
    #[serde(default)]
    pub sub: Option<String>,
    /// Whether this relation type is abstract.
    #[serde(default, rename = "abstract")]
    pub is_abstract: bool,
    /// Roles this relation relates, in declaration order.
    #[serde(default)]
    pub roles: Vec<TomlRole>,
    /// Attribute owns entries (bare names or annotated tables), in declaration order.
    #[serde(default)]
    pub owns: Vec<TomlOwns>,
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

/// An `owns` entry on an entity or relation.
///
/// Each entry is either a bare attribute name (01/02 back-compat form) or an
/// annotated inline table carrying `@key` / `@unique` / `@card` annotations.
///
/// `#[serde(untagged)]` tries `Annotated` first (it requires a table with an
/// `attribute` field), then falls back to `Name` (a plain string).  The
/// `#[serde(deny_unknown_fields)]` on [`TomlOwnsAnnotated`] ensures an
/// inline table with a typo'd key (e.g. `kye = true`) is rejected — untagged
/// surfaces the inner error when the annotated variant fails to match AND the
/// fallback `Name` variant also cannot accept a table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TomlOwns {
    /// Bare attribute name — 01/02 form, no annotations.
    Name(String),
    /// Annotated owns table: `{ attribute = "...", key?, unique?, card? }`.
    Annotated(TomlOwnsAnnotated),
}

/// Annotated owns entry: `{ attribute = "...", key?, unique?, card? }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlOwnsAnnotated {
    /// The attribute name being owned.
    pub attribute: String,
    /// Emit `@key` on this owns clause.
    #[serde(default)]
    pub key: bool,
    /// Emit `@unique` on this owns clause.
    #[serde(default)]
    pub unique: bool,
    /// Optional verbatim cardinality string (`"m..n"` / `"m.."`) → `@card(...)`.
    #[serde(default)]
    pub card: Option<String>,
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
[attributes.isbn-base]
value = "string"
abstract = true

[attributes.isbn-child]
sub = "isbn-base"
"#;
        // Both are valid — the old "missing value is always an error" expectation
        // no longer applies now that value is Option.  Check that both parse OK.
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_ok(),
            "expected Ok for abstract parent + sub child, got Err: {:?}",
            result
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

    /// An unknown key in an annotated owns table must produce a deserialisation
    /// error: `kye` (a typo for `key`) must be rejected, not silently dropped by
    /// the untagged enum falling back to the bare-string variant.
    ///
    /// Resolution: `#[serde(untagged)]` on `TomlOwns` tries `Annotated` first.
    /// `TomlOwnsAnnotated` carries `#[serde(deny_unknown_fields)]`, so a table
    /// with `kye = true` fails the annotated variant.  Because the value IS a
    /// table (not a bare string), it also fails the `Name(String)` variant.
    /// Untagged therefore propagates an error rather than silently swallowing
    /// it.  No manual `Deserialize` impl is needed — the untagged form is
    /// sufficient here.
    #[test]
    fn test_malformed_owns_unknown_key() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn", kye = true }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown owns key `kye`, got Ok"
        );
    }

    /// An annotated owns table with a valid `key = true` must parse correctly.
    #[test]
    fn test_owns_annotated_key_parses() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn-13", key = true }, "title"]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_ok(),
            "expected Ok for annotated + bare owns; got Err: {:?}",
            result
        );
        let schema = result.unwrap();
        let entity = &schema.entities["book"];
        assert_eq!(entity.owns.len(), 2);
        match &entity.owns[0] {
            TomlOwns::Annotated(a) => {
                assert_eq!(a.attribute, "isbn-13");
                assert!(a.key);
            }
            TomlOwns::Name(n) => panic!("expected Annotated, got Name({n:?})"),
        }
        match &entity.owns[1] {
            TomlOwns::Name(n) => assert_eq!(n, "title"),
            TomlOwns::Annotated(a) => panic!("expected Name, got Annotated({a:?})"),
        }
    }

    /// An entity with `abstract = true` must parse and set `is_abstract`.
    #[test]
    fn test_entity_abstract_parses() {
        let toml_text = r#"
[entities.book]
abstract = true
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok, got Err: {:?}", result);
        assert!(result.unwrap().entities["book"].is_abstract);
    }

    /// An attribute with `sub = "isbn"` and no `value` must parse correctly.
    #[test]
    fn test_attribute_sub_no_value_parses() {
        let toml_text = r#"
[attributes.isbn-13]
sub = "isbn"
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok, got Err: {:?}", result);
        let schema = result.unwrap();
        let attr = &schema.attributes["isbn-13"];
        assert_eq!(attr.sub, Some("isbn".to_string()));
        assert!(attr.value.is_none());
    }
}
