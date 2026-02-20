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
    match Database::connect("localhost:1729", "test-orm-integration", "admin", "password").await {
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
    schema.sync_schema(true, false).await.expect("schema sync failed");

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
    schema.sync_schema(true, false).await.expect("schema sync failed");

    let manager = EntityManager::<Person>::new(&db);

    // Batch insert
    let mut people = vec![
        Person { iid: None, name: Name("Batch-A".into()), age: Age(20) },
        Person { iid: None, name: Name("Batch-B".into()), age: Age(25) },
        Person { iid: None, name: Name("Batch-C".into()), age: Age(30) },
    ];
    let iids = manager.insert_many(&mut people).await.expect("insert_many failed");
    assert_eq!(iids.len(), 3);
    assert!(people.iter().all(|p| p.iid().is_some()));

    // Batch delete
    manager.delete_many(&people).await.expect("delete_many failed");
}

#[tokio::test]
#[ignore]
async fn query_builder_with_filters() {
    let Some(db) = setup_db().await else { return };

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema.sync_schema(true, false).await.expect("schema sync failed");

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
    schema.sync_schema(true, false).await.expect("schema sync failed");

    // Introspect the live database
    let live = schema.introspect().await.expect("introspect failed");

    // Should find our types
    assert!(live.attributes.contains_key("name"), "expected 'name' attribute");
    assert!(live.attributes.contains_key("age"), "expected 'age' attribute");
    assert!(live.entities.contains_key("person"), "expected 'person' entity");
}
