//! Integration tests for the schema IR, diff and read-only introspection.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use type_bridge_orm::_schema::info::*;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

// ── Mock backend ────────────────────────────────────────────────────

struct MockBackend {
    responses: Arc<Mutex<Vec<QueryResult>>>,
}

impl MockBackend {
    fn new(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
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
        Box::pin(
            async move { Ok(Box::new(MockTransaction { responses }) as Box<dyn TransactionOps>) },
        )
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct MockTransaction {
    responses: Arc<Mutex<Vec<QueryResult>>>,
}

impl TransactionOps for MockTransaction {
    fn query(
        &mut self,
        _typeql: &str,
    ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
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

// ── Schema diff tests ───────────────────────────────────────────────

const PERSON: &str = "define\nattribute name, value string;\nentity person, owns name @key;\n";

#[test]
fn schema_diff_detects_added_entity() {
    let old = SchemaInfo::from_typeql(PERSON).unwrap();
    let new = SchemaInfo::from_typeql(&format!("{PERSON}entity company, owns name;\n")).unwrap();

    let diff = old.compare(&new);
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
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                },
                OwnedAttributeEntry {
                    attr_name: "age".into(),
                    value_type: ValueType::Long,
                    annotations: vec![],
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                },
            ],
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
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
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
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
    let new = SchemaInfo::from_typeql(
        "define\nattribute name, value string;\n\
         entity person, owns name, plays employment:employee;\n\
         relation employment, relates employee;\n",
    )
    .unwrap();

    let diff = SchemaInfo::default().compare(&new);
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

// ── Schema introspection tests ─────────────────────────────────────

#[tokio::test]
async fn introspect_builds_schema_info() {
    // Responses are popped LIFO. The introspect method issues queries in order:
    // 1. attribute types, 2. entity types, 3. per-entity owned attrs,
    // 4. relation types, 5. per-relation owned attrs, 6. per-relation roles
    let backend = MockBackend::new(vec![
        // 6. employment roles (popped last)
        QueryResult::Documents(vec![
            serde_json::json!({"role": "employment:employee"}),
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
