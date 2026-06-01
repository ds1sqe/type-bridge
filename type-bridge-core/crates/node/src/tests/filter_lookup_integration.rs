use serde_json::{Value, json};

use super::integration_support::{
    attr_date, attr_double, attr_long, attr_string, row_attribute, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_filters_and_lookup_against_typedb() {
    let Some((db, schema)) = setup_node_database("filters") else {
        return;
    };
    let manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("entity manager should be created");

    manager
        .insert_many_json(
            json!([
                {"name": attr_string("Alice"), "age": attr_long(30), "score": attr_double(91.25)},
                {"name": attr_string("Bob"), "age": attr_long(40), "score": attr_double(91.25)},
                {"name": attr_string("Carol"), "age": attr_long(50), "score": attr_double(91.25)}
            ])
            .to_string(),
        )
        .expect("entity insert_many should return IIDs");

    let exact: Value = serde_json::from_str(
        &manager
            .get_json(Some(json!({"age": attr_long(40)}).to_string()))
            .expect("equality filter should return rows"),
    )
    .expect("entity rows should be JSON");
    assert_eq!(exact.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&exact[0], &schema.name_attr),
        Some(&json!({"String": "Bob"}))
    );

    let comparison: Value = serde_json::from_str(
        &manager
            .get_json(Some(
                json!([{"attr_name": "age", "operator": ">=", "value": attr_long(40)}]).to_string(),
            ))
            .expect("comparison filter should return rows"),
    )
    .expect("comparison rows should be JSON");
    assert_eq!(comparison.as_array().expect("rows are an array").len(), 2);

    let count = manager
        .count_json(Some(
            json!([{"attr_name": "score", "operator": ">", "value": attr_double(90.0)}])
                .to_string(),
        ))
        .expect("count_with_filters should apply comparison filters");
    assert_eq!(count, "3");

    let missing: Value = serde_json::from_str(
        &manager
            .get_json(Some(json!({"name": attr_string("Nobody")}).to_string()))
            .expect("missing lookup should return an empty row set"),
    )
    .expect("missing rows should be JSON");
    assert!(missing.as_array().expect("rows are an array").is_empty());
}

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_relation_filters_and_role_lookup_against_typedb() {
    let Some((db, schema)) = setup_node_database("rel-filters") else {
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

    let by_attr: Value = serde_json::from_str(
        &relation_manager
            .get_json(Some(json!({"since": attr_date("2026-05-27")}).to_string()))
            .expect("relation attribute filter should return rows"),
    )
    .expect("relation rows should be JSON");
    assert_eq!(by_attr.as_array().expect("rows are an array").len(), 1);

    let by_role: Value = serde_json::from_str(
        &relation_manager
            .get_with_role_players_json(None, Some(bob_roles.to_string()))
            .expect("relation role-player filter should return rows"),
    )
    .expect("relation role rows should be JSON");
    assert_eq!(by_role.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&by_role[0], &schema.since_attr),
        Some(&json!({"Date": "2026-05-28"}))
    );
}
