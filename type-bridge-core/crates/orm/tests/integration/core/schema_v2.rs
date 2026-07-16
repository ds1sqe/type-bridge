use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_orm::TxType;

#[tokio::test]
async fn typedb_3121_accepts_schema_v2_annotation_registry() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let prefix = unique_schema_suffix("rust", "schema-v2-annotations");
    let query = format!(
        r#"define
attribute {prefix}-text @independent @doc("text") @meta("source", "probe"),
  value string @regex("^.+$") @values("a", "b") @range("a".."z");
attribute {prefix}-key value string;
attribute {prefix}-unique value integer;
entity {prefix}-base @abstract @doc("base") @meta("source", "probe"),
  owns {prefix}-key @key @doc("key") @meta("source", "probe"),
  owns {prefix}-unique @unique @card(0..1) @values(1, 2) @range(0..10);
entity {prefix}-child sub {prefix}-base @doc("child subtype") @meta("source", "probe");
relation {prefix}-membership @abstract @doc("membership") @meta("source", "probe"),
  relates member @abstract @card(1..) @doc("member role") @meta("source", "probe");
entity {prefix}-player,
  plays {prefix}-membership:member @card(0..1) @doc("playing") @meta("source", "probe");"#
    );

    let result = db.execute_raw(&query, TxType::Schema).await;
    assert!(
        result.is_ok(),
        "TypeDB 3.12.1 rejected the supported annotation registry: {}",
        result.expect_err("checked as an error")
    );
}

#[tokio::test]
async fn typedb_3121_rejects_invalid_schema_v2_annotations() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let prefix = unique_schema_suffix("rust", "schema-v2-invalid-annotations");
    let invalid = [
        format!("define entity {prefix}-independent @independent;"),
        format!("define entity {prefix}-card @card(1);"),
        format!(
            "define entity {prefix}-abstract-owns, owns {prefix}-abstract-attribute @abstract; attribute {prefix}-abstract-attribute value string;"
        ),
        format!("define attribute {prefix}-integer-regex value integer @regex(\".+\");"),
        format!(
            "define entity {prefix}-integer-owner, owns {prefix}-owned-integer @regex(\".+\"); attribute {prefix}-owned-integer value integer;"
        ),
        format!("define attribute {prefix}-value-doc value string @doc(\"not a concept\");"),
        format!("define attribute {prefix}-value-meta value string @meta(\"source\", \"probe\");"),
        format!("define entity {prefix}-numeric-doc @doc(1);"),
        format!("define entity {prefix}-numeric-meta @meta(\"source\", 1);"),
        format!(
            "define entity {prefix}-double-owner, owns {prefix}-double-key @key; attribute {prefix}-double-key value double;"
        ),
    ];

    for query in invalid {
        let result = db.execute_raw(&query, TxType::Schema).await;
        assert!(
            result.is_err(),
            "TypeDB 3.12.1 unexpectedly accepted invalid annotation query: {query}"
        );
    }
}
