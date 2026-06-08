//! Integration tests for the schema management module.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use type_bridge_orm::schema::info::*;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

// ── Test entities ───────────────────────────────────────────────────

define_attribute!(Name, "name", "string");
define_attribute!(Age, "age", "long");
define_attribute!(Position, "position", "string");
define_attribute!(Email, "email", "string");

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
        Ok(Person {
            iid: None,
            name: Name(
                doc.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            age: Age(doc.get("age").and_then(|v| v.as_i64()).unwrap_or_default()),
        })
    }
}

#[derive(Debug)]
struct Company {
    iid: Option<String>,
    name: Name,
}

impl TypeBridgeEntity for Company {
    const TYPE_NAME: &'static str = "company";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        &[OwnedAttributeInfo {
            attr_name: "name",
            value_type: ValueType::String,
            annotations: &[Annotation::Key],
        }]
    }

    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
        vec![("name", self.name.to_value())]
    }
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        Ok(Company {
            iid: None,
            name: Name(
                doc.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
        })
    }
}

// ── Test relation ───────────────────────────────────────────────────

#[derive(Debug)]
struct Employment {
    iid: Option<String>,
    position: Position,
    employee_iid: String,
    employer_iid: String,
}

impl TypeBridgeRelation for Employment {
    const TYPE_NAME: &'static str = "employment";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        &[OwnedAttributeInfo {
            attr_name: "position",
            value_type: ValueType::String,
            annotations: &[],
        }]
    }

    fn role_info() -> &'static [RoleInfo] {
        &[
            RoleInfo {
                role_name: "employee",
                player_type_name: "person",
            },
            RoleInfo {
                role_name: "employer",
                player_type_name: "company",
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
        vec![("position", self.position.to_value())]
    }
    fn to_role_player_refs(&self) -> Vec<RolePlayerRef> {
        vec![
            RolePlayerRef {
                role: "employee",
                entity_type_name: "person",
                iid: Some(self.employee_iid.clone()),
                key: None,
            },
            RolePlayerRef {
                role: "employer",
                entity_type_name: "company",
                iid: Some(self.employer_iid.clone()),
                key: None,
            },
        ]
    }
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        Ok(Employment {
            iid: None,
            position: Position(
                doc.get("position")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            employee_iid: String::new(),
            employer_iid: String::new(),
        })
    }
}

// ── Mock backend ────────────────────────────────────────────────────

struct MockBackend {
    responses: Arc<Mutex<Vec<QueryResult>>>,
    queries: Arc<Mutex<Vec<(String, TxType)>>>,
}

