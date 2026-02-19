//! Integration tests for `RelationManager` with a mock backend.

use std::sync::{Arc, Mutex};

use type_bridge_orm::*;

// ── Attribute types ─────────────────────────────────────────────────

define_attribute!(Name, "name", "string");
define_attribute!(Position, "position", "string");

// ── Relation type ───────────────────────────────────────────────────

#[derive(Debug)]
struct Employment {
    iid: Option<String>,
    employee: RolePlayerRef,
    employer: RolePlayerRef,
    position: Option<String>,
}

impl TypeBridgeRelation for Employment {
    const TYPE_NAME: &'static str = "employment";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        static ATTRS: [OwnedAttributeInfo; 1] = [OwnedAttributeInfo {
            attr_name: "position",
            value_type: "string",
            is_key: false,
        }];
        &ATTRS
    }

    fn role_info() -> &'static [RoleInfo] {
        static ROLES: [RoleInfo; 2] = [
            RoleInfo {
                role_name: "employee",
                player_type_name: "person",
            },
            RoleInfo {
                role_name: "employer",
                player_type_name: "company",
            },
        ];
        &ROLES
    }

    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }

    fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
        let mut values = Vec::new();
        if let Some(ref pos) = self.position {
            values.push(("position", AttributeValue::String(pos.clone())));
        }
        values
    }

    fn to_role_player_refs(&self) -> Vec<RolePlayerRef> {
        vec![self.employee.clone(), self.employer.clone()]
    }

    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let position = doc.get("position").and_then(|v| v.as_str()).map(String::from);
        Ok(Self {
            iid: None,
            employee: RolePlayerRef {
                role: "employee",
                entity_type_name: "person",
                iid: None,
                key: None,
            },
            employer: RolePlayerRef {
                role: "employer",
                entity_type_name: "company",
                iid: None,
                key: None,
            },
            position,
        })
    }
}

// ── Mock backend (same pattern as entity_manager_tests) ─────────────

use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};

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

// ── Tests ───────────────────────────────────────────────────────────

fn make_employment(
    iid: Option<&str>,
    emp_iid: Option<&str>,
    emp_key: Option<(&'static str, AttributeValue)>,
    er_iid: Option<&str>,
    er_key: Option<(&'static str, AttributeValue)>,
    position: Option<&str>,
) -> Employment {
    Employment {
        iid: iid.map(String::from),
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: emp_iid.map(String::from),
            key: emp_key,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: er_iid.map(String::from),
            key: er_key,
        },
        position: position.map(String::from),
    }
}

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
    assert_eq!(attrs[0].value_type, "string");
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
