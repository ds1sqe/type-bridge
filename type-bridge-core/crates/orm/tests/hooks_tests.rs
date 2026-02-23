//! Tests for the lifecycle hook system.

use std::sync::{Arc, Mutex};

use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
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

// ── Recording hook ───────────────────────────────────────────────────

/// Hook that records all calls for verification.
struct RecordingHook {
    calls: Arc<Mutex<Vec<(String, CrudOperation)>>>,
}

impl RecordingHook {
    fn new() -> (Self, Arc<Mutex<Vec<(String, CrudOperation)>>>) {
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
struct RejectingHook {
    reason: String,
}

impl RejectingHook {
    fn new(reason: &str) -> Self {
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
struct OperationFilterHook {
    allowed_op: CrudOperation,
    calls: Arc<Mutex<Vec<CrudOperation>>>,
}

impl OperationFilterHook {
    fn new(op: CrudOperation) -> (Self, Arc<Mutex<Vec<CrudOperation>>>) {
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
struct FailingPostHook;

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

// ── Helper ───────────────────────────────────────────────────────────

fn insert_response(iid: &str) -> QueryResult {
    QueryResult::Documents(vec![serde_json::json!({ "iid": iid })])
}

fn make_person(name: &str, age: i64) -> Person {
    Person {
        iid: None,
        name: Name(name.into()),
        age: Age(age),
    }
}

fn make_person_with_iid(name: &str, age: i64, iid: &str) -> Person {
    Person {
        iid: Some(iid.to_string()),
        name: Name(name.into()),
        age: Age(age),
    }
}

// ── Tests: Pre/post hook execution ──────────────────────────────────

#[tokio::test]
async fn insert_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let mut person = make_person("Alice", 30);
    manager.insert(&mut person).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], ("pre:person".to_string(), CrudOperation::Insert));
    assert_eq!(
        calls[1],
        ("post:person".to_string(), CrudOperation::Insert)
    );
}

#[tokio::test]
async fn delete_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let person = make_person_with_iid("Alice", 30, "0xabc");
    manager.delete(&person).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], ("pre:person".to_string(), CrudOperation::Delete));
    assert_eq!(
        calls[1],
        ("post:person".to_string(), CrudOperation::Delete)
    );
}

#[tokio::test]
async fn update_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let person = make_person_with_iid("Alice", 31, "0xabc");
    manager.update(&person).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], ("pre:person".to_string(), CrudOperation::Update));
    assert_eq!(
        calls[1],
        ("post:person".to_string(), CrudOperation::Update)
    );
}

#[tokio::test]
async fn put_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![insert_response("0xput1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let mut person = make_person("Alice", 30);
    manager.put(&mut person).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], ("pre:person".to_string(), CrudOperation::Put));
    assert_eq!(calls[1], ("post:person".to_string(), CrudOperation::Put));
}

// ── Tests: Pre-hook rejection ───────────────────────────────────────

#[tokio::test]
async fn pre_hook_rejection_prevents_insert() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("validation failed")));

    let mut person = make_person("Alice", 30);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hook(HookError::Rejected { reason, .. }) => {
            assert_eq!(reason, "validation failed");
        }
        other => panic!("Expected Hook(Rejected), got: {other}"),
    }

    // No query should have been executed
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty(), "Rejected insert should not execute query");
}

#[tokio::test]
async fn pre_hook_rejection_prevents_delete() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("cannot delete")));

    let person = make_person_with_iid("Alice", 30, "0xabc");
    let result = manager.delete(&person).await;

    assert!(result.is_err());
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn pre_hook_rejection_prevents_update() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("cannot update")));

    let person = make_person_with_iid("Alice", 31, "0xabc");
    let result = manager.update(&person).await;

    assert!(result.is_err());
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn pre_hook_rejection_prevents_put() {
    let backend = MockBackend::new(vec![insert_response("0xput1")]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("cannot put")));

    let mut person = make_person("Alice", 30);
    let result = manager.put(&mut person).await;

    assert!(result.is_err());
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

// ── Tests: Post-hook errors are non-fatal ───────────────────────────