impl MockBackend {
    fn new(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    fn queries(&self) -> Vec<(String, TxType)> {
        self.queries.lock().unwrap().clone()
    }
}

impl DriverBackend for MockBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        let responses = Arc::clone(&self.responses);
        let queries = Arc::clone(&self.queries);
        Box::pin(async move {
            Ok(Box::new(MockTransaction {
                responses,
                queries,
                tx_type,
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct MockTransaction {
    responses: Arc<Mutex<Vec<QueryResult>>>,
    queries: Arc<Mutex<Vec<(String, TxType)>>>,
    tx_type: TxType,
}

impl TransactionOps for MockTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.queries
            .lock()
            .unwrap()
            .push((typeql.to_string(), self.tx_type));
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

// ── SchemaManager registration tests ────────────────────────────────

#[test]
fn register_entity_populates_schema_info() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();

    let info = schema.schema_info();
    assert!(info.entities.contains_key("person"));
    let entity = &info.entities["person"];
    assert_eq!(entity.type_name, "person");
    assert_eq!(entity.owned_attributes.len(), 2);
    assert_eq!(entity.owned_attributes[0].attr_name, "name");
    assert_eq!(entity.owned_attributes[0].value_type, ValueType::String);
    assert_eq!(entity.owned_attributes[0].flags_string(), "@key");
    assert_eq!(entity.owned_attributes[1].attr_name, "age");
    assert_eq!(entity.owned_attributes[1].value_type, ValueType::Long);

    // Attribute types should be registered too
    assert!(info.attributes.contains_key("name"));
    assert!(info.attributes.contains_key("age"));
    assert_eq!(info.attributes["name"].value_type, ValueType::String);
    assert_eq!(info.attributes["age"].value_type, ValueType::Long);
}

#[test]
fn register_relation_populates_roles() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_relation::<Employment>();

    let info = schema.schema_info();
    assert!(info.relations.contains_key("employment"));
    let relation = &info.relations["employment"];
    assert_eq!(relation.type_name, "employment");
    assert_eq!(relation.roles.len(), 2);
    assert_eq!(relation.roles[0].role_name, "employee");
    assert_eq!(relation.roles[0].player_type_names, vec!["person"]);
    assert_eq!(relation.roles[1].role_name, "employer");
    assert_eq!(relation.roles[1].player_type_names, vec!["company"]);

    // Owned attributes
    assert_eq!(relation.owned_attributes.len(), 1);
    assert_eq!(relation.owned_attributes[0].attr_name, "position");

    // Attribute types
    assert!(info.attributes.contains_key("position"));
}

// ── Schema generation tests ─────────────────────────────────────────

#[test]
fn generate_schema_produces_valid_typeql() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();

    let typeql = schema.generate_schema().unwrap();
    assert!(typeql.starts_with("define"));
    assert!(typeql.contains("attribute name, value string;"));
    assert!(typeql.contains("attribute age, value integer;"));
    assert!(typeql.contains("attribute position, value string;"));
    assert!(typeql.contains("entity person,"));
    assert!(typeql.contains("entity company,"));
    assert!(typeql.contains("relation employment,"));
}

#[test]
fn generate_schema_with_key_and_unique() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();

    let typeql = schema.generate_schema().unwrap();
    assert!(typeql.contains("owns name @key"));
}

#[test]
fn generate_schema_with_plays_clauses() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();

    let typeql = schema.generate_schema().unwrap();
    assert!(
        typeql.contains("person plays employment:employee;"),
        "expected plays clause for person, got:\n{typeql}"
    );
    assert!(
        typeql.contains("company plays employment:employer;"),
        "expected plays clause for company, got:\n{typeql}"
    );
}

#[test]
fn generate_schema_with_cardinality() {
    // Manually build a SchemaInfo with cardinality annotations
    let mut info = SchemaInfo::default();
    info.attributes.insert(
        "tag".into(),
        AttributeSchemaEntry {
            attr_name: "tag".into(),
            value_type: ValueType::String,
        },
    );
    info.entities.insert(
        "item".into(),
        EntitySchemaEntry {
            type_name: "item".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeEntry {
                attr_name: "tag".into(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Card(2, Some(5))],
            }],
            plays_cardinalities: BTreeMap::new(),
        },
    );

    let typeql = info.to_typeql().unwrap();
    assert!(
        typeql.contains("owns tag @card(2..5);"),
        "expected @card annotation, got:\n{typeql}"
    );
}

// ── Schema sync tests ───────────────────────────────────────────────

#[tokio::test]
async fn sync_schema_sends_typeql_to_backend() {
    let db = Database::with_backend(Box::new(MockBackend::new(vec![QueryResult::Ok])), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    // force=true skips existence check
    let result = schema.sync_schema(true, false).await;
    assert!(result.is_ok(), "sync_schema failed: {:?}", result.err());
}

#[tokio::test]
async fn sync_schema_detects_existing_types() {
    // has_existing_schema will run a query that returns documents → schema exists
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({})])]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let result = schema.sync_schema(false, false).await;
    assert!(result.is_err(), "should error when schema already exists");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("already exist"),
        "unexpected error message: {err_msg}"
    );
}

#[tokio::test]
async fn sync_schema_force_skips_check() {
    // With force=true, existence check is skipped, only define is sent
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let result = schema.sync_schema(true, false).await;
    assert!(
        result.is_ok(),
        "force sync should succeed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn sync_schema_skip_if_exists_no_error() {
    // has_existing_schema returns true, but skip_if_exists suppresses the error
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![serde_json::json!({})])]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let result = schema.sync_schema(false, true).await;
    assert!(
        result.is_ok(),
        "skip_if_exists should return Ok: {:?}",
        result.err()
    );
}

// ── Schema diff tests ───────────────────────────────────────────────

