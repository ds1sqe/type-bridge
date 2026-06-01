//! Integration tests for `EntityQuery` and `RelationQuery` using a mock backend.

use std::sync::{Arc, Mutex};

use type_bridge_orm::*;

// ── Test entity ──────────────────────────────────────────────────────

define_attribute!(Name, "name", "string");
define_attribute!(Age, "age", "long");
define_attribute!(Score, "score", "double");

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
        vec![("name", self.name.to_value()), ("age", self.age.to_value())]
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

// ── Test relation ──────────────────────────────────────────────────────

define_attribute!(Since, "since", "string");

#[derive(Debug)]
struct Friendship {
    iid: Option<String>,
    since: Since,
}

impl TypeBridgeRelation for Friendship {
    const TYPE_NAME: &'static str = "friendship";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        &[OwnedAttributeInfo {
            attr_name: "since",
            value_type: ValueType::String,
            annotations: &[],
        }]
    }

    fn role_info() -> &'static [RoleInfo] {
        &[
            RoleInfo {
                role_name: "friend",
                player_type_name: "person",
            },
            RoleInfo {
                role_name: "friend",
                player_type_name: "person",
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
        vec![("since", self.since.to_value())]
    }

    fn to_role_player_refs(&self) -> Vec<RolePlayerRef> {
        vec![]
    }

    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let since =
            doc.get("since")
                .and_then(|v| v.as_str())
                .ok_or_else(|| OrmError::Hydration {
                    type_name: "friendship".into(),
                    message: "missing since".into(),
                })?;
        Ok(Friendship {
            iid: None,
            since: Since(since.to_string()),
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
            Ok(Box::new(MockTransaction { responses, queries }) as Box<dyn TransactionOps>)
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
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
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

    fn rollback(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── Helper ──────────────────────────────────────────────────────────

fn person_doc(name: &str, age: i64) -> serde_json::Value {
    serde_json::json!({
        "_iid": format!("0x{name}"),
        "attributes": {
            "name": [{"value": name}],
            "age": [{"value": age}]
        }
    })
}

fn friendship_doc(since: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": format!("0xfr_{since}"),
        "attributes": {
            "since": [{"value": since}]
        }
    })
}

// ── EntityQuery tests ────────────────────────────────────────────────

#[tokio::test]
async fn query_with_gt_filter() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::gt("age", AttributeValue::Long(18)))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has age"), "should contain 'has age': {q}");
    assert!(q.contains("> 18"), "should contain '> 18': {q}");
}

#[tokio::test]
async fn query_with_string_contains() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::contains("name", "Ali"))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has name"), "should contain 'has name': {q}");
    assert!(
        q.contains(r#"contains "Ali""#),
        "should contain 'contains \"Ali\"': {q}"
    );
}

#[tokio::test]
async fn query_with_and_expression() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::And(vec![
            Expr::gte("age", AttributeValue::Long(18)),
            Expr::lte("age", AttributeValue::Long(65)),
        ]))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains(">= 18"), "should contain '>= 18': {q}");
    assert!(q.contains("<= 65"), "should contain '<= 65': {q}");
}

#[tokio::test]
async fn query_with_or_expression() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::Or(vec![
            Expr::eq("name", AttributeValue::String("Alice".into())),
            Expr::eq("name", AttributeValue::String("Bob".into())),
        ]))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("or"), "should contain 'or': {q}");
}

#[tokio::test]
async fn query_with_not_expression() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::not(Expr::eq(
            "name",
            AttributeValue::String("Bob".into()),
        )))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("not"), "should contain 'not': {q}");
}

#[tokio::test]
async fn query_with_sort() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![
        person_doc("Alice", 30),
        person_doc("Bob", 25),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .order_by("name", SortDir::Asc)
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("sort"), "should contain 'sort': {q}");
    assert!(q.contains("asc"), "should contain 'asc': {q}");
}

#[tokio::test]
async fn query_with_limit_offset() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager.query().limit(10).offset(5).execute().await.unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("limit 10"), "should contain 'limit 10': {q}");
    assert!(q.contains("offset 5"), "should contain 'offset 5': {q}");
}

#[tokio::test]
async fn query_execute_returns_entities() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![
        person_doc("Alice", 30),
        person_doc("Bob", 25),
    ])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let people = manager
        .query()
        .filter(Expr::gte("age", AttributeValue::Long(20)))
        .execute()
        .await
        .unwrap();

    assert_eq!(people.len(), 2);
    assert_eq!(people[0].name.0, "Alice");
    assert_eq!(people[1].name.0, "Bob");
}

