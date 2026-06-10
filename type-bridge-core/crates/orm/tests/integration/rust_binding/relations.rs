use crate::common::rust_binding::*;
use type_bridge_orm::*;

#[tokio::test]
async fn full_relation_lifecycle() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_full_schema(&db).await;

    let person_mgr = EntityManager::<Person>::new(&db);
    let company_mgr = EntityManager::<Company>::new(&db);
    let alice_name = unique_label("Alice-Rel");
    let company_name = unique_label("Acme-Rel");

    let mut alice = Person {
        iid: None,
        name: Name(alice_name),
        age: Age(30),
    };
    person_mgr
        .insert(&mut alice)
        .await
        .expect("insert person failed");

    let mut acme = Company {
        iid: None,
        name: Name(company_name),
    };
    company_mgr
        .insert(&mut acme)
        .await
        .expect("insert company failed");

    let rel_mgr = RelationManager::<Employment>::new(&db);
    let mut employment = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: alice.iid().map(String::from),
            key: None,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: acme.iid().map(String::from),
            key: None,
        },
        position: Some(Position("Engineer".into())),
    };
    let rel_iid = rel_mgr
        .insert(&mut employment)
        .await
        .expect("insert relation failed");
    assert!(!rel_iid.is_empty());

    let relations = rel_mgr.all().await.expect("all() relations failed");
    assert!(!relations.is_empty());

    rel_mgr
        .delete(&employment)
        .await
        .expect("delete relation failed");
    person_mgr
        .delete(&alice)
        .await
        .expect("delete person failed");
    company_mgr
        .delete(&acme)
        .await
        .expect("delete company failed");
}

#[tokio::test]
async fn relation_batch_filters_counts_and_role_player_query() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_full_schema(&db).await;

    let person_mgr = EntityManager::<Person>::new(&db);
    let company_mgr = EntityManager::<Company>::new(&db);
    let rel_mgr = RelationManager::<Employment>::new(&db);
    let prefix = unique_label("RelBreadth");

    let mut alice = Person {
        iid: None,
        name: Name(format!("{prefix}-Alice")),
        age: Age(30),
    };
    let mut bob = Person {
        iid: None,
        name: Name(format!("{prefix}-Bob")),
        age: Age(35),
    };
    person_mgr
        .insert_many(std::slice::from_mut(&mut alice))
        .await
        .expect("insert alice failed");
    person_mgr
        .insert_many(std::slice::from_mut(&mut bob))
        .await
        .expect("insert bob failed");

    let mut acme = Company {
        iid: None,
        name: Name(format!("{prefix}-Acme")),
    };
    company_mgr
        .insert(&mut acme)
        .await
        .expect("insert company failed");

    let employer = RolePlayerRef {
        role: "employer",
        entity_type_name: "company",
        iid: acme.iid().map(String::from),
        key: None,
    };
    let mut employments = vec![
        Employment {
            iid: None,
            employee: RolePlayerRef {
                role: "employee",
                entity_type_name: "person",
                iid: alice.iid().map(String::from),
                key: None,
            },
            employer: employer.clone(),
            position: Some(Position(format!("{prefix}-Engineer"))),
        },
        Employment {
            iid: None,
            employee: RolePlayerRef {
                role: "employee",
                entity_type_name: "person",
                iid: bob.iid().map(String::from),
                key: None,
            },
            employer,
            position: Some(Position(format!("{prefix}-Manager"))),
        },
    ];

    let iids = rel_mgr
        .insert_many(&mut employments)
        .await
        .expect("relation insert_many failed");
    assert_eq!(iids.len(), 2);

    let filtered = rel_mgr
        .get(&[Filter::string_eq("position", format!("{prefix}-Engineer"))])
        .await
        .expect("relation get by attribute failed");
    assert_eq!(filtered.len(), 1);

    let count = rel_mgr
        .count_with_filters(&[Filter::string_eq("position", format!("{prefix}-Manager"))])
        .await
        .expect("relation count_with_filters failed");
    assert_eq!(count, 1);

    let by_role_player = rel_mgr
        .query()
        .filter(Expr::role_player(
            "employee",
            Expr::eq("name", AttributeValue::String(format!("{prefix}-Alice"))),
        ))
        .execute()
        .await
        .expect("relation role-player query failed");
    assert_eq!(by_role_player.len(), 1);
    let expected_position = format!("{prefix}-Engineer");
    assert_eq!(
        by_role_player[0]
            .position
            .as_ref()
            .map(|position| position.0.as_str()),
        Some(expected_position.as_str())
    );

    rel_mgr
        .delete_many(&employments)
        .await
        .expect("relation delete_many failed");
    person_mgr
        .delete_many(&[alice, bob])
        .await
        .expect("person delete_many failed");
    company_mgr
        .delete(&acme)
        .await
        .expect("company delete failed");
}