#[tokio::test]
async fn post_hook_error_does_not_propagate() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(FailingPostHook));

    let mut person = make_person("Alice", 30);
    // Should succeed despite the failing post-hook
    let iid = manager.insert(&mut person).await.unwrap();
    assert_eq!(iid, "0x1");
}

// ── Tests: should_run filtering ─────────────────────────────────────

#[tokio::test]
async fn should_run_filters_by_operation() {
    let backend = MockBackend::new(vec![
        QueryResult::Ok, // for delete
        insert_response("0x1"), // for insert
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = OperationFilterHook::new(CrudOperation::Insert);
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    // Insert should trigger the hook
    let mut person = make_person("Alice", 30);
    manager.insert(&mut person).await.unwrap();

    // Delete should NOT trigger the hook
    let person = make_person_with_iid("Alice", 30, "0x1");
    manager.delete(&person).await.unwrap();

    let calls = calls.lock().unwrap();
    // Only 1 pre-hook call from insert (should_run=false skips delete)
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], CrudOperation::Insert);
}

// ── Tests: Multiple hooks ───────────────────────────────────────────

#[tokio::test]
async fn multiple_hooks_run_in_order() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook1, calls1) = RecordingHook::new();
    let (hook2, calls2) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook1));
    manager.add_hook(Arc::new(hook2));

    let mut person = make_person("Alice", 30);
    manager.insert(&mut person).await.unwrap();

    // Both hooks should have been called
    let c1 = calls1.lock().unwrap();
    let c2 = calls2.lock().unwrap();
    assert_eq!(c1.len(), 2); // pre + post
    assert_eq!(c2.len(), 2); // pre + post
}

#[tokio::test]
async fn rejection_short_circuits_subsequent_hooks() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (recorder, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    // Rejecting hook first, then recording hook
    manager.add_hook(Arc::new(RejectingHook::new("rejected")));
    manager.add_hook(Arc::new(recorder));

    let mut person = make_person("Alice", 30);
    let result = manager.insert(&mut person).await;

    assert!(result.is_err());
    // Recording hook should not have been called at all
    let calls = calls.lock().unwrap();
    assert!(calls.is_empty());
}

// ── Tests: Batch operations ─────────────────────────────────────────

#[tokio::test]
async fn insert_many_fires_hooks_per_entity() {
    let backend = MockBackend::new(vec![
        insert_response("0xb2"),
        insert_response("0xb1"),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let mut entities = vec![
        make_person("Alice", 30),
        make_person("Bob", 25),
    ];
    manager.insert_many(&mut entities).await.unwrap();

    let calls = calls.lock().unwrap();
    // 2 pre-hooks + 2 post-hooks = 4 total
    assert_eq!(calls.len(), 4);
    // Pre-hooks first (all before DB ops)
    assert_eq!(calls[0].1, CrudOperation::Insert);
    assert_eq!(calls[1].1, CrudOperation::Insert);
    // Then post-hooks (all after commit)
    assert_eq!(calls[2].1, CrudOperation::Insert);
    assert_eq!(calls[3].1, CrudOperation::Insert);
}

#[tokio::test]
async fn insert_many_rejection_aborts_entire_batch() {
    let backend = MockBackend::new(vec![
        insert_response("0xb2"),
        insert_response("0xb1"),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("batch rejected")));

    let mut entities = vec![
        make_person("Alice", 30),
        make_person("Bob", 25),
    ];
    let result = manager.insert_many(&mut entities).await;

    assert!(result.is_err());
    // No queries should have been executed
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn delete_many_fires_hooks_per_entity() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let entities = vec![
        make_person_with_iid("Alice", 30, "0x1"),
        make_person_with_iid("Bob", 25, "0x2"),
    ];
    manager.delete_many(&entities).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4); // 2 pre + 2 post
    assert_eq!(calls[0].1, CrudOperation::Delete);
    assert_eq!(calls[1].1, CrudOperation::Delete);
    assert_eq!(calls[2].1, CrudOperation::Delete);
    assert_eq!(calls[3].1, CrudOperation::Delete);
}

#[tokio::test]
async fn update_many_fires_hooks_per_entity() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(hook));

    let entities = vec![
        make_person_with_iid("Alice", 31, "0x1"),
        make_person_with_iid("Bob", 26, "0x2"),
    ];
    manager.update_many(&entities).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4); // 2 pre + 2 post
    assert_eq!(calls[0].1, CrudOperation::Update);
    assert_eq!(calls[1].1, CrudOperation::Update);
}

