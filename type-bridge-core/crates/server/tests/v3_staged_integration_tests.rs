//! `/v3/staged` against a live TypeDB 3.13.6 (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT); run it explicitly with `-- --ignored`.
//!
//! The scenarios are the one-query race programs shared with the ORM's
//! live plan execution test, executed through the proxy and directly. The
//! Rust SDK's live plan-builder tests share the same proxy fixture from
//! `crates/rust/tests/plan_builder_live.rs`.

#[path = "support/staged_live.rs"]
mod staged_live;

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
            let name = format!(
                "tb_sdk_writes_{binding}_{transport}_{}_{}",
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
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
