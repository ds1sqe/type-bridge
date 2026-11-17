//! Public Rust SDK consumer of the shared SDK deadline fixture.
#[path = "support/purchase_models.rs"]
mod purchase_models;

use std::time::{Duration, Instant};
use type_bridge::staged::{PlanAnswer, TransactionOptions};
use type_bridge::{
    ConnectionOptions, ProxyConnectionPolicy, ProxyEndpoint, QueryExecutionResourceLimits,
    QueryTimeout,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TB_DEADLINE_FIXTURE_DIR and the shared real-TypeDB deadline fixture"]
async fn staged_deadline_none_and_typed_timeout_direct_and_proxy() {
    let dir = std::env::var("TB_DEADLINE_FIXTURE_DIR").expect("fixture directory");
    let live: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(&dir).join("fixture.json")).unwrap(),
    )
    .unwrap();
    let package = purchase_models::package();
    let plan = purchase_models::purchase_plan(&package, "O1", "I1", "A1").unwrap();
    for transport in ["direct", "proxy"] {
        let entry = &live["entries"][format!("rust-{transport}")];
        let arm = std::path::Path::new(entry["arm"].as_str().unwrap());
        for (timeout, delay) in [(1_000, 2_000), (0, 61_000)] {
            let policy = QueryTimeout::from_signed_milliseconds(timeout);
            let options = TransactionOptions::default()
                .with_timeout(policy)
                .with_collect_phase_timestamps(true);
            let direct = if transport == "direct" {
                Some(
                    type_bridge::Database::connect(
                        ConnectionOptions::new(
                            entry["address"].as_str().unwrap(),
                            entry["database"].as_str().unwrap(),
                        )
                        .http_port(43131),
                    )
                    .await
                    .unwrap()
                    .with_schema(package)
                    .unwrap(),
                )
            } else {
                None
            };
            let remote = if transport == "proxy" {
                Some(
                    type_bridge::ProxyDatabase::connect(
                        ProxyConnectionPolicy::new(ProxyEndpoint::new(
                            entry["endpoint"].as_str().unwrap(),
                        ))
                        .answer_limits(QueryExecutionResourceLimits {
                            timeout_milliseconds: policy,
                            collect_phase_timestamps: true,
                            ..QueryExecutionResourceLimits::default()
                        }),
                    )
                    .await
                    .unwrap()
                    .with_schema(package)
                    .unwrap(),
                )
            } else {
                None
            };
            let mut transaction = if let Some(database) = &direct {
                database.transaction(options)
            } else {
                remote.as_ref().unwrap().transaction(options).await.unwrap()
            };
            let pending = transaction.stage(&plan).unwrap();
            std::fs::write(arm, delay.to_string()).unwrap();
            let started = Instant::now();
            let result = transaction.commit().await;
            if timeout != 0 {
                let diagnostic = result.unwrap_err().diagnostic();
                assert_eq!(diagnostic.code().as_str(), "transaction_deadline_exceeded");
                let host = serde_json::to_value(diagnostic.host_json()).unwrap();
                let stamps = host["details"]["execution_timeline"]["value"]["stamps"]
                    .as_array()
                    .unwrap();
                assert_eq!(stamps[0]["phase"], "fired");
                assert_eq!(stamps.last().unwrap()["phase"], "failed");
                assert_eq!(
                    stamps.last().unwrap()["failing_phase"],
                    if transport == "direct" {
                        "fired"
                    } else {
                        "accepted"
                    }
                );
                assert!(!stamps.iter().any(|stamp| stamp["phase"] == "db_write"));
                assert!(pending.get().is_err());
                tokio::time::sleep(Duration::from_millis(2100)).await;
            } else {
                result.unwrap();
                assert!(started.elapsed() > Duration::from_secs(60));
                assert_eq!(*pending.get().unwrap(), PlanAnswer::Matched(1));
            }
            assert!(!arm.exists(), "relay must actually delay the call");
            transaction.close().unwrap();
        }
    }
}
