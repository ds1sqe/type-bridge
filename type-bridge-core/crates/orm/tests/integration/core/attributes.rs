//! Dynamic attribute value CRUD integration tests against a real TypeDB instance.
//!

use crate::common::dynamic_crud::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_entity_all_primitive_attribute_values_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("attrs").await;
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());

    let iid = manager
        .insert(&all_value_attrs("AllTypes", 33))
        .await
        .expect("entity insert with all primitive attributes should return IID");
    assert!(!iid.is_empty());

    let rows = manager
        .get(&[Filter::string_eq("name", "AllTypes")])
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
    assert_eq!(
        attr_value(&rows[0], &schema.session_length_attr),
        Some(&AttributeValue::Duration("PT2H30M".into()))
    );

    let updated_attrs = vec![
        ("name".into(), AttributeValue::String("AllTypes".into())),
        ("age".into(), AttributeValue::Long(34)),
        ("score".into(), AttributeValue::Double(99.5)),
        ("active".into(), AttributeValue::Boolean(false)),
        ("birthday".into(), AttributeValue::Date("1991-03-04".into())),
        (
            "login_at".into(),
            AttributeValue::DateTime("2026-05-28T11:45:00".into()),
        ),
        (
            "seen_at".into(),
            AttributeValue::DateTimeTZ("2026-05-28T11:45:00+00:00".into()),
        ),
        ("balance".into(), AttributeValue::Decimal("4321.00".into())),
        (
            "session_length".into(),
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
        Some(&AttributeValue::Duration("PT45M".into()))
    );

    manager
        .delete_by_iid(&iid)
        .await
        .expect("entity delete should work");
}
