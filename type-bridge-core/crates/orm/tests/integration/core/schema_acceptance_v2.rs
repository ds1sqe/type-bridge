//! Live Schema V2 acceptance probes against TypeDB 3.12.1.

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_orm::TxType;

#[tokio::test]
async fn exact_zero_cardinality_is_rejected_for_every_interface() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-zero-card");

    let cases = [
        (
            "owns",
            format!(
                "define\n\
                 attribute {suffix}-owns-value, value string;\n\
                 entity {suffix}-owns-owner, owns {suffix}-owns-value @card(0);"
            ),
        ),
        (
            "relates",
            format!(
                "define\n\
                 relation {suffix}-relates-link, relates endpoint @card(0);"
            ),
        ),
        (
            "plays",
            format!(
                "define\n\
                 relation {suffix}-plays-link, relates endpoint;\n\
                 entity {suffix}-plays-player, \
                   plays {suffix}-plays-link:endpoint @card(0);"
            ),
        ),
    ];

    for (interface, query) in cases {
        let error = match db.execute_raw(&query, TxType::Schema).await {
            Ok(_) => {
                panic!(
                    "TypeDB 3.12.1 unexpectedly accepted exact-zero \
                     cardinality on {interface}: {query}"
                )
            }
            Err(error) => error,
        };
        assert!(
            !error.to_string().is_empty(),
            "TypeDB returned an empty exact-zero {interface} rejection"
        );
    }
}

