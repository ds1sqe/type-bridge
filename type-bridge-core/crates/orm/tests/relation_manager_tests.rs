//! Integration tests for `RelationManager` with a mock backend.

mod common;

use std::sync::Arc;

use common::*;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

#[tokio::test]
async fn insert_sets_iid_and_returns_it() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": "0xrel1"
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut employment = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );

    let manager = RelationManager::<Employment>::new(&db);
    let iid = manager.insert(&mut employment).await.unwrap();

    assert_eq!(iid, "0xrel1");
    assert_eq!(employment.iid(), Some("0xrel1"));
}

#[tokio::test]
async fn insert_with_wrapped_iid_response() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": {"value": "0xwrapped"}
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut employment = make_employment(
        None,
        Some("0xp1"),
        None,
        Some("0xc1"),
        None,
        None,
    );

    let manager = RelationManager::<Employment>::new(&db);
    let iid = manager.insert(&mut employment).await.unwrap();
    assert_eq!(iid, "0xwrapped");
}

#[tokio::test]
async fn get_returns_hydrated_relations() {
    let doc = serde_json::json!({
        "_iid": "0xrel1",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        }
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![doc])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let results = manager.all().await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].iid(), Some("0xrel1"));
    assert_eq!(results[0].position.as_deref(), Some("Engineer"));
}

#[tokio::test]
async fn get_with_filters() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "_iid": "0xrel2",
        "attributes": {
            "position": [{"value": "Manager"}]
        }
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let filters = [Filter::string_eq("position", "Manager")];
    let results = manager.get(&filters).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].position.as_deref(), Some("Manager"));
    let recorded = queries.lock().unwrap();
    assert!(recorded[0].contains("has position"));
}

#[tokio::test]
async fn get_one_returns_single() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "_iid": "0xrel3",
        "attributes": {
            "position": [{"value": "CTO"}]
        }
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let result = manager
        .get_one(&[Filter::string_eq("position", "CTO")])
        .await
        .unwrap();

    assert_eq!(result.iid(), Some("0xrel3"));
    assert_eq!(result.position.as_deref(), Some("CTO"));
}

#[tokio::test]
async fn get_one_not_found() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let result = manager
        .get_one(&[Filter::string_eq("position", "None")])
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::NotFound(msg) => assert!(msg.contains("employment")),
        other => panic!("Expected NotFound, got: {other}"),
    }
}

#[tokio::test]
async fn all_returns_multiple() {
    let docs = vec![
        serde_json::json!({
            "_iid": "0xr1",
            "attributes": { "position": [{"value": "Engineer"}] }
        }),
        serde_json::json!({
            "_iid": "0xr2",
            "attributes": { "position": [{"value": "Manager"}] }
        }),
    ];
    let backend = MockBackend::new(vec![QueryResult::Documents(docs)]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let results = manager.all().await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn delete_executes_query() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let employment = make_employment(Some("0xrel1"), None, None, None, None, None);

    let manager = RelationManager::<Employment>::new(&db);
    manager.delete(&employment).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert!(!recorded.is_empty());
    assert!(recorded[0].contains("match"));
    assert!(recorded[0].contains("delete"));
}

#[tokio::test]
async fn count_returns_value() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 5}),
    ])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let count = manager.count().await.unwrap();
    assert_eq!(count, 5);
}

#[tokio::test]
async fn count_with_filters() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 2}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Employment>::new(&db);
    let count = manager
        .count_with_filters(&[Filter::string_eq("position", "Engineer")])
        .await
        .unwrap();

    assert_eq!(count, 2);
    let recorded = queries.lock().unwrap();
    assert!(recorded[0].contains("has position"));
    assert!(recorded[0].contains("reduce"));
}

// ── Trait default method tests ──────────────────────────────────────

#[test]
fn relation_type_name() {
    assert_eq!(Employment::TYPE_NAME, "employment");
}

#[test]
fn relation_role_info() {
    let roles = Employment::role_info();
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[0].role_name, "employee");
    assert_eq!(roles[0].player_type_name, "person");
    assert_eq!(roles[1].role_name, "employer");
    assert_eq!(roles[1].player_type_name, "company");
}

