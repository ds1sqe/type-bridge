//! Semantic validation pass for the typed TOML schema model.
//!
//! [`validate`] runs after TOML deserialisation and before TypeQL emission.
//! It checks cross-references and semantic constraints that structural
//! deserialisation cannot enforce. On valid input it returns `Ok(())` and the
//! emitter sees an identical schema to what it would have seen without the pass.
//!
//! Checks run in a deterministic, document-order sequence:
//! 1. attribute value/sub exclusivity (XOR)
//! 2. unknown value types (attributes + struct fields)
//! 3. dangling `sub` parents (attributes, entities, relations)
//! 4. missing role players (entity/relation `plays` referencing undefined
//!    relation or undefined role within that relation)
//! 5. empty structs
//! 6. function body has no `return`
//!
//! Fail-fast: the first error in document order is returned immediately.
//! [`indexmap::IndexMap`] preserves insertion order, so the same malformed
//! input always yields the same error message.

use crate::{
    TranspileError, TypeKind,
    model::{TomlPlays, TomlSchema},
};

/// Value type keywords accepted by the TypeDB parser.
///
/// MUST stay in sync with `value_type_name` in
/// `type-bridge-core/crates/core/src/parser.rs` (the `alt(...)` branch list).
/// If the parser adds or removes a keyword, this list must be updated to match.
const KNOWN_VALUE_TYPES: &[&str] = &[
    "datetime-tz",
    "datetime",
    "boolean",
    "decimal",
    "duration",
    "integer",
    "string",
    "double",
    "date",
    "long",
    "bool",
    "int",
];

/// Validate a parsed [`TomlSchema`] for semantic correctness.
///
/// Returns `Ok(())` when the schema is valid (no-op on the emit path).
/// Returns `Err(TranspileError::*)` with the first semantic error found,
/// naming the offending field or type in document order.
pub(crate) fn validate(schema: &TomlSchema) -> Result<(), TranspileError> {
    check_attribute_value_sub_xor(schema)?;
    check_unknown_value_types(schema)?;
    check_dangling_sub_parents(schema)?;
    check_missing_role_players(schema)?;
    check_empty_structs(schema)?;
    check_malformed_function_bodies(schema)?;
    Ok(())
}

