use serde_json::{Value, json};

use super::integration_support::{
    attr_date, attr_long, attr_string, row_attribute, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_filter_then_iid_update_and_delete_against_typedb() {
    let Some((db, schema)) = setup_node_database("chainable-entity") else {
        return;
    };
    let manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("entity manager should be created");

    manager
        .insert_many_json(
            json!([
                {"name": attr_string("Alice"), "age": attr_long(30)},
                {"name": attr_string("Bob"), "age": attr_long(40)},
                {"name": attr_string("Carol"), "age": attr_long(50)}
            ])
            .to_string(),
        )
        .expect("entity insert_many should return IIDs");

    let selected: Value = serde_json::from_str(
        &manager
            .get_json(Some(
                json!([{"attr_name": "age", "operator": ">=", "value": attr_long(40)}]).to_string(),
            ))
            .expect("comparison filter should return rows"),
    )
    .expect("entity rows should be JSON");
    assert_eq!(selected.as_array().expect("rows are an array").len(), 2);

    for row in selected.as_array().expect("rows are an array") {
        let iid = row["iid"].as_str().expect("filtered row includes IID");
        let name = row_attribute(row, &schema.name_attr)
            .and_then(|value| value.get("String"))
            .and_then(Value::as_str)
            .expect("row has string name");
        let age = row_attribute(row, &schema.age_attr)
            .and_then(|value| value.get("Long"))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .expect("row has long age");
        manager
            .update_json(
                json!({"name": attr_string(name), "age": attr_long(age + 1)}).to_string(),
                Some(iid.to_string()),
            )
            .expect("IID-scoped update should work after filtered selection");
    }

    let to_delete: Value = serde_json::from_str(
        &manager
            .get_json(Some(
                json!([{"attr_name": "age", "operator": ">", "value": attr_long(50)}]).to_string(),
            ))
            .expect("delete selection should return rows"),
    )
    .expect("delete selection should be JSON");
    assert_eq!(to_delete.as_array().expect("rows are an array").len(), 1);
    let delete_iid = to_delete[0]["iid"]
        .as_str()
        .expect("delete row includes IID")
        .to_string();
    manager
        .delete_by_iid(delete_iid)
        .expect("IID-scoped delete should work after filtered selection");

    let remaining: Value = serde_json::from_str(
        &manager
            .get_json(None)
            .expect("remaining entity rows should fetch"),
    )
    .expect("remaining rows should be JSON");
    assert_eq!(remaining.as_array().expect("rows are an array").len(), 2);
}

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_relation_filter_then_iid_update_and_delete_against_typedb() {
    let Some((db, schema)) = setup_node_database("chainable-relation") else {
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

    let alice_roles = json!([
        {"role_name": "employee", "player_type_name": schema.person_type, "iid": alice_iid},
        {"role_name": "employer", "player_type_name": schema.company_type, "iid": acme_iid}
    ]);
    let bob_roles = json!([
        {"role_name": "employee", "player_type_name": schema.person_type, "iid": bob_iid},
        {"role_name": "employer", "player_type_name": schema.company_type, "iid": acme_iid}
    ]);
    relation_manager
        .insert_json(
            json!({"since": attr_date("2026-05-27")}).to_string(),
            alice_roles.to_string(),
        )
        .expect("relation insert should return IID");
    relation_manager
        .insert_json(
            json!({"since": attr_date("2026-05-28")}).to_string(),
            bob_roles.to_string(),
        )
        .expect("relation insert should return IID");

    let selected: Value = serde_json::from_str(
        &relation_manager
            .get_json(Some(json!({"since": attr_date("2026-05-28")}).to_string()))
            .expect("relation filter should return rows"),
    )
    .expect("relation rows should be JSON");
    assert_eq!(selected.as_array().expect("rows are an array").len(), 1);
    let selected_iid = selected[0]["iid"]
        .as_str()
        .expect("relation row includes IID")
        .to_string();
    relation_manager
        .update_json(
            json!({"since": attr_date("2026-06-01")}).to_string(),
            json!([]).to_string(),
            Some(selected_iid),
        )
        .expect("IID-scoped relation update should not require role players");

    let to_delete: Value = serde_json::from_str(
        &relation_manager
            .get_json(Some(json!({"since": attr_date("2026-05-27")}).to_string()))
            .expect("delete selection should return rows"),
    )
    .expect("delete selection should be JSON");
    assert_eq!(to_delete.as_array().expect("rows are an array").len(), 1);
    relation_manager
        .delete_by_iid(
            to_delete[0]["iid"]
                .as_str()
                .expect("delete row includes IID")
                .to_string(),
        )
        .expect("IID-scoped relation delete should work after filtered selection");

    let remaining: Value = serde_json::from_str(
        &relation_manager
            .get_json(None)
            .expect("remaining relation rows should fetch"),
    )
    .expect("remaining rows should be JSON");
    assert_eq!(remaining.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&remaining[0], &schema.since_attr),
        Some(&json!({"Date": "2026-06-01"}))
    );
}
