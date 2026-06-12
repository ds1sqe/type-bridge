//! Typed serde intermediate model for the TOML schema DSL.
//!
//! Attribute, entity-owns, relation/role, entity/relation-plays, sub/abstract,
//! annotation, function, and struct fields are all present.

use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level TOML schema document.
///
/// Document order is preserved via [`IndexMap`] for all sections, so the
/// emitted TypeQL declaration order matches the TOML source order byte-for-byte.
/// Sections emit in fixed order: attributes, entities, relations, functions,
/// structs.
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
    /// Function declarations, keyed by function name.
    ///
    /// Each entry carries a `signature` (without trailing `:`) and a `body`
    /// string emitted verbatim.  The emitter appends the `:` between them.
    #[serde(default)]
    pub functions: IndexMap<String, TomlFunction>,
    /// Struct type declarations, keyed by struct name.
    #[serde(default)]
    pub structs: IndexMap<String, TomlStruct>,
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

/// A single `plays` entry on an entity or relation: `{ relation, role, card? }`.
///
/// Emitted as `plays <relation>:<role>` with optional `@card(...)`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlPlays {
    /// The relation type name.
    pub relation: String,
    /// The role name within that relation.
    pub role: String,
    /// Optional cardinality: verbatim `m..n` or `m..` string.
    #[serde(default)]
    pub card: Option<String>,
}

/// A single relation type declaration.
///
/// `roles` is an ordered array of role descriptors; `owns` reuses the entity
/// owns convention (ordered array, default empty); `plays` lists the roles this
/// relation plays, in declaration order.
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
    /// Roles this relation plays, in declaration order.
    #[serde(default)]
    pub plays: Vec<TomlPlays>,
}

/// A single role descriptor inside a `[relations.NAME]` table.
///
/// - `name` is the role name.
/// - `card` is an optional verbatim cardinality string (`"1..3"`, `"2.."`, …)
///   passed through as-is into `@card(...)`.
/// - `overrides` (TOML key `as`) is the optional parent-role override target;
///   emitted as `relates <name> as <overrides>`.  The field is renamed in
///   TOML because `as` is a Rust keyword.
/// - `abstract` marks the role abstract at the TypeDB schema level, emitted as
///   `@abstract` on the `relates` clause.  The field is renamed via serde
///   because `abstract` is a reserved word in Rust.
/// - `ordered` declares this as a list role (`relates name[]`).  Schema-only;
///   instance-level list writes are not yet supported by the engine.
/// - `distinct` emits `@distinct` on the relates clause; requires `ordered`.
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
    /// Whether this role is abstract (TOML key `abstract`).
    #[serde(default, rename = "abstract")]
    pub is_abstract: bool,
    /// Whether this role is a list role (`relates name[]`).
    #[serde(default)]
    pub ordered: bool,
    /// Whether to emit `@distinct`; requires `ordered = true`.
    #[serde(default)]
    pub distinct: bool,
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

/// Annotated owns entry: `{ attribute = "...", key?, unique?, ordered?, distinct?, card? }`.
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
    /// Declare this as a list attribute (`owns attr[]`).  Schema-only.
    #[serde(default)]
    pub ordered: bool,
    /// Emit `@distinct` on this owns clause; requires `ordered = true`.
    #[serde(default)]
    pub distinct: bool,
    /// Optional verbatim cardinality string (`"m..n"` / `"m.."`) → `@card(...)`.
    #[serde(default)]
    pub card: Option<String>,
}

/// A single function declaration.
///
/// `signature` is the function head WITHOUT the trailing `:`, e.g.
/// `fun f($x: user) -> { book }`.  The emitter appends `:\n` before
/// the body so that the TOML author never needs to supply the colon.
///
/// `body` is the `match …; return …;` block, emitted verbatim (no parsing
/// by the transpiler — the downstream `parse_tql_schema` validates it).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlFunction {
    /// Function head without trailing colon, e.g. `fun f($x: t) -> { r }`.
    pub signature: String,
    /// Raw function body including `match … ; return … ;`, emitted verbatim.
    pub body: String,
}

/// A single struct type declaration.
///
/// Fields are emitted in declaration order as
/// `struct <name>, value <f> <t>[?], ...;`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlStruct {
    /// Ordered list of struct fields.
    pub fields: Vec<TomlStructField>,
}