// ── Tests: Zero-overhead guard ──────────────────────────────────────

#[tokio::test]
async fn no_hooks_means_no_overhead() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    // Manager with NO hooks registered
    let manager = EntityManager::<Person>::new(&db);
    let mut person = make_person("Alice", 30);
    let iid = manager.insert(&mut person).await.unwrap();

    assert_eq!(iid, "0x1");
    assert_eq!(person.iid(), Some("0x1"));
}

// ── Tests: HookContext attributes ───────────────────────────────────

#[tokio::test]
async fn pre_hook_receives_correct_context() {
    let backend = MockBackend::new(vec![insert_response("0x1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let captured_ctx: Arc<Mutex<Option<(String, TypeKind, CrudOperation, Vec<String>)>>> =
        Arc::new(Mutex::new(None));

    struct ContextCapture {
        captured: Arc<Mutex<Option<(String, TypeKind, CrudOperation, Vec<String>)>>>,
    }
    impl LifecycleHook for ContextCapture {
        fn name(&self) -> &str {
            "ctx-capture"
        }
        fn before_operation<'a>(
            &'a self,
            ctx: &'a mut HookContext,
        ) -> BoxFuture<'a, std::result::Result<PreHookResult, HookError>> {
            let attr_names: Vec<String> = ctx
                .attributes
                .iter()
                .map(|(name, _)| name.to_string())
                .collect();
            *self.captured.lock().unwrap() = Some((
                ctx.type_name.to_string(),
                ctx.type_kind,
                ctx.operation,
                attr_names,
            ));
            Box::pin(async { Ok(PreHookResult::Continue) })
        }
        fn after_operation<'a>(
            &'a self,
            _ctx: &'a HookContext,
        ) -> BoxFuture<'a, std::result::Result<(), HookError>> {
            Box::pin(async { Ok(()) })
        }
    }

    let mut manager = EntityManager::<Person>::new(&db);
    manager.add_hook(Arc::new(ContextCapture {
        captured: Arc::clone(&captured_ctx),
    }));

    let mut person = make_person("Alice", 30);
    manager.insert(&mut person).await.unwrap();

    let captured = captured_ctx.lock().unwrap();
    let (type_name, type_kind, operation, attrs) = captured.as_ref().unwrap();
    assert_eq!(type_name, "person");
    assert_eq!(*type_kind, TypeKind::Entity);
    assert_eq!(*operation, CrudOperation::Insert);
    assert!(attrs.contains(&"name".to_string()));
    assert!(attrs.contains(&"age".to_string()));
}

// ── Tests: HookRunner isolation ─────────────────────────────────────

#[tokio::test]
async fn hook_runner_has_hooks_returns_false_when_empty() {
    let runner = HookRunner::new();
    assert!(!runner.has_hooks());
}

#[tokio::test]
async fn hook_runner_has_hooks_returns_true_after_add() {
    let mut runner = HookRunner::new();
    let (hook, _) = RecordingHook::new();
    runner.add_hook(Arc::new(hook));
    assert!(runner.has_hooks());
}

#[tokio::test]
async fn hook_runner_pre_hooks_noop_when_empty() {
    let runner = HookRunner::new();
    let mut ctx = HookRunner::build_context(
        "test",
        TypeKind::Entity,
        CrudOperation::Insert,
        vec![],
        None,
    );
    // Should be a no-op, not an error
    runner.run_pre_hooks(&mut ctx).await.unwrap();
}

#[tokio::test]
async fn hook_runner_post_hooks_noop_when_empty() {
    let runner = HookRunner::new();
    let ctx = HookRunner::build_context(
        "test",
        TypeKind::Entity,
        CrudOperation::Insert,
        vec![],
        None,
    );
    // Should be a no-op
    runner.run_post_hooks(&ctx).await;
}
