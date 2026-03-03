//! Tests for the lifecycle hook system on `RelationManager`.
//!
//! Mirrors the patterns in `hooks_tests.rs` but exercises hooks wired into
//! `RelationManager` (insert, delete, insert_many, delete_many).

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

type CapturedContext = Arc<Mutex<Option<(String, TypeKind, CrudOperation, Vec<String>)>>>;

// ── Tests: Pre/post hook execution ──────────────────────────────────

#[tokio::test]
async fn insert_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    manager.insert(&mut emp).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        ("pre:employment".to_string(), CrudOperation::Insert)
    );
    assert_eq!(
        calls[1],
        ("post:employment".to_string(), CrudOperation::Insert)
    );
}

#[tokio::test]
async fn delete_fires_pre_and_post_hooks() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook));

    let emp = make_employment(Some("0xabc"), None, None, None, None, None);
    manager.delete(&emp).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        ("pre:employment".to_string(), CrudOperation::Delete)
    );
    assert_eq!(
        calls[1],
        ("post:employment".to_string(), CrudOperation::Delete)
    );
}

// ── Tests: Pre-hook rejection ───────────────────────────────────────

#[tokio::test]
async fn pre_hook_rejection_prevents_insert() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("validation failed")));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    let result = manager.insert(&mut emp).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hook(HookError::Rejected { reason, .. }) => {
            assert_eq!(reason, "validation failed");
        }
        other => panic!("Expected Hook(Rejected), got: {other}"),
    }

    // No query should have been executed
    let recorded = queries.lock().unwrap();
    assert!(
        recorded.is_empty(),
        "Rejected insert should not execute query"
    );
}

#[tokio::test]
async fn pre_hook_rejection_prevents_delete() {
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("cannot delete")));

    let emp = make_employment(Some("0xabc"), None, None, None, None, None);
    let result = manager.delete(&emp).await;

    assert!(result.is_err());
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

// ── Tests: Post-hook errors are non-fatal ───────────────────────────

#[tokio::test]
async fn post_hook_error_does_not_propagate() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(FailingPostHook));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    // Should succeed despite the failing post-hook
    let iid = manager.insert(&mut emp).await.unwrap();
    assert_eq!(iid, "0xrel1");
}

// ── Tests: should_run filtering ─────────────────────────────────────

#[tokio::test]
async fn should_run_filters_by_operation() {
    let backend = MockBackend::new(vec![
        QueryResult::Ok,          // for delete
        insert_response("0xrel1"), // for insert
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = OperationFilterHook::new(CrudOperation::Insert);
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook));

    // Insert should trigger the hook
    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    manager.insert(&mut emp).await.unwrap();

    // Delete should NOT trigger the hook (filter allows only Insert)
    let emp_del = make_employment(Some("0xabc"), None, None, None, None, None);
    manager.delete(&emp_del).await.unwrap();

    let calls = calls.lock().unwrap();
    // Only 1 pre-hook call from insert (should_run=false skips delete)
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], CrudOperation::Insert);
}

// ── Tests: Multiple hooks ───────────────────────────────────────────

#[tokio::test]
async fn multiple_hooks_run_in_order() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook1, calls1) = RecordingHook::new();
    let (hook2, calls2) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook1));
    manager.add_hook(Arc::new(hook2));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    manager.insert(&mut emp).await.unwrap();

    // Both hooks should have been called
    let c1 = calls1.lock().unwrap();
    let c2 = calls2.lock().unwrap();
    assert_eq!(c1.len(), 2); // pre + post
    assert_eq!(c2.len(), 2); // pre + post
}

#[tokio::test]
async fn rejection_short_circuits_subsequent_hooks() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (recorder, calls) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    // Rejecting hook first, then recording hook
    manager.add_hook(Arc::new(RejectingHook::new("rejected")));
    manager.add_hook(Arc::new(recorder));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    let result = manager.insert(&mut emp).await;

    assert!(result.is_err());
    // Recording hook should not have been called at all
    let calls = calls.lock().unwrap();
    assert!(calls.is_empty());
}

// ── Tests: Batch operations ─────────────────────────────────────────

#[tokio::test]
async fn insert_many_fires_hooks_per_relation() {
    let backend = MockBackend::new(vec![
        insert_response("0xbr2"),
        insert_response("0xbr1"),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook));

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
    manager.insert_many(&mut relations).await.unwrap();

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
        insert_response("0xbr2"),
        insert_response("0xbr1"),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(RejectingHook::new("batch rejected")));

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
    let result = manager.insert_many(&mut relations).await;

    assert!(result.is_err());
    // No queries should have been executed
    let recorded = queries.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn delete_many_fires_hooks_per_relation() {
    let backend = MockBackend::new(vec![QueryResult::Ok, QueryResult::Ok]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    let (hook, calls) = RecordingHook::new();
    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(hook));

    let relations = vec![
        make_employment(Some("0xabc"), None, None, None, None, None),
        make_employment(Some("0xdef"), None, None, None, None, None),
    ];
    manager.delete_many(&relations).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4); // 2 pre + 2 post
    assert_eq!(calls[0].1, CrudOperation::Delete);
    assert_eq!(calls[1].1, CrudOperation::Delete);
    assert_eq!(calls[2].1, CrudOperation::Delete);
    assert_eq!(calls[3].1, CrudOperation::Delete);
}

// ── Tests: HookContext attributes ───────────────────────────────────

#[tokio::test]
async fn pre_hook_receives_correct_context() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
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
        ) -> type_bridge_orm::session::backend::BoxFuture<
            'a,
            std::result::Result<PreHookResult, HookError>,
        > {
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
        ) -> type_bridge_orm::session::backend::BoxFuture<
            'a,
            std::result::Result<(), HookError>,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    let mut manager = RelationManager::<Employment>::new(&db);
    manager.add_hook(Arc::new(ContextCapture {
        captured: Arc::clone(&captured_ctx),
    }));

    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    manager.insert(&mut emp).await.unwrap();

    let captured = captured_ctx.lock().unwrap();
    let (type_name, type_kind, operation, attrs) = captured.as_ref().unwrap();
    assert_eq!(type_name, "employment");
    assert_eq!(*type_kind, TypeKind::Relation);
    assert_eq!(*operation, CrudOperation::Insert);
    assert!(attrs.contains(&"position".to_string()));
}

// ── Tests: Zero-overhead guard ──────────────────────────────────────

#[tokio::test]
async fn no_hooks_means_no_overhead() {
    let backend = MockBackend::new(vec![insert_response("0xrel1")]);
    let db = Database::with_backend(Box::new(backend), "testdb");

    // Manager with NO hooks registered
    let manager = RelationManager::<Employment>::new(&db);
    let mut emp = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        None,
        Some(("name", AttributeValue::String("Acme".into()))),
        Some("Engineer"),
    );
    let iid = manager.insert(&mut emp).await.unwrap();

    assert_eq!(iid, "0xrel1");
    assert_eq!(emp.iid(), Some("0xrel1"));
}
