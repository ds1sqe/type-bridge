//! Dynamic multi-value attribute CRUD integration tests against TypeDB.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_multivalue_crud_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_multi_value_attributes_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("multi").await else {
        return;
    };
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());

    let iid = manager
        .insert(&multi_value_attrs("MultiTypes"))
        .await
        .expect("multi-value insert should return IID");
    assert!(!iid.is_empty());

    let rows = manager
        .get(&[Filter::string_eq("name", "MultiTypes")])
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
        ("name".into(), AttributeValue::String("MultiTypes".into())),
        ("age".into(), AttributeValue::Long(100)),
        ("age".into(), AttributeValue::Long(200)),
        ("balance".into(), AttributeValue::Decimal("10.00".into())),
        ("balance".into(), AttributeValue::Decimal("20.00".into())),
        (
            "session_length".into(),
            AttributeValue::Duration("PT10M".into()),
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

fn multi_value_attrs(name: &str) -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String(name.into())),
        ("age".into(), AttributeValue::Long(85)),
        ("age".into(), AttributeValue::Long(90)),
        ("age".into(), AttributeValue::Long(78)),
        ("score".into(), AttributeValue::Double(1.5)),
        ("score".into(), AttributeValue::Double(2.7)),
        ("score".into(), AttributeValue::Double(3.9)),
        ("active".into(), AttributeValue::Boolean(true)),
        ("active".into(), AttributeValue::Boolean(false)),
        ("birthday".into(), AttributeValue::Date("2024-01-15".into())),
        ("birthday".into(), AttributeValue::Date("2024-03-01".into())),
        ("birthday".into(), AttributeValue::Date("2024-06-01".into())),
        (
            "login_at".into(),
            AttributeValue::DateTime("2024-01-01T10:00:00".into()),
        ),
        (
            "login_at".into(),
            AttributeValue::DateTime("2024-01-01T11:00:00".into()),
        ),
        (
            "login_at".into(),
            AttributeValue::DateTime("2024-01-01T12:00:00".into()),
        ),
        (
            "seen_at".into(),
            AttributeValue::DateTimeTZ("2024-01-01T10:00:00+00:00".into()),
        ),
        (
            "seen_at".into(),
            AttributeValue::DateTimeTZ("2024-01-01T14:00:00+00:00".into()),
        ),
        ("balance".into(), AttributeValue::Decimal("999.99".into())),
        ("balance".into(), AttributeValue::Decimal("899.99".into())),
        ("balance".into(), AttributeValue::Decimal("849.99".into())),
        (
            "session_length".into(),
            AttributeValue::Duration("PT30M".into()),
        ),
        (
            "session_length".into(),
            AttributeValue::Duration("PT1H".into()),
        ),
        (
            "session_length".into(),
            AttributeValue::Duration("PT2H".into()),
        ),
    ]
}
