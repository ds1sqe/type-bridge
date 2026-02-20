//! Integration tests for `EntityManager` using a mock backend.

use std::sync::{Arc, Mutex};

use type_bridge_orm::*;

// ── Test entity ──────────────────────────────────────────────────────

define_attribute!(Name, "name", "string");
define_attribute!(Age, "age", "long");

#[derive(Debug)]
struct Person {
    iid: Option<String>,
    name: Name,
    age: Age,
}

impl TypeBridgeEntity for Person {
    const TYPE_NAME: &'static str = "person";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        &[
            OwnedAttributeInfo {
                attr_name: "name",
                value_type: ValueType::String,
                annotations: &[Annotation::Key],
            },
            OwnedAttributeInfo {
                attr_name: "age",
                value_type: ValueType::Long,
                annotations: &[],
            },
        ]
    }

    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }

    fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
        vec![
            ("name", self.name.to_value()),
            ("age", self.age.to_value()),
        ]
    }

    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let name = doc
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| OrmError::Hydration {
                type_name: "person".into(),
                message: "missing name".into(),
            })?;
        let age = doc
            .get("age")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| OrmError::Hydration {
                type_name: "person".into(),
                message: "missing age".into(),
            })?;
        Ok(Person {
            iid: None,
            name: Name(name.to_string()),
            age: Age(age),
        })
    }
}

// ── Mock backend ─────────────────────────────────────────────────────

use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};

/// Records queries and returns pre-configured results.
struct MockBackend {
    responses: Arc<Mutex<Vec<QueryResult>>>,
    queries: Arc<Mutex<Vec<String>>>,
}

impl MockBackend {
    fn new(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl DriverBackend for MockBackend {
    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        let responses = Arc::clone(&self.responses);
        let queries = Arc::clone(&self.queries);
        Box::pin(async move {
            Ok(Box::new(MockTransaction {
                responses,
                queries,
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct MockTransaction {
    responses: Arc<Mutex<Vec<QueryResult>>>,
    queries: Arc<Mutex<Vec<String>>>,
}

impl TransactionOps for MockTransaction {
    fn query(
        &mut self,
        typeql: &str,
    ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.queries.lock().unwrap().push(typeql.to_string());
        let result = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(QueryResult::Ok);
        Box::pin(async move { Ok(result) })
    }

    fn commit(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn insert_sets_iid_and_returns_it() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": "0x123abc"
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
    };

    let manager = EntityManager::<Person>::new(&db);
    let iid = manager.insert(&mut person).await.unwrap();

    assert_eq!(iid, "0x123abc");
    assert_eq!(person.iid(), Some("0x123abc"));
}

#[tokio::test]
async fn get_returns_hydrated_entities() {
    let doc = serde_json::json!({
        "_iid": "0xaaa",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Bob", "type": {"value_type": "string"}}],
            "age": [{"value": 25, "type": {"value_type": "long"}}]
        }
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![doc])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let people = manager.get(&[]).await.unwrap();

    assert_eq!(people.len(), 1);
    assert_eq!(people[0].name.0, "Bob");
    assert_eq!(people[0].age.0, 25);
    assert_eq!(people[0].iid(), Some("0xaaa"));
}

#[tokio::test]
async fn get_with_filters() {
    let doc = serde_json::json!({
        "_iid": "0xbbb",
        "attributes": {
            "name": [{"value": "Charlie"}],
            "age": [{"value": 40}]
        }
    });
    let _backend = MockBackend::new(vec![QueryResult::Documents(vec![doc])]);
    let queries = {
        let b = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
            "_iid": "0xbbb",
            "attributes": {
                "name": [{"value": "Charlie"}],
                "age": [{"value": 40}]
            }
        })])]);
        let q = Arc::clone(&b.queries);
        let db = Database::with_backend(Box::new(b), "testdb");
        let manager = EntityManager::<Person>::new(&db);
        let filters = [Filter::string_eq("name", "Charlie")];
        let _ = manager.get(&filters).await.unwrap();
        q
    };

    let recorded = queries.lock().unwrap();
    assert!(!recorded.is_empty());
    assert!(recorded[0].contains("has name"));
}

#[tokio::test]
async fn get_one_returns_single() {
    let doc = serde_json::json!({
        "_iid": "0xccc",
        "attributes": {
            "name": [{"value": "Diana"}],
            "age": [{"value": 35}]
        }
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![doc])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let person = manager
        .get_one(&[Filter::string_eq("name", "Diana")])
        .await
        .unwrap();

    assert_eq!(person.name.0, "Diana");
    assert_eq!(person.age.0, 35);
}

#[tokio::test]
async fn get_one_not_found() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager
        .get_one(&[Filter::string_eq("name", "Nobody")])
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::NotFound(msg) => assert!(msg.contains("person")),
        other => panic!("Expected NotFound, got: {other}"),
    }
}

#[tokio::test]
async fn all_delegates_to_get() {
    let docs = vec![
        serde_json::json!({
            "_iid": "0x001",
            "attributes": { "name": [{"value": "Alice"}], "age": [{"value": 30}] }
        }),
        serde_json::json!({
            "_iid": "0x002",
            "attributes": { "name": [{"value": "Bob"}], "age": [{"value": 25}] }
        }),
    ];
    let backend = MockBackend::new(vec![QueryResult::Documents(docs)]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let people = manager.all().await.unwrap();

    assert_eq!(people.len(), 2);
}

#[tokio::test]
async fn delete_executes_query() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let person = Person {
        iid: Some("0xddd".into()),
        name: Name("Eve".into()),
        age: Age(28),
    };

    let manager = EntityManager::<Person>::new(&db);
    manager.delete(&person).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert!(!recorded.is_empty());
    assert!(recorded[0].contains("match"));
    assert!(recorded[0].contains("delete"));
}

#[tokio::test]
async fn count_returns_value() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 42}),
    ])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let count = manager.count().await.unwrap();

    assert_eq!(count, 42);
}

