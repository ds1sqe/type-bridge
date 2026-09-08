//! Integration tests for ORM error propagation.
//!
//! Verifies that backend failures, wrong response types, and edge-case
//! responses are correctly surfaced as the appropriate [`OrmError`] variants.

mod common;

use common::*;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

// ── FailingMockBackend tests ────────────────────────────────────────

#[tokio::test]
async fn entity_insert_backend_failure_propagates() {
    let backend = FailingMockBackend::new("simulated failure");
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = make_person("Alice", 30);
    let manager = EntityManager::<Person>::new(&db);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Transaction(msg) => assert!(msg.contains("simulated failure")),
        other => panic!("Expected Transaction error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_get_backend_failure_propagates() {
    let backend = FailingMockBackend::new("simulated failure");
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager.get(&[]).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Transaction(msg) => assert!(msg.contains("simulated failure")),
        other => panic!("Expected Transaction error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_delete_backend_failure_propagates() {
    let backend = FailingMockBackend::new("simulated failure");
    let db = Database::with_backend(Box::new(backend), "testdb");

    let person = make_person_with_iid("Alice", 30, "0x123");
    let manager = EntityManager::<Person>::new(&db);
    let result = manager.delete(&person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Transaction(msg) => assert!(msg.contains("simulated failure")),
        other => panic!("Expected Transaction error, got: {other}"),
    }
}

#[tokio::test]
async fn relation_insert_backend_failure_propagates() {
    let backend = FailingMockBackend::new("simulated failure");
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut employment = make_employment(
        None,
        Some("0xemp"),
        None,
        Some("0xer"),
        None,
        Some("Engineer"),
    );
    let manager = RelationManager::<Employment>::new(&db);
    let result = manager.insert(&mut employment).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Transaction(msg) => assert!(msg.contains("simulated failure")),
        other => panic!("Expected Transaction error, got: {other}"),
    }
}

#[tokio::test]
async fn relation_get_backend_failure_propagates() {
    let backend = FailingMockBackend::new("simulated failure");
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let result = manager.get(&[]).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Transaction(msg) => assert!(msg.contains("simulated failure")),
        other => panic!("Expected Transaction error, got: {other}"),
    }
}

// ── Wrong response type tests ───────────────────────────────────────

#[tokio::test]
async fn entity_insert_returns_ok_instead_of_documents() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = make_person("Alice", 30);
    let manager = EntityManager::<Person>::new(&db);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "person");
            assert!(
                message.contains("Ok"),
                "Expected message mentioning 'Ok', got: {message}"
            );
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_insert_returns_rows_instead_of_documents() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$count": 1
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = make_person("Alice", 30);
    let manager = EntityManager::<Person>::new(&db);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "person");
            assert!(
                message.contains("Rows"),
                "Expected message mentioning 'Rows', got: {message}"
            );
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_insert_empty_documents_response() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = make_person("Alice", 30);
    let manager = EntityManager::<Person>::new(&db);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "person");
            assert!(
                message.contains("no documents") || message.contains("No"),
                "Expected message about empty documents, got: {message}"
            );
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_count_wrong_response_type() {
    // count expects Rows, but we return Documents
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager.count().await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "count");
            assert!(
                message.contains("Documents"),
                "Expected message mentioning 'Documents', got: {message}"
            );
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}

#[tokio::test]
async fn entity_get_one_multiple_results() {
    // get_one expects exactly 1 result; returning 2 should produce a Hydration error
    let docs = vec![
        serde_json::json!({
            "_iid": "0x001",
            "attributes": {
                "name": [{"value": "Alice"}],
                "age": [{"value": 30}]
            }
        }),
        serde_json::json!({
            "_iid": "0x002",
            "attributes": {
                "name": [{"value": "Bob"}],
                "age": [{"value": 25}]
            }
        }),
    ];
    let backend = MockBackend::new(vec![QueryResult::Documents(docs)]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager.get_one(&[]).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "person");
            assert!(
                message.contains("Expected 1 result, got 2"),
                "Expected message about multiple results, got: {message}"
            );
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}
