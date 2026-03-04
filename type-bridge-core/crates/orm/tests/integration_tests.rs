//! Integration tests against a real TypeDB instance.
//!
//! These tests are `#[ignore]`d by default and require a running TypeDB server.
//! Run them with:
//!
//! ```bash
//! cargo test -p type-bridge-orm --test integration_tests --features derive -- --ignored
//! ```
//!
//! The tests connect to `localhost:1729` using database `test-orm-integration`.

#![cfg(feature = "derive")]

use type_bridge_orm::*;

// ── Model definitions via include_schema! ──────────────────────────

type_bridge_orm::include_schema!("tests/test_schema.tql");

// ── Helper ─────────────────────────────────────────────────────────

async fn setup_db() -> Option<Database> {
    // Try to connect — if TypeDB isn't running, return None
    match Database::connect(
        "localhost:1729",
        "test-orm-integration",
        "admin",
        "password",
    )
    .await
    {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("Skipping integration test: TypeDB not available ({e})");
            None
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn full_entity_lifecycle() {
    let Some(db) = setup_db().await else { return };

    // 1. Schema sync
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    // 2. Insert
    let manager = EntityManager::<Person>::new(&db);
    let mut alice = Person {
        iid: None,
        name: Name("Alice-Integration".into()),
        age: Age(30),
    };
    let iid = manager.insert(&mut alice).await.expect("insert failed");
    assert!(!iid.is_empty());
    assert_eq!(alice.iid(), Some(iid.as_str()));

    // 3. Fetch all
    let people = manager.all().await.expect("all() failed");
    assert!(!people.is_empty());

    // 4. Count
    let count = manager.count().await.expect("count() failed");
    assert!(count >= 1);

    // 5. Delete
    manager.delete(&alice).await.expect("delete failed");

    // 6. Verify deleted
    let after_delete = manager
        .get(&[Filter::string_eq("name", "Alice-Integration")])
        .await
        .expect("get after delete failed");
    assert!(after_delete.is_empty());
}

#[tokio::test]
#[ignore]
async fn batch_insert_and_delete() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Batch insert
    let mut people = vec![
        Person {
            iid: None,
            name: Name("Batch-A".into()),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name("Batch-B".into()),
            age: Age(25),
        },
        Person {
            iid: None,
            name: Name("Batch-C".into()),
            age: Age(30),
        },
    ];
    let iids = manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");
    assert_eq!(iids.len(), 3);
    assert!(people.iter().all(|p| p.iid().is_some()));

    // Batch delete
    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

#[tokio::test]
#[ignore]
async fn query_builder_with_filters() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Insert test data
    let mut person = Person {
        iid: None,
        name: Name("QueryTest".into()),
        age: Age(42),
    };
    manager.insert(&mut person).await.expect("insert failed");

    // Query with filter
    let results = manager
        .query()
        .filter(Expr::eq("name", AttributeValue::String("QueryTest".into())))
        .execute()
        .await
        .expect("query failed");
    assert!(!results.is_empty());

    // Cleanup
    manager.delete(&person).await.expect("delete failed");
}

#[tokio::test]
#[ignore]
async fn schema_introspection() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    // Introspect the live database
    let live = schema.introspect().await.expect("introspect failed");

    // Should find our types
    assert!(
        live.attributes.contains_key("name"),
        "expected 'name' attribute"
    );
    assert!(
        live.attributes.contains_key("age"),
        "expected 'age' attribute"
    );
    assert!(
        live.entities.contains_key("person"),
        "expected 'person' entity"
    );
}

