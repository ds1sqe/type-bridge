use serde_json::{Value, json};

use super::integration_support::{
    attr_boolean, attr_date, attr_datetime, attr_datetimetz, attr_decimal, attr_double,
    attr_duration, attr_long, attr_string, row_attributes, setup_node_database,
};

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_entity_multi_value_attributes_against_typedb() {
    let Some((db, schema)) = setup_node_database("multi") else {
        return;
    };
    let manager = db
        .entity_manager_json(schema.person_descriptor_json())
        .expect("entity manager should be created");

    let iid = manager
        .insert_json(
            json!({
                "name": attr_string("MultiTypes"),
                "age": [attr_long(85), attr_long(90), attr_long(78)],
                "score": [attr_double(1.5), attr_double(2.7), attr_double(3.9)],
                "active": [attr_boolean(true), attr_boolean(false)],
                "birthday": [attr_date("2024-01-15"), attr_date("2024-03-01"), attr_date("2024-06-01")],
                "login_at": [
                    attr_datetime("2024-01-01T10:00:00"),
                    attr_datetime("2024-01-01T11:00:00"),
                    attr_datetime("2024-01-01T12:00:00")
                ],
                "seen_at": [
                    attr_datetimetz("2024-01-01T10:00:00+00:00"),
                    attr_datetimetz("2024-01-01T14:00:00+00:00")
                ],
                "balance": [attr_decimal("999.99"), attr_decimal("899.99"), attr_decimal("849.99")],
                "session_length": [attr_duration("PT30M"), attr_duration("PT1H"), attr_duration("PT2H")]
            })
            .to_string(),
        )
        .expect("multi-value insert should return IID");
    assert!(!iid.is_empty());

    let rows: Value = serde_json::from_str(
        &manager
            .get_json(Some(json!({"name": attr_string("MultiTypes")}).to_string()))
            .expect("entity get should return rows"),
    )
    .expect("entity rows should be JSON");
    assert_eq!(rows.as_array().expect("rows are an array").len(), 1);
    assert_eq!(row_attributes(&rows[0], &schema.age_attr).len(), 3);
    assert_eq!(row_attributes(&rows[0], &schema.score_attr).len(), 3);
    assert_eq!(row_attributes(&rows[0], &schema.active_attr).len(), 2);
    assert_eq!(row_attributes(&rows[0], &schema.birthday_attr).len(), 3);
    assert_eq!(row_attributes(&rows[0], &schema.login_at_attr).len(), 3);
    assert_eq!(row_attributes(&rows[0], &schema.seen_at_attr).len(), 2);
    assert_eq!(row_attributes(&rows[0], &schema.balance_attr).len(), 3);
    assert_eq!(
        row_attributes(&rows[0], &schema.session_length_attr).len(),
        3
    );

    manager
        .update_json(
            json!({
                "name": attr_string("MultiTypes"),
                "age": [attr_long(100), attr_long(200)],
                "balance": [attr_decimal("10.00"), attr_decimal("20.00")],
                "session_length": [attr_duration("PT10M")]
            })
            .to_string(),
            Some(iid.clone()),
        )
        .expect("multi-value update should replace provided attributes");

    let updated: Value = serde_json::from_str(
        &manager
            .get_by_iid_json(iid.clone())
            .expect("entity get_by_iid should return row"),
    )
    .expect("entity get_by_iid row should be JSON");
    assert_eq!(row_attributes(&updated, &schema.age_attr).len(), 2);
    assert_eq!(row_attributes(&updated, &schema.balance_attr).len(), 2);
    assert_eq!(
        row_attributes(&updated, &schema.session_length_attr).len(),
        1
    );

    manager
        .delete_by_iid(iid)
        .expect("entity delete should work");
}
