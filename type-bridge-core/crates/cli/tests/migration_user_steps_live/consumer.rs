use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use type_bridge::{MigrationPlan, MigrationPreview, MigrationTransaction};
use type_bridge_generated_schema::{
    AppSchema, Gender, Person, PersonCreate, SCHEMA, open_migration_catalog,
};
static CANCEL: Mutex<Option<type_bridge::MigrationCancellation>> = Mutex::new(None);
static FAIL: AtomicBool = AtomicBool::new(true);
static ESCAPED: Mutex<Option<MigrationTransaction<AppSchema>>> = Mutex::new(None);
async fn derive(tx: MigrationTransaction<AppSchema>) -> type_bridge::Result<()> {
    let manager = tx.entities::<Person>();
    let people = manager.all().await?;
    assert_eq!(people.len(), 2);
    {
        let mut session = tx.query();
        let person = session.exact::<Person>()?;
        assert_eq!(session.query(person)?.count_by(person).await?, 2);
    }
    for person in people {
        let gender = if person.ssn().value().as_bytes().last().unwrap() % 2 == 0 {
            "female"
        } else {
            "male"
        };
        manager
            .update(
                person.iid(),
                PersonCreate::new(Some(Gender::try_new(gender).unwrap()), person.ssn().clone())
                    .unwrap(),
            )
            .await?;
    }
    if FAIL.load(Ordering::SeqCst) {
        return Err(type_bridge::Error::SchemaVerification {
            message: "private failure detail".into(),
            source: None,
        });
    }
    let cancellation = CANCEL.lock().unwrap().clone();
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
        std::future::pending::<()>().await;
    }
    let answer = tx
        .execute_query("match $p isa person; fetch {\"ssn\":$p.ssn, \"gender\":$p.gender};")
        .await
        .unwrap();
    assert_eq!(answer["answers"].as_array().unwrap().len(), 2);
    *ESCAPED.lock().unwrap() = Some(tx);
    Ok(())
}
async fn reverse(tx: MigrationTransaction<AppSchema>) -> type_bridge::Result<()> {
    tx.execute_query("match $p isa person, has gender $g; delete has $g of $p;")
        .await
        .unwrap();
    Ok(())
}
fn authorize(p: MigrationPreview<AppSchema>, approve: bool) -> MigrationPlan<AppSchema> {
    let mut b = p.approval_builder();
    if approve {
        b.approve(0).unwrap();
    }
    p.authorize(&b.finish().unwrap()).unwrap()
}
#[tokio::main]
async fn main() {
    let db = type_bridge::Database::connect(
        type_bridge::ConnectionOptions::new(
            std::env::var("TYPEDB_ADDRESS").unwrap(),
            std::env::var("TYPE_BRIDGE_CALLBACK_DATABASE").unwrap(),
        )
        .credentials(
            std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
            std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        )
        .http_port(std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap()),
    )
    .await
    .unwrap()
    .with_schema(SCHEMA)
    .unwrap();
    db.create_database().await.unwrap();
    let mut c = open_migration_catalog().unwrap();
    let first = c.entry(0).unwrap().id().clone();
    let second = c.entry(1).unwrap().id().clone();
    authorize(
        c.preview_apply([], Some(vec![first.clone()])).unwrap(),
        false,
    )
    .execute(&db, "rust-initial")
    .await
    .unwrap();
    db.execute_query(
        "insert $p isa person, has ssn \"1231\"; $q isa person, has ssn \"1232\";",
        type_bridge::raw::TransactionKind::Write,
    )
    .await
    .unwrap();
    c.register_with_reverse("derive-gender", derive, reverse)
        .unwrap();
    let p = authorize(
        c.preview_apply([first.clone()], Some(vec![second.clone()]))
            .unwrap(),
        true,
    );
    let error = p.execute(&db, "rust-error").await.unwrap_err();
    assert!(error.to_string().contains("migration_user_step_failed"));
    assert_eq!(
        db.execute_query(
            "match $p isa person, has gender $g; fetch {\"gender\":$g};",
            type_bridge::raw::TransactionKind::Read
        )
        .await
        .unwrap()["answers"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    FAIL.store(false, Ordering::SeqCst);
    let cancellation = type_bridge::MigrationCancellation::default();
    *CANCEL.lock().unwrap() = Some(cancellation.clone());
    let control =
        type_bridge::MigrationExecutionControl::new(cancellation, None, Default::default());
    let error = p
        .execute_controlled(&db, "rust-cancel", &control)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("migration_execution_cancelled"));
    *CANCEL.lock().unwrap() = None;
    assert_eq!(
        db.execute_query(
            "match $p isa person, has gender $g; fetch {\"gender\":$g};",
            type_bridge::raw::TransactionKind::Read
        )
        .await
        .unwrap()["answers"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    p.execute(&db, "rust-data").await.unwrap();
    let tx = ESCAPED.lock().unwrap().take().unwrap();
    assert_eq!(
        tx.execute_query("match $p isa person; fetch {\"ssn\":$p.ssn};")
            .await
            .unwrap_err()
            .code,
        "raw_transaction_closed"
    );
    authorize(
        c.preview_rollback([first.clone(), second.clone()], [second.clone()])
            .unwrap(),
        true,
    )
    .execute(&db, "rust-reverse")
    .await
    .unwrap();
    authorize(c.preview_apply([first], Some(vec![second])).unwrap(), true)
        .execute(&db, "rust-reapply")
        .await
        .unwrap();
    println!(
        "PASS Rust plain async function + generated typed managers + raw + failure rollback + reverse + invalid escaped handle"
    );
}