#[tokio::test]
async fn function_and_sub_annotations_preserve_multiple_meta_keys() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    if !crate::common::rust_binding::server_supports_v2_conformance(&db) {
        eprintln!("skipping: sub/function @doc/@meta require a proven TypeDB 3.12+ server");
        return;
    }
    let suffix = unique_schema_suffix("rust", "schema-v2-doc-meta");
    let base = format!("{suffix}-base");
    let child = format!("{suffix}-child");
    let function = format!("{suffix}-constant");
    let sub_doc = format!("{suffix}-sub-doc");
    let sub_owner = format!("{suffix}-sub-owner");
    let sub_source = format!("{suffix}-sub-source");
    let function_doc = format!("{suffix}-function-doc");
    let function_owner = format!("{suffix}-function-owner");
    let function_source = format!("{suffix}-function-source");

    db.execute_raw(
        &format!(
            "define\n\
             entity {base};\n\
             entity {child} sub {base} \
               @doc(\"{sub_doc}\") \
               @meta(\"owner\", \"{sub_owner}\") \
               @meta(\"source\", \"{sub_source}\");\n\
             fun {function}() -> integer \
               @doc(\"{function_doc}\") \
               @meta(\"owner\", \"{function_owner}\") \
               @meta(\"source\", \"{function_source}\"):\n\
               match\n\
                 let $value = 4;\n\
               return first $value;"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept doc/meta on sub definitions and functions");

    let exported = db
        .schema_text()
        .await
        .expect("TypeDB 3.12.1 must export the documented schema");
    for expected in [
        format!("@doc(\"{sub_doc}\")"),
        format!("@meta(\"owner\", \"{sub_owner}\")"),
        format!("@meta(\"source\", \"{sub_source}\")"),
        format!("@doc(\"{function_doc}\")"),
        format!("@meta(\"owner\", \"{function_owner}\")"),
        format!("@meta(\"source\", \"{function_source}\")"),
    ] {
        assert!(
            exported.contains(&expected),
            "schema export omitted {expected:?}:\n{exported}"
        );
    }
}

#[tokio::test]
async fn relation_instances_are_accepted_as_role_players() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-relation-player");
    let outer = format!("{suffix}-outer");
    let inner = format!("{suffix}-inner");
    let endpoint = format!("{suffix}-endpoint");

    db.execute_raw(
        &format!(
            "define\n\
             relation {outer}, relates item;\n\
             relation {inner}, relates member, plays {outer}:item;\n\
             entity {endpoint}, plays {inner}:member;"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept a relation type as a role player");

    db.execute_raw(
        &format!(
            "insert\n\
             $endpoint isa {endpoint};\n\
             $inner isa {inner}, links (member: $endpoint);\n\
             (item: $inner) isa {outer};"
        ),
        TxType::Write,
    )
    .await
    .expect("TypeDB 3.12.1 must accept a relation instance as a role player");
}

#[tokio::test]
async fn schema_export_distinguishes_explicit_defaults_from_omission() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-explicit-defaults");
    let explicit_attribute = format!("{suffix}-explicit-value");
    let omitted_attribute = format!("{suffix}-omitted-value");
    let owner = format!("{suffix}-owner");
    let relation = format!("{suffix}-link");
    let explicit_role = format!("{suffix}-explicit-role");
    let omitted_role = format!("{suffix}-omitted-role");
    let player = format!("{suffix}-player");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {explicit_attribute}, value string;\n\
             attribute {omitted_attribute}, value string;\n\
             entity {owner}, \
               owns {explicit_attribute} @card(0..1), \
               owns {omitted_attribute};\n\
             relation {relation}, \
               relates {explicit_role} @card(0..1), \
               relates {omitted_role};\n\
             entity {player}, \
               plays {relation}:{explicit_role} @card(0..), \
               plays {relation}:{omitted_role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept explicit and omitted default cardinalities");

    let exported = db
        .schema_text()
        .await
        .expect("TypeDB 3.12.1 must export explicit default cardinalities");

    for clause in [
        format!("owns {explicit_attribute}"),
        format!("relates {explicit_role}"),
        format!("plays {relation}:{explicit_role}"),
    ] {
        let rendered = exported
            .lines()
            .find(|line| line.trim_start().starts_with(&clause))
            .unwrap_or_else(|| panic!("schema export omitted {clause:?}:\n{exported}"));
        assert!(
            rendered.contains("@card"),
            "schema export erased explicit default cardinality: {rendered}"
        );
    }

    for clause in [
        format!("owns {omitted_attribute}"),
        format!("relates {omitted_role}"),
        format!("plays {relation}:{omitted_role}"),
    ] {
        let rendered = exported
            .lines()
            .find(|line| line.trim_start().starts_with(&clause))
            .unwrap_or_else(|| panic!("schema export omitted {clause:?}:\n{exported}"));
        assert!(
            !rendered.contains("@card"),
            "schema export synthesized an omitted default cardinality: {rendered}"
        );
    }
}

#[tokio::test]
async fn value_constraints_are_enforced_on_value_and_ownership_subjects() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-value-constraints");
    let value_values = format!("{suffix}-value-values");
    let value_range = format!("{suffix}-value-range");
    let owns_values = format!("{suffix}-owns-values");
    let owns_range = format!("{suffix}-owns-range");
    let value_owner = format!("{suffix}-value-owner");
    let constrained_owner = format!("{suffix}-constrained-owner");
    let unconstrained_owner = format!("{suffix}-unconstrained-owner");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {value_values}, value integer @values(1, 2);\n\
             attribute {value_range}, value integer @range(10..20);\n\
             attribute {owns_values}, value integer;\n\
             attribute {owns_range}, value integer;\n\
             entity {value_owner}, owns {value_values}, owns {value_range};\n\
             entity {constrained_owner}, \
               owns {owns_values} @values(3, 4), \
               owns {owns_range} @range(100..200);\n\
             entity {unconstrained_owner}, owns {owns_values}, owns {owns_range};"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept values/range on attribute-value and ownership subjects");

    db.execute_raw(
        &format!(
            "insert\n\
             $value_values isa {value_owner}, has {value_values} 1;\n\
             $value_range_lower isa {value_owner}, has {value_range} 10;\n\
             $value_range_upper isa {value_owner}, has {value_range} 20;\n\
             $owns_values isa {constrained_owner}, has {owns_values} 3;\n\
             $owns_range_lower isa {constrained_owner}, has {owns_range} 100;\n\
             $owns_range_upper isa {constrained_owner}, has {owns_range} 200;\n\
             $unconstrained_values isa {unconstrained_owner}, has {owns_values} 5;\n\
             $unconstrained_range isa {unconstrained_owner}, has {owns_range} 99;"
        ),
        TxType::Write,
    )
    .await
    .expect(
        "allowed values and inclusive range endpoints must commit, while ownership constraints remain owner-scoped",
    );

    let rejected = [
        (
            "value-subject values",
            format!("insert $x isa {value_owner}, has {value_values} 3;"),
        ),
        (
            "value-subject range lower bound",
            format!("insert $x isa {value_owner}, has {value_range} 9;"),
        ),
        (
            "value-subject range upper bound",
            format!("insert $x isa {value_owner}, has {value_range} 21;"),
        ),
        (
            "ownership-subject values",
            format!("insert $x isa {constrained_owner}, has {owns_values} 5;"),
        ),
        (
            "ownership-subject range lower bound",
            format!("insert $x isa {constrained_owner}, has {owns_range} 99;"),
        ),
        (
            "ownership-subject range upper bound",
            format!("insert $x isa {constrained_owner}, has {owns_range} 201;"),
        ),
    ];

    for (case, query) in rejected {
        let error = db
            .execute_raw(&query, TxType::Write)
            .await
            .expect_err(&format!("TypeDB 3.12.1 must reject {case}: {query}"));
        let message = error.to_string();
        assert!(
            !message.is_empty(),
            "TypeDB returned an empty {case} rejection for {query}"
        );
    }
}
