//! Shared test utilities for ORM integration tests.
//!
//! Provides a mock backend, helper functions, and common model definitions
//! used across multiple test files.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

// ── Mock backend ─────────────────────────────────────────────────────

/// Records queries and returns pre-configured results (LIFO order).
pub struct MockBackend {
    responses: Arc<Mutex<Vec<QueryResult>>>,
    pub queries: Arc<Mutex<Vec<String>>>,
}

impl MockBackend {
    pub fn new(responses: Vec<QueryResult>) -> Self {
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

pub struct MockTransaction {
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

// ── Failing mock backend ─────────────────────────────────────────────

/// Mock backend whose transactions always fail with a configurable error.
pub struct FailingMockBackend {
    error_message: String,
}

impl FailingMockBackend {
    pub fn new(message: &str) -> Self {
        Self {
            error_message: message.to_string(),
        }
    }
}

impl DriverBackend for FailingMockBackend {
    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        let msg = self.error_message.clone();
        Box::pin(async move {
            Ok(Box::new(FailingMockTransaction { error_message: msg }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        true
    }
}

pub struct FailingMockTransaction {
    error_message: String,
}

impl TransactionOps for FailingMockTransaction {
    fn query(
        &mut self,
        _typeql: &str,
    ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
        let msg = self.error_message.clone();
        Box::pin(async move { Err(OrmError::Transaction(msg)) })
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

// ── Helper functions ─────────────────────────────────────────────────

pub fn insert_response(iid: &str) -> QueryResult {
    QueryResult::Documents(vec![serde_json::json!({ "iid": iid })])
}

// ── Attribute types ──────────────────────────────────────────────────

define_attribute!(Name, "name", "string");
define_attribute!(Age, "age", "long");
define_attribute!(Position, "position", "string");

// ── Person entity ────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Person {
    pub iid: Option<String>,
    pub name: Name,
    pub age: Age,
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

pub fn make_person(name: &str, age: i64) -> Person {
    Person {
        iid: None,
        name: Name(name.into()),
        age: Age(age),
    }
}

pub fn make_person_with_iid(name: &str, age: i64, iid: &str) -> Person {
    Person {
        iid: Some(iid.to_string()),
        name: Name(name.into()),
        age: Age(age),
    }
}

// ── Employment relation ──────────────────────────────────────────────

#[derive(Debug)]
pub struct Employment {
    pub iid: Option<String>,
    pub employee: RolePlayerRef,
    pub employer: RolePlayerRef,
    pub position: Option<String>,
}

impl TypeBridgeRelation for Employment {
    const TYPE_NAME: &'static str = "employment";

    fn owned_attributes() -> &'static [OwnedAttributeInfo] {
        static ATTRS: [OwnedAttributeInfo; 1] = [OwnedAttributeInfo {
            attr_name: "position",
            value_type: ValueType::String,
            annotations: &[],
            doc: None,
            meta: &[],
        }];
        &ATTRS
    }

    fn role_info() -> &'static [RoleInfo] {
        static ROLES: [RoleInfo; 2] = [
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
        let position = doc
            .get("position")
            .and_then(|v| v.as_str())
            .map(String::from);
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

pub fn make_employment(
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

// ── Lifecycle hook helpers ───────────────────────────────────────────

/// Hook that records all calls for verification.
pub struct RecordingHook {
    pub calls: Arc<Mutex<Vec<(String, CrudOperation)>>>,
}

impl RecordingHook {
    #[allow(clippy::type_complexity)]
    pub fn new() -> (Self, Arc<Mutex<Vec<(String, CrudOperation)>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }
}

impl LifecycleHook for RecordingHook {
    fn name(&self) -> &str {
        "recording"
    }

    fn before_operation<'a>(
        &'a self,
        ctx: &'a mut HookContext,
    ) -> BoxFuture<'a, std::result::Result<PreHookResult, HookError>> {
        self.calls
            .lock()
            .unwrap()
            .push((format!("pre:{}", ctx.type_name), ctx.operation));
        Box::pin(async { Ok(PreHookResult::Continue) })
    }

    fn after_operation<'a>(
        &'a self,
        ctx: &'a HookContext,
    ) -> BoxFuture<'a, std::result::Result<(), HookError>> {
        self.calls
            .lock()
            .unwrap()
            .push((format!("post:{}", ctx.type_name), ctx.operation));
        Box::pin(async { Ok(()) })
    }
}

/// Hook that rejects all operations.
pub struct RejectingHook {
    pub reason: String,
}

impl RejectingHook {
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

impl LifecycleHook for RejectingHook {
    fn name(&self) -> &str {
        "rejecting"
    }

    fn before_operation<'a>(
        &'a self,
        _ctx: &'a mut HookContext,
    ) -> BoxFuture<'a, std::result::Result<PreHookResult, HookError>> {
        let reason = self.reason.clone();
        Box::pin(async move { Ok(PreHookResult::Reject { reason }) })
    }

    fn after_operation<'a>(
        &'a self,
        _ctx: &'a HookContext,
    ) -> BoxFuture<'a, std::result::Result<(), HookError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Hook that only runs for specific operations.
pub struct OperationFilterHook {
    pub allowed_op: CrudOperation,
    pub calls: Arc<Mutex<Vec<CrudOperation>>>,
}

impl OperationFilterHook {
    pub fn new(op: CrudOperation) -> (Self, Arc<Mutex<Vec<CrudOperation>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                allowed_op: op,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }
}

impl LifecycleHook for OperationFilterHook {
    fn name(&self) -> &str {
        "op-filter"
    }

    fn before_operation<'a>(
        &'a self,
        ctx: &'a mut HookContext,
    ) -> BoxFuture<'a, std::result::Result<PreHookResult, HookError>> {
        self.calls.lock().unwrap().push(ctx.operation);
        Box::pin(async { Ok(PreHookResult::Continue) })
    }

    fn after_operation<'a>(
        &'a self,
        _ctx: &'a HookContext,
    ) -> BoxFuture<'a, std::result::Result<(), HookError>> {
        Box::pin(async { Ok(()) })
    }

    fn should_run(&self, ctx: &HookContext) -> bool {
        ctx.operation == self.allowed_op
    }
}

/// Hook whose post-hook fails (errors should be logged, not propagated).
pub struct FailingPostHook;

impl LifecycleHook for FailingPostHook {
    fn name(&self) -> &str {
        "failing-post"
    }

    fn before_operation<'a>(
        &'a self,
        _ctx: &'a mut HookContext,
    ) -> BoxFuture<'a, std::result::Result<PreHookResult, HookError>> {
        Box::pin(async { Ok(PreHookResult::Continue) })
    }

    fn after_operation<'a>(
        &'a self,
        _ctx: &'a HookContext,
    ) -> BoxFuture<'a, std::result::Result<(), HookError>> {
        Box::pin(async {
            Err(HookError::Internal {
                hook_name: "failing-post".to_string(),
                source: "simulated failure".into(),
            })
        })
    }
}
