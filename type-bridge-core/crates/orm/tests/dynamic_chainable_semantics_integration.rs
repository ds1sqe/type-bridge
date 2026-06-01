//! Dynamic filter-driven mutation integration tests against TypeDB.
//!
//! The Python API exposes chainable `filter(...).delete()` and
//! `filter(...).update_with(...)`. The shared dynamic runtime currently exposes
//! the equivalent primitives as filtered reads followed by IID-scoped mutation.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_chainable_semantics_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_filter_then_iid_update_and_delete_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("chainable-entity").await else {
        return;
    };
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());

    manager
        .insert_many(&[
            person_attrs("Alice", 30),
            person_attrs("Bob", 40),
            person_attrs("Carol", 50),
        ])
        .await
        .expect("entity insert_many should return IIDs");

    let selected = manager
        .get(&[Filter::compare("age", ">=", AttributeValue::Long(40))])
        .await
        .expect("comparison filter should return rows");
    assert_eq!(selected.len(), 2);

    for row in selected {
        let iid = row.iid.as_deref().expect("filtered rows include IIDs");
        let name = attr_value(&row, &schema.name_attr).expect("row has name");
        let age = attr_value(&row, &schema.age_attr).expect("row has age");
        let (AttributeValue::String(name), AttributeValue::Long(age)) = (name, age) else {
            panic!("row has expected primitive values");
        };
        manager
            .update(Some(iid), &person_attrs(name, age + 1))
            .await
            .expect("IID-scoped update should work after filtered selection");
    }

    let updated = manager
        .get(&[Filter::compare("age", ">", AttributeValue::Long(40))])
        .await
        .expect("updated rows should be queryable");
    let updated_names: Vec<_> = updated
        .iter()
        .filter_map(|row| attr_value(row, &schema.name_attr))
        .collect();
    assert!(updated_names.contains(&&AttributeValue::String("Bob".into())));
    assert!(updated_names.contains(&&AttributeValue::String("Carol".into())));

    let to_delete = manager
        .get(&[Filter::compare("age", ">", AttributeValue::Long(50))])
        .await
        .expect("delete selection should return rows");
    assert_eq!(to_delete.len(), 1);
    for row in to_delete {
        manager
            .delete_by_iid(row.iid.as_deref().expect("delete row includes IID"))
            .await
            .expect("IID-scoped delete should work after filtered selection");
    }

    let remaining = manager.all().await.expect("remaining rows should fetch");
    let remaining_names: Vec<_> = remaining
        .iter()
        .filter_map(|row| attr_value(row, &schema.name_attr))
        .collect();
    assert_eq!(remaining_names.len(), 2);
    assert!(!remaining_names.contains(&&AttributeValue::String("Carol".into())));
}

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_relation_filter_then_iid_update_and_delete_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("chainable-relation").await else {
        return;
    };
    let person_manager = DynamicEntityManager::new(&db, schema.person_descriptor());
    let company_manager = DynamicEntityManager::new(&db, schema.company_descriptor());
    let relation_manager = DynamicRelationManager::new(&db, schema.employment_descriptor());

    let alice_iid = person_manager
        .insert(&person_attrs("Alice", 30))
        .await
        .expect("person insert should return IID");
    let bob_iid = person_manager
        .insert(&person_attrs("Bob", 40))
        .await
        .expect("person insert should return IID");
    let acme_iid = company_manager
        .insert(&company_attrs("Acme"))
        .await
        .expect("company insert should return IID");

    relation_manager
        .insert(
            &relation_attrs("2026-05-27"),
            &role_players(&schema, alice_iid, acme_iid.clone()),
        )
        .await
        .expect("relation insert should return IID");
    relation_manager
        .insert(
            &relation_attrs("2026-05-28"),
            &role_players(&schema, bob_iid, acme_iid),
        )
        .await
        .expect("relation insert should return IID");

    let selected = relation_manager
        .get(&[Filter::eq(
            "since",
            AttributeValue::Date("2026-05-28".into()),
        )])
        .await
        .expect("relation filter should return rows");
    assert_eq!(selected.len(), 1);
    let selected_iid = selected[0]
        .iid
        .as_deref()
        .expect("relation row includes IID");
    relation_manager
        .update(Some(selected_iid), &relation_attrs("2026-06-01"), &[])
        .await
        .expect("IID-scoped relation update should not require role players");

    let updated = relation_manager
        .get(&[Filter::eq(
            "since",
            AttributeValue::Date("2026-06-01".into()),
        )])
        .await
        .expect("updated relation should be queryable");
    assert_eq!(updated.len(), 1);
    assert_eq!(
        relation_attr_value(&updated[0], &schema.since_attr),
        Some(&AttributeValue::Date("2026-06-01".into()))
    );

    let to_delete = relation_manager
        .get(&[Filter::eq(
            "since",
            AttributeValue::Date("2026-05-27".into()),
        )])
        .await
        .expect("relation delete selection should return rows");
    assert_eq!(to_delete.len(), 1);
    relation_manager
        .delete_by_iid(
            to_delete[0]
                .iid
                .as_deref()
                .expect("delete row includes IID"),
        )
        .await
        .expect("IID-scoped relation delete should work after filtered selection");

    let remaining = relation_manager
        .all()
        .await
        .expect("remaining relations should fetch");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        relation_attr_value(&remaining[0], &schema.since_attr),
        Some(&AttributeValue::Date("2026-06-01".into()))
    );
}