/// A single field inside a [`TomlStruct`].
///
/// The TOML key for the value type is `type` (a Rust keyword), so the field
/// is renamed via `#[serde(rename = "type")]`.  `optional` defaults to
/// `false`; when `true` the emitter appends `?` to the type name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlStructField {
    /// Field name, e.g. `first-name`.
    pub name: String,
    /// Value type name, e.g. `string`.  TOML key is `type`.
    #[serde(rename = "type")]
    pub value_type: String,
    /// Whether the field is optional; emitted as `<type>?` when `true`.
    #[serde(default)]
    pub optional: bool,
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

    /// An unknown key inside a plays inline table must produce a
    /// deserialisation error — `TomlPlays` carries `#[serde(deny_unknown_fields)]`.
    #[test]
    fn test_malformed_toml_unknown_plays_key() {
        let toml_text = r#"
[entities.person]
plays = [{ relation = "review", role = "reviewer", cart = "0..1" }]

[relations.review]
roles = [{ name = "reviewer" }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown plays key `cart`, got Ok"
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

    /// An entity plays entry can carry an optional cardinality string.
    #[test]
    fn test_entity_plays_card_parses() {
        let toml_text = r#"
[entities.person]
plays = [{ relation = "review", role = "reviewer", card = "0..1" }]

[relations.review]
roles = [{ name = "reviewer" }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok, got Err: {:?}", result);
        let schema = result.unwrap();
        let plays = &schema.entities["person"].plays[0];
        assert_eq!(plays.relation, "review");
        assert_eq!(plays.role, "reviewer");
        assert_eq!(plays.card.as_deref(), Some("0..1"));
    }

    /// A relation can play roles in other relations, symmetric with entities.
    #[test]
    fn test_relation_plays_parses() {
        let toml_text = r#"
[relations.publication]
plays = [{ relation = "contribution", role = "work" }]

[relations.contribution]
roles = [{ name = "work" }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok, got Err: {:?}", result);
        let schema = result.unwrap();
        let plays = &schema.relations["publication"].plays[0];
        assert_eq!(plays.relation, "contribution");
        assert_eq!(plays.role, "work");
        assert_eq!(plays.card.as_deref(), None);
    }

    // -------------------------------------------------------------------------
    // Function and struct model deserialization
    // -------------------------------------------------------------------------

    /// A well-formed function declaration deserializes correctly.
    #[test]
    fn test_function_parses() {
        let toml_text = r#"
[functions.book-count]
signature = "fun book-count($u: user) -> { integer }"
body = "  match\n    $u isa user;\n  return { 1 };"
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_ok(),
            "expected Ok for function; got Err: {:?}",
            result
        );
        let schema = result.unwrap();
        assert!(schema.functions.contains_key("book-count"));
        let f = &schema.functions["book-count"];
        assert!(f.signature.starts_with("fun book-count"));
        assert!(
            !f.signature.ends_with(':'),
            "signature must not carry a trailing colon"
        );
    }

    /// An unknown key in a `[functions.NAME]` table must produce a deserialization
    /// error — `TomlFunction` carries `#[serde(deny_unknown_fields)]`.
    #[test]
    fn test_malformed_function_unknown_key() {
        let toml_text = r#"
[functions.f]
sig = "fun f($x: t) -> { r }"
body = "  match $x isa t;\n  return { $x };"
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown function key `sig`, got Ok"
        );
    }

    /// An unknown key inside a struct field inline table must produce a
    /// deserialization error — `TomlStructField` carries `#[serde(deny_unknown_fields)]`.
    #[test]
    fn test_malformed_struct_field_unknown_key() {
        let toml_text = r#"
[structs.person-name]
fields = [{ nam = "first", type = "string" }]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_err(),
            "expected Err for unknown struct field key `nam`, got Ok"
        );
    }

    /// A struct with no `optional` keys deserializes with all fields non-optional.
    #[test]
    fn test_struct_non_optional_fields_parse() {
        let toml_text = r#"
[structs.person-name]
fields = [
    { name = "first-name", type = "string" },
    { name = "last-name", type = "string" },
]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok; got Err: {:?}", result);
        let schema = result.unwrap();
        let s = &schema.structs["person-name"];
        assert_eq!(s.fields.len(), 2);
        assert!(
            !s.fields[0].optional,
            "first-name must default to non-optional"
        );
        assert!(
            !s.fields[1].optional,
            "last-name must default to non-optional"
        );
    }

    /// A struct with `optional = true` on one field deserializes correctly.
    #[test]
    fn test_struct_optional_field_parses() {
        let toml_text = r#"
[structs.person-name]
fields = [
    { name = "first-name", type = "string" },
    { name = "middle-name", type = "string", optional = true },
]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(result.is_ok(), "expected Ok; got Err: {:?}", result);
        let schema = result.unwrap();
        let s = &schema.structs["person-name"];
        assert!(!s.fields[0].optional, "first-name must be non-optional");
        assert!(s.fields[1].optional, "middle-name must be optional");
    }

    /// A TOML document with no `[functions]` or `[structs]` sections (the 01/02/03
    /// fixture shape) must deserialize without error — back-compat via
    /// `#[serde(default)]` on both new fields.
    #[test]
    fn test_functions_structs_absent_back_compat() {
        let toml_text = r#"
[attributes.name]
value = "string"

[entities.person]
owns = ["name"]
"#;
        let result: Result<TomlSchema, _> = toml::from_str(toml_text);
        assert!(
            result.is_ok(),
            "expected Ok for schema without functions/structs; got Err: {:?}",
            result
        );
        let schema = result.unwrap();
        assert!(
            schema.functions.is_empty(),
            "functions must default to empty map"
        );
        assert!(
            schema.structs.is_empty(),
            "structs must default to empty map"
        );
    }
}
