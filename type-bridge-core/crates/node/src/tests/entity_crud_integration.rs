use serde_json::{Value, json};

use super::integration_support::{
    attr_double, attr_long, attr_string, row_attribute, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_crud_against_typedb() {
    let Some((db, schema)) = setup_node_database("entity") else {
        return;
    };
    let manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("entity manager should be created");

    let alice = json!({
        "name": attr_string("Alice"),
        "age": attr_long(30),
        "score": attr_double(88.5),
    });
    let alice_iid = manager
        .insert_json(alice.to_string())
        .expect("entity insert should return IID");
    assert!(!alice_iid.is_empty());

    let rows: Value = serde_json::from_str(
        &manager
            .get_json(Some(json!({"name": attr_string("Alice")}).to_string()))
            .expect("entity get should return rows"),
    )
    .expect("entity rows should be JSON");
    assert_eq!(rows.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&rows[0], &schema.name_attr),
        Some(&json!({"String": "Alice"}))
    );

    manager
        .update_json(
            json!({
                "name": attr_string("Alice"),
                "age": attr_long(31),
                "score": attr_double(91.25),
            })
            .to_string(),
            Some(alice_iid.clone()),
        )
        .expect("entity update should succeed");

    let by_iid: Value = serde_json::from_str(
        &manager
            .get_by_iid_json(alice_iid.clone())
            .expect("entity get_by_iid should return row"),
    )
    .expect("entity get_by_iid row should be JSON");
    assert_eq!(
        row_attribute(&by_iid, &schema.age_attr),
        Some(&json!({"Long": "31"}))
    );

    let batch_iids: Vec<String> = serde_json::from_str(
        &manager
            .put_many_json(
                json!([
                    {"name": attr_string("Bob"), "age": attr_long(40), "score": attr_double(70.0)},
                    {"name": attr_string("Carol"), "age": attr_long(50), "score": attr_double(80.0)}
                ])
                .to_string(),
            )
            .expect("entity put_many should return IIDs"),
    )
    .expect("put_many IIDs should be JSON");
    assert_eq!(batch_iids.len(), 2);

    let count = manager.count_json(None).expect("entity count should work");
    assert_eq!(count, "3");

    let aggregate_rows: Value = serde_json::from_str(
        &manager
            .aggregate_json(
                json!([
                    {"result_key": "count", "function": "count", "attr_name": null},
                    {"result_key": "avg_age", "function": "mean", "attr_name": "age"}
                ])
                .to_string(),
                None,
            )
            .expect("entity aggregate should work"),
    )
    .expect("aggregate rows should be JSON");
    assert_eq!(aggregate_rows[0]["$count"]["value"], 3);

    let grouped_rows: Value = serde_json::from_str(
        &manager
            .group_by_aggregate_json(
                json!(["name"]).to_string(),
                json!([{"result_key": "count", "function": "count", "attr_name": null}])
                    .to_string(),
                None,
            )
            .expect("entity group_by_aggregate should work"),
    )
    .expect("grouped rows should be JSON");
    assert_eq!(
        grouped_rows
            .as_array()
            .expect("group rows are an array")
            .len(),
        3
    );

    manager
        .delete_by_iid(alice_iid.clone())
        .expect("entity delete should work");
    let deleted: Value = serde_json::from_str(
        &manager
            .get_by_iid_json(alice_iid)
            .expect("deleted entity lookup should succeed"),
    )
    .expect("deleted lookup should be JSON");
    assert!(deleted.is_null());
}
