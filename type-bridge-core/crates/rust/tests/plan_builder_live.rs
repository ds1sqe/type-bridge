//! The SDK plan builder against a live TypeDB 3.13.6 (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT), directly and through an in-process
//! `type-bridge-server` proxy; run it explicitly with `-- --ignored`.
//!
//! The proxy fixture is the server's own staged live fixture, so the server
//! crate keeps no dependency on this SDK.

#[path = "../../server/tests/support/staged_live.rs"]
mod staged_live;

#[path = "support/purchase_models.rs"]
mod purchase_models;

use std::future::Future;
use std::pin::Pin;

use axum::Router;

use staged_live::{
    LivePolicy, call, connect_options, drop_database, proxy_with, row_count, seeded, state,
};

/// The SDK's proxy route: every exchange goes to the in-process router.
struct RouterExchange(Router);

impl type_bridge::ProxyExchange for RouterExchange {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        Box::pin(async { Ok(call(&self.0, "GET", "/v2/capabilities", Vec::new()).await) })
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async { Ok(call(&self.0, "POST", "/v2/query", request.to_vec()).await) })
    }

    fn staged_capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        Box::pin(async { Ok(call(&self.0, "GET", "/v3/capabilities", Vec::new()).await) })
    }

    fn exchange_staged<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async { Ok(call(&self.0, "POST", "/v3/staged", request.to_vec()).await) })
    }
}

/// The SDK plan builder's purchase equals the hand-built plan byte for
/// byte, and the same staged program gives the same answers and state
/// directly and over the proxy: the first purchase matches, the second
/// finds no stock and writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn sdk_plan_builder_purchase_is_golden_and_commits_directly_and_over_the_proxy() {
    if !staged_live::given_rows_supported(
        "sdk_plan_builder_purchase_is_golden_and_commits_directly_and_over_the_proxy",
    )
    .await
    {
        return;
    }
    use type_bridge::staged::{PlanAnswer, TransactionOptions};
    let package = purchase_models::package();
    let first = purchase_models::purchase_plan(&package, "O1", "I1", "A1").expect("plan");
    let second = purchase_models::purchase_plan(&package, "O2", "I1", "A1").expect("plan");
    assert_eq!(
        first.canonical_bytes().expect("bytes"),
        purchase_models::hand_built_purchase("O1", "I1", "A1")
            .canonical_bytes()
            .expect("bytes"),
    );

    let (direct_name, direct) = seeded("sdk-builder-direct").await;
    let (proxy_name, proxied) = seeded("sdk-builder-proxy").await;
    let options = type_bridge::ConnectionOptions::new(
        std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into()),
        direct_name,
    )
    .http_port(connect_options().http_port);
    let db = type_bridge::Database::connect(options)
        .await
        .expect("sdk connect")
        .with_schema(package)
        .expect("schema bound");
    let program = |tx: &mut type_bridge::staged::Transaction<purchase_models::Shop>| {
        [&first, &second].map(|plan| tx.stage(plan).expect("staged"))
    };
    let answers = |pending: &[type_bridge::staged::Pending<PlanAnswer>; 2]| {
        pending
            .iter()
            .map(|answer| (*answer.get().expect("answer")).clone())
            .collect::<Vec<_>>()
    };
    let mut tx = db.transaction(TransactionOptions::default());
    let direct_answers = program(&mut tx);
    tx.commit().await.expect("direct commit");

    let (declared, _) = purchase_models::authority();
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = proxy_with(&proxy_name, policy, false, declared, purchase_models::SCOPE).await;
    let remote = type_bridge::ProxyDatabase::connect(
        type_bridge::ProxyConnectionPolicy::with_exchange(RouterExchange(proxy.router.clone())),
    )
    .await
    .expect("proxy connect")
    .with_schema(package)
    .expect("proxy schema bound");
    let mut remote_tx = remote
        .transaction(TransactionOptions::default())
        .await
        .expect("proxy transaction");
    let remote_answers = program(&mut remote_tx);
    remote_tx.commit().await.expect("proxy commit");

    assert_eq!(
        answers(&direct_answers),
        vec![PlanAnswer::Matched(1), PlanAnswer::Matched(0)]
    );
    assert_eq!(answers(&direct_answers), answers(&remote_answers));
    assert_eq!(state(&direct).await, (1, 1, 1, 1));
    assert_eq!(state(&direct).await, state(&proxied).await);
    drop(proxy);
    drop_database(direct).await;
    drop_database(proxied).await;
}

/// Two SDK transactions race the builder's purchase for the last item:
/// exactly one buys it, the other matches nothing or is rejected at commit,
/// and the item is never oversold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn sdk_plan_builder_purchase_race_never_oversells() {
    if !staged_live::given_rows_supported("sdk_plan_builder_purchase_race_never_oversells").await {
        return;
    }
    use type_bridge::staged::{PlanAnswer, TransactionOptions};
    let package = purchase_models::package();
    let (name, raw) = seeded("sdk-builder-race").await;
    let options = type_bridge::ConnectionOptions::new(
        std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into()),
        name,
    )
    .http_port(connect_options().http_port);
    let db = type_bridge::Database::connect(options)
        .await
        .expect("sdk connect")
        .with_schema(package)
        .expect("schema bound");
    let racer = |order: &'static str| {
        let plan = purchase_models::purchase_plan(&package, order, "I1", "A1").expect("plan");
        let mut tx = db.transaction(TransactionOptions::default());
        let answer = tx.stage(&plan).expect("staged");
        async move {
            match tx.commit().await {
                Ok(()) => Ok((*answer.get().expect("answer")).clone()),
                Err(error) => Err(error),
            }
        }
    };
    let (left, right) = tokio::join!(racer("O1"), racer("O2"));
    let bought = [&left, &right]
        .into_iter()
        .filter(|outcome| matches!(outcome, Ok(PlanAnswer::Matched(1))))
        .count();
    assert_eq!(bought, 1, "exactly one racer buys: {left:?} / {right:?}");
    for outcome in [&left, &right] {
        match outcome {
            Ok(PlanAnswer::Matched(_)) => {}
            Err(error) => assert!(
                error.failure.is_some(),
                "the loser carries a typed failure: {error}"
            ),
            other => panic!("unexpected answer {other:?}"),
        }
    }
    assert_eq!(
        row_count(&raw, "match $p isa purchase; select $p;").await,
        1
    );
    drop_database(raw).await;
}
