//! Dynamic entity CRUD integration tests against a real TypeDB instance.
//!
//! Run with:
//! `cargo test -p type-bridge-orm --test dynamic_entity_crud_integration -- --ignored`

mod dynamic_crud_support;

use dynamic_crud_support::*;
use type_bridge_orm::*;

#[tokio::test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_RUST_INTG_DATABASE"]
async fn dynamic_entity_crud_against_typedb() {
    let Some((db, schema)) = setup_dynamic_database("entity").await else {
        return;
    };
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());

    let alice_iid = manager
        .insert(&person_attrs("Alice", 30))
        .await
        .expect("entity insert should return IID");
    assert!(!alice_iid.is_empty());

    let rows = manager
        .get(&[Filter::string_eq("name", "Alice")])
        .await
        .expect("entity get should return rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        attr_value(&rows[0], &schema.name_attr),
        Some(&AttributeValue::String("Alice".into()))
    );

    manager
        .update(Some(&alice_iid), &person_attrs("Alice", 31))
        .await
        .expect("entity update should succeed");

    let by_iid = manager
        .get_by_iid(&alice_iid)
        .await
        .expect("entity get_by_iid should succeed")
        .expect("entity should still exist");
    assert_eq!(
        attr_value(&by_iid, &schema.age_attr),
        Some(&AttributeValue::Long(31))
    );

    let put_iids = manager
        .put_many(&[person_attrs("Bob", 40), person_attrs("Carol", 50)])
        .await
        .expect("entity put_many should return IIDs");
    assert_eq!(put_iids.len(), 2);

    assert_eq!(manager.count().await.expect("entity count should work"), 3);

    let aggregate_rows = manager
        .aggregate(&[], &[count_aggregate(), mean_age_aggregate()])
        .await
        .expect("entity aggregate should work");
    assert_eq!(aggregate_rows[0]["$count"]["value"], 3);

    let grouped_rows = manager
        .group_by_aggregate(&[], &[String::from("name")], &[count_aggregate()])
        .await
        .expect("entity group_by_aggregate should work");
    assert_eq!(grouped_rows.len(), 3);

    manager
        .delete_by_iid(&alice_iid)
        .await
        .expect("entity delete should work");
    assert!(
        manager
            .get_by_iid(&alice_iid)
            .await
            .expect("deleted entity lookup should succeed")
            .is_none()
    );
}