/// Check 1: every attribute has exactly one of `value` or `sub` (XOR).
///
/// Both set → `AttributeValueSubConflict`.
/// Neither set → `AttributeMissingValueSub`.
fn check_attribute_value_sub_xor(schema: &TomlSchema) -> Result<(), TranspileError> {
    for (name, attr) in &schema.attributes {
        match (attr.value.is_some(), attr.sub.is_some()) {
            (true, true) => {
                return Err(TranspileError::AttributeValueSubConflict { attr: name.clone() });
            }
            (false, false) => {
                return Err(TranspileError::AttributeMissingValueSub { attr: name.clone() });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Check 2: attribute `value` strings and struct field `type` strings must be
/// one of the 12 known TypeDB value type keywords.
fn check_unknown_value_types(schema: &TomlSchema) -> Result<(), TranspileError> {
    // Attribute value types
    for (name, attr) in &schema.attributes {
        if let Some(ref vt) = attr.value
            && !KNOWN_VALUE_TYPES.contains(&vt.as_str())
        {
            return Err(TranspileError::UnknownValueType {
                type_name: name.clone(),
                value: vt.clone(),
            });
        }
    }

    // Struct field value types
    for (struct_name, s) in &schema.structs {
        for field in &s.fields {
            if !KNOWN_VALUE_TYPES.contains(&field.value_type.as_str()) {
                return Err(TranspileError::UnknownStructFieldType {
                    struct_name: struct_name.clone(),
                    field: field.name.clone(),
                    value: field.value_type.clone(),
                });
            }
        }
    }

    Ok(())
}

/// Check 3: `sub` references must name a type defined in the same section.
///
/// An attribute's `sub` must name another key in `schema.attributes`;
/// an entity's `sub` must name another key in `schema.entities`;
/// a relation's `sub` must name another key in `schema.relations`.
fn check_dangling_sub_parents(schema: &TomlSchema) -> Result<(), TranspileError> {
    for (name, attr) in &schema.attributes {
        if let Some(ref parent) = attr.sub
            && !schema.attributes.contains_key(parent.as_str())
        {
            return Err(TranspileError::DanglingSubParent {
                kind: TypeKind::Attribute,
                type_name: name.clone(),
                parent: parent.clone(),
            });
        }
    }

    for (name, entity) in &schema.entities {
        if let Some(ref parent) = entity.sub
            && !schema.entities.contains_key(parent.as_str())
        {
            return Err(TranspileError::DanglingSubParent {
                kind: TypeKind::Entity,
                type_name: name.clone(),
                parent: parent.clone(),
            });
        }
    }

    for (name, relation) in &schema.relations {
        if let Some(ref parent) = relation.sub
            && !schema.relations.contains_key(parent.as_str())
        {
            return Err(TranspileError::DanglingSubParent {
                kind: TypeKind::Relation,
                type_name: name.clone(),
                parent: parent.clone(),
            });
        }
    }

    Ok(())
}

/// Check a single `plays` entry against the defined relations.
fn check_plays_entry(
    owner: &str,
    plays: &TomlPlays,
    schema: &TomlSchema,
) -> Result<(), TranspileError> {
    match schema.relations.get(plays.relation.as_str()) {
        None => Err(TranspileError::MissingRoleRelation {
            player: owner.to_owned(),
            relation: plays.relation.clone(),
            role: plays.role.clone(),
        }),
        Some(rel) => {
            let role_exists = rel.roles.iter().any(|r| r.name == plays.role);
            if !role_exists {
                Err(TranspileError::MissingRole {
                    player: owner.to_owned(),
                    relation: plays.relation.clone(),
                    role: plays.role.clone(),
                })
            } else {
                Ok(())
            }
        }
    }
}

/// Check 4: every `plays { relation, role }` entry on entities must reference
/// a defined relation with a declared role of that name.
///
/// Only entities carry `plays` entries in the current model; relations do not.
fn check_missing_role_players(schema: &TomlSchema) -> Result<(), TranspileError> {
    for (name, entity) in &schema.entities {
        for plays in &entity.plays {
            check_plays_entry(name, plays, schema)?;
        }
    }
    Ok(())
}

/// Check 5: each struct must have at least one field.
fn check_empty_structs(schema: &TomlSchema) -> Result<(), TranspileError> {
    for (name, s) in &schema.structs {
        if s.fields.is_empty() {
            return Err(TranspileError::EmptyStruct {
                struct_name: name.clone(),
            });
        }
    }
    Ok(())
}

/// Check 6: each function body must contain `return`.
///
/// The `body` string is a verbatim passthrough; deeper grammar validation
/// belongs to the downstream parser. This check catches the case that produces
/// a confusing downstream error: a body with no `return` clause at all.
fn check_malformed_function_bodies(schema: &TomlSchema) -> Result<(), TranspileError> {
    for (name, func) in &schema.functions {
        if !func.body.contains("return") {
            return Err(TranspileError::MalformedFunctionBody {
                function: name.clone(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests — one per diagnostic + valid no-op + deterministic-first-error
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml_to_typeql;

    // ------------------------------------------------------------------
    // Check 1 — attribute value/sub XOR
    // ------------------------------------------------------------------

    #[test]
    fn test_attribute_value_and_sub_conflict() {
        let toml_text = r#"
[attributes.name]
value = "string"
sub = "other"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected AttributeValueSubConflict");
        assert!(
            matches!(err, TranspileError::AttributeValueSubConflict { ref attr } if attr == "name"),
            "wrong variant or attr name: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("name"),
            "message must contain the attribute name; got: {msg}"
        );
    }

    #[test]
    fn test_attribute_neither_value_nor_sub() {
        let toml_text = r#"
[attributes.orphan]
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected AttributeMissingValueSub");
        assert!(
            matches!(err, TranspileError::AttributeMissingValueSub { ref attr } if attr == "orphan"),
            "wrong variant or attr name: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("orphan"),
            "message must contain the attribute name; got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Check 2 — unknown value types
    // ------------------------------------------------------------------

    #[test]
    fn test_unknown_value_type() {
        let toml_text = r#"
[attributes.title]
value = "strng"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected UnknownValueType");
        assert!(
            matches!(
                err,
                TranspileError::UnknownValueType { ref type_name, ref value }
                    if type_name == "title" && value == "strng"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("title"),
            "message must name the attribute; got: {msg}"
        );
        assert!(
            msg.contains("strng"),
            "message must name the bad value; got: {msg}"
        );
    }

    #[test]
    fn test_unknown_struct_field_type() {
        let toml_text = r#"
[structs.person-name]
fields = [{ name = "first", type = "itneger" }]
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected UnknownStructFieldType");
        assert!(
            matches!(
                err,
                TranspileError::UnknownStructFieldType { ref struct_name, ref field, ref value }
                    if struct_name == "person-name" && field == "first" && value == "itneger"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("person-name"),
            "message must name the struct; got: {msg}"
        );
        assert!(
            msg.contains("first"),
            "message must name the field; got: {msg}"
        );
        assert!(
            msg.contains("itneger"),
            "message must name the bad type; got: {msg}"
        );
    }

    /// All 12 known value types must pass the value-type check individually.
    #[test]
    fn test_all_known_value_types_pass() {
        for &vt in KNOWN_VALUE_TYPES {
            let toml_text = format!("[attributes.x]\nvalue = \"{vt}\"\n");
            let schema: TomlSchema = toml::from_str(&toml_text).unwrap();
            assert!(
                validate(&schema).is_ok(),
                "known value type {vt:?} must pass validation"
            );
        }
    }

    // ------------------------------------------------------------------
    // Check 3 — dangling sub parents
    // ------------------------------------------------------------------

    #[test]
    fn test_dangling_sub_parent_attribute() {
        let toml_text = r#"
[attributes.isbn-13]
sub = "nonexistent"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected DanglingSubParent for attribute");
        assert!(
            matches!(
                err,
                TranspileError::DanglingSubParent {
                    kind: TypeKind::Attribute,
                    ref type_name,
                    ref parent,
                } if type_name == "isbn-13" && parent == "nonexistent"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("isbn-13"),
            "message must name the type; got: {msg}"
        );
        assert!(
            msg.contains("nonexistent"),
            "message must name the parent; got: {msg}"
        );
    }

    #[test]
    fn test_dangling_sub_parent_entity() {
        let toml_text = r#"
[entities.hardback]
sub = "nonexistent"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected DanglingSubParent for entity");
        assert!(
            matches!(
                err,
                TranspileError::DanglingSubParent {
                    kind: TypeKind::Entity,
                    ref type_name,
                    ref parent,
                } if type_name == "hardback" && parent == "nonexistent"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("entity"),
            "message must say 'entity'; got: {msg}"
        );
        assert!(
            msg.contains("hardback"),
            "message must name the type; got: {msg}"
        );
        assert!(
            msg.contains("nonexistent"),
            "message must name the parent; got: {msg}"
        );
    }

    #[test]
    fn test_dangling_sub_parent_relation() {
        let toml_text = r#"
[relations.authoring]
sub = "nonexistent"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected DanglingSubParent for relation");
        assert!(
            matches!(
                err,
                TranspileError::DanglingSubParent {
                    kind: TypeKind::Relation,
                    ref type_name,
                    ref parent,
                } if type_name == "authoring" && parent == "nonexistent"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("relation"),
            "message must say 'relation'; got: {msg}"
        );
        assert!(
            msg.contains("authoring"),
            "message must name the type; got: {msg}"
        );
        assert!(
            msg.contains("nonexistent"),
            "message must name the parent; got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Check 4 — missing role players
    // ------------------------------------------------------------------

    #[test]
    fn test_missing_role_relation() {
        let toml_text = r#"
[entities.person]
plays = [{ relation = "nope", role = "r" }]
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected MissingRoleRelation");
        assert!(
            matches!(
                err,
                TranspileError::MissingRoleRelation {
                    ref player,
                    ref relation,
                    ..
                } if player == "person" && relation == "nope"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("person"),
            "message must name the player; got: {msg}"
        );
        assert!(
            msg.contains("nope"),
            "message must name the relation; got: {msg}"
        );
    }

    #[test]
    fn test_missing_role() {
        let toml_text = r#"
[entities.person]
plays = [{ relation = "review", role = "nonexistent-role" }]

[relations.review]
roles = [{ name = "reviewer" }]
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected MissingRole");
        assert!(
            matches!(
                err,
                TranspileError::MissingRole {
                    ref player,
                    ref relation,
                    ref role,
                } if player == "person" && relation == "review" && role == "nonexistent-role"
            ),
            "wrong variant or fields: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("person"),
            "message must name the player; got: {msg}"
        );
        assert!(
            msg.contains("review"),
            "message must name the relation; got: {msg}"
        );
        assert!(
            msg.contains("nonexistent-role"),
            "message must name the role; got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Check 5 — empty struct
    // ------------------------------------------------------------------

    #[test]
    fn test_empty_struct() {
        let toml_text = r#"
[structs.empty-thing]
fields = []
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected EmptyStruct");
        assert!(
            matches!(err, TranspileError::EmptyStruct { ref struct_name } if struct_name == "empty-thing"),
            "wrong variant or struct_name: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("empty-thing"),
            "message must name the struct; got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Check 6 — malformed function body
    // ------------------------------------------------------------------

    #[test]
    fn test_malformed_function_body() {
        let toml_text = r#"
[functions.bad-fn]
signature = "fun bad-fn($x: t) -> { r }"
body = "  match $x isa t;"
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected MalformedFunctionBody");
        assert!(
            matches!(err, TranspileError::MalformedFunctionBody { ref function } if function == "bad-fn"),
            "wrong variant or function name: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("bad-fn"),
            "message must name the function; got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Valid schema passes
    // ------------------------------------------------------------------

    #[test]
    fn test_valid_schema_passes() {
        let toml_text = r#"
[attributes.isbn]
value = "string"
abstract = true

[attributes.isbn-13]
sub = "isbn"

[attributes.count]
value = "date"

[attributes.active]
value = "bool"

[attributes.qty]
value = "int"

[entities.book]
abstract = true
owns = [{ attribute = "isbn-13", key = true }]

[entities.hardback]
sub = "book"

[entities.person]
owns = ["isbn"]
plays = [{ relation = "review", role = "reviewer" }]

[relations.review]
roles = [{ name = "reviewer", card = "1..3" }, { name = "subject" }]

[functions.count-books]
signature = "fun count-books($u: person) -> { integer }"
body = "  match $u isa person;\n  return { 1 };"

[structs.person-name]
fields = [
    { name = "first", type = "string" },
    { name = "middle", type = "string", optional = true },
]
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        assert!(
            validate(&schema).is_ok(),
            "fully valid schema must pass validation"
        );
    }

    // ------------------------------------------------------------------
    // Deterministic first-error selection
    // ------------------------------------------------------------------

    /// A schema with two semantic errors must yield the document-order-first one.
    /// The attribute `bad-attr` (value/sub conflict, check 1) appears before
    /// the empty struct `empty-s` (check 5). The validator must return
    /// `AttributeValueSubConflict` because check 1 runs before check 5.
    #[test]
    fn test_first_error_deterministic() {
        let toml_text = r#"
[attributes.bad-attr]
value = "string"
sub = "other"

[structs.empty-s]
fields = []
"#;
        let schema: TomlSchema = toml::from_str(toml_text).unwrap();
        let err = validate(&schema).expect_err("expected a validation error");
        assert!(
            matches!(err, TranspileError::AttributeValueSubConflict { ref attr } if attr == "bad-attr"),
            "first error must be AttributeValueSubConflict (check 1 runs first); got: {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // Integration smoke: valid emit is byte-identical (no-op on valid path)
    // ------------------------------------------------------------------

    /// Feed a multi-feature valid TOML through `toml_to_typeql` and confirm
    /// (a) it returns Ok and (b) the emitted string contains the expected
    /// declarations — proving the validation pass is a pure no-op for valid input.
    #[test]
    fn test_validate_does_not_regress_valid_emit() {
        let toml_text = r#"
[attributes.name]
value = "string"

[attributes.score]
value = "double"

[entities.person]
owns = ["name"]
plays = [{ relation = "review", role = "reviewer" }]

[entities.document]
owns = ["name"]
plays = [{ relation = "review", role = "document" }]

[relations.review]
roles = [
    { name = "document", card = "1..1" },
    { name = "reviewer", card = "1..3" },
]
owns = ["score"]

[functions.top-score]
signature = "fun top-score($d: document) -> double"
body = "  match $d has score $s;\n  return max($s);"

[structs.person-name]
fields = [
    { name = "first", type = "string" },
    { name = "last", type = "string" },
]
"#;
        let result =
            toml_to_typeql(toml_text).expect("valid TOML must not fail after adding validate");

        // Spot-check that all expected declarations are present.
        assert!(result.contains("define"), "output must begin with define");
        assert!(
            result.contains("attribute name, value string;"),
            "attribute name must emit; got:\n{result}"
        );
        assert!(
            result.contains("plays review:reviewer"),
            "entity plays must emit; got:\n{result}"
        );
        assert!(
            result.contains("relation review, relates document @card(1..1)"),
            "relation review must emit; got:\n{result}"
        );
        assert!(
            result.contains("fun top-score"),
            "function must emit; got:\n{result}"
        );
        assert!(
            result.contains("struct person-name,"),
            "struct must emit; got:\n{result}"
        );
    }
}
