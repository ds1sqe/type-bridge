//! Tests for the lifecycle hook system.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use type_bridge_orm::session::backend::{BoxFuture, QueryResult};
use type_bridge_orm::*;

type CapturedContext = Arc<Mutex<Option<(String, TypeKind, CrudOperation, Vec<String>)>>>;

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

    let captured_ctx: CapturedContext = Arc::new(Mutex::new(None));

    struct ContextCapture {
        captured: CapturedContext,
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
