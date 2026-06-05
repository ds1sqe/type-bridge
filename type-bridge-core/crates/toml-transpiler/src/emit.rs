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
/// entity <name>, owns <a>, owns <b>;   -- or --
/// entity <name>;                        -- when owns is empty
/// ...
/// ```
///
/// Attribute and entity declarations appear in TOML document order (preserved
/// by [`indexmap::IndexMap`]). Within an entity, `owns` clauses appear in the
/// order they were declared in the TOML array.
pub fn emit(schema: &TomlSchema) -> String {
    let mut out = String::from("define\n");

    for (name, attr) in &schema.attributes {
        out.push_str(&format!("attribute {}, value {};\n", name, attr.value));
    }

    for (name, entity) in &schema.entities {
        if entity.owns.is_empty() {
            out.push_str(&format!("entity {};\n", name));
        } else {
            let owns_clauses: Vec<String> =
                entity.owns.iter().map(|a| format!("owns {}", a)).collect();
            out.push_str(&format!("entity {}, {};\n", name, owns_clauses.join(", ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use crate::toml_to_typeql;

    /// Feed the canonical slice TOML through `toml_to_typeql` and verify the
    /// emitted TypeQL contains all expected declarations in order.
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
}