#[test]
fn relation_owned_attributes() {
    let attrs = Employment::owned_attributes();
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].attr_name, "position");
    assert_eq!(attrs[0].value_type, ValueType::String);
}

#[test]
fn insert_clauses_produce_match_and_insert() {
    let employment = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        Some("0xcomp1"),
        None,
        Some("Engineer"),
    );

    let clauses = employment.to_insert_with_iid_fetch("$r");
    // Should have: match (for role players), insert (relation), fetch (IID)
    assert!(clauses.len() >= 3);
}

#[test]
fn match_pattern_with_iid() {
    let employment = make_employment(Some("0xrel1"), None, None, None, None, None);
    let patterns = employment.to_match_pattern("$r");
    // Should produce a single pattern matching by IID
    assert_eq!(patterns.len(), 1);
}

#[test]
fn match_pattern_without_iid_uses_role_players() {
    let employment = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );

    let patterns = employment.to_match_pattern("$r");
    // Should include: 2 role player match patterns + 1 relation pattern
    assert!(patterns.len() >= 3);
}

#[test]
fn hydration_from_flat_document() {
    let mut map = serde_json::Map::new();
    map.insert("position".into(), serde_json::json!("Engineer"));
    let emp = Employment::from_document(&map).unwrap();
    assert_eq!(emp.position.as_deref(), Some("Engineer"));
}

#[test]
fn hydration_with_missing_optional_attrs() {
    let map = serde_json::Map::new();
    let emp = Employment::from_document(&map).unwrap();
    assert!(emp.position.is_none());
}

#[test]
fn hydration_with_nested_document() {
    let doc = serde_json::json!({
        "_iid": "0xrel1",
        "attributes": {
            "position": [{"value": "Engineer"}]
        }
    });
    // Simulate what hydrate_relation does
    let result = type_bridge_orm::session::backend::QueryResult::Documents(vec![doc]);
    if let QueryResult::Documents(docs) = result {
        use type_bridge_orm::manager::hydration::hydrate_relation;
        let emp = hydrate_relation::<Employment>(&docs[0]).unwrap();
        assert_eq!(emp.iid(), Some("0xrel1"));
        assert_eq!(emp.position.as_deref(), Some("Engineer"));
    }
}

// ── Batch operation tests ───────────────────────────────────────────

#[tokio::test]
async fn insert_many_sets_iids() {
    // Responses popped LIFO
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xbr2"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xbr1"})]),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut relations = vec![
        make_employment(
            None,
            None,
            Some(("name", AttributeValue::String("Alice".into()))),
            None,
            Some(("name", AttributeValue::String("Acme".into()))),
            Some("Engineer"),
        ),
        make_employment(
            None,
            None,
            Some(("name", AttributeValue::String("Bob".into()))),
            None,
            Some(("name", AttributeValue::String("Acme".into()))),
            Some("Manager"),
        ),
    ];

    let manager = RelationManager::<Employment>::new(&db);
    let iids = manager.insert_many(&mut relations).await.unwrap();

    assert_eq!(iids.len(), 2);
    assert_eq!(iids[0], "0xbr1");
    assert_eq!(iids[1], "0xbr2");
    assert_eq!(relations[0].iid(), Some("0xbr1"));
    assert_eq!(relations[1].iid(), Some("0xbr2"));
}

#[tokio::test]
async fn insert_many_empty_slice() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut relations: Vec<Employment> = vec![];
    let manager = RelationManager::<Employment>::new(&db);
    let iids = manager.insert_many(&mut relations).await.unwrap();
    assert!(iids.is_empty());
}

#[tokio::test]
async fn delete_many_executes_all_queries() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let relations = vec![
        make_employment(Some("0xr1"), None, None, None, None, Some("Engineer")),
        make_employment(Some("0xr2"), None, None, None, None, Some("Manager")),
    ];

    let manager = RelationManager::<Employment>::new(&db);
    manager.delete_many(&relations).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("delete"));
    assert!(recorded[1].contains("delete"));
}
