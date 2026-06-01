//! Dynamic attribute value CRUD integration tests against a real TypeDB instance.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_attribute_crud_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_all_primitive_attribute_values_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("attrs").await else {
        return;
    };
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
    assert_eq!(
        attr_value(&rows[0], &schema.login_at_attr),
        Some(&AttributeValue::DateTime("2026-05-27T10:30:00".into()))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.seen_at_attr),
        Some(&AttributeValue::DateTimeTZ(
            "2026-05-27T10:30:00+00:00".into()
        ))
    );
    assert_eq!(
        attr_value(&rows[0], &schema.balance_attr),
        Some(&AttributeValue::Decimal("1234.56".into()))
    );
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
    assert_eq!(
        attr_value(&updated, &schema.balance_attr),
        Some(&AttributeValue::Decimal("4321.00".into()))
    );
    assert_eq!(
        attr_value(&updated, &schema.session_length_attr),
        Some(&AttributeValue::Duration("PT45M".into()))
    );

    manager
        .delete_by_iid(&iid)
        .await
        .expect("entity delete should work");
}
