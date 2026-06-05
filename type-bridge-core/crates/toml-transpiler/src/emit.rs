//! TypeQL define-block emitter.
//!
//! [`emit`] is a total function over the typed model — it never inspects raw
//! `toml::Value` tables and never calls `.unwrap()` on optional fields.

use crate::model::TomlSchema;

/// Emit a canonical TypeQL `define` block from a typed schema model.
///
/// Output format:
/// ```text
/// define
/// attribute <name>, value <type>;
/// ...
/// entity <name>, owns <a>, owns <b>, plays <r>:<role>;   -- or --
/// entity <name>;                                          -- when owns+plays empty
/// ...
/// relation <name>, relates <r> @card(<m..n>), relates <s> as <t>, owns <a>;
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
        out.push_str(&format!("attribute {}, value {};\n", name, attr.value));
    }

    // --- entities ---
    for (name, entity) in &schema.entities {
        let mut clauses: Vec<String> = entity.owns.iter().map(|a| format!("owns {}", a)).collect();
        for p in &entity.plays {
            clauses.push(format!("plays {}:{}", p.relation, p.role));
        }
        if clauses.is_empty() {
            out.push_str(&format!("entity {};\n", name));
        } else {
            out.push_str(&format!("entity {}, {};\n", name, clauses.join(", ")));
        }
    }

    // --- relations ---
    for (name, relation) in &schema.relations {
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
        for a in &relation.owns {
            clauses.push(format!("owns {}", a));
        }
        if clauses.is_empty() {
            out.push_str(&format!("relation {};\n", name));
        } else {
            out.push_str(&format!("relation {}, {};\n", name, clauses.join(", ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use crate::toml_to_typeql;

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

    /// A role with an `as` override emits `relates <name> as <target>`. The
    /// parser accepts a parentless override, so this round-trips without a
    /// relation parent.
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
}
