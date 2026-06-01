use crate::common::rust_binding::*;
use type_bridge_orm::*;

#[tokio::test]
async fn full_entity_lifecycle() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_full_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let alice_name = unique_label("Alice-Integration");
    let mut alice = Person {
        iid: None,
        name: Name(alice_name.clone()),
        age: Age(30),
    };
    let iid = manager.insert(&mut alice).await.expect("insert failed");
    assert!(!iid.is_empty());
    assert_eq!(alice.iid(), Some(iid.as_str()));

    let people = manager.all().await.expect("all() failed");
    assert!(!people.is_empty());

    let count = manager.count().await.expect("count() failed");
    assert!(count >= 1);

    manager.delete(&alice).await.expect("delete failed");

    let after_delete = manager
        .get(&[Filter::string_eq("name", alice_name)])
        .await
        .expect("get after delete failed");
    assert!(after_delete.is_empty());
}

#[tokio::test]
async fn batch_insert_and_delete() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let batch_prefix = unique_label("Batch");
    let mut people = vec![
        Person {
            iid: None,
            name: Name(format!("{batch_prefix}-A")),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name(format!("{batch_prefix}-B")),
            age: Age(25),
        },
        Person {
            iid: None,
            name: Name(format!("{batch_prefix}-C")),
            age: Age(30),
        },
    ];
    let iids = manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");
    assert_eq!(iids.len(), 3);
    assert!(people.iter().all(|p| p.iid().is_some()));

    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

#[tokio::test]
async fn entity_get_one_count_and_update_many() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let batch_prefix = unique_label("UpdateMany");
    let mut people = vec![
        Person {
            iid: None,
            name: Name(format!("{batch_prefix}-A")),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name(format!("{batch_prefix}-B")),
            age: Age(21),
        },
    ];

    manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");

    let first = manager
        .get_one(&[Filter::string_eq("name", format!("{batch_prefix}-A"))])
        .await
        .expect("get_one should return the keyed row");
    assert_eq!(first.age.0, 20);

    assert_eq!(
        manager
            .count_with_filters(&[Filter::string_eq("name", format!("{batch_prefix}-B"))])
            .await
            .expect("count_with_filters failed"),
        1
    );

    people[0].age = Age(30);
    people[1].age = Age(31);
    manager
        .update_many(&people)
        .await
        .expect("update_many failed");

    let updated = manager
        .query()
        .filter(Expr::contains("name", batch_prefix.clone()))
        .order_by("age", SortDir::Asc)
        .execute()
        .await
        .expect("updated rows should query");
    assert_eq!(updated.len(), 2);
    assert_eq!(updated[0].age.0, 30);
    assert_eq!(updated[1].age.0, 31);

    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

#[tokio::test]
async fn entity_update_lifecycle() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let person_name = unique_label("UpdateTest");
    let mut person = Person {
        iid: None,
        name: Name(person_name.clone()),
        age: Age(25),
    };
    manager.insert(&mut person).await.expect("insert failed");

    person.age = Age(26);
    manager.update(&person).await.expect("update failed");

    let results = manager
        .get(&[Filter::string_eq("name", person_name)])
        .await
        .expect("get after update failed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].age.0, 26);

    manager.delete(&person).await.expect("delete failed");
}

#[tokio::test]
async fn entity_put_creates_and_updates() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let person_name = unique_label("PutTest");
    let mut person = Person {
        iid: None,
        name: Name(person_name.clone()),
        age: Age(30),
    };
    let iid1 = manager.put(&mut person).await.expect("put create failed");
    assert!(!iid1.is_empty());

    let mut person2 = Person {
        iid: None,
        name: Name(person_name.clone()),
        age: Age(31),
    };
    let iid2 = manager.put(&mut person2).await.expect("put update failed");
    assert!(!iid2.is_empty());

    let results = manager
        .get(&[Filter::string_eq("name", person_name)])
        .await
        .expect("get after put failed");
    assert!(!results.is_empty());

    manager.delete(&person2).await.expect("delete failed");
}
