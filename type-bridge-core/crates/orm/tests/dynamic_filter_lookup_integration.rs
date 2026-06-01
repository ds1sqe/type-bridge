//! Dynamic filter and lookup integration tests against TypeDB.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_filter_lookup_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_filters_and_lookup_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("filters").await else {
        return;
    };
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());

    manager
        .insert_many(&[
            all_value_attrs("Alice", 30),
            all_value_attrs("Bob", 40),
            all_value_attrs("Carol", 50),
        ])
        .await
        .expect("entity insert_many should return IIDs");

    let exact = manager
        .get(&[Filter::long_eq("age", 40)])
        .await
        .expect("equality filter should return rows");
    assert_eq!(exact.len(), 1);
    assert_eq!(
        attr_value(&exact[0], &schema.name_attr),
        Some(&AttributeValue::String("Bob".into()))
    );

    let comparison = manager
        .get(&[Filter::compare("age", ">=", AttributeValue::Long(40))])
        .await
        .expect("comparison filter should return rows");
    assert_eq!(comparison.len(), 2);

    let count = manager
        .count_with_filters(&[Filter::compare("score", ">", AttributeValue::Double(90.0))])
        .await
        .expect("count_with_filters should apply comparison filters");
    assert_eq!(count, 3);

    let missing = manager
        .get(&[Filter::string_eq("name", "Nobody")])
        .await
        .expect("missing lookup should return an empty row set");
    assert!(missing.is_empty());
}

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_relation_filters_and_role_lookup_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("rel-filters").await else {
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

    let alice_roles = role_players(&schema, alice_iid, acme_iid.clone());
    let bob_roles = role_players(&schema, bob_iid, acme_iid);
    relation_manager
        .insert(&relation_attrs("2026-05-27"), &alice_roles)
        .await
        .expect("relation insert should return IID");
    relation_manager
        .insert(&relation_attrs("2026-05-28"), &bob_roles)
        .await
        .expect("relation insert should return IID");

    let by_attr = relation_manager
        .get(&[Filter::eq(
            "since",
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
