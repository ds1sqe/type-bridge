fn canonical(source: &str) -> typeql::Query {
    let query = typeql::parse_query(source).expect("parse TypeQL");
    typeql::parse_query(&query.to_string()).expect("parse canonical TypeQL")
}

#[test]
fn released_toml_fixture_outputs_keep_their_official_ast() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/pre_ast.json")).expect("TOML snapshots");
    let mut compared = 0;
    let mut corrected = 0;
    let mut rejected = 0;
    for (index, case) in cases.iter().enumerate() {
        let input = case["toml"].as_str().expect("TOML input");
        let old = case["typeql"].as_str().expect("old output");
        let new = type_bridge_toml_transpiler::toml_to_typeql(input);
        // Explicit old-invalid-output correction: historical TOML aliases
        // were emitted as unresolved struct labels (TypeDB SYR4).
        let corrected_aliases = old
            .replace("value long", "value integer")
            .replace("value int;", "value integer;")
            .replace("value bool;", "value boolean;");
        if corrected_aliases != old && typeql::parse_query(&corrected_aliases).is_ok() {
            assert_eq!(
                canonical(&corrected_aliases),
                canonical(&new.expect("aliases render"))
            );
            corrected += 1;
        } else if typeql::parse_query(old).is_ok() {
            let new = new.expect("valid old schema must still render");
            assert_eq!(canonical(old), canonical(&new), "TOML fixture {index}");
            compared += 1;
        } else if old.contains("return { 1 }") {
            assert!(matches!(
                new,
                Err(type_bridge_toml_transpiler::TranspileError::MalformedFunctionBody { .. })
            ));
            rejected += 1;
        } else {
            assert!(
                old.contains("struct "),
                "unreviewed old syntax error in fixture {index}: {old}"
            );
            canonical(&new.expect("corrected struct schema must render"));
            corrected += 1;
        }
    }
    println!(
        "TOML parity: {compared} equivalent, {corrected} corrected invalid outputs, {rejected} invalid function bodies rejected"
    );
    assert_eq!(compared + corrected + rejected, 33);
}

#[test]
fn historical_value_aliases_use_builtin_domains_in_attributes_and_structs() {
    for (alias, builtin) in [("long", "integer"), ("int", "integer"), ("bool", "boolean")] {
        let input = format!(
            r#"[attributes.sample]
value = "{alias}"
[structs.record]
fields = [{{ name = "field", type = "{alias}" }}]
"#
        );
        let output = type_bridge_toml_transpiler::toml_to_typeql(&input).unwrap();
        assert!(output.contains(&format!("attribute sample, value {builtin}")));
        assert!(output.contains(&format!("field value {builtin}")));
        canonical(&output);
    }
}

#[test]
fn toml_function_body_keeps_comments_and_whitespace_verbatim() {
    let body = "match\n  # retained body comment\n  let $value = $seed;\nreturn first $value;\n";
    let input = format!(
        "[functions.keep-body]\nsignature = \"fun keep-body($seed: integer) -> integer\"\nbody = \"\"\"\n{body}\"\"\"\n"
    );
    let output = type_bridge_toml_transpiler::toml_to_typeql(&input).unwrap();
    assert!(output.contains(body), "{output}");
    assert_eq!(
        canonical(&format!(
            "define fun keep-body($seed: integer) -> integer:\n{body}"
        )),
        canonical(&output)
    );
}
