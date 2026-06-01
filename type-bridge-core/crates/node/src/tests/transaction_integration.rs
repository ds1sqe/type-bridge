use serde_json::{Value, json};

use super::integration_support::{attr_long, attr_string, setup_node_database};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_transaction_commit_and_rollback_against_typedb() {
    let Some((db, schema)) = setup_node_database("tx") else {
        return;
    };
    let db_manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("database manager should be created");

    let rollback_tx = db
        .transaction(Some("write".to_string()))
        .expect("write transaction should open");
    let rollback_manager = rollback_tx
        .entity_manager_json(schema.person_descriptor_json())
        .expect("transaction-bound manager should be created");
    rollback_manager
        .insert_json(json!({"name": attr_string("Rollback"), "age": attr_long(20)}).to_string())
        .expect("transaction-bound insert should succeed");
    assert_eq!(
        rollback_manager
            .count_json(Some(json!({"name": attr_string("Rollback")}).to_string()))
            .expect("transaction-bound read should see uncommitted write"),
        "1"
    );
    rollback_tx.rollback().expect("rollback should succeed");
    let rolled_back: Value = serde_json::from_str(
        &db_manager
            .get_json(Some(json!({"name": attr_string("Rollback")}).to_string()))
            .expect("post-rollback lookup should succeed"),
    )
    .expect("rolled-back lookup should be JSON");
    assert!(
        rolled_back
            .as_array()
            .expect("rows are an array")
            .is_empty()
    );

    let commit_tx = db
        .transaction(Some("write".to_string()))
        .expect("write transaction should open");
    let commit_manager = commit_tx
        .entity_manager_json(schema.person_descriptor_json())
        .expect("transaction-bound manager should be created");
    commit_manager
        .insert_json(json!({"name": attr_string("Commit"), "age": attr_long(21)}).to_string())
        .expect("transaction-bound insert should succeed");
    commit_tx.commit().expect("commit should succeed");

    let committed: Value = serde_json::from_str(
        &db_manager
            .get_json(Some(json!({"name": attr_string("Commit")}).to_string()))
            .expect("post-commit lookup should succeed"),
    )
    .expect("committed lookup should be JSON");
    assert_eq!(committed.as_array().expect("rows are an array").len(), 1);
}
