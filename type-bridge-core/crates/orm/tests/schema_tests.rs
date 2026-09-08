//! Integration tests for the schema management module.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT, LEGACY_CUTOVER_ANCHOR_KEY,
    LEGACY_CUTOVER_ANCHOR_SCOPE, LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
    LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_NAME, LEGACY_LEDGER_APP_LABEL, LEGACY_LEDGER_APPLIED_AT,
    LEGACY_LEDGER_APPLIED_ENTITY, LEGACY_LEDGER_CHECKSUM, LEGACY_LEDGER_MIGRATION_ID,
    LEGACY_LEDGER_NAME, LEGACY_WRITER_CUTOVER_MESSAGE, LEGACY_WRITER_GUARD_QUERY_TAG,
    MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_LEASE_FENCE, MANAGED_CONTROL_LEASE_HOLDER,
    MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_SCOPE,
};
use type_bridge_orm::_schema::info::*;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;
use type_bridge_schema_compat::{LEGACY_LEDGER_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL};

// ── Test entities ───────────────────────────────────────────────────

_define_attribute!(Name, "name", "string");
_define_attribute!(Age, "age", "long");
_define_attribute!(Position, "position", "string");
_define_attribute!(Email, "email", "string");

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
                doc: None,
                meta: &[],
            },
            OwnedAttributeInfo {
                attr_name: "age",
                value_type: ValueType::Long,
                annotations: &[],
                doc: None,
                meta: &[],
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
            doc: None,
            meta: &[],
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
            doc: None,
            meta: &[],
        }]
    }

    fn role_info() -> &'static [RoleInfo] {
        &[
            RoleInfo {
                role_name: "employee",
                player_type_name: "person",
                doc: None,
                meta: &[],
            },
            RoleInfo {
                role_name: "employer",
                player_type_name: "company",
                doc: None,
                meta: &[],
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
    legacy_cutover_present: bool,
    managed_core_present: bool,
    legacy_ledger_without_anchor: bool,
    malformed_cutover_binding: bool,
    legacy_guard_error: bool,
    schema_snapshot_error: bool,
    legacy_ledger_missing: bool,
    schema_snapshot_override: Option<String>,
}

impl MockBackend {
    fn new(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: false,
            managed_core_present: false,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_legacy_cutover() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: true,
            managed_core_present: true,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_malformed_legacy_cutover() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: true,
            managed_core_present: true,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: true,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_legacy_name_collision_without_anchor(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: false,
            managed_core_present: true,
            legacy_ledger_without_anchor: true,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_legacy_guard_error() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: false,
            managed_core_present: false,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: true,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_cutover_lookalikes_without_managed_core(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: true,
            managed_core_present: false,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_schema_snapshot_error(responses: Vec<QueryResult>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: false,
            managed_core_present: false,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: true,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_managed_rows_and_missing_legacy_ledger() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: true,
            managed_core_present: true,
            legacy_ledger_without_anchor: false,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: true,
            schema_snapshot_override: None,
        }
    }

    fn with_managed_rows_and_partial_legacy_ledger() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            legacy_cutover_present: true,
            managed_core_present: true,
            legacy_ledger_without_anchor: true,
            malformed_cutover_binding: false,
            legacy_guard_error: false,
            schema_snapshot_error: false,
            legacy_ledger_missing: false,
            schema_snapshot_override: None,
        }
    }

    fn with_writer_fence_schema(schema: String, legacy_cutover_present: bool) -> Self {
        let mut backend = Self::new(vec![QueryResult::Ok]);
        backend.schema_snapshot_override = Some(schema);
        backend.legacy_cutover_present = legacy_cutover_present;
        backend.managed_core_present = legacy_cutover_present;
        backend
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
        let legacy_cutover_present = self.legacy_cutover_present;
        let managed_core_present = self.managed_core_present;
        let legacy_ledger_without_anchor = self.legacy_ledger_without_anchor;
        let malformed_cutover_binding = self.malformed_cutover_binding;
        let legacy_guard_error = self.legacy_guard_error;
        let schema_snapshot_error = self.schema_snapshot_error;
        let legacy_ledger_missing = self.legacy_ledger_missing;
        let schema_snapshot_override = self.schema_snapshot_override.clone();
        Box::pin(async move {
            Ok(Box::new(MockTransaction {
                responses,
                queries,
                tx_type,
                legacy_cutover_present,
                managed_core_present,
                legacy_ledger_without_anchor,
                malformed_cutover_binding,
                legacy_guard_error,
                schema_snapshot_error,
                legacy_ledger_missing,
                schema_snapshot_override,
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
    legacy_cutover_present: bool,
    managed_core_present: bool,
    legacy_ledger_without_anchor: bool,
    malformed_cutover_binding: bool,
    legacy_guard_error: bool,
    schema_snapshot_error: bool,
    legacy_ledger_missing: bool,
    schema_snapshot_override: Option<String>,
}

const MOCK_CUTOVER_FINGERPRINT: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn managed_fence_schema_with_extensions() -> String {
    MANAGED_FENCE_SCHEMA_TYPEQL.replace(
        "owns typebridge-internal-v2-lease-holder @card(0..1)",
        "owns typebridge-internal-v2-lease-holder[] @distinct @card(0..1)",
    )
}

fn legacy_ledger_schema_with_extensions() -> String {
    LEGACY_LEDGER_SCHEMA_TYPEQL.replacen(
        "owns migration_checksum;",
        "owns migration_checksum[] @distinct;",
        1,
    )
}

fn canonical_writer_fence_schema_with_function_reference() -> String {
    format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
         define\nentity person;\n\
         fun inspect($candidate: person) -> {{ person }}:\n\
           match $candidate isa person;\n\
           $control isa typebridge-internal-v2-migration-control;\n\
           return {{ $candidate }};\n"
    )
}

fn canonical_writer_fence_schema_with_structured_user_attribute() -> String {
    format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
         define\n\
         struct payload: field value string;\n\
         attribute payload-attr, value payload;"
    )
}

fn legacy_guard_result(
    typeql: &str,
    cutover_present: bool,
    managed_core_present: bool,
    ledger_without_anchor: bool,
    malformed_binding: bool,
) -> QueryResult {
    if !cutover_present {
        if ledger_without_anchor && typeql.contains("match entity $t") {
            return QueryResult::Documents(vec![
                serde_json::json!({"label": LEGACY_LEDGER_APPLIED_ENTITY}),
                serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_ENTITY}),
            ]);
        }
        if ledger_without_anchor && typeql.contains("match attribute $t") {
            return QueryResult::Documents(
                [
                    LEGACY_CUTOVER_ANCHOR_KEY,
                    LEGACY_CUTOVER_ANCHOR_SCOPE,
                    LEGACY_CUTOVER_ANCHOR_FINGERPRINT,
                    LEGACY_LEDGER_MIGRATION_ID,
                    LEGACY_LEDGER_APP_LABEL,
                    LEGACY_LEDGER_NAME,
                    LEGACY_LEDGER_APPLIED_AT,
                    LEGACY_LEDGER_CHECKSUM,
                ]
                .into_iter()
                .map(|label| serde_json::json!({"label": label}))
                .collect(),
            );
        }
        if ledger_without_anchor && typeql.contains(&format!("isa {LEGACY_LEDGER_APPLIED_ENTITY}"))
        {
            return QueryResult::Documents(vec![serde_json::json!({"exists": true})]);
        }
        return QueryResult::Documents(Vec::new());
    }
    if typeql.contains("match entity $t") {
        let mut labels = vec![
            serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_ENTITY}),
            serde_json::json!({"label": LEGACY_LEDGER_APPLIED_ENTITY}),
        ];
        if managed_core_present {
            labels.push(serde_json::json!({"label": MANAGED_CONTROL_ENTITY}));
        }
        return QueryResult::Documents(labels);
    }
    if typeql.contains("match attribute $t") {
        let mut labels = vec![
            LEGACY_CUTOVER_ANCHOR_KEY,
            LEGACY_CUTOVER_ANCHOR_SCOPE,
            LEGACY_CUTOVER_ANCHOR_FINGERPRINT,
            LEGACY_LEDGER_MIGRATION_ID,
            LEGACY_LEDGER_APP_LABEL,
            LEGACY_LEDGER_NAME,
            LEGACY_LEDGER_APPLIED_AT,
            LEGACY_LEDGER_CHECKSUM,
        ];
        if managed_core_present {
            labels.extend([
                MANAGED_CONTROL_SCOPE,
                MANAGED_CONTROL_LEASE_HOLDER,
                MANAGED_CONTROL_LEASE_FENCE,
                MANAGED_CONTROL_LEASE_STATE,
            ]);
        }
        return QueryResult::Documents(
            labels
                .into_iter()
                .map(|label| serde_json::json!({"label": label}))
                .collect(),
        );
    }
    if typeql.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) {
        if typeql.contains("\"scope\": $scope") {
            return QueryResult::Documents(vec![serde_json::json!({
                "scope": "mock-scope",
                "fence": "1",
                "state": "free",
            })]);
        }
        if typeql.contains("\"holder\": $holder") {
            return QueryResult::Documents(Vec::new());
        }
        return QueryResult::Documents(vec![serde_json::json!({"exists": true})]);
    }
    if typeql.contains(&format!("isa {LEGACY_CUTOVER_ANCHOR_ENTITY}")) {
        if typeql.contains("\"fingerprint\": $fingerprint") {
            return QueryResult::Documents(vec![serde_json::json!({
                "key": LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
                "scope": "mock-scope",
                "fingerprint": MOCK_CUTOVER_FINGERPRINT,
            })]);
        }
        return QueryResult::Documents(vec![serde_json::json!({"exists": true})]);
    }
    if typeql.contains(&format!("isa {LEGACY_LEDGER_APPLIED_ENTITY}")) {
        if typeql.contains("\"checksum\": $checksum") {
            let checksum = if malformed_binding {
                "1111111111111111111111111111111111111111111111111111111111111111"
            } else {
                MOCK_CUTOVER_FINGERPRINT
            };
            return QueryResult::Documents(vec![serde_json::json!({
                "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
                "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
                "checksum": checksum,
            })]);
        }
        return QueryResult::Documents(vec![serde_json::json!({"exists": true})]);
    }
    QueryResult::Documents(Vec::new())
}

