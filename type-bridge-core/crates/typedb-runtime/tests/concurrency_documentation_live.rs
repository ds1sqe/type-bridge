//! Executable TypeQL examples from the TypeDB 3.13.6 concurrency guide.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_typedb_runtime::{
    ConnectOptions, QueryResult, RuntimeTransaction, TxType, TypeDBRuntime,
};

const GUIDE: &str = include_str!("../../../../docs/guide/concurrency.md");

type Snippets = BTreeMap<&'static str, String>;

fn snippets() -> Snippets {
    let mut snippets = BTreeMap::new();
    for block in GUIDE.split("```typeql title=\"").skip(1) {
        let (name, block) = block.split_once("\"\n").expect("named TypeQL fence");
        let (query, _) = block.split_once("\n```").expect("closed TypeQL fence");
        assert!(!query.trim().is_empty(), "empty snippet {name}");
        assert!(
            snippets.insert(name, query.into()).is_none(),
            "duplicate {name}"
        );
    }
    assert_eq!(snippets.len(), GUIDE.matches("```typeql").count());
    assert!(!snippets.is_empty());
    snippets
}

fn sql<'a>(
    snippets: &'a Snippets,
    used: &mut BTreeSet<&'static str>,
    name: &'static str,
) -> &'a str {
    used.insert(name);
    snippets
        .get(name)
        .unwrap_or_else(|| panic!("missing snippet {name}"))
}

async fn run(
    tx: &mut RuntimeTransaction,
    snippets: &Snippets,
    used: &mut BTreeSet<&'static str>,
    name: &'static str,
) -> QueryResult {
    tx.query(sql(snippets, used, name))
        .await
        .unwrap_or_else(|error| panic!("snippet {name}: {error}"))
}

fn rows(result: QueryResult) -> usize {
    match result {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected concept rows, received {other:?}"),
    }
}

async fn seeded_database(
    runtime: &TypeDBRuntime,
    snippets: &Snippets,
    used: &mut BTreeSet<&'static str>,
    databases: &mut Vec<String>,
) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let database = format!("tb_w11i_docs_{}_{nonce}", std::process::id());
    runtime.create_database(&database).await.unwrap();
    databases.push(database.clone());
    let mut tx = runtime
        .open_transaction(&database, TxType::Schema)
        .await
        .unwrap();
    run(&mut tx, snippets, used, "schema").await;
    tx.commit().await.unwrap();
    let mut tx = runtime
        .open_transaction(&database, TxType::Write)
        .await
        .unwrap();
    run(&mut tx, snippets, used, "seed").await;
    tx.commit().await.unwrap();
    database
}

async fn expect_rows(runtime: &TypeDBRuntime, database: &str, query: &str, expected: usize) {
    let mut tx = runtime
        .open_transaction(database, TxType::Read)
        .await
        .unwrap();
    assert_eq!(rows(tx.query(query).await.unwrap()), expected);
    tx.close().await.unwrap();
}

/// The TypeBridge section's examples are included files, tested by
/// `tests/unit/docs/test_docs_concurrency_guide.py`; only TypeQL is inline.
#[test]
fn guide_snippets_are_named_and_typebridge_examples_are_included_files() {
    snippets();
    let (_, typebridge) = GUIDE
        .split_once("## Using TypeBridge transactions\n")
        .expect("TypeBridge section");
    assert!(!typebridge.contains("```typeql"));
    assert!(typebridge.contains("--8<-- \"examples/purchase/python/recovery.py:direct\""));
}

