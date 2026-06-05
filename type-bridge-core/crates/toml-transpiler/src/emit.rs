//! TypeQL define-block emitter.
//!
//! [`emit`] is a total function over the typed model — it never inspects raw
//! `toml::Value` tables and never calls `.unwrap()` on optional fields.

use crate::model::{TomlOwns, TomlSchema};

/// Normalise a `TomlOwns` entry into `(name, key, unique, card)`.
fn owns_parts(entry: &TomlOwns) -> (&str, bool, bool, Option<&str>) {
    match entry {
        TomlOwns::Name(n) => (n.as_str(), false, false, None),
        TomlOwns::Annotated(a) => (a.attribute.as_str(), a.key, a.unique, a.card.as_deref()),
    }
}

/// Emit a canonical TypeQL `define` block from a typed schema model.
///
/// Output format:
/// ```text
/// define
/// attribute <name>[sub <parent>], value <type>[@ regex(...)];
///   -- OR --
/// attribute <name> @abstract, value <type>;
///   -- OR --
/// attribute <name> sub <parent>;            (no value clause)
/// ...
/// entity <name>[@abstract | sub <parent>][, owns <a>[@key|@unique|@card(m..n)], plays <r>:<role>];
/// ...
/// relation <name>[@abstract | sub <parent>][, relates <r>[@ card(m..n)], owns <a>[@key|...]];
/// ...
/// ```
///
/// Declaration order within each section matches TOML document order (preserved
/// by [`indexmap::IndexMap`]).  Sections appear in the fixed order: attributes,
/// entities, relations.
pub fn emit(schema: &TomlSchema) -> String {
    let mut out = String::from("define\n");

    // --- attributes ---
    for (name, attr) in &schema.attributes {
        // Build clause list (comma-separated items after the type+sub+abstract head).
        // The head itself is: `attribute <name>[sub <parent>] [@abstract]`
        // Then value + value-annotations follow as additional clauses.

        let mut head = format!("attribute {}", name);

        if let Some(ref parent) = attr.sub {
            // `sub` replaces the `value` clause; head becomes `attribute X sub Y`
            head.push_str(&format!(" sub {}", parent));
            // No value clause — emit bare declaration.
            out.push_str(&format!("{};\n", head));
            continue;
        }

        if attr.is_abstract {
            // `@abstract` goes before `value`: `attribute X @abstract, value T;`
            head.push_str(" @abstract");
        }

        // Build the value clause (with optional value-level annotations).
        let value_clause = if let Some(ref vt) = attr.value {
            let mut clause = format!("value {}", vt);
            if let Some(ref rx) = attr.regex {
                clause.push_str(&format!(r#" @regex("{}")"#, rx));
            }
            if let Some(ref vals) = attr.values {
                let quoted: Vec<String> = vals.iter().map(|v| format!("\"{}\"", v)).collect();
                clause.push_str(&format!(" @values({})", quoted.join(", ")));
            }
            if let Some(ref rng) = attr.range {
                clause.push_str(&format!(" @range({})", rng));
            }
            Some(clause)
        } else {
            None
        };

        match value_clause {
            Some(vc) => {
                // `attribute X [@abstract], value T[@annotation...];`
                out.push_str(&format!("{}, {};\n", head, vc));
            }
            None => {
                // No value and no sub — emit bare `attribute X;` (best-effort; 05 adds diag)
                out.push_str(&format!("{};\n", head));
            }
        }
    }

    // --- entities ---
    for (name, entity) in &schema.entities {
        // Build the type-head: `entity <name>` + optional sub/abstract token.
        let mut head = format!("entity {}", name);
        if let Some(ref parent) = entity.sub {
            head.push_str(&format!(" sub {}", parent));
        } else if entity.is_abstract {
            head.push_str(" @abstract");
        }

        let mut clauses: Vec<String> = Vec::new();
        for entry in &entity.owns {
            let (attr_name, key, unique, card) = owns_parts(entry);
            let mut clause = format!("owns {}", attr_name);
            if key {
                clause.push_str(" @key");
            }
            if unique {
                clause.push_str(" @unique");
            }
            if let Some(c) = card {
                clause.push_str(&format!(" @card({})", c));
            }
            clauses.push(clause);
        }
        for p in &entity.plays {
            clauses.push(format!("plays {}:{}", p.relation, p.role));
        }

        if clauses.is_empty() {
            out.push_str(&format!("{};\n", head));
        } else {
            out.push_str(&format!("{}, {};\n", head, clauses.join(", ")));
        }
    }

    // --- relations ---
    for (name, relation) in &schema.relations {
        // Build the type-head: `relation <name>` + optional sub/abstract token.
        let mut head = format!("relation {}", name);
        if let Some(ref parent) = relation.sub {
            head.push_str(&format!(" sub {}", parent));
        } else if relation.is_abstract {
            head.push_str(" @abstract");
        }

        let mut clauses: Vec<String> = Vec::new();
        for role in &relation.roles {
            let mut clause = format!("relates {}", role.name);
            if let Some(ref ov) = role.overrides {
                clause.push_str(&format!(" as {}", ov));
            }
            if let Some(ref card) = role.card {
                clause.push_str(&format!(" @card({})", card));
            }
            clauses.push(clause);
        }
        for entry in &relation.owns {
            let (attr_name, key, unique, card) = owns_parts(entry);
            let mut clause = format!("owns {}", attr_name);
            if key {
                clause.push_str(" @key");
            }
            if unique {
                clause.push_str(" @unique");
            }
            if let Some(c) = card {
                clause.push_str(&format!(" @card({})", c));
            }
            clauses.push(clause);
        }

        if clauses.is_empty() {
            out.push_str(&format!("{};\n", head));
        } else {
            out.push_str(&format!("{}, {};\n", head, clauses.join(", ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use crate::toml_to_typeql;

    // -------------------------------------------------------------------------
    // 01/02 back-compat regression guard
    // -------------------------------------------------------------------------

    /// Feed the canonical attribute+entity slice TOML through `toml_to_typeql`
    /// and verify the emitted TypeQL contains all expected declarations in order.
    #[test]
    fn test_emit_smoke() {
        let toml_text = r#"
[attributes.name]
value = "string"

[attributes.age]
value = "long"

[entities.person]
owns = ["name", "age"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(result.contains("define"), "output must contain `define`");
        assert!(
            result.contains("attribute name, value string;"),
            "output must contain attribute name declaration; got:\n{result}"
        );
        assert!(
            result.contains("attribute age, value long;"),
            "output must contain attribute age declaration; got:\n{result}"
        );
        assert!(
            result.contains("entity person, owns name, owns age;"),
            "output must contain entity person declaration; got:\n{result}"
        );

        // Verify owns order: "name" must appear before "age" in the output.
        let name_pos = result
            .find("owns name")
            .expect("owns name not found in output");
        let age_pos = result
            .find("owns age")
            .expect("owns age not found in output");
        assert!(
            name_pos < age_pos,
            "`owns name` must appear before `owns age`; got:\n{result}"
        );
    }

    /// A relation with an empty `roles` array emits `relation N;`.
    #[test]
    fn test_emit_empty_roles() {
        let toml_text = r#"
[relations.empty_rel]
roles = []
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relation empty_rel;"),
            "expected `relation empty_rel;`; got:\n{result}"
        );
    }

    /// A role with no `card` or `as` emits a bare `relates <name>` (no
    /// `@card(...)`, no `as ...`).
    #[test]
    fn test_emit_bare_relates() {
        let toml_text = r#"
[relations.bare_rel]
roles = [{ name = "participant" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relates participant"),
            "expected bare `relates participant`; got:\n{result}"
        );
        assert!(
            !result.contains("@card"),
            "bare role must not emit @card; got:\n{result}"
        );
        assert!(
            !result.contains(" as "),
            "bare role must not emit `as`; got:\n{result}"
        );
    }

    /// A role with an `as` override emits `relates <name> as <target>`.
    #[test]
    fn test_emit_role_as_override() {
        let toml_text = r#"
[relations.authoring]
roles = [{ name = "author", as = "contributor" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        assert!(
            result.contains("relates author as contributor"),
            "expected `relates author as contributor`; got:\n{result}"
        );
    }

    /// Two roles declared in a specific array order must appear in that same
    /// order in the emitted TypeQL.
    #[test]
    fn test_emit_roles_order_preserved() {
        let toml_text = r#"
[relations.ordered_rel]
roles = [
    { name = "first" },
    { name = "second" },
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");
        let first_pos = result
            .find("relates first")
            .expect("`relates first` not found");
        let second_pos = result
            .find("relates second")
            .expect("`relates second` not found");
        assert!(
            first_pos < second_pos,
            "`relates first` must appear before `relates second`; got:\n{result}"
        );
    }

    /// Integration smoke: a full relation+plays TOML feeds through `toml_to_typeql`
    /// and the output contains the expected relation and plays declarations.
    #[test]
    fn test_emit_relation_and_plays_smoke() {
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
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed on valid input");

        assert!(
            result.contains("relation review, relates document @card(1..1), relates reviewer @card(1..3), owns score;"),
            "expected full review relation declaration; got:\n{result}"
        );
        assert!(
            result.contains("plays review:reviewer"),
            "expected `plays review:reviewer` on person; got:\n{result}"
        );
        assert!(
            result.contains("plays review:document"),
            "expected `plays review:document` on document entity; got:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // attribute abstract / sub / value-annotation emission
    // -------------------------------------------------------------------------

    /// `abstract = true` on an attribute emits `attribute X @abstract, value T;`.
    #[test]
    fn test_emit_attribute_abstract() {
        let toml_text = r#"
[attributes.isbn]
value = "string"
abstract = true
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute isbn @abstract, value string;"),
            "expected `attribute isbn @abstract, value string;`; got:\n{result}"
        );
    }

    /// `sub = "isbn"` with no `value` emits `attribute isbn-13 sub isbn;` (no `value` token).
    #[test]
    fn test_emit_attribute_sub() {
        let toml_text = r#"
[attributes.isbn-13]
sub = "isbn"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute isbn-13 sub isbn;"),
            "expected `attribute isbn-13 sub isbn;`; got:\n{result}"
        );
        assert!(
            !result.contains("value"),
            "sub attribute must NOT emit a `value` clause; got:\n{result}"
        );
    }

    /// `regex = "..."` emits `@regex("...")` after `value`.
    #[test]
    fn test_emit_attribute_regex() {
        let toml_text = r#"
[attributes.status]
value = "string"
regex = "^(paid|dispatched)$"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains(r#"attribute status, value string @regex("^(paid|dispatched)$");"#),
            "expected @regex after value; got:\n{result}"
        );
    }

    /// `values = [...]` emits `@values("a", "b", ...)` after `value`.
    #[test]
    fn test_emit_attribute_values() {
        let toml_text = r#"
[attributes.reaction]
value = "string"
values = ["like", "love", "funny"]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result
                .contains(r#"attribute reaction, value string @values("like", "love", "funny");"#),
            "expected @values after value; got:\n{result}"
        );
    }

    /// `range = "0..150"` emits `@range(0..150)` after `value`.
    #[test]
    fn test_emit_attribute_range() {
        let toml_text = r#"
[attributes.age]
value = "integer"
range = "0..150"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("attribute age, value integer @range(0..150);"),
            "expected @range after value; got:\n{result}"
        );
    }

    /// `entity X @abstract` and `entity Y sub X` both emit correctly.
    #[test]
    fn test_emit_entity_abstract_and_sub() {
        let toml_text = r#"
[entities.book]
abstract = true

[entities.hardback]
sub = "book"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("entity book @abstract;"),
            "expected `entity book @abstract;`; got:\n{result}"
        );
        assert!(
            result.contains("entity hardback sub book;"),
            "expected `entity hardback sub book;`; got:\n{result}"
        );
    }

    /// `relation authoring sub contribution, ...` emits correctly.
    #[test]
    fn test_emit_relation_sub() {
        let toml_text = r#"
[relations.contribution]
roles = [{ name = "contributor" }, { name = "work" }]

[relations.authoring]
sub = "contribution"
roles = [{ name = "author", as = "contributor" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("relation authoring sub contribution,"),
            "expected `relation authoring sub contribution,`; got:\n{result}"
        );
        assert!(
            result.contains("relates author as contributor"),
            "expected `relates author as contributor`; got:\n{result}"
        );
    }

    /// Integration smoke: feeds abstract/sub/regex TOML through `toml_to_typeql`
    /// and asserts the output contains expected declarations.
    #[test]
    fn test_emit_p0_integration_smoke() {
        let toml_text = r#"
[attributes.isbn]
value = "string"
abstract = true

[attributes.isbn-13]
sub = "isbn"

[attributes.status]
value = "string"
regex = "^(paid|dispatched|delivered)$"

[entities.book]
abstract = true
owns = [{ attribute = "isbn-13", key = true }]

[entities.hardback]
sub = "book"
owns = ["stock"]

[attributes.stock]
value = "integer"
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        assert!(
            result.contains("attribute isbn @abstract, value string;"),
            "expected `attribute isbn @abstract, value string;`; got:\n{result}"
        );
        assert!(
            result.contains("attribute isbn-13 sub isbn;"),
            "expected `attribute isbn-13 sub isbn;`; got:\n{result}"
        );
        assert!(
            result.contains("entity book @abstract"),
            "expected `entity book @abstract`; got:\n{result}"
        );
        assert!(
            result.contains("entity hardback sub book"),
            "expected `entity hardback sub book`; got:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // owns-level annotation surface (@key / @unique / @card on owned attributes)
    // -------------------------------------------------------------------------

    /// `{ attribute = "isbn-13", key = true }` emits `owns isbn-13 @key`.
    #[test]
    fn test_emit_owns_key() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn-13", key = true }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-13 @key"),
            "expected `owns isbn-13 @key`; got:\n{result}"
        );
    }

    /// `{ attribute = "isbn-10", unique = true }` emits `owns isbn-10 @unique`.
    #[test]
    fn test_emit_owns_unique() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn-10", unique = true }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-10 @unique"),
            "expected `owns isbn-10 @unique`; got:\n{result}"
        );
    }

    /// `{ attribute = "isbn", card = "0..2" }` emits `owns isbn @card(0..2)`.
    #[test]
    fn test_emit_owns_card() {
        let toml_text = r#"
[entities.book]
owns = [{ attribute = "isbn", card = "0..2" }]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "expected `owns isbn @card(0..2)`; got:\n{result}"
        );
    }

    /// Mixed owns array: annotated table + bare string must emit in array order.
    #[test]
    fn test_emit_owns_mixed() {
        let toml_text = r#"
[entities.book]
owns = [
    { attribute = "isbn-13", key = true },
    { attribute = "isbn-10", unique = true },
    { attribute = "isbn", card = "0..2" },
    "title",
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");

        // All four present
        assert!(
            result.contains("owns isbn-13 @key"),
            "missing `owns isbn-13 @key`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn-10 @unique"),
            "missing `owns isbn-10 @unique`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "missing `owns isbn @card(0..2)`; got:\n{result}"
        );
        assert!(
            result.contains("owns title"),
            "missing `owns title`; got:\n{result}"
        );

        // Order preserved
        let pos_key = result.find("owns isbn-13 @key").unwrap();
        let pos_unique = result.find("owns isbn-10 @unique").unwrap();
        let pos_card = result.find("owns isbn @card(0..2)").unwrap();
        let pos_title = result.find("owns title").unwrap();
        assert!(
            pos_key < pos_unique,
            "isbn-13 @key must precede isbn-10 @unique"
        );
        assert!(
            pos_unique < pos_card,
            "isbn-10 @unique must precede isbn @card"
        );
        assert!(pos_card < pos_title, "isbn @card must precede title");
    }

    /// Integration smoke: TOML with annotated owns feeds through correctly.
    #[test]
    fn test_emit_p1_integration_smoke() {
        let toml_text = r#"
[entities.book]
owns = [
    { attribute = "isbn-13", key = true },
    { attribute = "isbn", card = "0..2" },
    "title",
]
"#;
        let result = toml_to_typeql(toml_text).expect("toml_to_typeql failed");
        assert!(
            result.contains("owns isbn-13 @key"),
            "missing `owns isbn-13 @key`; got:\n{result}"
        );
        assert!(
            result.contains("owns isbn @card(0..2)"),
            "missing `owns isbn @card(0..2)`; got:\n{result}"
        );
        assert!(
            result.contains("owns title"),
            "missing `owns title`; got:\n{result}"
        );

        // Order: isbn-13 @key < isbn @card < title
        let k = result.find("owns isbn-13 @key").unwrap();
        let c = result.find("owns isbn @card(0..2)").unwrap();
        let t = result.find("owns title").unwrap();
        assert!(k < c && c < t, "owns order not preserved; got:\n{result}");
    }
}