#[tokio::test]
async fn query_first_returns_single() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let person = manager
        .query()
        .filter(Expr::eq("name", AttributeValue::String("Alice".into())))
        .first()
        .await
        .unwrap();

    assert!(person.is_some());
    assert_eq!(person.unwrap().name.0, "Alice");

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("limit 1"), "first() should set limit 1: {q}");
}

#[tokio::test]
async fn query_first_returns_none_when_empty() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let person = manager.query().first().await.unwrap();

    assert!(person.is_none());
}

#[tokio::test]
async fn query_count_returns_value() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 42}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let count = manager
        .query()
        .filter(Expr::gte("age", AttributeValue::Long(18)))
        .count()
        .await
        .unwrap();

    assert_eq!(count, 42);

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("reduce"), "should contain 'reduce': {q}");
    assert!(q.contains("count"), "should contain 'count': {q}");
}

#[tokio::test]
async fn query_aggregate_sum() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$sum": 1500}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager
        .query()
        .aggregate(&[Agg::Sum("age".to_string())])
        .await
        .unwrap();

    assert_eq!(result.get_i64("$sum"), Some(1500));

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("sum"), "should contain 'sum': {q}");
    assert!(q.contains("reduce"), "should contain 'reduce': {q}");
}

#[tokio::test]
async fn query_chain_all_modifiers() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let people = manager
        .query()
        .filter(Expr::gte("age", AttributeValue::Long(18)))
        .filter(Expr::lte("age", AttributeValue::Long(65)))
        .order_by("name", SortDir::Asc)
        .limit(10)
        .offset(20)
        .execute()
        .await
        .unwrap();

    assert_eq!(people.len(), 1);

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains(">= 18"), "should contain '>= 18': {q}");
    assert!(q.contains("<= 65"), "should contain '<= 65': {q}");
    assert!(q.contains("sort"), "should contain 'sort': {q}");
    assert!(q.contains("limit 10"), "should contain 'limit 10': {q}");
    assert!(q.contains("offset 20"), "should contain 'offset 20': {q}");
}

// ── RelationQuery tests ──────────────────────────────────────────────

#[tokio::test]
async fn relation_query_with_filter() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![friendship_doc(
        "2024-01-01",
    )])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let results = manager
        .query()
        .filter(Expr::eq(
            "since",
            AttributeValue::String("2024-01-01".into()),
        ))
        .execute()
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].since.0, "2024-01-01");

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has since"), "should contain 'has since': {q}");
    assert!(
        q.contains(r#"== "2024-01-01""#),
        "should contain '== \"2024-01-01\"': {q}"
    );
}

#[tokio::test]
async fn relation_query_with_sort_and_limit() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![
        friendship_doc("2024-01-01"),
        friendship_doc("2023-06-15"),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let _ = manager
        .query()
        .order_by("since", SortDir::Desc)
        .limit(5)
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("sort"), "should contain 'sort': {q}");
    assert!(q.contains("desc"), "should contain 'desc': {q}");
    assert!(q.contains("limit 5"), "should contain 'limit 5': {q}");
}

#[tokio::test]
async fn relation_query_count() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 7}),
    ])]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let count = manager.query().count().await.unwrap();

    assert_eq!(count, 7);
}

#[tokio::test]
async fn relation_query_first() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![friendship_doc(
        "2024-01-01",
    )])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let result = manager.query().first().await.unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().since.0, "2024-01-01");

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("limit 1"), "first() should set limit 1: {q}");
}

#[tokio::test]
async fn relation_query_aggregate() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$count": 10}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let result = manager.query().aggregate(&[Agg::Count]).await.unwrap();

    assert_eq!(result.count(), Some(10));

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("reduce"), "should contain 'reduce': {q}");
    assert!(q.contains("count"), "should contain 'count': {q}");
}

#[tokio::test]
async fn query_with_like_filter() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::like("name", "Ali.*"))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has name"), "should contain 'has name': {q}");
    assert!(
        q.contains(r#"like "Ali.*""#),
        "should contain 'like \"Ali.*\"': {q}"
    );
}

