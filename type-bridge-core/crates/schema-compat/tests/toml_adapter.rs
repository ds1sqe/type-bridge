use type_bridge_contract::schema::DocumentId;
use type_bridge_schema_compat::{toml_to_declared, toml_to_facts, typeql_to_declared};

fn rendered_document() -> DocumentId {
    DocumentId::new("generated/schema-from-toml.tql").expect("valid rendered document identity")
}

#[test]
fn toml_and_equivalent_typeql_have_equal_declared_identity() {
    let toml = r#"[attributes.name]
value = "string"

[entities.person]
owns = ["name"]
"#;
    let typeql = r#"define
attribute name, value string;
entity person, owns name;
"#;

    let from_toml = toml_to_declared(rendered_document(), toml).expect("TOML schema adapts");
    let from_typeql = typeql_to_declared(
        DocumentId::new("schema/main.tql").expect("valid TypeQL document identity"),
        typeql,
    )
    .expect("TypeQL schema adapts");
    let facts = toml_to_facts(rendered_document(), toml).expect("TOML facts adapt");

    assert_eq!(
        from_toml.declared_identity_fingerprint(),
        from_typeql.declared_identity_fingerprint()
    );
    assert_eq!(from_toml.facts().len(), facts.len());
}

#[test]
fn toml_transpiler_diagnostics_do_not_fabricate_source_spans() {
    let diagnostics = toml_to_declared(rendered_document(), "[attributes.orphan]\n")
        .expect_err("invalid legacy TOML schema must fail");
    let diagnostic = diagnostics.iter().next().expect("one diagnostic");

    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "invalid_toml_schema"
    );
    assert!(diagnostic.primary().is_none());
}

#[test]
fn commented_toml_function_keeps_released_typeql_body_and_fingerprint() {
    let body = "match\n  # retained body comment\n  let $value = $seed;\nreturn first $value;";
    let toml = format!(
        "[functions.keep-body]\nsignature = \"fun keep-body($seed: integer) -> integer\"\nbody = \"\"\"\n{body}\n\"\"\"\n"
    );
    let from_toml = toml_to_declared(rendered_document(), &toml).unwrap();
    let released = typeql_to_declared(
        rendered_document(),
        &format!("define fun keep-body($seed: integer) -> integer:\n{body}\n"),
    )
    .unwrap();
    assert_eq!(
        from_toml.declared_identity_fingerprint(),
        released.declared_identity_fingerprint()
    );
    let function = from_toml
        .facts()
        .find_map(|fact| match fact {
            type_bridge_contract::schema::SchemaFact::Function(function) => Some(function),
            _ => None,
        })
        .unwrap();
    assert_eq!(function.body().text(), body);
}
