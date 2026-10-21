use type_bridge_orm::_schema::SchemaInfo;

fn canonical(source: &str) -> typeql::Query {
    let first = typeql::parse_query(source).expect("parse schema");
    typeql::parse_query(&first.to_string()).expect("parse canonical schema")
}

#[test]
fn pre_ast_schema_fixtures_keep_official_ast_semantics() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/schema_pre_ast.json")).expect("snapshot cases");
    let mut compared = 0;
    for (index, case) in cases.iter().enumerate() {
        let old = case["typeql"].as_str().expect("old TypeQL");
        // An empty schema is an API sentinel, not a TypeQL query.
        if old.trim() == "define" {
            continue;
        }
        let schema: SchemaInfo =
            serde_json::from_value(case["schema"].clone()).expect("schema fixture");
        let new = schema.to_typeql().expect("emit schema");
        // Case 58: the old emitter repeated inherited plays, which is invalid:
        // TypeDB SVL42 rejects unchanged inherited plays on assembly.
        if index == 58 {
            let mut corrected = old.to_owned();
            for line in old.lines().filter(|line| line.contains(" plays ")) {
                let (player, capability) = line.split_once(" plays ").unwrap();
                let mut ancestor = schema
                    .entities
                    .get(player)
                    .and_then(|e| e.parent_type.as_deref());
                while let Some(parent) = ancestor {
                    if old
                        .lines()
                        .any(|candidate| candidate == format!("{parent} plays {capability}"))
                    {
                        corrected = corrected.replace(line, "");
                        break;
                    }
                    ancestor = schema
                        .entities
                        .get(parent)
                        .and_then(|e| e.parent_type.as_deref());
                }
            }
            assert_eq!(canonical(&corrected), canonical(&new));
            continue;
        }
        assert_eq!(
            canonical(old),
            canonical(&new),
            "schema fixture {index}: {old} -> {new}"
        );
        compared += 1;
    }
    assert_eq!(compared, 57);
}
