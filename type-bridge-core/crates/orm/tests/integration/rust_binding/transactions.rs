use crate::common::rust_binding::*;
use type_bridge_orm::*;

#[tokio::test]
async fn transaction_context_batch_commit() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_person_schema(&db).await;

    let manager = EntityManager::<Person>::new(&db);
    let tx_prefix = unique_label("TxBatch");
    let mut people = vec![
        Person {
            iid: None,
            name: Name(format!("{tx_prefix}-1")),
            age: Age(20),
        },
        Person {
            iid: None,
            name: Name(format!("{tx_prefix}-2")),
            age: Age(25),
        },
    ];
    let iids = manager
        .insert_many(&mut people)
        .await
        .expect("insert_many failed");
    assert_eq!(iids.len(), 2);

    let results = manager
        .query()
        .filter(Expr::contains("name", tx_prefix))
        .execute()
        .await
        .expect("query after batch failed");
    assert_eq!(results.len(), 2);

    manager
        .delete_many(&people)
        .await
        .expect("delete_many failed");
}
