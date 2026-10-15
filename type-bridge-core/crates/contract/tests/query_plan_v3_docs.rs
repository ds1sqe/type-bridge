//! Keep the public worked examples executable against the shared codec.
use std::path::Path;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::schema::decode_declared_schema;

#[test]
fn documented_plan_v3_bytes_and_fingerprints() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let fixtures = root.join("tests/contracts/plan_v3");
    let schema =
        decode_declared_schema(&std::fs::read(fixtures.join("schema.json")).unwrap()).unwrap();
    let document = std::fs::read_to_string(root.join("docs/reference/query-plan-v3.md")).unwrap();
    let mut names = Vec::new();
    for example in document.split("<!-- plan-v3-example: ").skip(1) {
        let (filename, rest) = example.split_once(" -->").unwrap();
        let (_, rest) = rest.split_once("```json\n").unwrap();
        let (bytes, rest) = rest.split_once("\n```").unwrap();
        let (_, rest) = rest.split_once("Fingerprint: `").unwrap();
        let (expected, _) = rest.split_once('`').unwrap();
        assert_eq!(
            bytes.as_bytes(),
            std::fs::read(fixtures.join(filename)).unwrap()
        );
        let plan = decode_query_plan_v3(bytes.as_bytes(), &schema).unwrap();
        assert_eq!(plan.canonical_bytes().unwrap(), bytes.as_bytes());
        assert_eq!(
            plan.fingerprint()
                .unwrap()
                .as_fingerprint()
                .digest()
                .to_hex(),
            expected
        );
        names.push(filename);
    }
    assert_eq!(
        names,
        [
            "purchase.json",
            "touch.json",
            "relation_assign.json",
            "batch.json"
        ]
    );
}
