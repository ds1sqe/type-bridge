//! Integration tests for `TransactionContext`.

mod common;

use std::sync::Arc;

use common::*;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

// ── Basic TransactionContext tests ──────────────────────────────────

#[tokio::test]
async fn query_executes_on_shared_transaction() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![
        serde_json::json!({"name": "Alice", "age": 30}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let result = tx.query("match $p isa person; fetch $p: name, age;").await.unwrap();

    // Verify the query was recorded.
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("match"));

    // Verify we got the expected result back.
    match result {
        QueryResult::Documents(docs) => {
            assert_eq!(docs.len(), 1);
            assert_eq!(docs[0]["name"], "Alice");
        }
        other => panic!("Expected Documents, got: {other:?}"),
    }
}

#[tokio::test]
async fn commit_succeeds() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let result = tx.commit().await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn tx_type_returns_configured_type() {
    // Test Read type.
    let backend_read = MockBackend::new(vec![]);
    let db_read = Database::with_backend(Box::new(backend_read), "testdb");
    let tx_read = db_read.transaction_context(TxType::Read).await.unwrap();
    assert_eq!(tx_read.tx_type(), TxType::Read);

    // Test Write type.
    let backend_write = MockBackend::new(vec![]);
    let db_write = Database::with_backend(Box::new(backend_write), "testdb");
    let tx_write = db_write.transaction_context(TxType::Write).await.unwrap();
    assert_eq!(tx_write.tx_type(), TxType::Write);
}

#[tokio::test]
async fn clone_shares_same_transaction() {
    // Two queries: second popped first (LIFO).
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let tx_clone = tx.clone();

    // Query on the original.
    tx.query("insert $p isa person, has name 'Alice';")
        .await
        .unwrap();

    // Query on the clone -- should go to the same underlying transaction.
    tx_clone
        .query("insert $p isa person, has name 'Bob';")
        .await
        .unwrap();

    // Both queries should be recorded in the shared queries vec.
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("Alice"));
    assert!(recorded[1].contains("Bob"));
}

#[tokio::test]
async fn multiple_queries_in_sequence() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let tx = db.transaction_context(TxType::Write).await.unwrap();

    tx.query("insert $a isa person, has name 'Alice';")
        .await
        .unwrap();
    tx.query("insert $b isa person, has name 'Bob';")
        .await
        .unwrap();
    tx.query("insert $c isa person, has name 'Carol';")
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].contains("Alice"));
    assert!(recorded[1].contains("Bob"));
    assert!(recorded[2].contains("Carol"));
}

#[tokio::test]
async fn query_and_commit_sequence() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let tx = db.transaction_context(TxType::Write).await.unwrap();

    tx.query("insert $a isa person, has name 'Alice';")
        .await
        .unwrap();
    tx.query("insert $b isa person, has name 'Bob';")
        .await
        .unwrap();

    // Commit should succeed after queries.
    let commit_result = tx.commit().await;
    assert!(commit_result.is_ok());

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
}

// ── Batch operation tests using TransactionContext internally ────────

#[tokio::test]
async fn batch_insert_many_uses_transaction_context() {
    // insert_many opens a TransactionContext internally and runs one query per entity.
    // Responses are popped LIFO, so push in reverse order.
    let backend = MockBackend::new(vec![
        insert_response("0x003"),
        insert_response("0x002"),
        insert_response("0x001"),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut entities = vec![
        make_person("Alice", 30),
        make_person("Bob", 25),
        make_person("Carol", 35),
    ];

    let manager = EntityManager::<Person>::new(&db);
    let iids = manager.insert_many(&mut entities).await.unwrap();

    assert_eq!(iids.len(), 3);
    assert_eq!(iids[0], "0x001");
    assert_eq!(iids[1], "0x002");
    assert_eq!(iids[2], "0x003");

    // All 3 insert queries should have been routed through the same
    // shared queries vec (i.e., the same underlying transaction).
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].contains("insert"));
    assert!(recorded[1].contains("insert"));
    assert!(recorded[2].contains("insert"));
}

#[tokio::test]
async fn batch_delete_many_uses_transaction_context() {
    // delete_many opens a TransactionContext internally and runs one query per entity.
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let entities = vec![
        make_person_with_iid("Alice", 30, "0x001"),
        make_person_with_iid("Bob", 25, "0x002"),
        make_person_with_iid("Carol", 35, "0x003"),
    ];

    let manager = EntityManager::<Person>::new(&db);
    manager.delete_many(&entities).await.unwrap();

    // All 3 delete queries should have been routed through the shared
    // queries vec (same underlying transaction via TransactionContext).
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].contains("delete"));
    assert!(recorded[1].contains("delete"));
    assert!(recorded[2].contains("delete"));
}