#[tokio::test]
async fn query_multiple_sort_fields() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .order_by("age", SortDir::Desc)
        .order_by("name", SortDir::Asc)
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("sort"), "should contain 'sort': {q}");
    assert!(q.contains("desc"), "should contain 'desc': {q}");
    assert!(q.contains("asc"), "should contain 'asc': {q}");
}

// ── Range / startswith / endswith tests ────────────────────────────

#[tokio::test]
async fn query_with_in_range() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::in_range(
            "age",
            AttributeValue::Long(20),
            AttributeValue::Long(40),
        ))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains(">= 20"), "should contain '>= 20': {q}");
    assert!(q.contains("<= 40"), "should contain '<= 40': {q}");
}

#[tokio::test]
async fn query_with_startswith() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::startswith("name", "Ali"))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has name"), "should contain 'has name': {q}");
    assert!(
        q.contains(r#"like "^Ali.*""#),
        "should contain 'like \"^Ali.*\"': {q}"
    );
}

#[tokio::test]
async fn query_with_endswith() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![person_doc("Alice", 30)])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::endswith("name", "ice"))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("has name"), "should contain 'has name': {q}");
    assert!(
        q.contains(r#"like ".*ice$""#),
        "should contain 'like \".*ice$\"': {q}"
    );
}

// ── Group-by tests ──────────────────────────────────────────────────

#[tokio::test]
async fn entity_group_by_aggregate() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$group0": "Engineering", "$mean": 35.5}),
        serde_json::json!({"$group0": "Sales", "$mean": 28.3}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager
        .query()
        .group_by("department")
        .aggregate(&[Agg::Mean("age".into())])
        .await
        .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(
        result.get_by_str("Engineering").unwrap().get_f64("$mean"),
        Some(35.5)
    );
    assert_eq!(
        result.get_by_str("Sales").unwrap().get_f64("$mean"),
        Some(28.3)
    );

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("groupby"), "should contain 'groupby': {q}");
    assert!(q.contains("$group0"), "should contain '$group0': {q}");
    assert!(
        q.contains("has department"),
        "should contain 'has department': {q}"
    );
    assert!(q.contains("mean"), "should contain 'mean': {q}");
}

#[tokio::test]
async fn entity_group_by_with_filter() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$group0": "Engineering", "$count": 5}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = EntityManager::<Person>::new(&db);
    let result = manager
        .query()
        .filter(Expr::gte("age", AttributeValue::Long(18)))
        .group_by("department")
        .aggregate(&[Agg::Count])
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result.get_by_str("Engineering").unwrap().count(), Some(5));

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains(">= 18"), "should contain '>= 18': {q}");
    assert!(q.contains("groupby"), "should contain 'groupby': {q}");
}

#[tokio::test]
async fn relation_group_by_aggregate() {
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({"$group0": "2024", "$count": 3}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let result = manager
        .query()
        .group_by("since")
        .aggregate(&[Agg::Count])
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result.get_by_str("2024").unwrap().count(), Some(3));

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(q.contains("groupby"), "should contain 'groupby': {q}");
}

// ── Role player filter tests ────────────────────────────────────────

#[tokio::test]
async fn relation_query_with_role_player_filter() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![friendship_doc(
        "2024-01-01",
    )])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::role_player(
            "friend",
            Expr::gt("age", AttributeValue::Long(30)),
        ))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    // Should bind role player in the relation pattern
    assert!(
        q.contains("friend: $friend"),
        "should contain role binding 'friend: $friend': {q}"
    );
    // Inner expression should target the role player variable
    assert!(
        q.contains("$friend has age"),
        "should contain '$friend has age': {q}"
    );
    assert!(q.contains("> 30"), "should contain '> 30': {q}");
}

#[tokio::test]
async fn relation_query_with_multiple_role_player_filters() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![friendship_doc(
        "2024-01-01",
    )])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let manager = RelationManager::<Friendship>::new(&db);
    let _ = manager
        .query()
        .filter(Expr::role_player(
            "friend",
            Expr::gt("age", AttributeValue::Long(18)),
        ))
        .filter(Expr::eq(
            "since",
            AttributeValue::String("2024-01-01".into()),
        ))
        .execute()
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    let q = &recorded[0];
    assert!(
        q.contains("friend: $friend"),
        "should bind role player: {q}"
    );
    assert!(
        q.contains("$friend has age"),
        "role player filter should target $friend: {q}"
    );
    // Also has a direct relation attribute filter
    assert!(q.contains("has since"), "should have direct filter: {q}");
}
