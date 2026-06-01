//! Dynamic transaction-bound manager integration tests against TypeDB.
//!

use crate::common::dynamic_crud::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_entity_transaction_commit_and_rollback_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("tx").await;
    let descriptor = schema.person_descriptor();
    let db_manager = DynamicEntityManager::new(&db, descriptor.clone());

    let rollback_tx = db
        .transaction_context(TxType::Write)
        .await
        .expect("write transaction should open");
    let rollback_manager =
        DynamicEntityManager::with_transaction(rollback_tx.clone(), descriptor.clone());
    rollback_manager
        .insert(&person_attrs("Rollback", 20))
        .await
        .expect("transaction-bound insert should succeed");
    assert_eq!(
        rollback_manager
            .count_with_filters(&[Filter::string_eq("name", "Rollback")])
            .await
            .expect("transaction-bound read should see uncommitted write"),
        1
    );
    rollback_tx
        .rollback()
        .await
        .expect("rollback should succeed");
    assert!(
        db_manager
            .get(&[Filter::string_eq("name", "Rollback")])
            .await
            .expect("post-rollback lookup should succeed")
            .is_empty()
    );

    let commit_tx = db
        .transaction_context(TxType::Write)
        .await
        .expect("write transaction should open");
    let commit_manager = DynamicEntityManager::with_transaction(commit_tx.clone(), descriptor);
    commit_manager
        .insert(&person_attrs("Commit", 21))
        .await
        .expect("transaction-bound insert should succeed");
    commit_tx.commit().await.expect("commit should succeed");

    let committed = db_manager
        .get(&[Filter::string_eq("name", "Commit")])
        .await
        .expect("post-commit lookup should succeed");
    assert_eq!(committed.len(), 1);
}
