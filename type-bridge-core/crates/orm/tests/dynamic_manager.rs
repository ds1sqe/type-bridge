//! Dynamic manager smoke tests using the existing mock backend abstraction.

mod common;

use std::sync::Arc;

use common::*;
use type_bridge_orm::manager::query_builder;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::*;

fn person_descriptor() -> EntityDescriptor {
    EntityDescriptor {
        type_name: "person".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            OwnedAttributeDescriptor {
                field_name: "name".into(),
                attr_name: "name".into(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Key],
                is_optional: false,
            },
            OwnedAttributeDescriptor {
                field_name: "age".into(),
                attr_name: "age".into(),
                value_type: ValueType::Long,
                annotations: vec![],
                is_optional: false,
            },
        ],
    }
}

fn employment_descriptor() -> RelationDescriptor {
    RelationDescriptor {
        type_name: "employment".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![OwnedAttributeDescriptor {
            field_name: "position".into(),
            attr_name: "position".into(),
            value_type: ValueType::String,
            annotations: vec![],
            is_optional: true,
        }],
        roles: vec![
            RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec!["person".into()],
                cardinality: None,
            },
            RoleDescriptor {
                role_name: "employer".into(),
                player_type_names: vec!["company".into()],
                cardinality: None,
            },
        ],
    }
}

fn person_attrs() -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("age".into(), AttributeValue::Long(30)),
    ]
}

#[test]
fn dynamic_entity_insert_query_matches_typed_equivalent() {
    let typed = make_person("Alice", 30);
    let dynamic = query_builder::build_dynamic_entity_insert_with_iid(
        &person_descriptor(),
        &person_attrs(),
        "$e",
    )
    .unwrap();
    let typed = query_builder::build_insert_with_iid::<Person>(&typed, "$e").unwrap();

    assert_eq!(dynamic, typed);
}

#[test]
fn dynamic_relation_insert_query_matches_typed_shape() {
    let typed_relation = make_employment(
        None,
        None,
        Some(("name", AttributeValue::String("Alice".into()))),
        Some("0xcomp1"),
        None,
        Some("Engineer"),
    );
    let dynamic = query_builder::build_dynamic_relation_insert_with_iid(
        &employment_descriptor(),
        &vec![("position".into(), AttributeValue::String("Engineer".into()))],
        &[
            DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: "person".into(),
                iid: None,
                key: Some(("name".into(), AttributeValue::String("Alice".into()))),
            },
            DynamicRolePlayerInput {
                role_name: "employer".into(),
                player_type_name: "company".into(),
                iid: Some("0xcomp1".into()),
                key: None,
            },
        ],
        "$r",
    )
    .unwrap();
    let typed =
        query_builder::build_relation_insert_with_iid::<Employment>(&typed_relation, "$r").unwrap();

    assert_eq!(dynamic, typed);
}

#[tokio::test]
async fn dynamic_entity_manager_insert_fetch_count_delete() {
    let descriptor = Arc::new(person_descriptor());
    let fetch_doc = serde_json::json!({
        "_iid": "0xaaa",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        }
    });
    let backend = MockBackend::new(vec![
        QueryResult::Ok,
        QueryResult::Rows(vec![serde_json::json!({"$count": 1})]),
        QueryResult::Documents(vec![fetch_doc]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xinsert"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    assert_eq!(manager.insert(&person_attrs()).await.unwrap(), "0xinsert");
    let rows = manager
        .get(&[Filter::string_eq("name", "Alice")])
        .await
        .unwrap();
    assert_eq!(rows[0].iid.as_deref(), Some("0xaaa"));
    assert_eq!(rows[0].type_name.as_deref(), Some("person"));
    assert_eq!(rows[0].attributes, person_attrs());
    assert_eq!(manager.count().await.unwrap(), 1);
    manager.delete_by_iid("0xaaa").await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].contains("insert"));
    assert!(recorded[1].contains("has name"));
    assert!(recorded[2].contains("reduce"));
    assert!(recorded[3].contains("delete"));
}

#[tokio::test]
async fn dynamic_relation_manager_insert_fetch_count_delete() {
    let descriptor = Arc::new(employment_descriptor());
    let relation_attrs = vec![("position".into(), AttributeValue::String("Engineer".into()))];
    let role_players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0xperson".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0xcompany".into()),
            key: None,
        },
    ];
    let fetch_doc = serde_json::json!({
        "_iid": "0xrel",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        },
        "role_players": [
            {"role_name": "employee", "player_iid": "0xperson", "player_type_name": "person"}
        ]
    });
    let backend = MockBackend::new(vec![
        QueryResult::Ok,
        QueryResult::Rows(vec![serde_json::json!({"$count": 1})]),
        QueryResult::Documents(vec![fetch_doc]),
        QueryResult::Documents(vec![serde_json::json!({"iid": {"value": "0xinsertrel"}})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager
            .insert(&relation_attrs, &role_players)
            .await
            .unwrap(),
        "0xinsertrel"
    );
    let rows = manager.all().await.unwrap();
    assert_eq!(rows[0].iid.as_deref(), Some("0xrel"));
    assert_eq!(rows[0].attributes, relation_attrs);
    assert_eq!(rows[0].role_players[0].role_name, "employee");
    assert_eq!(manager.count().await.unwrap(), 1);
    manager.delete_by_iid("0xrel").await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].contains("links (employee: $rp0, employer: $rp1)"));
    assert!(recorded[1].contains("sub employment"));
    assert!(recorded[2].contains("reduce"));
    assert!(recorded[3].contains("delete"));
}

#[tokio::test]
async fn dynamic_get_one_not_found_returns_not_found() {
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager
            .get_one(&[Filter::string_eq("name", "Nobody")])
            .await,
        Err(OrmError::NotFound(_))
    ));
}