#[test]
fn schema_diff_detects_added_entity() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut old_schema = SchemaManager::new(&db);
    old_schema.register_entity::<Person>();

    let mut new_schema = SchemaManager::new(&db);
    new_schema.register_entity::<Person>();
    new_schema.register_entity::<Company>();

    let diff = old_schema.schema_info().compare(new_schema.schema_info());
    assert!(diff.has_changes());
    assert!(!diff.has_breaking_changes());
    assert!(diff.added_entities.contains(&"company".to_string()));
}

#[test]
fn schema_diff_detects_removed_attribute() {
    // Build old schema with person having name + age
    let mut old_info = SchemaInfo::default();
    old_info.entities.insert(
        "person".into(),
        EntitySchemaEntry {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                },
                OwnedAttributeEntry {
                    attr_name: "age".into(),
                    value_type: ValueType::Long,
                    annotations: vec![],
                },
            ],
            plays_cardinalities: BTreeMap::new(),
        },
    );

    // New schema without age
    let mut new_info = SchemaInfo::default();
    new_info.entities.insert(
        "person".into(),
        EntitySchemaEntry {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeEntry {
                attr_name: "name".into(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Key],
            }],
            plays_cardinalities: BTreeMap::new(),
        },
    );

    let diff = old_info.compare(&new_info);
    assert!(diff.has_changes());
    assert!(diff.has_breaking_changes());
    let changes = diff.modified_entities.get("person").unwrap();
    assert_eq!(changes.removed_attributes, vec!["age"]);
}

#[test]
fn schema_diff_summary_readable() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let old_info = SchemaInfo::default();
    let mut new_schema = SchemaManager::new(&db);
    new_schema.register_entity::<Person>();
    new_schema.register_relation::<Employment>();

    let diff = old_info.compare(new_schema.schema_info());
    let summary = diff.summary();
    assert!(
        summary.contains("+ entity person"),
        "summary should mention added entity: {summary}"
    );
    assert!(
        summary.contains("+ relation employment"),
        "summary should mention added relation: {summary}"
    );
}

// ── SchemaInfo comparison with registered models ────────────────────

#[test]
fn full_schema_roundtrip_registration_and_generation() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();

    let typeql = schema.generate_schema().unwrap();

    // Verify structure
    assert!(typeql.starts_with("define\n"));

    // Attributes sorted alphabetically: age, name, position
    let age_pos = typeql.find("attribute age").unwrap();
    let name_pos = typeql.find("attribute name").unwrap();
    let pos_pos = typeql.find("attribute position").unwrap();
    assert!(age_pos < name_pos, "age should come before name");
    assert!(name_pos < pos_pos, "name should come before position");

    // Entity definitions
    assert!(typeql.contains("entity company,"));
    assert!(typeql.contains("entity person,"));

    // Company before person (alphabetical)
    let company_pos = typeql.find("entity company").unwrap();
    let person_pos = typeql.find("entity person").unwrap();
    assert!(
        company_pos < person_pos,
        "company should come before person"
    );

    // Relation definition
    assert!(typeql.contains("relation employment,"));
    assert!(typeql.contains("    relates employee,"));
    assert!(typeql.contains("    relates employer,"));
    assert!(typeql.contains("    owns position;"));

    // Plays clauses
    assert!(typeql.contains("company plays employment:employer;"));
    assert!(typeql.contains("person plays employment:employee;"));
}

#[test]
fn schema_info_validate_passes_for_registered_models() {
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);

    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();

    assert!(schema.schema_info().validate().is_ok());
}

// ── Schema introspection tests ─────────────────────────────────────

