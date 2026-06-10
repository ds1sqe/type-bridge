use crate::common::rust_binding::*;
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