impl TransactionOps for MockTransaction {
    fn schema_snapshot(&mut self) -> BoxFuture<'_, std::result::Result<Option<String>, OrmError>> {
        if self.schema_snapshot_error {
            return Box::pin(async {
                Err(OrmError::Connection(
                    "injected pre-authority schema export failure".to_owned(),
                ))
            });
        }
        if let Some(schema_snapshot) = self.schema_snapshot_override.clone() {
            return Box::pin(async move { Ok(Some(schema_snapshot)) });
        }
        if self.managed_core_present && self.legacy_ledger_missing {
            return Box::pin(async { Ok(Some(MANAGED_FENCE_SCHEMA_TYPEQL.to_owned())) });
        }
        if self.managed_core_present && self.legacy_ledger_without_anchor {
            return Box::pin(async {
                Ok(Some(format!(
                    "{MANAGED_FENCE_SCHEMA_TYPEQL}\ndefine\n\
                     attribute migration_app_label, value string;\n\
                     attribute migration_applied_at, value datetime;\n\
                     attribute migration_checksum, value string;\n\
                     attribute migration_direction, value string;\n\
                     attribute migration_error, value string;\n\
                     attribute migration_executor_ip, value string;\n\
                     attribute migration_executor_mac, value string;\n\
                     attribute migration_finished_at, value datetime;\n\
                     attribute migration_id, value string;\n\
                     attribute migration_name, value string;\n\
                     attribute migration_run_id, value string;\n\
                     attribute migration_started_at, value datetime;\n\
                     attribute migration_status, value string;\n\
                     entity type_bridge_migration;\n\
                     entity type_bridge_migration_run;"
                )))
            });
        }
        if self.managed_core_present || self.legacy_guard_error {
            return Box::pin(async {
                Ok(Some(format!(
                    "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}"
                )))
            });
        }
        if self.legacy_cutover_present {
            return Box::pin(async {
                Ok(Some(
                    r#"define
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-legacy-cutover-key, value string;
attribute typebridge-internal-v2-legacy-cutover-scope, value string;
attribute typebridge-internal-v2-legacy-cutover-fingerprint, value string;
entity typebridge-internal-v2-migration-control;
entity typebridge-internal-v2-legacy-cutover;
"#
                    .to_owned(),
                ))
            });
        }
        Box::pin(async { Ok(None) })
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        self.queries
            .lock()
            .unwrap()
            .push((typeql.to_string(), self.tx_type));
        if typeql.starts_with(LEGACY_WRITER_GUARD_QUERY_TAG) {
            if self.legacy_guard_error {
                return Box::pin(async {
                    Err(OrmError::Connection(
                        "injected legacy-writer guard failure".to_owned(),
                    ))
                });
            }
            let result = legacy_guard_result(
                typeql,
                self.legacy_cutover_present,
                self.managed_core_present,
                self.legacy_ledger_without_anchor,
                self.malformed_cutover_binding,
            );
            return Box::pin(async move { Ok(result) });
        }
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
        AttributeSchemaEntry::new("tag", ValueType::String),
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
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
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

#[tokio::test]
async fn sync_schema_rejects_an_anchor_bound_cutover_before_user_typeql() {
    let backend = MockBackend::with_legacy_cutover();
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("an adopted database must reject the V1 schema writer");

    assert!(error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .all(|(query, _)| { query.starts_with(LEGACY_WRITER_GUARD_QUERY_TAG) })
    );
}

#[tokio::test]
async fn sync_schema_keeps_a_canonical_cutover_closed_with_a_reserved_function_reference() {
    let backend = MockBackend::with_writer_fence_schema(
        canonical_writer_fence_schema_with_function_reference(),
        true,
    );
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("a function body reference must not reopen canonical cutover authority");

    assert!(error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .any(|(query, _)| { query.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) })
    );
    assert!(
        queries
            .iter()
            .any(|(query, _)| query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_keeps_a_canonical_cutover_closed_with_a_structured_user_attribute() {
    let backend = MockBackend::with_writer_fence_schema(
        canonical_writer_fence_schema_with_structured_user_attribute(),
        true,
    );
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("an unrelated structured value must not reopen canonical cutover authority");

    assert!(error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .any(|(query, _)| { query.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) })
    );
    assert!(
        queries
            .iter()
            .any(|(query, _)| query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_fails_before_row_probes_for_an_unclassifiable_authority_export() {
    let export = format!("define attribute {MANAGED_CONTROL_SCOPE}, value");
    let backend = MockBackend::with_writer_fence_schema(export, false);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("an unclassifiable export mentioning authority must fail closed");

    assert!(error.to_string().contains(
        "the live schema could not establish absence or exact presence of the managed fence"
    ));
    assert!(queries.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sync_schema_allows_an_unrelated_structured_user_schema() {
    let backend = MockBackend::with_writer_fence_schema(
        "define struct payload: field value string; attribute payload-attr, value payload;"
            .to_owned(),
        false,
    );
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("an unsupported user value shape without authority labels remains V1-writable");

    assert!(queries.lock().unwrap().iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
}

#[tokio::test]
async fn sync_schema_allows_a_structured_extension_on_an_incomplete_label_collision() {
    let export = format!(
        "define\n\
         struct payload: field value string;\n\
         attribute payload-attr, value payload;\n\
         entity {MANAGED_CONTROL_ENTITY}, owns payload-attr;"
    );
    let backend = MockBackend::with_writer_fence_schema(export, false);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("a determinate incomplete label collision remains V1-writable");

    assert!(queries.lock().unwrap().iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
}

#[tokio::test]
async fn sync_schema_allows_a_structured_value_on_an_incomplete_attribute_collision() {
    let export = format!(
        "define\n\
         struct payload: field value string;\n\
         attribute {MANAGED_CONTROL_SCOPE}, value payload;"
    );
    let backend = MockBackend::with_writer_fence_schema(export, false);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("a determinate structured attribute collision remains V1-writable");

    assert!(queries.lock().unwrap().iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
}

#[tokio::test]
async fn sync_schema_keeps_canonical_cutover_closed_with_a_structured_control_extension() {
    let export = format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
         define\n\
         struct payload: field value string;\n\
         attribute payload-attr, value payload;\n\
         entity {MANAGED_CONTROL_ENTITY}, owns payload-attr;"
    );
    let backend = MockBackend::with_writer_fence_schema(export, true);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("a structured control extension cannot reopen canonical cutover authority");

    assert!(error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
    assert!(
        queries
            .lock()
            .unwrap()
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_treats_an_unoccupied_managed_extension_as_a_released_collision() {
    let export = format!(
        "{}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}",
        managed_fence_schema_with_extensions()
    );
    let backend = MockBackend::with_writer_fence_schema(export, false);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("released-only managed extensions without marker rows remain open");

    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .any(|(query, _)| { query.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) })
    );
    assert!(queries.iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
}

#[tokio::test]
async fn sync_schema_fails_before_ledger_probes_when_managed_extensions_have_marker_rows() {
    let export = format!(
        "{}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}",
        managed_fence_schema_with_extensions()
    );
    let backend = MockBackend::with_writer_fence_schema(export, true);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("managed marker rows turn a frozen-fact extension into corruption");

    assert!(error.to_string().contains("cutover state is inconsistent"));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .any(|(query, _)| { query.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) })
    );
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_treats_an_unoccupied_ledger_extension_as_a_released_collision() {
    let export = format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{}",
        legacy_ledger_schema_with_extensions()
    );
    let backend = MockBackend::with_writer_fence_schema(export, false);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("released-only ledger extensions without marker rows remain open");

    let queries = queries.lock().unwrap();
    assert!(queries.iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
}

#[tokio::test]
async fn sync_schema_fails_before_sentinel_probes_when_ledger_extensions_have_marker_rows() {
    let export = format!(
        "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{}",
        legacy_ledger_schema_with_extensions()
    );
    let backend = MockBackend::with_writer_fence_schema(export, true);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("ledger marker rows turn a frozen-fact extension into corruption");

    assert!(error.to_string().contains("cutover state is inconsistent"));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.contains(LEGACY_CUTOVER_SENTINEL_NAME))
    );
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_allows_legacy_ledger_lookalikes_beside_an_exact_managed_core() {
    let backend = MockBackend::with_legacy_name_collision_without_anchor(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("legacy labels without frozen owns are not writer-fence authority");

    let queries = queries.lock().unwrap();
    assert!(queries.iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
    assert!(
        queries
            .iter()
            .all(|(query, _)| { !query.contains(LEGACY_CUTOVER_SENTINEL_NAME) })
    );
}

#[tokio::test]
async fn sync_schema_allows_complete_cutover_row_lookalikes_without_the_managed_core() {
    let backend = MockBackend::with_cutover_lookalikes_without_managed_core(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    schema
        .sync_schema(true, false)
        .await
        .expect("marker-like rows cannot establish authority without the frozen managed core");

    let queries = queries.lock().unwrap();
    assert!(queries.iter().any(|(query, tx_type)| {
        *tx_type == TxType::Schema && query.trim_start().starts_with("define")
    }));
    assert!(
        queries
            .iter()
            .all(|(query, _)| { !query.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) })
    );
}

#[tokio::test]
async fn sync_schema_fails_closed_when_managed_rows_lose_the_frozen_legacy_schema() {
    for backend in [
        MockBackend::with_managed_rows_and_missing_legacy_ledger(),
        MockBackend::with_managed_rows_and_partial_legacy_ledger(),
    ] {
        let queries = Arc::clone(&backend.queries);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let mut schema = SchemaManager::new(&db);
        schema.register_entity::<Person>();

        let error = schema
            .sync_schema(true, false)
            .await
            .expect_err("managed control/anchor rows make ledger-schema loss corruption");

        assert!(error.to_string().contains("cutover state is inconsistent"));
        let queries = queries.lock().unwrap();
        assert!(
            queries
                .iter()
                .all(|(query, _)| !query.trim_start().starts_with("define"))
        );
        assert!(
            queries
                .iter()
                .all(|(query, _)| !query.contains(LEGACY_CUTOVER_SENTINEL_NAME)),
            "an unproven ledger schema must never be queried: {queries:?}"
        );
    }
}

#[tokio::test]
async fn sync_schema_fails_closed_on_a_mismatched_anchor_sentinel_pair() {
    let backend = MockBackend::with_malformed_legacy_cutover();
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("a malformed anchored pair must fail closed");

    assert!(error.to_string().contains("cutover state is inconsistent"));
    assert!(!error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
    let queries = queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_never_treats_a_provider_guard_error_as_open() {
    let backend = MockBackend::with_legacy_guard_error();
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("provider failure must reject the V1 schema writer");

    assert!(matches!(error, OrmError::Connection(_)));
    assert!(
        queries
            .lock()
            .unwrap()
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
    );
}

#[tokio::test]
async fn sync_schema_fails_closed_when_an_authoritative_backend_cannot_export_schema() {
    let backend = MockBackend::with_schema_snapshot_error(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();

    let error = schema
        .sync_schema(true, false)
        .await
        .expect_err("a real schema-export failure must not bypass an adopted fence");

    assert!(matches!(error, OrmError::Connection(_)));
    assert!(
        queries
            .lock()
            .unwrap()
            .iter()
            .all(|(query, _)| !query.trim_start().starts_with("define"))
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
