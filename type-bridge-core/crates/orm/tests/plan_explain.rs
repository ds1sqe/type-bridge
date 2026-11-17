use std::path::PathBuf;
use type_bridge_contract::query_plan::decode_query_plan;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::plan_explain::explain;

#[test]
fn fixtures_explain_with_one_common_canonical_shape() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/contracts/plan");
    let schema = decode_declared_schema(&std::fs::read(root.join("schema.json")).unwrap()).unwrap();
    let mut explained = 0;
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "json")
            || matches!(
                path.file_stem().unwrap().to_str().unwrap(),
                "schema" | "manifest"
            )
        {
            continue;
        }
        let plan = decode_query_plan(&std::fs::read(&path).unwrap(), &schema).unwrap();
        let explanation =
            explain(&plan).unwrap_or_else(|error| panic!("{}: {error:?}", path.display()));
        assert_eq!(
            explanation.statements.len(),
            explanation.read_statements + explanation.write_statements
        );
        let bytes = explanation.canonical_bytes().unwrap();
        if path.file_stem().unwrap() == "purchase" {
            let fixture = root.join("explain/purchase.json");
            if std::env::var_os("UPDATE_EXPECT").is_some() {
                std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
                std::fs::write(&fixture, &bytes).unwrap();
            }
            assert_eq!(bytes, std::fs::read(fixture).unwrap());
            let text = String::from_utf8(bytes).unwrap();
            assert!(!text.contains("purchase-1"));
            assert!(!text.contains("item-1"));
            assert!(!text.contains("buyer-1"));
        }
        explained += 1;
    }
    assert!(explained > 30);
}

#[test]
fn sdk_purchase_explanation_matches_committed_fixture() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let authority: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("tests/fixtures/plan_builder/purchase.json")).unwrap(),
    )
    .unwrap();
    let expected: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("tests/contracts/sdk_conformance/writes/purchase/expected.json"))
            .unwrap(),
    )
    .unwrap();
    let decode_hex = |text: &str| {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
    };
    let schema =
        decode_declared_schema(&decode_hex(authority["declared_hex"].as_str().unwrap())).unwrap();
    let plan = decode_query_plan(
        &decode_hex(expected["plans"]["purchase"].as_str().unwrap()),
        &schema,
    )
    .unwrap();
    let profile = expected["semantic_profile"]
        .as_str()
        .unwrap()
        .replace('/', "-");
    let fixture = root.join(format!(
        "tests/contracts/plan/explain/purchase-sdk.{profile}.json"
    ));
    let bytes = explain(&plan).unwrap().canonical_bytes().unwrap();
    if std::env::var_os("UPDATE_EXPECT").is_some() {
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&fixture, &bytes).unwrap();
    }
    assert_eq!(bytes, std::fs::read(fixture).unwrap());
}
