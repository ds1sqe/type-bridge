//! `/v3/staged` against a live TypeDB 3.13.6 (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT); run it explicitly with `-- --ignored`.
//!
//! The scenarios are the one-query race programs shared with the ORM's
//! live plan execution test, executed through the proxy and directly. The
//! Rust SDK's live plan-builder tests share the same proxy fixture from
//! `crates/rust/tests/plan_builder_live.rs`.

#[path = "support/staged_live.rs"]
mod staged_live;

#[path = "support/deadline_live.rs"]
mod deadline_live;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use type_bridge_contract::query_plan::InputRow;
use type_bridge_contract::query_plan::v3::{Operation, QueryPlanV3};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v3::{
    StagedAnswerV3, StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3, StagedRequestV3,
    VerifiedStagedReplyV3,
};
use type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_core_lib::version::semantic_profile_id;
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2_remote::Ed25519RemoteReplyVerifier;
use type_bridge_orm::session::staged::TransactionOptions;

use staged_live::{
    LivePolicy, Proxy, SEQUENCE, call, drop_database, proxy, race, row_count, seed, seeded, state,
};

fn text(value: &str) -> Option<CanonicalValue> {
    Some(CanonicalValue::String(
        CanonicalString::new(value).expect("text"),
    ))
}

/// A staged entry: the purchase template bound to one order, or a plan as is.
fn purchase_entry(proxy: &Proxy, order: &str, item: &str, account: &str) -> StagedPlanV3 {
    let row = InputRow::new(vec![text(order), text(item), text(account)]);
    StagedPlanV3::new(&race::purchase_template(), vec![row], &proxy.declared).expect("entry")
}

fn plain_entry(proxy: &Proxy, plan: &QueryPlanV3) -> StagedPlanV3 {
    StagedPlanV3::new(plan, vec![], &proxy.declared).expect("entry")
}

fn request(proxy: &Proxy, plans: Vec<StagedPlanV3>) -> StagedRequestV3 {
    let nonce = format!(
        "tb-live-staged-nonce-{}-{:06}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let spec = StagedRequestSpecV3 {
        nonce,
        kind: None,
        limits: StagedLimitsV3::default(),
        plans,
        prepared_at_unix_ms: u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .expect("millis"),
    };
    StagedRequestV3::new(spec, &proxy.declared, &proxy.advertisement).expect("staged request")
}

async fn post(proxy: &Proxy, request: &StagedRequestV3) -> Vec<u8> {
    let body = request.encode().expect("request bytes");
    call(&proxy.router, "POST", "/v3/staged", body).await
}

fn verify(proxy: &Proxy, request: &StagedRequestV3, reply: &[u8]) -> VerifiedStagedReplyV3 {
    VerifiedStagedReplyV3::decode(
        reply,
        request,
        &proxy.advertisement,
        proxy.advertisement.reply_key(),
        &Ed25519RemoteReplyVerifier,
    )
    .expect("signed staged reply verifies")
}

fn failure_code(reply: &VerifiedStagedReplyV3) -> String {
    reply.diagnostic().expect("failed reply").code().to_owned()
}

