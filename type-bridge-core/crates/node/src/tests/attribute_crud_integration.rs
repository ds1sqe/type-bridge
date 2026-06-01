use serde_json::{Value, json};

use super::integration_support::{
    attr_boolean, attr_date, attr_datetime, attr_datetimetz, attr_decimal, attr_double,
    attr_duration, attr_long, attr_string, row_attribute, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_all_primitive_attribute_values_against_typedb() {
    let Some((db, schema)) = setup_node_database("attrs") else {
        return;
    };
    let manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("entity manager should be created");

    let iid = manager
        .insert_json(
            json!({
                "name": attr_string("AllTypes"),
                "age": attr_long(33),
                "score": attr_double(91.25),
                "active": attr_boolean(true),
                "birthday": attr_date("1990-01-02"),
                "login_at": attr_datetime("2026-05-27T10:30:00"),
                "seen_at": attr_datetimetz("2026-05-27T10:30:00+00:00"),
                "balance": attr_decimal("1234.56"),
                "session_length": attr_duration("PT2H30M")
            })
            .to_string(),
        )
        .expect("entity insert with all primitive attributes should return IID");
    assert!(!iid.is_empty());

    let rows: Value = serde_json::from_str(
        &manager
            .get_json(Some(json!({"name": attr_string("AllTypes")}).to_string()))
            .expect("entity get should return rows"),
    )
    .expect("entity rows should be JSON");
    assert_eq!(rows.as_array().expect("rows are an array").len(), 1);
    assert_eq!(
        row_attribute(&rows[0], &schema.name_attr),
        Some(&json!({"String": "AllTypes"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.age_attr),
        Some(&json!({"Long": "33"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.score_attr),
        Some(&json!({"Double": 91.25}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.active_attr),
        Some(&json!({"Boolean": true}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.birthday_attr),
        Some(&json!({"Date": "1990-01-02"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.login_at_attr),
        Some(&json!({"DateTime": "2026-05-27T10:30:00"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.seen_at_attr),
        Some(&json!({"DateTimeTZ": "2026-05-27T10:30:00+00:00"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.balance_attr),
        Some(&json!({"Decimal": "1234.56"}))
    );
    assert_eq!(
        row_attribute(&rows[0], &schema.session_length_attr),
        Some(&json!({"Duration": "PT2H30M"}))
    );

    manager
        .update_json(
            json!({
                "name": attr_string("AllTypes"),
                "age": attr_long(34),
                "score": attr_double(99.5),
                "active": attr_boolean(false),
                "birthday": attr_date("1991-03-04"),
                "login_at": attr_datetime("2026-05-28T11:45:00"),
                "seen_at": attr_datetimetz("2026-05-28T11:45:00+00:00"),
                "balance": attr_decimal("4321.00"),
                "session_length": attr_duration("PT45M")
            })
            .to_string(),
            Some(iid.clone()),
        )
        .expect("entity update with all primitive attributes should succeed");

    let updated: Value = serde_json::from_str(
        &manager
            .get_by_iid_json(iid.clone())
            .expect("entity get_by_iid should return row"),
    )
    .expect("entity get_by_iid row should be JSON");
    assert_eq!(
        row_attribute(&updated, &schema.active_attr),
        Some(&json!({"Boolean": false}))
    );
    assert_eq!(
        row_attribute(&updated, &schema.balance_attr),
        Some(&json!({"Decimal": "4321.00"}))
    );
    assert_eq!(
        row_attribute(&updated, &schema.session_length_attr),
        Some(&json!({"Duration": "PT45M"}))
    );

    manager
        .delete_by_iid(iid)
        .expect("entity delete should work");
}