/// Explicit invocation requires an isolated TypeDB 3.13.6 endpoint; it never
/// silently succeeds when the endpoint is missing or the server is different.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.13.6 server; run through test.sh or the exact-test runner"]
async fn concurrency_guide_snippets_run_on_3_13_6() {
    let address = std::env::var("TYPE_BRIDGE_CONCURRENCY_LIVE_ADDRESS")
        .expect("TYPE_BRIDGE_CONCURRENCY_LIVE_ADDRESS must name the isolated server");
    let runtime = TypeDBRuntime::connect(
        &address,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .expect("TYPEDB_HTTP_PORT")
                .parse()
                .unwrap(),
            tls: false,
            server_version: None,
        },
    )
    .await
    .unwrap();
    let version = runtime.server_version().expect("server version was probed");
    assert_eq!((version.major, version.minor, version.patch), (3, 13, 6));
    let snippets = snippets();
    let mut used = BTreeSet::new();
    let mut databases = Vec::new();

    {
        // Two snapshots can both produce a purchase answer, but only one commits.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut a = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let mut b = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        assert_eq!(rows(run(&mut a, &snippets, &mut used, "purchase").await), 1);
        // Different business keys ensure this conflict is stock/balance protection,
        // rather than the independent uniqueness lock tested below.
        let second_purchase =
            sql(&snippets, &mut used, "purchase").replace("\"purchase-1\"", "\"purchase-2\"");
        assert_ne!(second_purchase, snippets["purchase"]);
        assert_eq!(rows(b.query(&second_purchase).await.unwrap()), 1);
        a.commit().await.unwrap();
        assert!(b.commit().await.unwrap_err().to_string().contains("[STC2]"));
        expect_rows(
            &runtime,
            &db,
            "match $i isa item, has stock 0; $a isa account, has balance 60; select $i, $a;",
            1,
        )
        .await;
        expect_rows(&runtime, &db, "match $p isa purchase; select $p;", 1).await;
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        assert_eq!(
            rows(run(&mut tx, &snippets, &mut used, "purchase").await),
            0
        );
        tx.commit().await.unwrap();
        expect_rows(&runtime, &db, "match $p isa purchase; select $p;", 1).await;

        // The balance comparison also produces a successful no-op commit.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        tx.query("match $a isa account; update $a has balance 10;")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        assert_eq!(
            rows(run(&mut tx, &snippets, &mut used, "purchase").await),
            0
        );
        tx.commit().await.unwrap();
        expect_rows(
            &runtime,
            &db,
            "match $i isa item, has stock 1; $a isa account, has balance 10; select $i, $a;",
            1,
        )
        .await;
        expect_rows(&runtime, &db, "match $p isa purchase; select $p;", 0).await;

        // Disjoint updates exhibit write skew, and an open reader keeps its snapshot.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut reader = runtime.open_transaction(&db, TxType::Read).await.unwrap();
        let mut a = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let mut b = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        for tx in [&mut reader, &mut a, &mut b] {
            assert_eq!(rows(run(tx, &snippets, &mut used, "read-on-call").await), 2);
        }
        run(&mut a, &snippets, &mut used, "doctor-a-off").await;
        run(&mut b, &snippets, &mut used, "doctor-b-off").await;
        a.commit().await.unwrap();
        b.commit().await.unwrap();
        assert_eq!(
            rows(run(&mut reader, &snippets, &mut used, "read-on-call").await),
            2
        );
        reader.close().await.unwrap();
        expect_rows(&runtime, &db, snippets["read-on-call"].as_str(), 0).await;

        // Either a same-value touch or a persistent guard creates a shared lock.
        for guard in ["touch-on-call", "touch-guard"] {
            let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
            let mut a = runtime.open_transaction(&db, TxType::Write).await.unwrap();
            let mut b = runtime.open_transaction(&db, TxType::Write).await.unwrap();
            for tx in [&mut a, &mut b] {
                assert_eq!(rows(run(tx, &snippets, &mut used, "read-on-call").await), 2);
                run(tx, &snippets, &mut used, guard).await;
            }
            run(&mut a, &snippets, &mut used, "doctor-a-off").await;
            run(&mut b, &snippets, &mut used, "doctor-b-off").await;
            a.commit().await.unwrap();
            assert!(b.commit().await.unwrap_err().to_string().contains("[STC2]"));
            expect_rows(&runtime, &db, snippets["read-on-call"].as_str(), 1).await;
        }

        // plays cardinality serializes distinct relation instances for one player.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut a = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let mut b = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut a, &snippets, &mut used, "assign-first-shift").await;
        run(&mut b, &snippets, &mut used, "assign-second-shift").await;
        a.commit().await.unwrap();
        assert!(b.commit().await.unwrap_err().to_string().contains("[STC2]"));
        let mut late = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut late, &snippets, &mut used, "assign-second-shift").await;
        assert!(
            late.commit()
                .await
                .unwrap_err()
                .to_string()
                .contains("[CNT5]")
        );
        expect_rows(&runtime, &db, "match $a isa assignment; select $a;", 1).await;

        // A key collision has different query/commit phases for later snapshots.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut a = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let mut b = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut a, &snippets, &mut used, "keyed-purchase").await;
        run(&mut b, &snippets, &mut used, "keyed-purchase").await;
        a.commit().await.unwrap();
        assert!(b.commit().await.unwrap_err().to_string().contains("[STC2]"));
        let mut late = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let error = late
            .query(sql(&snippets, &mut used, "keyed-purchase"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("[CNT9]"));
        drop(late);
        expect_rows(&runtime, &db, "match $p isa purchase; select $p;", 1).await;

        // A committed lock survives its acquisition transaction until explicit release.
        let db = seeded_database(&runtime, &snippets, &mut used, &mut databases).await;
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut tx, &snippets, &mut used, "acquire-external-lock").await;
        tx.commit().await.unwrap();
        let mut waiter = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        let error = waiter
            .query(sql(&snippets, &mut used, "acquire-external-lock"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("[CNT9]"));
        drop(waiter);
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut tx, &snippets, &mut used, "release-external-lock").await;
        tx.commit().await.unwrap();
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut tx, &snippets, &mut used, "acquire-external-lock").await;
        tx.commit().await.unwrap();
        let mut tx = runtime.open_transaction(&db, TxType::Write).await.unwrap();
        run(&mut tx, &snippets, &mut used, "release-external-lock").await;
        tx.commit().await.unwrap();
        expect_rows(
            &runtime,
            &db,
            "match $l isa lock-row, has lock-key \"doctor-a-external\"; select $l;",
            0,
        )
        .await;

        assert_eq!(used, snippets.keys().copied().collect());
        println!(
            "executed all {} guide snippets across {} isolated scenarios",
            used.len(),
            databases.len()
        );
    }
    for database in databases {
        runtime.delete_database(&database).await.unwrap();
    }
    runtime.force_close().unwrap();
}
