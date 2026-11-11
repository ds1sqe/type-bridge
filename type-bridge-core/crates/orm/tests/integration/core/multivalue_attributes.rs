//! Dynamic multi-value attribute CRUD integration tests against TypeDB.
//!

use crate::common::dynamic_crud::*;
use crate::internal::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_entity_multi_value_attributes_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("multi").await;
    let manager =
        DynamicEntityManager::from_registry(&db, schema.registry(), schema.person_descriptor());

    let iid = manager
        .insert(&multi_value_attrs(&schema, "MultiTypes"))
        .await
        .expect("multi-value insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    assert!(!iid.is_empty());

    let rows = manager
        .get(&[Filter::string_eq(&schema.name_attr, "MultiTypes")])
        .await
        .expect("entity get should return rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(attr_values(&rows[0], &schema.age_attr).len(), 3);
    assert_eq!(attr_values(&rows[0], &schema.score_attr).len(), 3);
    assert_eq!(attr_values(&rows[0], &schema.active_attr).len(), 2);
    assert_eq!(attr_values(&rows[0], &schema.birthday_attr).len(), 3);
    assert_eq!(attr_values(&rows[0], &schema.login_at_attr).len(), 3);
    assert_eq!(attr_values(&rows[0], &schema.seen_at_attr).len(), 2);
    assert_eq!(attr_values(&rows[0], &schema.balance_attr).len(), 3);
    assert_eq!(attr_values(&rows[0], &schema.session_length_attr).len(), 3);

    let update_attrs = vec![
        (
            schema.name_attr.clone(),
            AttributeValue::String("MultiTypes".into()),
        ),
        (schema.age_attr.clone(), AttributeValue::Long(100)),
        (schema.age_attr.clone(), AttributeValue::Long(200)),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("10.00".into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("20.00".into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration("PT600S".into()),
        ),
    ];
    manager
        .update(Some(&iid), &update_attrs)
        .await
        .expect("multi-value update should replace provided attributes");

    let updated = manager
        .get_by_iid(&iid)
        .await
        .expect("entity get_by_iid should succeed")
        .expect("entity should exist after update");
    assert_eq!(attr_values(&updated, &schema.age_attr).len(), 2);
    assert_eq!(attr_values(&updated, &schema.balance_attr).len(), 2);
    assert_eq!(attr_values(&updated, &schema.session_length_attr).len(), 1);

    manager
        .delete_by_iid(&iid)
        .await
        .expect("entity delete should work");
}

fn multi_value_attrs(schema: &DynamicCrudSchema, name: &str) -> DynamicAttributeMap {
    vec![
        (
            schema.name_attr.clone(),
            AttributeValue::String(name.into()),
        ),
        (schema.age_attr.clone(), AttributeValue::Long(85)),
        (schema.age_attr.clone(), AttributeValue::Long(90)),
        (schema.age_attr.clone(), AttributeValue::Long(78)),
        (schema.score_attr.clone(), AttributeValue::Double(1.5)),
        (schema.score_attr.clone(), AttributeValue::Double(2.7)),
        (schema.score_attr.clone(), AttributeValue::Double(3.9)),
        (schema.active_attr.clone(), AttributeValue::Boolean(true)),
        (schema.active_attr.clone(), AttributeValue::Boolean(false)),
        (
            schema.birthday_attr.clone(),
            AttributeValue::Date("2024-01-15".into()),
        ),
        (
            schema.birthday_attr.clone(),
            AttributeValue::Date("2024-03-01".into()),
        ),
        (
            schema.birthday_attr.clone(),
            AttributeValue::Date("2024-06-01".into()),
        ),
        (
            schema.login_at_attr.clone(),
            AttributeValue::DateTime("2024-01-01T10:00:00".into()),
        ),
        (
            schema.login_at_attr.clone(),
            AttributeValue::DateTime("2024-01-01T11:00:00".into()),
        ),
        (
            schema.login_at_attr.clone(),
            AttributeValue::DateTime("2024-01-01T12:00:00".into()),
        ),
        (
            schema.seen_at_attr.clone(),
            AttributeValue::DateTimeTZ("2024-01-01T10:00:00+00:00".into()),
        ),
        (
            schema.seen_at_attr.clone(),
            AttributeValue::DateTimeTZ("2024-01-01T14:00:00+00:00".into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("999.99".into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("899.99".into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("849.99".into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration("PT1800S".into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration("PT3600S".into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration("PT7200S".into()),
        ),
    ]
}
