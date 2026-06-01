//! Dynamic transaction-bound manager integration tests against TypeDB.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_transaction_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_transaction_commit_and_rollback_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("tx").await else {
        return;
    };
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
