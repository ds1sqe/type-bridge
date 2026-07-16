//! Live Schema V2 semantic-profile probes against TypeDB.

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_orm::TxType;
use type_bridge_orm::session::backend::QueryResult;

#[tokio::test]
async fn type_and_struct_labels_share_typedb_namespace() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-global-labels");
    let label = format!("{suffix}-shared");

    let error = db
        .execute_raw(
            &format!(
                "define\n\
                 struct {label}, value count integer;\n\
                 entity {label};"
            ),
            TxType::Schema,
        )
        .await
        .expect_err("TypeDB 3.12.1 must reject a struct and type with the same label");
    assert!(
        error.to_string().contains(&label),
        "collision diagnostic must name the shared label: {error}"
    );
}

#[tokio::test]
async fn omitted_interface_cards_match_typedb_defaults() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-card-defaults");
    let attribute = format!("{suffix}-value");
    let owner = format!("{suffix}-owner");
    let player = format!("{suffix}-player");
    let relation = format!("{suffix}-link");
    let role = format!("{suffix}-endpoint");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {attribute}, value string;\n\
             entity {owner}, owns {attribute};\n\
             relation {relation}, relates {role};\n\
             entity {player}, plays {relation}:{role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept omitted interface cardinalities");

    let exported = db
        .schema_text()
        .await
        .expect("TypeDB 3.12.1 must export the probe schema");

    for clause in [
        format!("owns {attribute}"),
        format!("relates {role}"),
        format!("plays {relation}:{role}"),
    ] {
        let rendered = exported
            .lines()
            .find(|line| line.trim_start().starts_with(&clause))
            .unwrap_or_else(|| panic!("schema export omitted {clause:?}:\n{exported}"));
        assert!(
            !rendered.contains("@card"),
            "TypeDB must render an omitted cardinality without synthesizing @card: {rendered}"
        );
    }

    db.execute_raw(&format!("insert $x isa {owner};"), TxType::Write)
        .await
        .expect("omitted owns cardinality must allow zero values");

    let owns_error = db
        .execute_raw(
            &format!(
                "insert $x isa {owner}, \
                 has {attribute} \"one\", \
                 has {attribute} \"two\";"
            ),
            TxType::Write,
        )
        .await
        .expect_err("omitted owns cardinality must reject two values");
    let owns_message = owns_error.to_string();
    assert!(
        owns_message.contains("[CNT5]")
            && owns_message.contains("@card(0..1)")
            && owns_message.contains("attribute ownership"),
        "unexpected omitted-owns violation: {owns_message}"
    );

    db.execute_raw(&format!("insert $r isa {relation};"), TxType::Write)
        .await
        .expect("omitted relates cardinality must allow zero players");

    let relates_error = db
        .execute_raw(
            &format!(
                "insert \
                 $a isa {player}; \
                 $b isa {player}; \
                 ({role}: $a, {role}: $b) isa {relation};"
            ),
            TxType::Write,
        )
        .await
        .expect_err("omitted relates cardinality must reject two players");
    let relates_message = relates_error.to_string();
    assert!(
        relates_message.contains("[CNT5]")
            && relates_message.contains("@card(0..1)")
            && relates_message.contains("relates constraint violation"),
        "unexpected omitted-relates violation: {relates_message}"
    );

    db.execute_raw(
        &format!(
            "insert \
             $p isa {player}; \
             ({role}: $p) isa {relation}; \
             ({role}: $p) isa {relation};"
        ),
        TxType::Write,
    )
    .await
    .expect("omitted plays cardinality must allow repeated role playing");
}

#[tokio::test]
async fn specialized_roles_require_explicit_child_role_players() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-role-specialization");
    let parent_relation = format!("{suffix}-contribution");
    let parent_role = format!("{suffix}-contributor");
    let child_relation = format!("{suffix}-authoring");
    let child_role = format!("{suffix}-author");
    let parent_player = format!("{suffix}-parent-player");
    let child_player = format!("{suffix}-child-player");

    db.execute_raw(
        &format!(
            "define\n\
             relation {parent_relation} @abstract, relates {parent_role};\n\
             relation {child_relation} sub {parent_relation}, \
               relates {child_role} as {parent_role};\n\
             entity {parent_player}, plays {parent_relation}:{parent_role};\n\
             entity {child_player}, plays {child_relation}:{child_role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 must accept specialized roles");

    let parent_error = db
        .execute_raw(
            &format!(
                "insert $p isa {parent_player}; \
                 ({child_role}: $p) isa {child_relation};"
            ),
            TxType::Write,
        )
        .await
        .expect_err("parent-role players must not implicitly play the specialized child role");
    let parent_message = parent_error.to_string();
    assert!(
        parent_message.contains("[INF11]")
            && parent_message.contains(&parent_player)
            && parent_message.contains(&format!("{child_relation}:{child_role}")),
        "unexpected specialized-role rejection: {parent_message}"
    );

    db.execute_raw(
        &format!(
            "insert $p isa {child_player}; \
             ({child_role}: $p) isa {child_relation};"
        ),
        TxType::Write,
    )
    .await
    .expect("an explicit child-role player must be accepted");
}

