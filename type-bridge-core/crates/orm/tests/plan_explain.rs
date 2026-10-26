use std::path::PathBuf;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::plan_explain::explain;

#[test]
fn fixtures_explain_with_one_common_canonical_shape() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/contracts/plan_v3");
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
        let plan = decode_query_plan_v3(&std::fs::read(&path).unwrap(), &schema).unwrap();
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
