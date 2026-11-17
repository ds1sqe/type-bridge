//! Dynamic attribute value CRUD integration tests against a real TypeDB instance.
//!

use crate::common::dynamic_crud::*;
use crate::internal::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_entity_all_primitive_attribute_values_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("attrs").await;
    let manager =
        DynamicEntityManager::from_registry(&db, schema.registry(), schema.person_descriptor());

    let iid = manager
        .insert(&schema.all_value_attrs("AllTypes", 33))
        .await
        .expect("entity insert with all primitive attributes should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    assert!(!iid.is_empty());

    let rows = manager
        .get(&[Filter::string_eq(&schema.name_attr, "AllTypes")])
        .await
        .expect("entity get should return rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        attr_value(&rows[0], &schema.name_attr),
        Some(&AttributeValue::String("AllTypes".into()))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.age_attr),
        Some(&AttributeValue::Long(33))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.score_attr),
        Some(&AttributeValue::Double(91.25))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.active_attr),
        Some(&AttributeValue::Boolean(true))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.birthday_attr),
        Some(&AttributeValue::Date("1990-01-02".into()))
    );
    assert!(matches!(
        attr_value(&rows[0], &schema.login_at_attr),
        Some(AttributeValue::DateTime(value))
            if value.starts_with("2026-05-27T10:30:00")
    ));
    assert!(matches!(
        attr_value(&rows[0], &schema.seen_at_attr),
        Some(AttributeValue::DateTimeTZ(value))
            if value.starts_with("2026-05-27T10:30:00")
    ));
    assert!(matches!(
        attr_value(&rows[0], &schema.balance_attr),
        Some(AttributeValue::Decimal(value)) if value.starts_with("1234.56")
    ));
    // ISO-8601 input PT2H30M is stored and read back canonically.
    assert_eq!(
        attr_value(&rows[0], &schema.session_length_attr),
        Some(&AttributeValue::Duration("PT9000S".into()))
    );

    let updated_attrs = vec![
        (
            schema.name_attr.clone(),
            AttributeValue::String("AllTypes".into()),
        ),
        (schema.age_attr.clone(), AttributeValue::Long(34)),
        (schema.score_attr.clone(), AttributeValue::Double(99.5)),
        (schema.active_attr.clone(), AttributeValue::Boolean(false)),
        (
            schema.birthday_attr.clone(),
            AttributeValue::Date("1991-03-04".into()),
        ),
        (
            schema.login_at_attr.clone(),
            AttributeValue::DateTime("2026-05-28T11:45:00".into()),
        ),
        (
            schema.seen_at_attr.clone(),
            AttributeValue::DateTimeTZ("2026-05-28T11:45:00+00:00".into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal("4321.00".into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration("PT45M".into()),
        ),
    ];
    manager
        .update(Some(&iid), &updated_attrs)
        .await
        .expect("entity update with all primitive attributes should succeed");

    let updated = manager
        .get_by_iid(&iid)
        .await
        .expect("entity get_by_iid should succeed")
        .expect("entity should exist after update");
    assert_eq!(
        attr_value(&updated, &schema.active_attr),
        Some(&AttributeValue::Boolean(false))
    );
    assert!(matches!(
        attr_value(&updated, &schema.balance_attr),
        Some(AttributeValue::Decimal(value)) if value.starts_with("4321")
    ));
    assert_eq!(
        attr_value(&updated, &schema.session_length_attr),
        Some(&AttributeValue::Duration("PT2700S".into()))
    );

    manager
        .delete_by_iid(&iid)
        .await
        .expect("entity delete should work");
}