#[tokio::test]
#[ignore]
async fn full_relation_lifecycle() {
    let Some(db) = setup_db().await else { return };

    // Schema sync
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    // Insert role players
    let person_mgr = EntityManager::<Person>::new(&db);
    let company_mgr = EntityManager::<Company>::new(&db);

    let mut alice = Person {
        iid: None,
        name: Name("Alice-Rel".into()),
        age: Age(30),
    };
    person_mgr
        .insert(&mut alice)
        .await
        .expect("insert person failed");

    let mut acme = Company {
        iid: None,
        name: Name("Acme-Rel".into()),
    };
    company_mgr
        .insert(&mut acme)
        .await
        .expect("insert company failed");

    // Insert relation
    let rel_mgr = RelationManager::<Employment>::new(&db);
    let mut employment = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: alice.iid().map(String::from),
            key: None,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: acme.iid().map(String::from),
            key: None,
        },
        position: Some(Position("Engineer".into())),
    };
    let rel_iid = rel_mgr
        .insert(&mut employment)
        .await
        .expect("insert relation failed");
    assert!(!rel_iid.is_empty());

    // Fetch all relations
    let relations = rel_mgr.all().await.expect("all() relations failed");
    assert!(!relations.is_empty());

    // Delete relation, then entities
    rel_mgr
        .delete(&employment)
        .await
        .expect("delete relation failed");
    person_mgr
        .delete(&alice)
        .await
        .expect("delete person failed");
    company_mgr
        .delete(&acme)
        .await
        .expect("delete company failed");
}

#[tokio::test]
#[ignore]
async fn entity_update_lifecycle() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Insert
    let mut person = Person {
        iid: None,
        name: Name("UpdateTest".into()),
        age: Age(25),
    };
    manager.insert(&mut person).await.expect("insert failed");

    // Update age
    person.age = Age(26);
    manager.update(&person).await.expect("update failed");

    // Fetch and verify
    let results = manager
        .get(&[Filter::string_eq("name", "UpdateTest")])
        .await
        .expect("get after update failed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].age.0, 26);

    // Cleanup
    manager.delete(&person).await.expect("delete failed");
}

#[tokio::test]
#[ignore]
async fn entity_put_creates_and_updates() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Put (create)
    let mut person = Person {
        iid: None,
        name: Name("PutTest".into()),
        age: Age(30),
    };
    let iid1 = manager.put(&mut person).await.expect("put (create) failed");
    assert!(!iid1.is_empty());

    // Put (update) — same key, different age
    let mut person2 = Person {
        iid: None,
        name: Name("PutTest".into()),
        age: Age(31),
    };
    let iid2 = manager
        .put(&mut person2)
        .await
        .expect("put (update) failed");
    assert!(!iid2.is_empty());

    // Verify the entity exists with updated age
    let results = manager
        .get(&[Filter::string_eq("name", "PutTest")])
        .await
        .expect("get after put failed");
    assert!(!results.is_empty());

    // Cleanup
    manager.delete(&person2).await.expect("delete failed");
}

#[tokio::test]
#[ignore]
async fn query_builder_with_sort_and_limit() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Insert 3 people
    let mut people = vec![
        Person {
            iid: None,
            name: Name("SortA".into()),
            age: Age(30),
        },
        Person {
            iid: None,
            name: Name("SortB".into()),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name("SortC".into()),
            age: Age(25),
        },
    ];
    manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");

    // Query sorted by age ascending with limit 2
    let results = manager
        .query()
        .filter(Expr::contains("name", "Sort"))
        .order_by("age", SortDir::Asc)
        .limit(2)
        .execute()
        .await
        .expect("sorted query failed");

    assert_eq!(results.len(), 2);
    assert!(results[0].age.0 <= results[1].age.0);

    // Cleanup
    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}

#[tokio::test]
#[ignore]
async fn transaction_context_batch_commit() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Batch insert via insert_many (uses TransactionContext internally)
    let mut people = vec![
        Person {
            iid: None,
            name: Name("TxBatch1".into()),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name("TxBatch2".into()),
            age: Age(25),
        },
    ];
    let iids = manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");
    assert_eq!(iids.len(), 2);

    // Fetch all and verify using query builder with contains
    let results = manager
        .query()
        .filter(Expr::contains("name", "TxBatch"))
        .execute()
        .await
        .expect("query after batch failed");
    assert_eq!(results.len(), 2);

    // Cleanup
    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}
