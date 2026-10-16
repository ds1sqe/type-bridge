//! Opt-in proof that `query-plan/v3` programs execute staged against TypeDB 3.13.6.
#![cfg(feature = "typedb")]
#[path = "support/plan_v3_race.rs"]
mod race;

use type_bridge_contract::query_plan::v3::Operation;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::session::staged::{PlanAnswer, StagedQuery, TransactionOptions};
use type_bridge_orm::{ConnectOptions, Database, TxType};

fn rows(answer: &QueryResult) -> usize {
    match answer {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_query_purchase_and_write_plans_resolve_at_commit() {
    let Ok(address) = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS") else {
        return;
    };
    let name = format!(
        "tb_w11e_plans_{}_{}",
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
    // Plan inputs travel as `given` rows (TypeDB 3.12+ over band 9).
    if !db.supports_given_stage() {
        println!("skipped: TypeDB at {address} cannot carry the given stage plans need");
        return;
    }
    db.create_database().await.unwrap();
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $i isa item, has item-id "I1", has stock 1, has price 40;
           $a isa account, has account-id "A1", has balance 100;
           $d isa doctor, has name "alice", has on-call true;"#,
        TxType::Write,
    )
    .await
    .unwrap();

    let mut tx = db.transaction(TransactionOptions::default());
    let bought = tx.stage_plan(&race::purchase("O1", "I1", "A1")).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(*bought.get().unwrap(), PlanAnswer::Matched(1));

    // Stock is now 0: the same pipeline matches no row and writes nothing.
    let refused = tx.stage_plan(&race::purchase("O2", "I1", "A1")).unwrap();
    let orders = tx
        .stage(StagedQuery::new("match $p isa purchase; select $p;", false))
        .unwrap();
    let stock = tx
        .stage(StagedQuery::new(
            "match $i isa item, has stock 0; $a isa account, has balance 60; select $i, $a;",
            false,
        ))
        .unwrap();
    let stocked = tx.stage_plan(&race::stock_read(Operation::Rows)).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(*refused.get().unwrap(), PlanAnswer::Matched(0));
    assert_eq!(rows(&orders.get().unwrap()), 1);
    assert_eq!(rows(&stock.get().unwrap()), 1);
    assert!(matches!(&*stocked.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));

    let touched = tx.stage_plan(&race::touch_rows()).unwrap();
    let touch = tx.stage_plan(&race::touch()).unwrap();
    let dropped = tx.stage_plan(&race::drop_price()).unwrap();
    let exists = tx.stage_plan(&race::stock_read(Operation::Exists)).unwrap();
    let count = tx.stage_plan(&race::stock_read(Operation::Count)).unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(&*touched.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));
    assert_eq!(*touch.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*dropped.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*exists.get().unwrap(), PlanAnswer::Exists(true));
    assert_eq!(*count.get().unwrap(), PlanAnswer::Count(1));
    let prices = db
        .execute_raw("match $i isa item, has price $p; select $i;", TxType::Read)
        .await
        .unwrap();
    assert_eq!(rows(&prices), 0);
    let on_call = db
        .execute_raw(
            "match $d isa doctor, has on-call true; select $d;",
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&on_call), 1);

    db.delete_database().await.unwrap();
    db.close().unwrap();
}