#[tokio::test]
async fn mismatching_bulk_documents_roll_back_direct_and_signed_proxy_writes() {
    use std::sync::Mutex;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::query_plan::v3::PlanBuilder;
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_orm::OrmError;
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
    };
    use type_bridge_orm::session::database::Database;

    #[derive(Default)]
    struct State {
        committed: usize,
        events: Vec<&'static str>,
    }
    struct Backend(Arc<Mutex<State>>);
    struct Transaction {
        state: Arc<Mutex<State>>,
        pending: usize,
    }
    impl DriverBackend for Backend {
        fn open_transaction(
            &self,
            _: &str,
            _: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            let transaction = Transaction {
                state: Arc::clone(&self.0),
                pending: 0,
            };
            Box::pin(async { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }
        fn is_open(&self) -> bool {
            true
        }
        fn supports_given_rows(&self) -> bool {
            true
        }
        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            "3.13.6".parse().ok()
        }
        fn schema_text(&self, _: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            let schema = race::SCHEMA_TYPEQL
                .replace(" @key", "")
                .replace(" @card(0..1)", "");
            Box::pin(async move { Ok(schema) })
        }
    }
    impl TransactionOps for Transaction {
        fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.pending += 1;
            self.state.lock().unwrap().events.push("query");
            Box::pin(async {
                Ok(QueryResult::Documents(vec![
                    serde_json::json!({"_iid":"0x01","_type":"item","attributes":{"item-id":"I1","stock":999}}),
                ]))
            })
        }
        fn query_with_rows(
            &mut self,
            query: &str,
            _: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.query(query)
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let mut state = self.state.lock().unwrap();
            state.committed += self.pending;
            self.pending = 0;
            state.events.push("commit");
            Box::pin(async { Ok(()) })
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.pending = 0;
            self.state.lock().unwrap().events.push("rollback");
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().events.push("close");
            Box::pin(async { Ok(()) })
        }
    }
    let mut builder = PlanBuilder::new();
    let item = TypeId::new(TypeKind::Entity, "item").unwrap();
    let created = builder.insert(item.clone());
    let field = |name| OwnsFactId::new(item.clone(), AttributeId::new(name).unwrap()).unwrap();
    builder
        .bind_model_rows(
            created,
            vec![vec![
                (field("item-id"), text("I1").unwrap()),
                (field("stock"), CanonicalValue::Long(1)),
            ]],
        )
        .fetch_held(created, vec![]);
    let built = builder
        .build_plans(
            &race::schema(),
            race::purchase_template().managed_semantics().clone(),
            false,
            type_bridge_contract::split::SplitLimits::CANONICAL,
        )
        .unwrap();
    let plan = &built.plans()[0];
    for proxy_transport in [false, true] {
        let state = Arc::new(Mutex::new(State::default()));
        let database = Database::with_backend(
            Box::new(Backend(Arc::clone(&state))),
            "type_bridge_bulk_validation",
        );
        if proxy_transport {
            let proxy = staged_live::proxy_with_database(
                "type_bridge_bulk_validation",
                LivePolicy {
                    vouch: true,
                    ..Default::default()
                },
                false,
                race::schema(),
                "type_bridge_bulk_validation",
                database,
            )
            .await;
            let entry =
                StagedPlanV3::new(plan, plan.given_rows().unwrap().to_vec(), &proxy.declared)
                    .unwrap();
            let request = request(&proxy, vec![entry]);
            let reply = verify(&proxy, &request, &post(&proxy, &request).await);
            assert!(!reply.committed());
            assert_eq!(failure_code(&reply), "query_remote_v3_document_value");
        } else {
            let mut transaction = database.transaction(TransactionOptions::default());
            let pending = transaction.stage_plan(plan).unwrap();
            assert_eq!(
                transaction
                    .commit()
                    .await
                    .unwrap_err()
                    .diagnostic()
                    .code()
                    .as_str(),
                "query_remote_v3_document_value"
            );
            assert!(pending.get().is_err());
        }
        let state = state.lock().unwrap();
        assert_eq!(state.committed, 0);
        assert_eq!(state.events, ["query", "rollback", "close"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn staged_purchase_matches_direct_transaction_answers_and_state() {
    if !staged_live::given_rows_supported(
        "staged_purchase_matches_direct_transaction_answers_and_state",
    )
    .await
    {
        return;
    }
    let (_, direct) = seeded("direct").await;
    let (proxy_name, proxied) = seeded("proxy").await;
    let proxy = proxy(&proxy_name, LivePolicy::default(), true).await;

    // Two transactions, the same plans each way: the purchase that takes the
    // last item, then a refused second purchase with reads and a delete.
    let reads = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
        race::drop_price(),
    ];
    let mut direct_answers = Vec::new();
    let mut tx = direct.transaction(TransactionOptions::default());
    let bought = tx
        .stage_plan(&race::purchase("O1", "I1", "A1"))
        .expect("stage");
    tx.commit().await.expect("commit");
    let first = race::purchase_template();
    direct_answers.push(
        bought
            .get()
            .expect("answer")
            .to_staged_v3(&first)
            .expect("v3"),
    );
    let refused = tx
        .stage_plan(&race::purchase("O2", "I1", "A1"))
        .expect("stage");
    let pending = reads
        .iter()
        .map(|plan| tx.stage_plan(plan).expect("stage"))
        .collect::<Vec<_>>();
    tx.commit().await.expect("commit");
    direct_answers.push(
        refused
            .get()
            .expect("answer")
            .to_staged_v3(&first)
            .expect("v3"),
    );
    for (answer, plan) in pending.iter().zip(&reads) {
        direct_answers.push(
            answer
                .get()
                .expect("answer")
                .to_staged_v3(plan)
                .expect("v3"),
        );
    }

    let mut proxy_answers: Vec<StagedAnswerV3> = Vec::new();
    let first_request = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")]);
    let reply = verify(&proxy, &first_request, &post(&proxy, &first_request).await);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    proxy_answers.extend_from_slice(reply.answers());
    let mut entries = vec![purchase_entry(&proxy, "O2", "I1", "A1")];
    entries.extend(reads.iter().map(|plan| plain_entry(&proxy, plan)));
    let second_request = request(&proxy, entries);
    let reply = verify(
        &proxy,
        &second_request,
        &post(&proxy, &second_request).await,
    );
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    proxy_answers.extend_from_slice(reply.answers());

    assert_eq!(
        direct_answers,
        [
            StagedAnswerV3::Matched { value: 1 },
            StagedAnswerV3::Matched { value: 0 },
            StagedAnswerV3::Count { value: 1 },
            StagedAnswerV3::Exists { value: true },
            StagedAnswerV3::Matched { value: 1 },
        ]
    );
    assert_eq!(proxy_answers, direct_answers);
    assert_eq!(state(&direct).await, (1, 1, 1, 0));
    assert_eq!(state(&proxied).await, state(&direct).await);

    drop(proxy);
    drop_database(direct).await;
    drop_database(proxied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn library_entry_purchase_matches_the_router() {
    if !staged_live::given_rows_supported("library_entry_purchase_matches_the_router").await {
        return;
    }
    let (routed_name, routed) = seeded("routed").await;
    let (entered_name, entered) = seeded("entered").await;
    let through_router = proxy(&routed_name, LivePolicy::default(), true).await;
    let through_entry = proxy(&entered_name, LivePolicy::default(), true).await;

    let reads = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
    ];
    let mut entries = vec![purchase_entry(&through_router, "O1", "I1", "A1")];
    entries.extend(reads.iter().map(|plan| plain_entry(&through_router, plan)));
    let routed_request = request(&through_router, entries);
    let routed_reply = verify(
        &through_router,
        &routed_request,
        &post(&through_router, &routed_request).await,
    );
    assert!(routed_reply.committed(), "{:?}", routed_reply.diagnostic());

    let mut entries = vec![purchase_entry(&through_entry, "O1", "I1", "A1")];
    entries.extend(reads.iter().map(|plan| plain_entry(&through_entry, plan)));
    let entered_request = request(&through_entry, entries);
    let outcome = through_entry
        .execute_direct(entered_request.encode().expect("request bytes"))
        .await;
    assert_eq!(
        outcome.kind,
        type_bridge_server::transport::v3::StagedOutcomeKind::Committed
    );
    assert_eq!(outcome.code, "ok");
    let entered_reply = verify(&through_entry, &entered_request, &outcome.reply);
    assert!(
        entered_reply.committed(),
        "{:?}",
        entered_reply.diagnostic()
    );

    assert_eq!(entered_reply.answers(), routed_reply.answers());
    assert_eq!(
        entered_reply.answers(),
        [
            StagedAnswerV3::Matched { value: 1 },
            StagedAnswerV3::Count { value: 1 },
            StagedAnswerV3::Exists { value: true },
        ]
    );
    assert_eq!(state(&entered).await, state(&routed).await);
    assert_eq!(
        through_entry.commits.lock().expect("commits").len(),
        through_router.commits.lock().expect("commits").len()
    );

    drop(through_router);
    drop(through_entry);
    drop_database(routed).await;
    drop_database(entered).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn unauthenticated_staged_write_is_refused_and_writes_nothing() {
    let (name, db) = seeded("unauthenticated").await;
    let proxy = proxy(&name, LivePolicy::default(), false).await;
    let staged = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")]);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "staged_write_not_authorized");
    assert_eq!(state(&db).await, (0, 0, 0, 1));
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn resent_write_bytes_apply_once_and_read_as_unknown() {
    if !staged_live::given_rows_supported("resent_write_bytes_apply_once_and_read_as_unknown").await
    {
        return;
    }
    let (name, db) = seeded("resent").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    // A fresh stock so a second application would also match and write.
    db.execute_raw(
        r#"match $i isa item, has item-id "I1", has stock $s; update $i has stock 5;"#,
        TxType::Write,
    )
    .await
    .expect("restock");
    let staged = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")]);
    let first = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(first.committed(), "{:?}", first.diagnostic());
    // The same bytes again: the spent nonce refuses them, and the earlier
    // copy committed, so the refusal reads as unknown, never as nothing
    // applied.
    let resent = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert_eq!(failure_code(&resent), "commit_outcome_unknown");
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(
        row_count(
            &db,
            r#"match $i isa item, has item-id "I1", has stock 4; select $i;"#
        )
        .await,
        1
    );
    assert_eq!(proxy.commits.lock().expect("commits").len(), 1);
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn reply_withheld_after_commit_reports_an_unknown_outcome() {
    if !staged_live::given_rows_supported("reply_withheld_after_commit_reports_an_unknown_outcome")
        .await
    {
        return;
    }
    let (name, db) = seeded("withheld").await;
    let withhold = Arc::new(AtomicBool::new(false));
    let policy = LivePolicy {
        vouch: true,
        withhold_next_reply: Arc::clone(&withhold),
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    withhold.store(true, Ordering::SeqCst);
    let staged = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")]);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    // The purchase committed, so the withheld reply must not read as a
    // safe-to-retry policy refusal.
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(proxy.commits.lock().expect("commits").len(), 1);
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn plan_policy_refuses_a_staged_delete_and_nothing_commits() {
    let (name, db) = seeded("policy").await;
    let policy = LivePolicy {
        vouch: true,
        refuse: Some("query.write.delete"),
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    // The purchase comes first; the delete refusal must still drop it.
    let entries = vec![
        purchase_entry(&proxy, "O1", "I1", "A1"),
        plain_entry(&proxy, &race::drop_price()),
    ];
    let staged = request(&proxy, entries);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
    assert_eq!(state(&db).await, (0, 0, 0, 1));
    // Refused before anything executed, so the commit observer never runs.
    assert!(proxy.commits.lock().expect("commits").is_empty());
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn commit_hook_observes_an_stc2_conflict_between_racing_staged_purchases() {
    if !staged_live::given_rows_supported(
        "commit_hook_observes_an_stc2_conflict_between_racing_staged_purchases",
    )
    .await
    {
        return;
    }
    let (name, db) = seeded("race").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = Arc::new(proxy(&name, policy, false).await);
    // Two purchases of the same last item race; when their snapshots
    // overlap, one commits and the other is rejected with STC2. Overlap is
    // timing dependent, so race fresh items until one round conflicts.
    for round in 0..40 {
        let (item, account) = (format!("R{round}"), format!("B{round}"));
        seed(&db, &item, &account).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let racers = ["a", "b"].map(|side| {
            let proxy = Arc::clone(&proxy);
            let barrier = Arc::clone(&barrier);
            let order = format!("O{round}{side}");
            let (item, account) = (item.clone(), account.clone());
            tokio::spawn(async move {
                let staged = request(
                    &proxy,
                    vec![purchase_entry(&proxy, &order, &item, &account)],
                );
                let body = staged.encode().expect("request bytes");
                barrier.wait().await;
                let reply = call(&proxy.router, "POST", "/v3/staged", body).await;
                verify(&proxy, &staged, &reply)
            })
        });
        let mut replies = Vec::new();
        for racer in racers {
            replies.push(racer.await.expect("racer"));
        }
        let bought = row_count(
            &db,
            &format!(r#"match $i isa item, has item-id "{item}"; $p isa purchase, links (item: $i); select $p;"#),
        )
        .await;
        assert_eq!(bought, 1, "exactly one racer buys the last item");
        let conflicted = replies.iter().filter(|reply| !reply.committed()).count();
        if conflicted == 0 {
            continue;
        }
        assert_eq!(conflicted, 1);
        let failed = replies
            .iter()
            .find(|reply| !reply.committed())
            .expect("failed");
        assert_eq!(failure_code(failed), "commit_definitely_aborted");
        let commits = proxy.commits.lock().expect("commits").clone();
        assert!(
            commits.contains(&(
                false,
                "commit_definitely_aborted".to_owned(),
                Some(SdkCommitFailureOutcome::DefinitelyAborted),
                Some("STC2".to_owned()),
            )),
            "{commits:?}"
        );
        drop(proxy);
        drop_database(db).await;
        return;
    }
    panic!("no racing round produced an STC2 conflict");
}

// The SDK seam: the core `Transaction` over the proxy route.

/// A staged transaction whose exchange posts to the in-process router.
/// `drop_first` loses the first reply after the server handled it.
fn sdk_transaction(
    proxy: &Proxy,
    drop_first: bool,
) -> (
    type_bridge_orm::session::staged::Transaction,
    Arc<AtomicU64>,
) {
    let sent = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&sent);
    let router = proxy.router.clone();
    let dropped = Arc::new(AtomicBool::new(!drop_first));
    let exchange: type_bridge_orm::session::StagedExchangeFn = Arc::new(move |bytes| {
        let router = router.clone();
        let counter = Arc::clone(&counter);
        let dropped = Arc::clone(&dropped);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = call(&router, "POST", "/v3/staged", bytes).await;
            if dropped.swap(true, Ordering::SeqCst) {
                Ok(reply)
            } else {
                Err(
                    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::provider_failure(
                        type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write,
                    ),
                )
            }
        })
    });
    let advertisement = call_blocking_free(&proxy.advertisement);
    let tx = type_bridge_orm::session::staged::Transaction::over_proxy(
        proxy.declared.clone(),
        &advertisement,
        exchange,
        TransactionOptions::default(),
    )
    .expect("proxy transaction");
    (tx, sent)
}

fn call_blocking_free(advertisement: &RemoteCapabilities) -> Vec<u8> {
    advertisement.encode().expect("advertisement bytes")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn sdk_staged_purchase_over_the_proxy_matches_direct() {
    if !staged_live::given_rows_supported("sdk_staged_purchase_over_the_proxy_matches_direct").await
    {
        return;
    }
    use type_bridge_orm::session::staged::PlanAnswer;
    let (_, direct) = seeded("sdk-direct").await;
    let (name, proxied) = seeded("sdk-proxy").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    let plans = [
        race::purchase("O1", "I1", "A1"),
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
    ];
    let mut tx = direct.transaction(TransactionOptions::default());
    let direct_pending: Vec<_> = plans
        .iter()
        .map(|plan| tx.stage_plan(plan).expect("stage"))
        .collect();
    tx.commit().await.expect("direct commit");
    let (mut remote, sent) = sdk_transaction(&proxy, false);
    let remote_pending: Vec<_> = plans
        .iter()
        .map(|plan| remote.stage_plan(plan).expect("stage"))
        .collect();
    remote.commit().await.expect("proxy commit");
    let answers = |pending: &[type_bridge_orm::session::staged::Pending<PlanAnswer>]| {
        pending
            .iter()
            .map(|answer| (*answer.get().expect("answer")).clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(answers(&direct_pending), answers(&remote_pending));
    assert_eq!(answers(&remote_pending)[0], PlanAnswer::Matched(1));
    assert_eq!(state(&direct).await, state(&proxied).await);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    // A raw query never travels over the proxy.
    assert!(
        remote
            .stage(type_bridge_orm::session::staged::StagedQuery::new(
                "match $x isa item;",
                false
            ))
            .is_err()
    );
    drop(proxy);
    drop_database(direct).await;
    drop_database(proxied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn sdk_dropped_reply_is_unknown_and_the_database_tells() {
    if !staged_live::given_rows_supported("sdk_dropped_reply_is_unknown_and_the_database_tells")
        .await
    {
        return;
    }
    let (name, db) = seeded("sdk-lost-reply").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    let (mut tx, sent) = sdk_transaction(&proxy, true);
    let bought = tx
        .stage_plan(&race::purchase("O1", "I1", "A1"))
        .expect("stage");
    let error = tx.commit().await.expect_err("the reply was dropped");
    assert_eq!(error.diagnostic().code().as_str(), "commit_outcome_unknown");
    assert!(
        bought.get().is_err(),
        "an unknown outcome publishes nothing"
    );
    // The caller reads the database: the purchase applied once.
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(proxy.commits.lock().expect("commits").len(), 1);
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn sdk_unauthenticated_write_is_refused_with_a_typed_error() {
    let (name, db) = seeded("sdk-unauthenticated").await;
    let proxy = proxy(&name, LivePolicy::default(), false).await;
    let (mut tx, _) = sdk_transaction(&proxy, false);
    let bought = tx
        .stage_plan(&race::purchase("O1", "I1", "A1"))
        .expect("stage");
    let error = tx.commit().await.expect_err("refused");
    // The signed refusal keeps its code and applied nothing.
    assert_eq!(
        error.error.commit_failure_certainty(),
        Some(type_bridge_orm::error::CommitFailureCertainty::DefinitelyAborted)
    );
    assert_eq!(
        error.diagnostic().code().as_str(),
        "staged_write_not_authorized"
    );
    assert!(bought.get().is_err());
    assert_eq!(state(&db).await, (0, 0, 0, 1));
    drop(proxy);
    drop_database(db).await;
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `writes` SDK conformance program's schema and managed scope.
const WRITES_BINDINGS: [&str; 4] = ["python", "node", "rust", "c"];

/// One suite of the `writes` SDK conformance suite: its schema fragment,
/// workspace, managed scope, live TypeQL and state checks.
struct WritesSuite {
    document: &'static str,
    schema: &'static str,
    workspace: &'static str,
    scope: &'static str,
    /// Live TypeQL, matching the declared schema exactly.
    typeql: fn() -> String,
    /// The program's state as the provider sees it, one count per check.
    state: &'static [(&'static str, &'static str)],
}

const WRITES_SUITES: [(&str, WritesSuite); 3] = [
    (
        "purchase",
        WritesSuite {
            document: "purchase.yaml",
            schema: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/purchase/schema/fragments/purchase.yaml"
            ),
            workspace: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/purchase/typebridge.yaml"
            ),
            scope: "sdk-writes-purchase",
            // The suite schema has no keys or cardinalities.
            typeql: || {
                race::SCHEMA_TYPEQL
                    .replace(" @key", "")
                    .replace(" @card(0..1)", "")
            },
            state: &[
                (
                    "sold_out",
                    r#"match $i isa item, has item-id "I1", has stock 0; select $i;"#,
                ),
                (
                    "charged",
                    r#"match $a isa account, has account-id "A1", has balance 60; select $a;"#,
                ),
                ("orders", "match $p isa purchase; select $p;"),
                (
                    "on_call",
                    "match $d isa doctor, has on-call true; select $d;",
                ),
            ],
        },
    ),
    (
        "constraints",
        WritesSuite {
            document: "constraints.yaml",
            schema: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/constraints/schema/fragments/constraints.yaml"
            ),
            workspace: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/constraints/typebridge.yaml"
            ),
            scope: "sdk-writes-constraints",
            typeql: || race::SCHEMA_TYPEQL.to_owned(),
            state: &[
                ("doctors", "match $d isa doctor; select $d;"),
                (
                    "on_call",
                    "match $d isa doctor, has on-call true; select $d;",
                ),
            ],
        },
    ),
    (
        "shop",
        WritesSuite {
            document: "shop.yaml",
            schema: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/shop/schema/fragments/shop.yaml"
            ),
            workspace: include_str!(
                "../../../../tests/contracts/sdk_conformance/writes/shop/typebridge.yaml"
            ),
            scope: "sdk-writes-shop",
            typeql: || SHOP_SCHEMA_TYPEQL.to_owned(),
            state: &[
                (
                    "a1_balance_54",
                    r#"match $a isa account, has account-id "A1", has balance 54; select $a;"#,
                ),
                (
                    "a2_balance_0",
                    r#"match $a isa account, has account-id "A2", has balance 0; select $a;"#,
                ),
                (
                    "book_stock_0",
                    r#"match $i isa item, has sku "book", has stock 0; select $i;"#,
                ),
                (
                    "pen_stock_2",
                    r#"match $i isa item, has sku "pen", has stock 2; select $i;"#,
                ),
                (
                    "mug_stock_4",
                    r#"match $i isa item, has sku "mug", has stock 4; select $i;"#,
                ),
                (
                    "lamp_stock_0",
                    r#"match $i isa item, has sku "lamp", has stock 0; select $i;"#,
                ),
                (
                    "r1_balance_90",
                    r#"match $a isa account, has account-id "R1", has balance 90; select $a;"#,
                ),
                ("order_lines", "match $l isa order-line; select $l;"),
            ],
        },
    ),
];

/// The live schema of the `shop` writes suite, exactly its `shop.yaml`.
const SHOP_SCHEMA_TYPEQL: &str = "define
attribute account-id, value string; attribute balance, value integer;
attribute sku, value string; attribute price, value integer; attribute stock, value integer;
attribute order-id, value string; attribute quantity, value integer;
entity account, owns account-id @key, owns balance @card(0..1), plays order-line:buyer;
entity item, owns sku @key, owns price @card(0..1), owns stock @card(0..1), plays order-line:item;
relation order-line, relates buyer, relates item, owns order-id @card(0..1), owns quantity @card(0..1);";

async fn writes_state(
    suite: &WritesSuite,
    db: &type_bridge_orm::session::database::Database,
) -> serde_json::Value {
    let mut state = serde_json::Map::new();
    for (name, query) in suite.state {
        state.insert((*name).to_owned(), row_count(db, query).await.into());
    }
    serde_json::Value::Object(state)
}

/// Not a test: the live fixture of the `writes` SDK conformance suite. With
/// `TB_SDK_WRITES_FIXTURE_DIR` set, for the suite `TB_SDK_WRITES_SUITE`
/// names (default `purchase`), it creates, per binding, an empty direct
/// database and an empty database behind a vouching proxy, both holding
/// the suite schema; the proxy serves the suite's declared schema under its
/// managed scope, so plans built from the generated packages verify there.
/// It writes `fixture.json`, serves until a `stop` file appears, then
/// writes `observed.json` (proxy commits and the state of every database)
/// and drops the databases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "SDK writes fixture; requires TB_SDK_WRITES_FIXTURE_DIR and a live TypeDB"]
async fn sdk_writes_fixture_serves_until_stopped() {
    let Ok(dir) = std::env::var("TB_SDK_WRITES_FIXTURE_DIR") else {
        return;
    };
    let suite_name = std::env::var("TB_SDK_WRITES_SUITE").unwrap_or_else(|_| "purchase".into());
    let (_, suite) = WRITES_SUITES
        .iter()
        .find(|(name, _)| *name == suite_name)
        .expect("a known writes suite");
    assert!(
        suite
            .workspace
            .contains(&format!("managed-scope: {}\n", suite.scope)),
        "the fixture serves the workspace's managed scope"
    );
    let dir = std::path::PathBuf::from(dir);
    let documents = type_bridge_schema::SchemaDocumentSet::parse([(
        type_bridge_contract::schema::DocumentId::new(suite.document).expect("document id"),
        suite.schema,
    )])
    .expect("suite schema parses");
    let declared = type_bridge_schema::normalize_documents(&documents).expect("suite schema");
    // The live-authority check is exact: the live schema is the declared one.
    let typeql = (suite.typeql)();
    let mut bindings = serde_json::Map::new();
    let mut held = Vec::new();
    let mut profile = None;
    for binding in WRITES_BINDINGS {
        let mut databases = Vec::new();
        for transport in ["direct", "proxy"] {
            let prefix = std::env::var("TB_SDK_WRITES_DATABASE_PREFIX")
                .unwrap_or_else(|_| "tb_sdk_writes".to_owned());
            let name = format!(
                "{prefix}_{binding}_{transport}_{}_{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let db = staged_live::connect(&name).await;
            db.create_database().await.expect("database created");
            db.execute_raw(&typeql, TxType::Schema)
                .await
                .expect("schema");
            profile.get_or_insert_with(|| {
                semantic_profile_id(&db.server_version().expect("server version"))
                    .expect("supported profile")
            });
            databases.push((name, db));
        }
        let policy = LivePolicy {
            vouch: true,
            ..LivePolicy::default()
        };
        let proxy = staged_live::proxy_with(
            &databases[1].0,
            policy,
            false,
            declared.clone(),
            suite.scope,
        )
        .await;
        let port = serve(&proxy).await;
        bindings.insert(
            binding.to_owned(),
            serde_json::json!({
                "direct_database": databases[0].0,
                "proxy_database": databases[1].0,
                "proxy_endpoint": format!("http://127.0.0.1:{port}"),
            }),
        );
        held.push((binding, proxy, databases));
    }
    let fixture = serde_json::json!({
        "profile": profile,
        "scope": suite.scope,
        "bindings": bindings,
    });
    std::fs::write(dir.join("fixture.json"), fixture.to_string()).expect("fixture written");
    while !dir.join("stop").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let mut observed = serde_json::Map::new();
    for (binding, proxy, databases) in &held {
        let commits: Vec<_> = proxy
            .commits
            .lock()
            .expect("commits")
            .iter()
            // A program retries a commit that lost a race (STC2); how many
            // attempts lose depends on timing, so they are not recorded.
            .filter(|(.., failure)| failure.as_deref() != Some("STC2"))
            .map(|(committed, code, ..)| format!("{committed}:{code}"))
            .collect();
        observed.insert(
            (*binding).to_owned(),
            serde_json::json!({
                "proxy_commits": commits,
                "direct_state": writes_state(suite, &databases[0].1).await,
                "proxy_state": writes_state(suite, &databases[1].1).await,
            }),
        );
    }
    std::fs::write(
        dir.join("observed.json"),
        serde_json::Value::Object(observed).to_string(),
    )
    .expect("observed written");
    for (_, proxy, databases) in held {
        drop(proxy);
        for (_, db) in databases {
            drop_database(db).await;
        }
    }
}

/// Serve one live proxy over HTTP. A request carrying `x-tb-drop-reply`
/// loses its first reply after the server handled it, as a dropped
/// connection would.
async fn serve(proxy: &Proxy) -> u16 {
    let dropped = Arc::new(AtomicBool::new(false));
    let router = proxy.router.clone().layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let dropped = Arc::clone(&dropped);
            async move {
                let drop_reply = request.headers().contains_key("x-tb-drop-reply")
                    && request.uri().path() == "/v3/staged";
                let response = next.run(request).await;
                if drop_reply && !dropped.swap(true, Ordering::SeqCst) {
                    let _ = response.into_body().collect().await;
                    return axum::response::Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(Body::from("reply lost"))
                        .expect("lost reply");
                }
                response
            }
        },
    ));
    static NEXT_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    let listener = if let Ok(range) = std::env::var("TB_STAGED_PROXY_PORT_RANGE") {
        let (first, last) = range.split_once('-').expect("proxy port range first-last");
        let first = first.parse::<u16>().expect("first proxy port");
        let last = last.parse::<u16>().expect("last proxy port");
        assert!(first > 0 && first <= last, "invalid proxy port range");
        loop {
            let port = first
                .checked_add(NEXT_PORT.fetch_add(1, Ordering::Relaxed))
                .filter(|port| *port <= last)
                .expect("assigned proxy port range exhausted");
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => break listener,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
                Err(error) => panic!("proxy bind: {error}"),
            }
        }
    } else {
        tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind")
    };
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move { axum::serve(listener, router).await });
    port
}

/// Not a test: the live fixture the Node and Python SDK suites stage
/// against. With `TB_STAGED_FIXTURE_DIR` set it seeds four databases,
/// serves a vouching proxy (purchase), a vouching proxy that loses one
/// reply on request (lost_reply) and a non-vouching proxy (refusal), writes
/// `fixture.json` there and serves until a `stop` file appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "SDK live fixture; requires TB_STAGED_FIXTURE_DIR and a live TypeDB"]
async fn sdk_staged_fixture_serves_until_stopped() {
    let Ok(dir) = std::env::var("TB_STAGED_FIXTURE_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let (direct_name, direct) = seeded("fixture-direct").await;
    let mut proxies = serde_json::Map::new();
    let mut held = Vec::new();
    for (role, vouch) in [("purchase", true), ("lost_reply", true), ("refused", false)] {
        let (name, db) = seeded(&format!("fixture-{role}")).await;
        let policy = LivePolicy {
            vouch,
            ..LivePolicy::default()
        };
        let proxy = proxy(&name, policy, false).await;
        let port = serve(&proxy).await;
        let commits = Arc::clone(&proxy.commits);
        proxies.insert(
            role.to_owned(),
            serde_json::json!({
                "database": name,
                "endpoint": format!("http://127.0.0.1:{port}"),
                "scope": format!("server-v3-live-{name}"),
            }),
        );
        held.push((proxy, db, commits));
    }
    let profile = semantic_profile_id(&direct.server_version().expect("server version"))
        .expect("supported profile");
    let plan = |plan: QueryPlanV3| hex(&plan.canonical_bytes().expect("plan bytes"));
    let fixture = serde_json::json!({
        "direct_database": direct_name,
        "profile": profile,
        "declared_hex": hex(
            &type_bridge_contract::schema::encode_declared_schema(&race::schema())
                .expect("declared bytes"),
        ),
        "plans_hex": {
            "purchase": plan(race::purchase("O1", "I1", "A1")),
            "count": plan(race::stock_read(Operation::Count)),
            "exists": plan(race::stock_read(Operation::Exists)),
        },
        "proxies": proxies,
    });
    std::fs::write(dir.join("fixture.json"), fixture.to_string()).expect("fixture written");
    while !dir.join("stop").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    // Commits the proxies observed, one line per proxy, for the caller to check.
    let observed: Vec<_> = held
        .iter()
        .map(|(_, _, commits)| {
            commits
                .lock()
                .expect("commits")
                .iter()
                .map(|(committed, code, ..)| format!("{committed}:{code}"))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut states = Vec::new();
    for (_, db, _) in &held {
        states.push(format!("{:?}", state(db).await));
    }
    std::fs::write(
        dir.join("observed.json"),
        serde_json::json!({ "commits": observed, "states": states, "direct": format!("{:?}", state(&direct).await) }).to_string(),
    )
    .expect("observed written");
    for (proxy, db, _) in held {
        drop(proxy);
        drop_database(db).await;
    }
    drop_database(direct).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn staged_fetch_documents_equal_direct_answers_with_role_evidence() {
    let (name, db) = seeded("fetch_docs").await;
    let proxy = proxy(&name, LivePolicy::default(), true).await;
    let mut tx = db.transaction(TransactionOptions::default());
    tx.stage_plan(&race::purchase("O1", "I1", "A1")).unwrap();
    tx.commit().await.unwrap();
    let insert = race::insert_doctor_fetch("bob");
    let inserted_request = request(&proxy, vec![plain_entry(&proxy, &insert)]);
    let inserted_reply = verify(
        &proxy,
        &inserted_request,
        &post(&proxy, &inserted_request).await,
    );
    assert!(
        inserted_reply.committed(),
        "{:?}",
        inserted_reply.diagnostic()
    );
    assert!(
        matches!(&inserted_reply.answers()[0], StagedAnswerV3::Documents { documents } if documents.len() == 1)
    );
    // Reading the same persisted things preserves their exact identities.
    for plan in [race::touch_fetch(), race::purchase_fetch()] {
        let pending = tx.stage_plan(&plan).unwrap();
        tx.commit().await.unwrap();
        let expected = pending.get().unwrap().to_staged_v3(&plan).unwrap();
        assert!(
            matches!(&expected, StagedAnswerV3::Documents { documents } if !documents.is_empty())
        );
        let request = request(&proxy, vec![plain_entry(&proxy, &plan)]);
        let reply = verify(&proxy, &request, &post(&proxy, &request).await);
        assert!(reply.committed(), "{:?}", reply.diagnostic());
        assert_eq!(reply.answers(), [expected]);
    }
    db.execute_raw(
        "match $p isa purchase, links (item: $i); delete links (item: $i) of $p;",
        TxType::Write,
    )
    .await
    .unwrap();
    let plan = race::purchase_fetch();
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let expected = pending.get().unwrap().to_staged_v3(&plan).unwrap();
    let optional_request = request(&proxy, vec![plain_entry(&proxy, &plan)]);
    let reply = verify(
        &proxy,
        &optional_request,
        &post(&proxy, &optional_request).await,
    );
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    assert_eq!(reply.answers(), [expected]);

    let large_name = "x".repeat(10_000);
    db.execute_raw(
        &format!("insert $d isa doctor, has name \"{large_name}\", has on-call true;"),
        TxType::Write,
    )
    .await
    .unwrap();
    let plan = race::touch_fetch();
    let mut bounded = db.transaction(TransactionOptions::default().with_answer_limits(
        type_bridge_orm::session::backend::BoundedAnswerLimits {
            max_bytes: 4096,
            ..Default::default()
        },
    ));
    bounded.stage_plan(&plan).unwrap();
    let direct_failure = bounded.commit().await.unwrap_err().diagnostic();
    let mut bounded_spec = request(&proxy, vec![plain_entry(&proxy, &plan)])
        .encode()
        .unwrap();
    let mut wire: serde_json::Value = serde_json::from_slice(&bounded_spec).unwrap();
    wire["limits"]["max_bytes"] = 4096.into();
    bounded_spec = type_bridge_contract::codec::to_canonical_json(&wire).unwrap();
    let bounded_request =
        StagedRequestV3::decode(&bounded_spec, &proxy.declared, &proxy.advertisement).unwrap();
    let reply = verify(
        &proxy,
        &bounded_request,
        &post(&proxy, &bounded_request).await,
    );
    assert_eq!(failure_code(&reply), "response_byte_limit");
    let remote_failure = reply.diagnostic().unwrap().host_json().unwrap();
    let direct_failure = serde_json::to_value(direct_failure.host_json()).unwrap();
    assert_eq!(remote_failure["details"], direct_failure["details"]);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB"]
async fn staged_deletion_snapshots_equal_direct_pre_images() {
    let (name, db) = seeded("delete_snapshots").await;
    db.execute_raw(
        "insert $d isa doctor, has name \"alice\", has on-call true;",
        TxType::Write,
    )
    .await
    .unwrap();
    let proxy = proxy(&name, LivePolicy::default(), true).await;
    let plan = race::delete_doctors_returning();
    let mut direct = db.transaction(TransactionOptions::default().with_never_commit(true));
    let pending = direct.stage_plan(&plan).unwrap();
    direct.commit().await.unwrap();
    let expected = pending.get().unwrap().to_staged_v3(&plan).unwrap();
    assert!(matches!(&expected, StagedAnswerV3::Documents { documents } if !documents.is_empty()));
    let deletion = request(&proxy, vec![plain_entry(&proxy, &plan)]);
    let reply = verify(&proxy, &deletion, &post(&proxy, &deletion).await);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    assert_eq!(reply.answers(), [expected]);
    let empty = request(&proxy, vec![plain_entry(&proxy, &plan)]);
    let reply = verify(&proxy, &empty, &post(&proxy, &empty).await);
    assert!(reply.committed());
    assert!(
        matches!(&reply.answers()[0], StagedAnswerV3::Documents { documents } if documents.is_empty())
    );
    drop_database(db).await;
}

/// Includes commit and answer materialization; setup is outside the timed region.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TYPE_BRIDGE_COST_LIVE=1 and a live TypeDB"]
async fn deletion_snapshot_cost_direct_and_staged() {
    use std::time::Instant;
    use type_bridge_orm::{GivenRowsSpec, GivenValue};

    if std::env::var("TYPE_BRIDGE_COST_LIVE").as_deref() != Ok("1") {
        return;
    }
    let (name, db) = seeded("delete_cost").await;
    let proxy = proxy(&name, LivePolicy::default(), true).await;
    let plans = [race::delete_doctors(), race::delete_doctors_returning()];
    let mut samples = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
    // One warm-up and three measured runs; alternate operation order.
    for run in 0..4 {
        for (route, route_samples) in samples.iter_mut().enumerate() {
            for operation in if run % 2 == 0 { [0, 1] } else { [1, 0] } {
                let tx = db.transaction_context(TxType::Write).await.unwrap();
                tx.query_with_rows(
                    "given $name: string; insert $d isa doctor, has name == $name, has on-call true;",
                    GivenRowsSpec {
                        variables: vec!["name".into()],
                        rows: (0..5000)
                            .map(|index| vec![GivenValue::String(format!("cost-{index}"))])
                            .collect(),
                    },
                )
                .await
                .unwrap();
                tx.commit_classified().await.unwrap();
                let plan = &plans[operation];
                let (elapsed, answer) = if route == 0 {
                    let mut transaction = db.transaction(TransactionOptions::default());
                    let pending = transaction.stage_plan(plan).unwrap();
                    let started = Instant::now();
                    transaction.commit().await.unwrap();
                    let answer = pending.get().unwrap().to_staged_v3(plan).unwrap();
                    (started.elapsed().as_secs_f64(), answer)
                } else {
                    let request = request(&proxy, vec![plain_entry(&proxy, plan)]);
                    let started = Instant::now();
                    let reply = verify(&proxy, &request, &post(&proxy, &request).await);
                    assert!(reply.committed(), "{:?}", reply.diagnostic());
                    (started.elapsed().as_secs_f64(), reply.answers()[0].clone())
                };
                match answer {
                    StagedAnswerV3::Documents { documents } => assert_eq!(documents.len(), 5000),
                    StagedAnswerV3::Matched { value } => assert_eq!(value, 5000),
                    _ => panic!("unexpected deletion answer"),
                }
                let remaining = db
                    .execute_raw(
                        "match $d isa doctor; fetch { \"iid\": iid($d) };",
                        TxType::Read,
                    )
                    .await
                    .unwrap();
                assert!(
                    matches!(remaining, type_bridge_orm::session::backend::QueryResult::Documents(documents) if documents.is_empty())
                );
                if run != 0 {
                    route_samples[operation].push(elapsed);
                }
            }
        }
    }
    for (route, mut samples) in samples.into_iter().enumerate() {
        for values in &mut samples {
            values.sort_by(f64::total_cmp);
        }
        let plain = samples[0][1];
        let returning = samples[1][1];
        println!(
            "delete-cost route={} rows=5000 runs=3 plain_median_s={plain:.6} returning_median_s={returning:.6} overhead_percent={:.2}",
            if route == 0 {
                "direct"
            } else {
                "staged-router"
            },
            (returning / plain - 1.0) * 100.0
        );
    }
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB"]
async fn deletion_snapshots_preserve_subtypes_and_players_across_routes() {
    use type_bridge_contract::query_remote::RemoteFieldValue;
    let (name, db) = seeded("delete_subtypes").await;
    db.execute_raw(
        "define entity doctor-report sub doctor; entity doctor-memo sub doctor;",
        TxType::Schema,
    )
    .await
    .unwrap();
    db.execute_raw(
        "insert $a isa doctor-report, has name \"report\", has on-call true; $b isa doctor-memo, has name \"memo\", has on-call true;",
        TxType::Write,
    ).await.unwrap();
    db.execute_raw(
        "match $a isa account; $i isa item; insert $p isa purchase, links (buyer: $a, item: $i), has order-id \"snapshot\";",
        TxType::Write,
    ).await.unwrap();
    let proxy = staged_live::proxy_with(
        &name,
        LivePolicy::default(),
        true,
        race::deletion_subtype_schema(),
        "delete-subtypes",
    )
    .await;
    let plans = [
        race::delete_doctor_subtypes_returning(),
        race::delete_purchases_returning(true),
    ];
    let mut direct = db.transaction(TransactionOptions::default().with_never_commit(true));
    let pending = plans
        .iter()
        .map(|plan| direct.stage_plan(plan).unwrap())
        .collect::<Vec<_>>();
    direct.commit().await.unwrap();
    let mut expected = pending
        .iter()
        .zip(&plans)
        .map(|(pending, plan)| pending.get().unwrap().to_staged_v3(plan).unwrap())
        .collect::<Vec<_>>();
    let StagedAnswerV3::Documents { documents } = &expected[0] else {
        panic!("expected subtype snapshots")
    };
    let mut labels = documents
        .iter()
        .map(|document| {
            let RemoteFieldValue::Scalar {
                value: CanonicalValue::String(label),
            } = &document["_type"]
            else {
                panic!("expected exact type")
            };
            label.as_str().to_owned()
        })
        .collect::<Vec<_>>();
    labels.sort();
    assert_eq!(labels, ["doctor-memo", "doctor-report"]);
    let StagedAnswerV3::Documents { documents } = &expected[1] else {
        panic!("expected relation snapshot")
    };
    assert_eq!(documents.len(), 1);
    for (index, label, attributes) in [
        (
            0,
            "account",
            vec![
                ("account-id", text("A1").unwrap()),
                ("balance", CanonicalValue::Long(100)),
            ],
        ),
        (
            1,
            "item",
            vec![
                ("item-id", text("I1").unwrap()),
                ("stock", CanonicalValue::Long(1)),
                ("price", CanonicalValue::Long(40)),
            ],
        ),
    ] {
        let RemoteFieldValue::Scalar {
            value: CanonicalValue::String(iid),
        } = &documents[0][&format!("_role_{index}_iid")]
        else {
            panic!("expected a present player IID")
        };
        assert!(type_bridge_contract::id::is_canonical_thing_iid(
            iid.as_str()
        ));
        assert_eq!(
            documents[0][&format!("_role_{index}_type")],
            RemoteFieldValue::Scalar {
                value: text(label).unwrap()
            }
        );
        let RemoteFieldValue::Document { fields } =
            &documents[0][&format!("_role_{index}_attributes")]
        else {
            panic!("expected stored player attributes")
        };
        for (name, expected) in attributes {
            match &fields[name] {
                RemoteFieldValue::Scalar { value } => assert_eq!(value, &expected),
                RemoteFieldValue::List { values } => assert_eq!(values, &[expected]),
                _ => panic!("expected a present player attribute: {name}"),
            }
        }
    }
    let deletion = request(
        &proxy,
        plans.iter().map(|plan| plain_entry(&proxy, plan)).collect(),
    );
    let reply = verify(&proxy, &deletion, &post(&proxy, &deletion).await);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    let mut actual = reply.answers().to_vec();
    // This plan has no ordering stage; compare the complete document sets.
    for answers in [&mut expected, &mut actual] {
        for answer in answers {
            if let StagedAnswerV3::Documents { documents } = answer {
                documents.sort_by_cached_key(|document| serde_json::to_string(document).unwrap());
            }
        }
    }
    assert_eq!(actual, expected);
    let absent = request(
        &proxy,
        plans.iter().map(|plan| plain_entry(&proxy, plan)).collect(),
    );
    let reply = verify(&proxy, &absent, &post(&proxy, &absent).await);
    assert!(reply.committed());
    assert!(reply.answers().iter().all(
        |answer| matches!(answer, StagedAnswerV3::Documents { documents } if documents.is_empty())
    ));
    for label in ["account", "item"] {
        let answer = db
            .execute_raw(
                &format!("match $p isa {label}; fetch {{ \"iid\": iid($p) }};"),
                TxType::Read,
            )
            .await
            .unwrap();
        assert!(
            matches!(answer, type_bridge_orm::session::backend::QueryResult::Documents(documents) if documents.len() == 1)
        );
    }
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn string_comparator_plans_and_semantic_limits_match_direct_and_staged() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_orm::session::backend::AnswerLimits;
    // Keep callsites aware that concurrent tests execute outside our dispatch.
    let _unlistened = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let name = staged_live::live_database::name("strings");
    let db = staged_live::connect(&name).await;
    db.create_database().await.unwrap();
    db.execute_raw(
        &race::SCHEMA_TYPEQL
            .replace(" @key", "")
            .replace(" @card(0..1)", ""),
        TxType::Schema,
    )
    .await
    .unwrap();
    db.execute_raw(r#"insert $a isa doctor, has name "Ada.L", has on-call true; $b isa doctor, has name "Grace", has on-call false;"#, TxType::Write).await.unwrap();
    let proxy = proxy(&name, LivePolicy::default(), true).await;
    for (comparator, needles) in [
        (ValueComparator::Contains, vec!["Ada", "Grace"]),
        (ValueComparator::StartsWith, vec!["Ada."]),
        (ValueComparator::EndsWith, vec![".L"]),
        (ValueComparator::Like, vec!["^Ada.*", "^Grace$"]),
    ] {
        let plan = race::string_read(comparator, &needles);
        let mut tx = db.transaction(TransactionOptions::default());
        let pending = tx.stage_plan(&plan).unwrap();
        let records = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = Arc::clone(&records);
        let dispatch = tracing::Dispatch::new(
            tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::new("type_bridge=trace"))
                .with(type_bridge_orm::debug::DebugLayer(Arc::new(move |event| {
                    captured.lock().unwrap().push(event.clone());
                }))),
        );
        tx.commit().with_subscriber(dispatch).await.unwrap();
        let statements: Vec<_> = records
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| {
                event
                    .fields
                    .get("statement")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(
            statements,
            type_bridge_orm::plan_explain::explain(&plan)
                .unwrap()
                .statements
        );
        let direct = pending.get().unwrap().to_staged_v3(&plan).unwrap();
        let entry =
            StagedPlanV3::new(&plan, plan.given_rows().unwrap().to_vec(), &proxy.declared).unwrap();
        let staged = request(&proxy, vec![entry]);
        let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
        assert_eq!(reply.answers(), &[direct]);
    }
    let type_bridge_orm::session::backend::QueryResult::Documents(identities) = db
        .execute_raw(
            r#"match $doctor isa doctor; fetch {"iid":iid($doctor)};"#,
            TxType::Read,
        )
        .await
        .unwrap()
    else {
        panic!("IID probe must return documents")
    };
    for (iid, name) in [
        (identities[0]["iid"].as_str().unwrap(), "nobody"),
        ("0x1e0000000000000000ffff", "Grace"),
    ] {
        let plan = race::doctor_iid_or_name(iid, name);
        let explanation = type_bridge_orm::plan_explain::explain(&plan).unwrap();
        assert!(!explanation.statements.join("\n").contains(iid));
        let mut tx = db.transaction(TransactionOptions::default());
        let pending = tx.stage_plan(&plan).unwrap();
        tx.commit().await.unwrap();
        let direct = pending.get().unwrap().to_staged_v3(&plan).unwrap();
        assert!(matches!(&direct, StagedAnswerV3::Documents { documents } if documents.len() == 1));
        let entry =
            StagedPlanV3::new(&plan, plan.given_rows().unwrap().to_vec(), &proxy.declared).unwrap();
        let staged = request(&proxy, vec![entry]);
        let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
        assert_eq!(reply.answers(), &[direct]);
    }
    let plan = race::rematch_doctor_window();
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let direct = pending.get().unwrap().to_staged_v3(&plan).unwrap();
    assert!(matches!(&direct, StagedAnswerV3::Documents { documents } if documents.len() == 1));
    let entry = StagedPlanV3::new(&plan, vec![], &proxy.declared).unwrap();
    let staged = request(&proxy, vec![entry]);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert_eq!(reply.answers(), &[direct]);
    let plan = race::string_read(ValueComparator::Contains, &["Ada"]);
    let mut tx = db.transaction(
        TransactionOptions::default().with_answer_limits(AnswerLimits {
            max_attribute_values: 0,
            ..AnswerLimits::default()
        }),
    );
    tx.stage_plan(&plan).unwrap();
    let error = tx.commit().await.unwrap_err();
    assert_eq!(
        error.diagnostic().code().as_str(),
        "hydrated_attribute_value_limit"
    );
    let entry =
        StagedPlanV3::new(&plan, plan.given_rows().unwrap().to_vec(), &proxy.declared).unwrap();
    let staged = StagedRequestV3::new(
        StagedRequestSpecV3 {
            nonce: format!(
                "tb270-04a-limits-{}",
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ),
            kind: None,
            limits: StagedLimitsV3 {
                max_attribute_values: Some(0),
                ..StagedLimitsV3::default()
            },
            plans: vec![entry],
            prepared_at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        },
        &proxy.declared,
        &proxy.advertisement,
    )
    .unwrap();
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert_eq!(failure_code(&reply), "hydrated_attribute_value_limit");
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn one_shot_read_stack_matches_borrowed_and_staged_answers() {
    use type_bridge_orm::session::backend::AnswerCancellation;
    use type_bridge_orm::{QueryExecutionDeadline, QueryExecutionResourceLimits};
    let name = staged_live::live_database::name("stack");
    let db = staged_live::connect(&name).await;
    db.create_database().await.unwrap();
    db.execute_raw(
        &race::SCHEMA_TYPEQL
            .replace(" @key", "")
            .replace(" @card(0..1)", ""),
        TxType::Schema,
    )
    .await
    .unwrap();
    seed(&db, "I1", "A1").await;
    db.execute_raw(
        r#"insert $d isa doctor, has name "alice", has on-call true;"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let proxy = proxy(&name, LivePolicy::default(), true).await;
    let plans = [
        race::string_read(
            type_bridge_contract::migration_assertion::ValueComparator::Contains,
            &["alice"],
        ),
        race::stock_reduce_count(),
    ];
    let policy = QueryExecutionResourceLimits::default();
    let cancellation = AnswerCancellation::default();
    let direct = db
        .execute_plans(
            &plans,
            &policy,
            &cancellation,
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap();
    let context = db.transaction_context(TxType::Read).await.unwrap();
    let borrowed = context
        .execute_plans(
            &plans,
            &policy,
            &cancellation,
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap();
    context.close().await.unwrap();
    assert_eq!(direct, borrowed);
    assert!(
        matches!(&direct[0], type_bridge_orm::session::staged::PlanAnswer::Documents(documents) if documents.len() == 1)
    );
    let staged = request(
        &proxy,
        plans
            .iter()
            .map(|plan| {
                StagedPlanV3::new(
                    plan,
                    plan.given_rows().unwrap_or(&[]).to_vec(),
                    &proxy.declared,
                )
                .unwrap()
            })
            .collect(),
    );
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    let remote = reply
        .answers()
        .iter()
        .zip(&plans)
        .map(|(answer, plan)| {
            type_bridge_orm::session::staged::PlanAnswer::from_staged_v3(answer, plan).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(direct, remote);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn explicitly_raised_string_limits_round_trip_large_values_direct_and_staged() {
    use type_bridge_contract::limits::QueryLimits;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_orm::session::staged::PlanAnswer;
    println!("WIDE_RUNNER pid={}", std::process::id());
    let name = staged_live::live_database::name("wide");
    let db = staged_live::connect(&name).await;
    db.create_database().await.unwrap();
    db.execute_raw(
        &race::SCHEMA_TYPEQL
            .replace(" @key", "")
            .replace(" @card(0..1)", ""),
        TxType::Schema,
    )
    .await
    .unwrap();
    let large = "x".repeat(1024 * 1024 + 1);
    db.execute_raw(
        &format!("insert $d isa doctor, has name \"{large}\", has on-call true;"),
        TxType::Write,
    )
    .await
    .unwrap();
    let limits =
        QueryLimits::from_overrides([("canonical_string_bytes", 2 * 1024 * 1024)]).unwrap();
    let proxy = proxy(
        &name,
        LivePolicy {
            query_limits: Some(limits),
            ..Default::default()
        },
        true,
    )
    .await;
    let plan = race::string_read_with_limits(ValueComparator::Equal, &[&large], limits);
    let mut direct_tx = db.transaction(TransactionOptions::default().with_never_commit(true));
    let pending = direct_tx.stage_plan(&plan).unwrap();
    direct_tx.commit().await.unwrap();
    let direct = pending.get().unwrap();
    assert!(matches!(&*direct, PlanAnswer::Documents(documents) if documents.len() == 1));
    let request_spec = || StagedRequestSpecV3 {
        nonce: format!("tb270-04a-wide-request-{}", std::process::id()),
        kind: None,
        limits: StagedLimitsV3::default(),
        plans: vec![
            StagedPlanV3::new(&plan, plan.given_rows().unwrap().to_vec(), &proxy.declared).unwrap(),
        ],
        prepared_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    };
    assert!(StagedRequestV3::new(request_spec(), &proxy.declared, &proxy.advertisement).is_err());
    let staged = StagedRequestV3::new_with_limits(
        request_spec(),
        &proxy.declared,
        &proxy.advertisement,
        limits,
    )
    .unwrap();
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    assert_eq!(reply.answers(), [direct.to_staged_v3(&plan).unwrap()]);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn generated_manager_filter_targets_share_answers_and_one_staged_exchange() {
    use std::sync::Mutex;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::projection::{
        BindingTarget, ProjectedTokenIdentity, ProjectionConfig, ProjectionHandler,
    };
    use type_bridge_contract::query_remote_v3::StagedKindV3;
    use type_bridge_contract::schema::{DocumentId, OwnsFactId};
    use type_bridge_orm::projected_crud::ProjectedCrudInvocationControl;
    use type_bridge_orm::query_v2_prepared::QueryAuthority;
    use type_bridge_orm::{
        AnswerCancellation, InstalledRuntimeProjection, ProjectedAttributeValue,
        ProjectedCrudExecutor, ProjectedManagerComparison, ProjectedManagerFilter,
        ProjectedManagerFilterExecutor, ProjectedManagerFilterInvocationControl, ProxyEndpoint,
        ProxyQueryConnection, QueryExecutionResourceLimits, QueryTarget,
    };
    let name = staged_live::live_database::name("manager_filter_proxy");
    let database = staged_live::connect(&name).await;
    database.create_database().await.unwrap();
    database.execute_raw("define attribute filter-key, value string; entity filter-person, owns filter-key @key, plays filter-edge:member; relation filter-edge, owns filter-key @key, relates member @card(0..2);", TxType::Schema).await.unwrap();
    database.execute_raw("insert $a isa filter-person, has filter-key \"alice\"; $b isa filter-person, has filter-key \"bob\"; $e isa filter-edge, has filter-key \"pair\", links (member: $a, member: $b);", TxType::Write).await.unwrap();
    let documents = type_bridge_schema::SchemaDocumentSet::parse([(
        DocumentId::new("manager-filter-live.yaml").unwrap(),
        "format: typebridge.schema/v2\nattributes:\n  filter-key: { value: string }\nentities:\n  filter-person:\n    owns:\n      filter-key: { key: true }\nrelations:\n  filter-edge:\n    owns:\n      filter-key: { key: true }\n    relates:\n      member: { card: { min: 0, max: 2 } }\nplays:\n  filter-person:\n    filter-edge: [member]\n",
    )]).unwrap();
    let declared = type_bridge_schema::normalize_documents(&documents).unwrap();
    let profile =
        SemanticProfileId::new(semantic_profile_id(&database.server_version().unwrap()).unwrap())
            .unwrap();
    let resolved = type_bridge_schema::resolve(&declared, &profile).unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        type_bridge_schema::project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let scope = format!("manager-filter-{name}");
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new(&scope).unwrap(),
        profile.clone(),
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let verified = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    let installed = installed.with_schema_authority(&verified);

    let mut proxy = staged_live::proxy_with(
        &name,
        LivePolicy::default(),
        false,
        declared.clone(),
        &scope,
    )
    .await;
    fn record_staged(proxy: &mut Proxy) -> Arc<Mutex<Vec<Vec<u8>>>> {
        let requests = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let captured = Arc::clone(&requests);
        proxy.router = proxy.router.clone().layer(axum::middleware::from_fn(
            move |request: Request<Body>, next: axum::middleware::Next| {
                let captured = Arc::clone(&captured);
                async move {
                    if request.uri().path() == "/v3/staged" {
                        let (parts, body) = request.into_parts();
                        let bytes = body.collect().await.unwrap().to_bytes();
                        captured.lock().unwrap().push(bytes.to_vec());
                        next.run(Request::from_parts(parts, Body::from(bytes)))
                            .await
                    } else {
                        next.run(request).await
                    }
                }
            },
        ));
        requests
    }
    let requests = record_staged(&mut proxy);
    let port = serve(&proxy).await;
    let authority = Arc::new(
        QueryAuthority::from_declared_bytes(
            &type_bridge_contract::schema::encode_declared_schema(&declared).unwrap(),
            &scope,
            profile.as_str(),
        )
        .unwrap(),
    );
    let policy = QueryExecutionResourceLimits {
        items: 100,
        ..QueryExecutionResourceLimits::default()
    };
    let connection = ProxyQueryConnection::connect(
        &ProxyEndpoint::new(format!("http://127.0.0.1:{port}")),
        Some(proxy.advertisement.encode().unwrap()),
        Arc::clone(&authority),
        policy,
        policy,
        &AnswerCancellation::default(),
    )
    .await
    .unwrap();
    let control =
        || ProjectedManagerFilterInvocationControl::capture(policy, AnswerCancellation::default());
    let model = TypeId::new(TypeKind::Entity, "filter-person").unwrap();
    let all = ProjectedManagerFilter::try_new(&installed, model.clone()).unwrap();
    let key = OwnsFactId::new(model.clone(), AttributeId::new("filter-key").unwrap()).unwrap();
    let by_key = |text: &str| {
        let value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "filter-key").unwrap(),
            CanonicalValue::String(CanonicalString::new(text).unwrap()),
        )
        .unwrap();
        all.try_and(
            &installed,
            &ProjectedTokenIdentity::Field {
                owner: model.clone(),
                field: key.clone(),
            },
            ProjectedManagerComparison::Eq,
            &value,
        )
        .unwrap()
    };
    let executor = ProjectedManagerFilterExecutor::new(&installed);
    let direct = executor
        .all_with_control(&database, &all, control())
        .await
        .unwrap();
    let remote = executor
        .all_on_target(QueryTarget::Proxy(&connection), &all, control())
        .await
        .unwrap();
    assert_eq!(direct.len(), 2);
    assert_eq!(remote.len(), 2);
    for thing in &remote {
        let expected = direct
            .iter()
            .find(|candidate| candidate.iid() == thing.iid())
            .unwrap();
        assert_eq!(thing.fields(), expected.fields());
        assert!(thing.origin_carrier().is_some());
    }
    let direct = executor
        .first_with_control(&database, &by_key("alice"), control())
        .await
        .unwrap()
        .unwrap();
    let remote = executor
        .first_on_target(QueryTarget::Proxy(&connection), &by_key("alice"), control())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote.iid(), direct.iid());
    assert_eq!(remote.fields(), direct.fields());
    assert_eq!(
        executor
            .count_with_control(&database, &all, control())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        executor
            .count_on_target(QueryTarget::Proxy(&connection), &all, control())
            .await
            .unwrap(),
        2
    );
    assert!(
        executor
            .exists_with_control(&database, &all, control())
            .await
            .unwrap()
    );
    assert!(
        executor
            .exists_on_target(QueryTarget::Proxy(&connection), &all, control())
            .await
            .unwrap()
    );
    assert!(
        executor
            .first_on_target(
                QueryTarget::Proxy(&connection),
                &by_key("missing"),
                control()
            )
            .await
            .unwrap()
            .is_none()
    );
    let crud = ProjectedCrudExecutor::new(&installed);
    let crud_control =
        || ProjectedCrudInvocationControl::capture(policy, AnswerCancellation::default());
    let edge_model = TypeId::new(TypeKind::Relation, "filter-edge").unwrap();
    let edges = executor
        .all_with_control(
            &database,
            &ProjectedManagerFilter::try_new(&installed, edge_model.clone()).unwrap(),
            control(),
        )
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    for (model, iid, expected_count) in
        [(&model, direct.iid(), 2), (&edge_model, edges[0].iid(), 1)]
    {
        let direct_thing = crud
            .get_by_iid_on_target(QueryTarget::Database(&database), model, iid, crud_control())
            .await
            .unwrap()
            .unwrap();
        let remote_thing = crud
            .get_by_iid_on_target(QueryTarget::Proxy(&connection), model, iid, crud_control())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remote_thing.iid(), direct_thing.iid());
        assert_eq!(remote_thing.fields(), direct_thing.fields());
        assert!(remote_thing.origin_carrier().is_some());
        if model.kind() == TypeKind::Relation {
            let direct_players = direct_thing
                .roles()
                .values()
                .flatten()
                .map(|player| player.iid())
                .collect::<Vec<_>>();
            let remote_players = remote_thing
                .roles()
                .values()
                .flatten()
                .map(|player| player.iid())
                .collect::<Vec<_>>();
            assert_eq!(remote_players, direct_players);
            assert_eq!(remote_players.len(), 2);
        }
        assert_eq!(
            crud.count_on_target(QueryTarget::Database(&database), model, crud_control())
                .await
                .unwrap(),
            expected_count
        );
        assert_eq!(
            crud.count_on_target(QueryTarget::Proxy(&connection), model, crud_control())
                .await
                .unwrap(),
            expected_count
        );
        assert!(
            crud.get_by_iid_on_target(
                QueryTarget::Proxy(&connection),
                model,
                "0xffff",
                crud_control()
            )
            .await
            .unwrap()
            .is_none()
        );
    }
    {
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 11);
        for bytes in captured.iter() {
            let request = StagedRequestV3::decode(bytes, &declared, &proxy.advertisement).unwrap();
            assert_eq!(request.kind(), StagedKindV3::Read);
            assert_eq!(request.plans().len(), 1);
        }
    }
    assert_eq!(proxy.commits.lock().unwrap().len(), 11);
    let defaults =
        QueryExecutionResourceLimits::default_for(type_bridge_orm::AnswerTransport::Proxy);
    let default_connection = ProxyQueryConnection::connect(
        &ProxyEndpoint::new(format!("http://127.0.0.1:{port}")),
        Some(proxy.advertisement.encode().unwrap()),
        Arc::clone(&authority),
        defaults,
        defaults,
        &AnswerCancellation::default(),
    )
    .await
    .unwrap()
    .with_caller_item_limit(None);
    assert_eq!(
        executor
            .all_on_target(
                QueryTarget::Proxy(&default_connection),
                &all,
                ProjectedManagerFilterInvocationControl::capture(
                    Default::default(),
                    Default::default()
                )
            )
            .await
            .unwrap()
            .len(),
        2
    );
    {
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 12);
        let request =
            StagedRequestV3::decode(captured.last().unwrap(), &declared, &proxy.advertisement)
                .unwrap();
        assert_eq!(request.limits().max_items, 65_536);
        assert!(
            request.plans()[0]
                .plan()
                .pipeline()
                .iter()
                .any(|stage| matches!(
                    stage,
                    type_bridge_contract::query_plan::v3::Stage::Limit { rows: 100_001 }
                ))
        );
    }
    assert_eq!(proxy.commits.lock().unwrap().len(), 12);
    let mut bounded = staged_live::proxy_with(
        &name,
        LivePolicy {
            answer_limits: Some(QueryExecutionResourceLimits {
                items: 1,
                ..defaults
            }),
            ..Default::default()
        },
        false,
        declared.clone(),
        &scope,
    )
    .await;
    let bounded_requests = record_staged(&mut bounded);
    let bounded_port = serve(&bounded).await;
    let bounded_connection = ProxyQueryConnection::connect(
        &ProxyEndpoint::new(format!("http://127.0.0.1:{bounded_port}")),
        Some(bounded.advertisement.encode().unwrap()),
        Arc::clone(&authority),
        policy,
        policy,
        &AnswerCancellation::default(),
    )
    .await
    .unwrap();
    let diagnostic = executor
        .all_on_target(QueryTarget::Proxy(&bounded_connection), &all, control())
        .await
        .unwrap_err()
        .into_diagnostic(type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read);
    assert_eq!(diagnostic.code().as_str(), "processed_item_limit");
    {
        let captured = bounded_requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let request =
            StagedRequestV3::decode(&captured[0], &declared, &bounded.advertisement).unwrap();
        assert_eq!(request.limits().max_items, 1);
        assert!(
            request.plans()[0]
                .plan()
                .pipeline()
                .iter()
                .any(|stage| matches!(
                    stage,
                    type_bridge_contract::query_plan::v3::Stage::Limit { rows: 101 }
                ))
        );
    }
    {
        let bounded_outcomes = bounded.commits.lock().unwrap();
        assert_eq!(bounded_outcomes.len(), 1);
        assert!(!bounded_outcomes[0].0);
    }
    drop(bounded_connection);
    drop(bounded);
    drop(default_connection);
    drop(connection);
    drop(proxy);
    drop_database(database).await;
}
