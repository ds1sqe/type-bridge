//! Dynamic relation CRUD integration tests against a real TypeDB instance.
//!

use crate::common::dynamic_crud::*;
use crate::internal::*;
use type_bridge_orm::*;

#[tokio::test]
async fn dynamic_relation_crud_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("relation").await;
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
    let relation_iid = relation_manager
        .insert(&relation_attrs("2026-05-27"), &alice_roles)
        .await
        .expect("relation insert should return IID");
    assert!(!relation_iid.is_empty());

    let rows = relation_manager
        .get_with_role_filters(
            &[Filter::eq(
                "since",
                AttributeValue::Date("2026-05-27".into()),
            )],
            &alice_roles,
        )
        .await
        .expect("relation role-player get should return rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        relation_attr_value(&rows[0], &schema.since_attr),
        Some(&AttributeValue::Date("2026-05-27".into()))
    );
    assert!(
        rows[0]
            .role_players
            .iter()
            .any(|player| player.role_name == "employee")
    );

    relation_manager
        .update(
            Some(&relation_iid),
            &relation_attrs("2026-05-28"),
            &alice_roles,
        )
        .await
        .expect("relation update should succeed");

    let by_iid = relation_manager
        .get_by_iid(&relation_iid)
        .await
        .expect("relation get_by_iid should return rows");
    assert_eq!(
        relation_attr_value(&by_iid[0], &schema.since_attr),
        Some(&AttributeValue::Date("2026-05-28".into()))
    );

    let put_iids = relation_manager
        .put_many(&[
            (relation_attrs("2026-05-29"), alice_roles),
            (relation_attrs("2026-05-30"), bob_roles),
        ])
        .await
        .expect("relation put_many should return IIDs");
    assert_eq!(put_iids.len(), 2);

    assert_eq!(
        relation_manager
            .count()
            .await
            .expect("relation count should work"),
        3
    );

    let aggregate_rows = relation_manager
        .aggregate(&[], &[count_aggregate()])
        .await
        .expect("relation aggregate should work");
    assert_eq!(aggregate_i64(&aggregate_rows[0], "count"), Some(3));

    let grouped_rows = relation_manager
        .group_by_aggregate(&[], &[String::from("since")], &[count_aggregate()])
        .await
        .expect("relation group_by_aggregate should work");
    assert_eq!(grouped_rows.len(), 3);

    relation_manager
        .delete_by_iid(&relation_iid)
        .await
        .expect("relation delete should work");
}