#[tokio::test]
async fn key_owns_enforces_exactly_one_on_typedb_3_12_1() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-key-cardinality");
    let attribute = format!("key-cardinality-attribute-{suffix}");
    let owner = format!("key-cardinality-owner-{suffix}");

    db.execute_raw(
        &format!(
            "define\nattribute {attribute}, value string;\nentity {owner}, owns {attribute} @key;"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 accepts @key ownership");

    let zero = db
        .execute_raw(&format!("insert $x isa {owner};"), TxType::Write)
        .await
        .expect_err("@key rejects an owner with no key value");
    db.execute_raw(
        &format!("insert $x isa {owner}, has {attribute} \"accepted\";"),
        TxType::Write,
    )
    .await
    .expect("@key accepts an owner with exactly one key value");
    let two = db
        .execute_raw(
            &format!(
                "insert $x isa {owner}, has {attribute} \"left\", has {attribute} \"right\";"
            ),
            TxType::Write,
        )
        .await
        .expect_err("@key rejects an owner with two key values");

    for error in [zero, two] {
        let message = error.to_string();
        assert!(message.contains("[CNT5]"), "unexpected TypeDB error: {message}");
        assert!(
            message.contains("[CNT5] Constraint '@card(1..1)'"),
            "TypeDB did not render the exact-one constraint: {message}"
        );
        assert!(
            message.contains("attribute ownership"),
            "TypeDB did not identify attribute ownership: {message}"
        );
    }
}

#[tokio::test]
async fn has_attribute_label_includes_subtypes_but_isa_exact_filters_them() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "assertion-has-exactness");
    let base = format!("{suffix}-base");
    let child = format!("{suffix}-child");
    let owner = format!("{suffix}-owner");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {base}, value string;\n\
             attribute {child} sub {base};\n\
             entity {owner}, owns {base} @card(0..2), owns {child};"
        ),
        TxType::Schema,
    )
    .await
    .expect("TypeDB 3.12.1 accepts the attribute-subtype fixture");
    db.execute_raw(
        &format!(
            "insert $owner isa {owner}, \
             has {base} \"base-value\", has {child} \"child-value\";"
        ),
        TxType::Write,
    )
    .await
    .expect("an owner with both interfaces accepts exact base and child attributes");

    let broad = db
        .execute_raw(
            &format!(
                "match $owner isa! {owner}, has {base} $attribute; \
                 $attribute == \"child-value\"; \
                 select $attribute; limit 1;"
            ),
            TxType::Read,
        )
        .await
        .expect("a labeled has pattern includes attribute subtypes");
    let exact_child = db
        .execute_raw(
            &format!(
                "match $owner isa! {owner}, has {base} $attribute; \
                 $attribute isa! {base}; $attribute == \"child-value\"; \
                 select $attribute; limit 1;"
            ),
            TxType::Read,
        )
        .await
        .expect("the exact attribute guard accepts a child-value comparison");
    let exact_base = db
        .execute_raw(
            &format!(
                "match $owner isa! {owner}, has {base} $attribute; \
                 $attribute isa! {base}; $attribute == \"base-value\"; \
                 select $attribute; limit 1;"
            ),
            TxType::Read,
        )
        .await
        .expect("the exact attribute guard accepts a base-value comparison");

    assert!(
        matches!(broad, QueryResult::Rows(rows) if rows.len() == 1),
        "TypeDB 3.12.1 must expose the subtype through the broad has pattern"
    );
    assert!(
        matches!(exact_child, QueryResult::Rows(rows) if rows.is_empty()),
        "isa! must remove the child value from exact Has assertion semantics"
    );
    assert!(
        matches!(exact_base, QueryResult::Rows(rows) if rows.len() == 1),
        "isa! must retain the exact base value"
    );
}
