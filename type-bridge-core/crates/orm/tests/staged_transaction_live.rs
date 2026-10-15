//! Opt-in proof of staged atomic execution against TypeDB 3.13.6.
#![cfg(feature = "typedb")]
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::session::staged::{StagedQuery, TransactionOptions};
use type_bridge_orm::{ConnectOptions, Database, TxType};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staged_atomicity_and_rollback_reuse() {
    let Ok(address) = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS") else {
        return;
    };
    let name = format!(
        "tb_w11a_staged_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let db = Database::connect_with_options(
        &address,
        &name,
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
    db.create_database().await.unwrap();
    db.execute_raw("define entity marker;", TxType::Schema)
        .await
        .unwrap();
    let mut tx = db.transaction(TransactionOptions::default());
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    let answer = tx
        .stage(StagedQuery::new("match $m isa marker; select $m;", false))
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(&*answer.get().unwrap(), QueryResult::Rows(rows) if rows.len() == 2));
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    tx.stage(StagedQuery::new("insert $m isa missing_type;", true))
        .unwrap();
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    assert_eq!(tx.commit().await.unwrap_err().op_index, Some(1));
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    tx.rollback().unwrap();
    tx.stage(StagedQuery::new("insert $m isa marker;", true))
        .unwrap();
    let answer = tx
        .stage(StagedQuery::new("match $m isa marker; select $m;", false))
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(&*answer.get().unwrap(), QueryResult::Rows(rows) if rows.len() == 3));
    let context = db.transaction_context(TxType::Write).await.unwrap();
    context.query("insert $m isa marker;").await.unwrap();
    context.rollback().await.unwrap();
    context.query("insert $m isa marker;").await.unwrap();
    context.commit().await.unwrap();
    let mut owned = db.write_transaction().await.unwrap();
    owned.query("insert $m isa marker;").await.unwrap();
    owned.rollback().await.unwrap();
    owned.query("insert $m isa marker;").await.unwrap();
    owned.commit().await.unwrap();
    db.execute_raw("insert $m isa marker;", TxType::Write)
        .await
        .unwrap();
    let rows = db
        .execute_raw("match $m isa marker; select $m;", TxType::Read)
        .await
        .unwrap();
    assert!(matches!(rows, QueryResult::Rows(rows) if rows.len() == 6));
    let (mut fence, schema) = db
        .schema_fenced_read_transaction(std::time::Duration::from_secs(10))
        .await
        .unwrap();
    assert!(schema.contains("marker"));
    let admission = db.schema_transaction();
    tokio::pin!(admission);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut admission)
            .await
            .is_err()
    );
    fence.close().await.unwrap();
    let mut schema = tokio::time::timeout(std::time::Duration::from_secs(5), &mut admission)
        .await
        .unwrap()
        .unwrap();
    schema.close().await.unwrap();
    let mut read = db.transaction(TransactionOptions::default());
    let answer = read
        .stage(StagedQuery::new("match $m isa marker; select $m;", false))
        .unwrap();
    read.commit().await.unwrap();
    assert!(matches!(&*answer.get().unwrap(), QueryResult::Rows(rows) if !rows.is_empty()));
    db.delete_database().await.unwrap();
    db.close().unwrap();
}
