//! Live TypeDB outcomes reused by SDK error-projection tests.
use type_bridge_contract::sdk_diagnostic::{SdkExecutionDiagnostic, SdkProviderOperation};
use type_bridge_orm::error::ClassifiedCommitError;
use type_bridge_orm::session::backend::TxType;
use type_bridge_orm::{ConnectOptions, Database};

/// Always restore the owned server after a commit fault injection.
pub struct Unpause(String);
/// Pause only the explicitly assigned failure-smoke container.
pub fn pause_owned_server() -> Unpause {
    let container = std::env::var("TYPE_BRIDGE_FAILURE_CONTAINER").unwrap();
    assert!(container.starts_with("tb-w11b-"));
    assert!(
        std::process::Command::new("docker")
            .args(["pause", &container])
            .status()
            .unwrap()
            .success()
    );
    Unpause(container)
}
impl Drop for Unpause {
    fn drop(&mut self) {
        assert!(
            std::process::Command::new("docker")
                .args(["unpause", &self.0])
                .status()
                .unwrap()
                .success()
        );
    }
}
/// Produce real provider failures, including the driver's paused-commit result.
pub async fn observations(prefix: &str) -> Vec<(&'static str, ClassifiedCommitError)> {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/typed-failure-scenarios.json")).unwrap();
    let query = |key: &str| fixture[key].as_str().unwrap();
    let address = std::env::var("TYPEDB_ADDRESS").expect("isolated TypeDB address required");
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap();
    let container = std::env::var("TYPE_BRIDGE_FAILURE_CONTAINER").unwrap();
    assert!(
        container.starts_with("tb-w11b-"),
        "only the owned failure-smoke container may be paused"
    );
    let db = Database::connect_with_options(
        &address,
        &format!("{prefix}_{}", std::process::id()),
        "admin",
        "password",
        ConnectOptions {
            http_port,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    let schema = db.transaction_context(TxType::Schema).await.unwrap();
    schema.query(query("schema")).await.unwrap();
    schema.commit().await.unwrap();
    schema.close().await.unwrap();
    let seed = db.transaction_context(TxType::Write).await.unwrap();
    seed.query(query("seed")).await.unwrap();
    seed.commit().await.unwrap();
    seed.close().await.unwrap();
    let a = db.transaction_context(TxType::Write).await.unwrap();
    let b = db.transaction_context(TxType::Write).await.unwrap();
    a.query(query("purchase")).await.unwrap();
    b.query(query("purchase")).await.unwrap();
    let (a_result, b_result) = tokio::join!(a.commit_classified(), b.commit_classified());
    assert_eq!(
        usize::from(a_result.is_ok()) + usize::from(b_result.is_ok()),
        1
    );
    let mut result = vec![("stc2", a_result.err().or(b_result.err()).unwrap())];
    let read = db.transaction_context(TxType::Read).await.unwrap();
    let stock = read.query(query("stock_read")).await.unwrap();
    assert!(
        matches!(stock, type_bridge_orm::session::backend::QueryResult::Documents(ref documents) if documents == &vec![serde_json::json!({"stock":0})])
    );
    read.close().await.unwrap();
    a.close().await.unwrap();
    b.close().await.unwrap();
    for (case, query) in [("cnt5", query("cnt5")), ("cnt9", query("cnt9"))] {
        let tx = db.transaction_context(TxType::Write).await.unwrap();
        let error = match tx.query(query).await {
            Err(error) => error.into(),
            Ok(_) => tx.commit_classified().await.unwrap_err(),
        };
        result.push((case, error));
        tx.close().await.unwrap();
    }
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    tx.query(query("paused")).await.unwrap();
    let unpause = pause_owned_server();
    let error = tokio::time::timeout(std::time::Duration::from_secs(60), tx.commit_classified())
        .await
        .expect("driver must return an actual failure")
        .unwrap_err();
    drop(unpause);
    result.push(("paused", error));
    tx.close().await.unwrap();
    db.delete_database().await.unwrap();
    db.close().unwrap();
    result
}
/// Lower real provider errors through the SDK's production diagnostic boundary.
#[allow(dead_code)]
pub fn diagnostic(error: ClassifiedCommitError) -> SdkExecutionDiagnostic {
    match error {
        ClassifiedCommitError::Orm(error) => {
            type_bridge_orm::lower_execution_error(error, SdkProviderOperation::Write)
        }
        error => type_bridge_orm::lower_classified_commit_error(error),
    }
}