#[tokio::test]
async fn count_with_filters() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 5}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let filters = [Filter::long_eq("age", 30)];
    let count = manager.count_with_filters(&filters).await.unwrap();

    assert_eq!(count, 5);
    let recorded = queries.lock().unwrap();
    assert!(recorded[0].contains("has age"));
    assert!(recorded[0].contains("reduce"));
}

#[tokio::test]
async fn hydration_with_flat_document() {
    // Test documents without the "attributes" wrapper
    let doc = serde_json::json!({
        "_iid": "0xflat",
        "name": "Flat",
        "age": 99
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![doc])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let people = manager.all().await.unwrap();

    assert_eq!(people.len(), 1);
    assert_eq!(people[0].name.0, "Flat");
    assert_eq!(people[0].age.0, 99);
    assert_eq!(people[0].iid(), Some("0xflat"));
}

#[tokio::test]
async fn insert_with_wrapped_iid_response() {
    // TypeDB may return IID wrapped in {"value": "..."}
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": {"value": "0xwrapped"}
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = Person {
        iid: None,
        name: Name("Wrapped".into()),
        age: Age(1),
    };

    let manager = EntityManager::<Person>::new(&db);
    let iid = manager.insert(&mut person).await.unwrap();

    assert_eq!(iid, "0xwrapped");
}

#[tokio::test]
async fn update_executes_query() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let person = Person {
        iid: Some("0xaaa".into()),
        name: Name("Alice".into()),
        age: Age(31),
    };

    let manager = EntityManager::<Person>::new(&db);
    manager.update(&person).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert!(!recorded.is_empty());
    assert!(recorded[0].contains("match"));
    assert!(recorded[0].contains("update"));
    assert!(recorded[0].contains("has age"));
}

#[tokio::test]
async fn put_sets_iid_and_returns_it() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": "0xput1"
    })])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
    };

    let manager = EntityManager::<Person>::new(&db);
    let iid = manager.put(&mut person).await.unwrap();

    assert_eq!(iid, "0xput1");
    assert_eq!(person.iid(), Some("0xput1"));
}

#[tokio::test]
async fn put_query_contains_put_keyword() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({
        "iid": "0xput2"
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut person = Person {
        iid: None,
        name: Name("Bob".into()),
        age: Age(25),
    };

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager.put(&mut person).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert!(recorded[0].contains("put"));
    assert!(!recorded[0].starts_with("insert"));
}

// ── Batch operation tests ───────────────────────────────────────────

#[tokio::test]
async fn insert_many_sets_iids() {
    // Responses are popped (LIFO), so push in reverse order
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xbatch3"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xbatch2"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xbatch1"})]),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut entities = vec![
        Person { iid: None, name: Name("Alice".into()), age: Age(30) },
        Person { iid: None, name: Name("Bob".into()), age: Age(25) },
        Person { iid: None, name: Name("Carol".into()), age: Age(35) },
    ];

    let manager = EntityManager::<Person>::new(&db);
    let iids = manager.insert_many(&mut entities).await.unwrap();

    assert_eq!(iids.len(), 3);
    assert_eq!(iids[0], "0xbatch1");
    assert_eq!(iids[1], "0xbatch2");
    assert_eq!(iids[2], "0xbatch3");
    assert_eq!(entities[0].iid(), Some("0xbatch1"));
    assert_eq!(entities[1].iid(), Some("0xbatch2"));
    assert_eq!(entities[2].iid(), Some("0xbatch3"));
}

#[tokio::test]
async fn insert_many_empty_slice() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut entities: Vec<Person> = vec![];
    let manager = EntityManager::<Person>::new(&db);
    let iids = manager.insert_many(&mut entities).await.unwrap();
    assert!(iids.is_empty());
}

#[tokio::test]
async fn delete_many_executes_all_queries() {
    // 2 deletes = 2 OK responses (LIFO)
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let entities = vec![
        Person { iid: Some("0x001".into()), name: Name("Alice".into()), age: Age(30) },
        Person { iid: Some("0x002".into()), name: Name("Bob".into()), age: Age(25) },
    ];

    let manager = EntityManager::<Person>::new(&db);
    manager.delete_many(&entities).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("delete"));
    assert!(recorded[1].contains("delete"));
}

#[tokio::test]
async fn update_many_executes_all_queries() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let entities = vec![
        Person { iid: Some("0x001".into()), name: Name("Alice".into()), age: Age(31) },
        Person { iid: Some("0x002".into()), name: Name("Bob".into()), age: Age(26) },
    ];

    let manager = EntityManager::<Person>::new(&db);
    manager.update_many(&entities).await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("update"));
    assert!(recorded[1].contains("update"));
}