#[tokio::test]
async fn introspect_builds_schema_info() {
    // Responses are popped LIFO. The introspect method issues queries in order:
    // 1. attribute types, 2. entity types, 3. per-entity owned attrs,
    // 4. relation types, 5. per-relation owned attrs, 6. per-relation roles
    let backend = MockBackend::new(vec![
        // 6. employment roles (popped last)
        QueryResult::Documents(vec![
            serde_json::json!({"role": "employee"}),
            serde_json::json!({"role": "employer"}),
        ]),
        // 5. employment owned attributes
        QueryResult::Documents(vec![serde_json::json!({"attr": "position"})]),
        // 4. relation types
        QueryResult::Documents(vec![serde_json::json!({"name": "employment"})]),
        // 3. person owned attributes
        QueryResult::Documents(vec![
            serde_json::json!({"attr": "name"}),
            serde_json::json!({"attr": "age"}),
        ]),
        // 2. entity types
        QueryResult::Documents(vec![serde_json::json!({"name": "person"})]),
        // 1. attribute types (popped first)
        QueryResult::Documents(vec![
            serde_json::json!({"name": "name", "value_type": "string"}),
            serde_json::json!({"name": "age", "value_type": "long"}),
            serde_json::json!({"name": "position", "value_type": "string"}),
        ]),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let schema = SchemaManager::new(&db);

    let info = schema.introspect().await.unwrap();

    // Attribute types
    assert_eq!(info.attributes.len(), 3);
    assert_eq!(info.attributes["name"].value_type, ValueType::String);
    assert_eq!(info.attributes["age"].value_type, ValueType::Long);
    assert_eq!(info.attributes["position"].value_type, ValueType::String);

    // Entity types
    assert_eq!(info.entities.len(), 1);
    let person = &info.entities["person"];
    assert_eq!(person.type_name, "person");
    assert_eq!(person.owned_attributes.len(), 2);

    // Relation types
    assert_eq!(info.relations.len(), 1);
    let employment = &info.relations["employment"];
    assert_eq!(employment.type_name, "employment");
    assert_eq!(employment.owned_attributes.len(), 1);
    assert_eq!(employment.roles.len(), 2);
    assert_eq!(employment.roles[0].role_name, "employee");
    assert_eq!(employment.roles[1].role_name, "employer");
}

#[test]
fn schema_info_from_typeql_export_preserves_annotations_roles_and_players() {
    let typeql = r#"define

attribute name,
 value string;
attribute age,
 value integer;
entity person,
  owns age @card(0..1),
  owns name @key,
  plays employment:employee;
relation employment,
  relates employee @card(1..1);
"#;

    let info = SchemaInfo::from_typeql(typeql).unwrap();

    assert_eq!(info.attributes["age"].value_type, ValueType::Long);
    let person = &info.entities["person"];
    let name = person
        .owned_attributes
        .iter()
        .find(|attr| attr.attr_name == "name")
        .unwrap();
    assert_eq!(name.annotations, vec![Annotation::Key]);
    let age = person
        .owned_attributes
        .iter()
        .find(|attr| attr.attr_name == "age")
        .unwrap();
    assert_eq!(age.annotations, vec![Annotation::Card(0, Some(1))]);

    let employment = &info.relations["employment"];
    assert_eq!(employment.roles.len(), 1);
    assert_eq!(employment.roles[0].role_name, "employee");
    assert_eq!(employment.roles[0].player_type_names, vec!["person"]);
    assert_eq!(employment.roles[0].cardinality, Some((1, Some(1))));
}

#[tokio::test]
async fn introspect_empty_database() {
    // All introspection queries return empty results
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![]), // relation types
        QueryResult::Documents(vec![]), // entity types
        QueryResult::Documents(vec![]), // attribute types
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let schema = SchemaManager::new(&db);

    let info = schema.introspect().await.unwrap();
    assert!(info.attributes.is_empty());
    assert!(info.entities.is_empty());
    assert!(info.relations.is_empty());
}

#[tokio::test]
async fn introspect_with_wrapped_values() {
    // TypeDB may return values wrapped in {"value": "..."}
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![]), // relation types
        QueryResult::Documents(vec![serde_json::json!({"attr": {"value": "email"}})]),
        QueryResult::Documents(vec![serde_json::json!({"name": {"value": "user"}})]),
        QueryResult::Documents(vec![
            serde_json::json!({"name": {"value": "email"}, "value_type": {"value": "string"}}),
        ]),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let schema = SchemaManager::new(&db);

    let info = schema.introspect().await.unwrap();
    assert!(info.attributes.contains_key("email"));
    assert!(info.entities.contains_key("user"));
    assert_eq!(info.entities["user"].owned_attributes.len(), 1);
    assert_eq!(info.entities["user"].owned_attributes[0].attr_name, "email");
}
