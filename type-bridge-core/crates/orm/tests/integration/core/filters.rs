//! Dynamic filter and lookup integration tests against TypeDB.
//!

use crate::common::dynamic_crud::*;
use crate::internal::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_entity_filters_and_lookup_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("filters").await;
    let manager =
        DynamicEntityManager::from_registry(&db, schema.registry(), schema.person_descriptor());

    manager
        .insert_many(&[
            schema.all_value_attrs("Alice", 30),
            schema.all_value_attrs("Bob", 40),
            schema.all_value_attrs("Carol", 50),
        ])
        .await
        .expect("entity insert_many should return IIDs");

    let exact = manager
        .get(&[Filter::long_eq(&schema.age_attr, 40)])
        .await
        .expect("equality filter should return rows");
    assert_eq!(exact.len(), 1);
    assert_eq!(
        attr_value(&exact[0], &schema.name_attr),
        Some(&AttributeValue::String("Bob".into()))
    );

    let comparison = manager
        .get(&[Filter::compare(
            &schema.age_attr,
            ">=",
            AttributeValue::Long(40),
        )])
        .await
        .expect("comparison filter should return rows");
    assert_eq!(comparison.len(), 2);

    let count = manager
        .count_with_filters(&[Filter::compare(
            &schema.score_attr,
            ">",
            AttributeValue::Double(90.0),
        )])
        .await
        .expect("count_with_filters should apply comparison filters");
    assert_eq!(count, 3);

    let missing = manager
        .get(&[Filter::string_eq(&schema.name_attr, "Nobody")])
        .await
        .expect("missing lookup should return an empty row set");
    assert!(missing.is_empty());
}

#[tokio::test]
async fn dynamic_relation_filters_and_role_lookup_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("rel-filters").await;
    let person_manager =
        DynamicEntityManager::from_registry(&db, schema.registry(), schema.person_descriptor());
    let company_manager =
        DynamicEntityManager::from_registry(&db, schema.registry(), schema.company_descriptor());
    let relation_manager = DynamicRelationManager::from_registry(
        &db,
        schema.registry(),
        schema.employment_descriptor(),
    );

    let alice_iid = person_manager
        .insert(&schema.person_attrs("Alice", 30))
        .await
        .expect("person insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let bob_iid = person_manager
        .insert(&schema.person_attrs("Bob", 40))
        .await
        .expect("person insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let acme_iid = company_manager
        .insert(&schema.company_attrs("Acme"))
        .await
        .expect("company insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();

    let alice_roles = role_players(&schema, alice_iid, acme_iid.clone());
    let bob_roles = role_players(&schema, bob_iid, acme_iid);
    relation_manager
        .insert(&schema.relation_attrs("2026-05-27"), &alice_roles)
        .await
        .expect("relation insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    relation_manager
        .insert(&schema.relation_attrs("2026-05-28"), &bob_roles)
        .await
        .expect("relation insert should return IID")
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();

    let by_attr = relation_manager
        .get(&[Filter::eq(
            &schema.since_attr,
            AttributeValue::Date("2026-05-27".into()),
        )])
        .await
        .expect("relation attribute filter should return rows");
    assert_eq!(by_attr.len(), 1);

    let by_role = relation_manager
        .get_with_role_filters(&[], &bob_roles)
        .await
        .expect("relation role-player filter should return rows");
    assert_eq!(by_role.len(), 1);
    assert_eq!(
        relation_attr_value(&by_role[0], &schema.since_attr),
        Some(&AttributeValue::Date("2026-05-28".into()))
    );
}
