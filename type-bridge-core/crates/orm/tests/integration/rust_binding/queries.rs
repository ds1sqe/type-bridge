use crate::common::rust_binding::*;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

#[tokio::test]
async fn query_builder_with_filters() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let person_name = unique_label("QueryTest");
    let mut person = Person {
        iid: None,
        name: Name(person_name.clone()),
        age: Age(42),
    };
    manager.insert(&mut person).await.expect("insert failed");

    let results = manager
        .query()
        .filter(Expr::eq("name", AttributeValue::String(person_name)))
        .execute()
        .await
        .expect("query failed");
    assert!(!results.is_empty());

    manager.delete(&person).await.expect("delete failed");
}

#[tokio::test]
async fn query_builder_with_sort_and_limit() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let sort_prefix = unique_label("Sort");
    let mut people = vec![
        Person {
            iid: None,
            name: Name(format!("{sort_prefix}-A")),
            age: Age(30),
        },
        Person {
            iid: None,
            name: Name(format!("{sort_prefix}-B")),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name(format!("{sort_prefix}-C")),
            age: Age(25),
        },
    ];
    manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");

    let results = manager
        .query()
        .filter(Expr::contains("name", sort_prefix))
        .order_by("age", SortDir::Asc)
        .limit(2)
        .execute()
        .await
        .expect("sorted query failed");

    assert_eq!(results.len(), 2);
    assert!(results[0].age.0 <= results[1].age.0);

    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

#[tokio::test]
async fn query_builder_first_count_aggregate_and_group_by() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let query_prefix = unique_label("QueryBreadth");
    let mut people = vec![
        Person {
            iid: None,
            name: Name(format!("{query_prefix}-A")),
            age: Age(31),
        },
        Person {
            iid: None,
            name: Name(format!("{query_prefix}-B")),
            age: Age(32),
        },
        Person {
            iid: None,
            name: Name(format!("{query_prefix}-C")),
            age: Age(32),
        },
    ];
    manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");

    let first = manager
        .query()
        .filter(Expr::contains("name", query_prefix.clone()))
        .order_by("age", SortDir::Desc)
        .first()
        .await
        .expect("first query failed")
        .expect("first should return a row");
    assert_eq!(first.age.0, 32);

    let count = manager
        .query()
        .filter(Expr::contains("name", query_prefix.clone()))
        .count()
        .await
        .expect("query count failed");
    assert_eq!(count, 3);

    let aggregate = manager
        .query()
        .filter(Expr::contains("name", query_prefix.clone()))
        .aggregate(&[Agg::Count])
        .await
        .expect("query aggregate failed");
    assert_eq!(aggregate.count(), Some(3));

    let grouped = manager
        .query()
        .filter(Expr::contains("name", query_prefix))
        .group_by("age")
        .aggregate(&[Agg::Count])
        .await
        .expect("query group-by aggregate failed");
    assert_eq!(grouped.len(), 2);

    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

/// Proves that the orm answer path emits structurally identical JSON regardless
/// of which embedded driver band served the query.  Run this binary against a
/// band-7 server (TypeDB 3.8.3) and a band-8 server (3.11.5); identical golden
/// shape = band parity.
#[tokio::test]
async fn test_answer_json_shape_band_invariant() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    // Insert a person that owns both a string attribute (name) and an integer
    // attribute (age) so each value-type encoding branch is exercised.
    let manager = EntityManager::<Person>::new(&db);
    let person_name = unique_label("BandParity");
    let mut person = Person {
        iid: None,
        name: Name(person_name.clone()),
        age: Age(99),
    };
    manager.insert(&mut person).await.expect("insert failed");

    // Row query: match person + both attributes so every concept kind that the
    // concept_to_json helpers emit (entity, string-attribute, integer-attribute)
    // appears in a single result row.
    let typeql = format!(
        "match $p isa person, has name $n, has age $a; $n == \"{person_name}\"; select $p, $n, $a;"
    );
    let result = db
        .execute_raw(&typeql, TxType::Read)
        .await
        .expect("raw row query failed");

    let rows = match result {
        QueryResult::Rows(rows) => rows,
        other => panic!("expected QueryResult::Rows, got {other:?}"),
    };

    assert!(!rows.is_empty(), "expected at least one result row");

    for row in &rows {
        let obj = row.as_object().expect("each row must be a JSON object");

        // The query selects $p, $n, $a — all three columns must be present.
        assert!(obj.contains_key("p"), "row missing 'p' column: {row}");
        assert!(obj.contains_key("n"), "row missing 'n' column: {row}");
        assert!(obj.contains_key("a"), "row missing 'a' column: {row}");

        // --- entity concept ($p) ---
        // Both bands emit: category (String), label (String), iid (String).
        // No value/value_type keys for entities.
        let p = obj["p"].as_object().expect("'p' must be a JSON object");
        assert!(
            p["category"].is_string(),
            "'p.category' must be a string: {p:?}"
        );
        assert!(p["label"].is_string(), "'p.label' must be a string: {p:?}");
        assert!(
            p["iid"].is_string(),
            "'p.iid' must be a string (iid): {p:?}"
        );
        assert!(
            !p.contains_key("value"),
            "entity concept must not have 'value': {p:?}"
        );
        assert!(
            !p.contains_key("value_type"),
            "entity concept must not have 'value_type': {p:?}"
        );

        // --- string attribute ($n / name) ---
        // Both bands emit: category, label, value (JSON String), value_type (String).
        // iid is not present for attribute instances.
        let n = obj["n"].as_object().expect("'n' must be a JSON object");
        assert!(
            n["category"].is_string(),
            "'n.category' must be a string: {n:?}"
        );
        assert!(n["label"].is_string(), "'n.label' must be a string: {n:?}");
        assert!(
            n["value"].is_string(),
            "'n.value' must be a JSON string (string attribute): {n:?}"
        );
        assert!(
            n["value_type"].is_string(),
            "'n.value_type' must be a string: {n:?}"
        );

        // --- integer attribute ($a / age) ---
        // Both bands emit: category, label, value (JSON Number), value_type (String).
        let a = obj["a"].as_object().expect("'a' must be a JSON object");
        assert!(
            a["category"].is_string(),
            "'a.category' must be a string: {a:?}"
        );
        assert!(a["label"].is_string(), "'a.label' must be a string: {a:?}");
        assert!(
            a["value"].is_number(),
            "'a.value' must be a JSON number (integer attribute): {a:?}"
        );
        assert!(
            a["value_type"].is_string(),
            "'a.value_type' must be a string: {a:?}"
        );
    }

    manager.delete(&person).await.expect("delete failed");
}
