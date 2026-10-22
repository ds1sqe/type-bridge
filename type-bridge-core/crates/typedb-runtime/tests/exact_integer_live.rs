//! Opt-in proof that integers beyond 2^53 survive rows and fetch documents.
//!
//! Older official drivers render concept documents through an `f64` JSON
//! number; the runtime reads typed concept values instead, so the value is
//! exact on every supported server and driver band.

use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_typedb_runtime::{ConnectOptions, QueryResult, TxType, TypeDBRuntime};

/// 2^53 + 1: the first integer an `f64` cannot represent.
const BEYOND_F64: i64 = 9_007_199_254_740_993;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integers_beyond_two_to_the_53_round_trip_exactly() {
    let Ok(address) = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS") else {
        return;
    };
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
            server_version: std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")
                .ok()
                .map(|version| version.parse().unwrap()),
        },
    )
    .await
    .unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let database = format!("tb_exact_integer_{}_{nonce}", std::process::id());
    runtime.create_database(&database).await.unwrap();
    let result = async {
        let mut schema = runtime.open_transaction(&database, TxType::Schema).await?;
        schema
            .query("define attribute n, value integer; entity p, owns n;")
            .await?;
        schema.commit().await?;
        let mut write = runtime.open_transaction(&database, TxType::Write).await?;
        write
            .query(&format!("insert $p isa p, has n {BEYOND_F64};"))
            .await?;
        write.commit().await?;
        let mut read = runtime.open_transaction(&database, TxType::Read).await?;
        let rows = read.query("match $p isa p, has n $n; select $n;").await?;
        let documents = read.query("match $p isa p; fetch { 'n': $p.n };").await?;
        Ok::<_, type_bridge_typedb_runtime::RuntimeError>((rows, documents))
    }
    .await;
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
    let (rows, documents) = result.unwrap();
    let QueryResult::Rows(rows) = rows else {
        panic!("expected rows, got {rows:?}");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["n"]["value"],
        serde_json::json!(BEYOND_F64),
        "row: {rows:?}"
    );
    let QueryResult::Documents(documents) = documents else {
        panic!("expected documents, got {documents:?}");
    };
    assert_eq!(
        documents,
        [serde_json::json!({ "n": BEYOND_F64 })],
        "fetch documents must not round through f64"
    );
}
