//! Opt-in transaction reuse and server deadline proofs on TypeDB 3.13.6.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use type_bridge_typedb_runtime::{
    ConnectOptions, QueryResult, TransactionOptions, TxType, TypeDBRuntime,
};

async fn fixture() -> Option<(TypeDBRuntime, String)> {
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").ok()?;
    let runtime = TypeDBRuntime::connect(
        &address,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".into())
                .parse()
                .expect("valid TypeDB HTTP port"),
            tls: false,
            server_version: Some(
                std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")
                    .unwrap_or_else(|_| "3.13.6".into())
                    .parse()
                    .unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_PREFIX")
        .unwrap_or_else(|_| "tb_transaction_".into());
    let database = format!("{prefix}{}_{nonce}", std::process::id());
    runtime.create_database(&database).await.unwrap();
    let mut schema = runtime
        .open_transaction(&database, TxType::Schema)
        .await
        .unwrap();
    schema.query("define entity marker;").await.unwrap();
    schema.commit().await.unwrap();
    drop(schema);
    Some((runtime, database))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_then_query_then_commit_keeps_the_runtime_transaction() {
    let Some((runtime, database)) = fixture().await else {
        return;
    };
    let result = async {
        let mut tx = runtime.open_transaction(&database, TxType::Write).await?;
        tx.query("insert $m isa marker;").await?;
        tx.rollback().await?;
        tx.query("insert $m isa marker;").await?;
        let answer = tx.query("match $m isa marker; select $m;").await?;
        tx.commit().await?;
        Ok::<_, type_bridge_typedb_runtime::RuntimeError>(answer)
    }
    .await;
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
    let QueryResult::Rows(rows) = result.unwrap() else {
        panic!("expected marker rows");
    };
    assert_eq!(rows.len(), 1, "the pre-rollback marker must be discarded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_transaction_deadline_closes_an_idle_transaction() {
    let Some((runtime, database)) = fixture().await else {
        return;
    };
    let mut options = TransactionOptions::default();
    options.transaction_timeout = Some(Duration::from_secs(1));
    let mut tx = runtime
        .open_transaction_with_options(&database, TxType::Write, &options)
        .await
        .unwrap();
    let query = "match $m isa marker; select $m;";
    tx.query(query).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result = tx.query(query).await;
    let _ = tx.close().await;
    drop(tx);
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
    assert!(
        result.is_err(),
        "an idle transaction past its server deadline must be closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_observer_counts_sent_statements_and_skips_preflight_refusals() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use type_bridge_typedb_runtime::{RuntimeAnswerControl, RuntimeAnswerLimits};
    static SENT: AtomicUsize = AtomicUsize::new(0);
    fn observed() {
        SENT.fetch_add(1, Ordering::SeqCst);
    }
    let Some((runtime, database)) = fixture().await else {
        return;
    };
    let result = async {
        let mut tx = runtime.open_transaction(&database, TxType::Read).await?;
        let query = "match $m isa marker; select $m;";
        tx.query(query).await?;
        assert_eq!(
            SENT.load(Ordering::SeqCst),
            0,
            "observer is absent by default"
        );
        tx.set_statement_dispatch_observer(Some(observed));
        for sent in 1..=2 {
            tx.query(query).await?;
            assert_eq!(SENT.load(Ordering::SeqCst), sent);
        }
        let limits = RuntimeAnswerLimits {
            max_items: u64::MAX,
            max_bytes: u64::MAX,
            deadline: None,
            cancellation: Default::default(),
        };
        limits.cancellation.cancel();
        let error = tx
            .query_bounded(query, limits, &mut |_| Ok(RuntimeAnswerControl::Continue))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            type_bridge_typedb_runtime::RuntimeError::ResourceLimit {
                code: "provider_cancelled",
                ..
            }
        ));
        assert_eq!(SENT.load(Ordering::SeqCst), 2);
        let mut limits = RuntimeAnswerLimits {
            max_items: u64::MAX,
            max_bytes: u64::MAX,
            deadline: None,
            cancellation: Default::default(),
        };
        limits.deadline = Some(std::time::Instant::now());
        let error = tx
            .query_bounded(query, limits, &mut |_| Ok(RuntimeAnswerControl::Continue))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            type_bridge_typedb_runtime::RuntimeError::ResourceLimit {
                code: "transaction_deadline_exceeded",
                ..
            }
        ));
        assert_eq!(SENT.load(Ordering::SeqCst), 2);
        tx.set_statement_dispatch_observer(None);
        tx.query(query).await?;
        assert_eq!(SENT.load(Ordering::SeqCst), 2);
        tx.close().await
    }
    .await;
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
    result.unwrap();
}
