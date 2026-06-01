use serde_json::{Value, json};

use super::integration_support::{
    attr_date, attr_long, attr_string, row_attribute, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_relation_crud_against_typedb() {
    let Some((db, schema)) = setup_node_database("relation") else {
        return;
    };
    let person_manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("person manager should be created");
    let company_manager = db
        .entity_manager_json(schema.company_descriptor_json())
        .expect("company manager should be created");
    let relation_manager = db
        .relation_manager_json(schema.employment_descriptor_json())
        .expect("relation manager should be created");

    let alice_iid = person_manager
        .insert_json(json!({"name": attr_string("Alice"), "age": attr_long(30)}).to_string())
        .expect("person insert should return IID");
    let bob_iid = person_manager
        .insert_json(json!({"name": attr_string("Bob"), "age": attr_long(40)}).to_string())
        .expect("person insert should return IID");
    let acme_iid = company_manager
        .insert_json(json!({"name": attr_string("Acme")}).to_string())
        .expect("company insert should return IID");

    let alice_role_players = json!([
        {"role_name": "employee", "player_type_name": schema.person_type, "iid": alice_iid},
        {"role_name": "employer", "player_type_name": schema.company_type, "iid": acme_iid}
    ]);
    let bob_role_players = json!([
        {"role_name": "employee", "player_type_name": schema.person_type, "iid": bob_iid},
        {"role_name": "employer", "player_type_name": schema.company_type, "iid": acme_iid}
    ]);

    let relation_iid = relation_manager
        .insert_json(
            json!({"since": attr_date("2026-05-27")}).to_string(),
            alice_role_players.to_string(),
        )
        .expect("relation insert should return IID");
    assert!(!relation_iid.is_empty());

    let rows: Value = serde_json::from_str(
        &relation_manager
            .get_with_role_players_json(
                Some(json!({"since": attr_date("2026-05-27")}).to_string()),
                Some(alice_role_players.to_string()),
            )
            .expect("relation role-player get should return rows"),
    )
    .expect("relation rows should be JSON");
    assert_eq!(rows.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&rows[0], &schema.since_attr),
        Some(&json!({"Date": "2026-05-27"}))
    );
    assert_eq!(rows[0]["role_players"][0]["role_name"], "employee");

    relation_manager
        .update_json(
            json!({"since": attr_date("2026-05-28")}).to_string(),
            alice_role_players.to_string(),
            Some(relation_iid.clone()),
        )
        .expect("relation update should succeed");

    let by_iid: Value = serde_json::from_str(
        &relation_manager
            .get_by_iid_json(relation_iid.clone())
            .expect("relation get_by_iid should return rows"),
    )
    .expect("relation get_by_iid rows should be JSON");
    assert_eq!(
        row_attribute(&by_iid[0], &schema.since_attr),
        Some(&json!({"Date": "2026-05-28"}))
    );

    let put_iids: Vec<String> = serde_json::from_str(
        &relation_manager
            .put_many_json(
                json!([
                    {
                        "attributes": {"since": attr_date("2026-05-29")},
                        "role_players": alice_role_players
                    },
                    {
                        "attributes": {"since": attr_date("2026-05-30")},
                        "role_players": bob_role_players
                    }
                ])
                .to_string(),
            )
            .expect("relation put_many should return IIDs"),
    )
    .expect("relation put_many IIDs should be JSON");
    assert_eq!(put_iids.len(), 2);

    let count = relation_manager
        .count_json(None)
        .expect("relation count should work");
    assert_eq!(count, "2");

    let aggregate_rows: Value = serde_json::from_str(
        &relation_manager
            .aggregate_json(
                json!([{"result_key": "count", "function": "count", "attr_name": null}])
                    .to_string(),
                None,
            )
            .expect("relation aggregate should work"),
    )
    .expect("relation aggregate rows should be JSON");
    assert_eq!(aggregate_rows[0]["$count"]["value"], 2);

    let grouped_rows: Value = serde_json::from_str(
        &relation_manager
            .group_by_aggregate_json(
                json!(["since"]).to_string(),
                json!([{"result_key": "count", "function": "count", "attr_name": null}])
                    .to_string(),
                None,
            )
            .expect("relation group_by_aggregate should work"),
    )
    .expect("relation group rows should be JSON");
    assert_eq!(
        grouped_rows
            .as_array()
            .expect("group rows are an array")
            .len(),
        2
    );

    relation_manager
        .delete_by_iid(relation_iid)
        .expect("relation delete should work");
}
