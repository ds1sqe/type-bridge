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
                is_ordered: false,
            },
            OwnedAttributeDescriptor {
                field_name: "age".into(),
                attr_name: "age".into(),
                value_type: ValueType::Long,
                annotations: vec![],
                is_optional: false,
                is_ordered: false,
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
            is_ordered: false,
        }],
        roles: vec![
            RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec!["person".into()],
                ..Default::default()
            },
            RoleDescriptor {
                role_name: "employer".into(),
                player_type_names: vec!["company".into()],
                ..Default::default()
            },
        ],
    }
}

fn tagged_employment_descriptor() -> RelationDescriptor {
    let mut descriptor = employment_descriptor();
    descriptor.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "labels".into(),
        attr_name: "label".into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Card(0, Some(4))],
        is_optional: true,
        is_ordered: false,
    });
    descriptor
}

fn person_attrs() -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("age".into(), AttributeValue::Long(30)),
    ]
}

fn employment_attrs(position: &str) -> DynamicAttributeMap {
    vec![("position".into(), AttributeValue::String(position.into()))]
}

fn count_aggregate() -> DynamicAggregate {
    DynamicAggregate {
        result_key: "count".into(),
        function: "count".into(),
        attr_name: None,
    }
}

fn mean_age_aggregate() -> DynamicAggregate {
    DynamicAggregate {
        result_key: "avg_age".into(),
        function: "mean".into(),
        attr_name: Some("age".into()),
    }
}

fn employment_role_players() -> Vec<DynamicRolePlayerInput> {
    vec![
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
    ]
}

fn tagged_person_descriptor() -> EntityDescriptor {
    let mut descriptor = person_descriptor();
    descriptor.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "tags".into(),
        attr_name: "tag".into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Card(0, Some(4))],
        is_optional: true,
        is_ordered: false,
    });
    descriptor
}

fn scored_person_descriptor() -> EntityDescriptor {
    let mut descriptor = person_descriptor();
    descriptor.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "scores".into(),
        attr_name: "score".into(),
        value_type: ValueType::Long,
        annotations: vec![Annotation::Card(1, Some(4))],
        is_optional: false,
        is_ordered: false,
    });
    descriptor
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
fn dynamic_entity_put_query_uses_put_clause() {
    let dynamic =
        query_builder::build_dynamic_entity_put(&person_descriptor(), &person_attrs(), "$e")
            .unwrap();

    assert!(dynamic.starts_with("put"));
    assert!(!dynamic.starts_with("insert"));
    assert!(dynamic.contains("isa person"));
    assert!(dynamic.contains("has name"));
    assert!(dynamic.contains("fetch"));
}

#[test]
fn dynamic_entity_update_query_matches_by_iid_and_skips_key() {
    let dynamic = query_builder::build_dynamic_entity_update(
        &person_descriptor(),
        Some("0xaaa"),
        &person_attrs(),
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("match"));
    assert!(dynamic.contains("iid 0xaaa"));
    assert!(dynamic.contains("delete"));
    assert!(dynamic.contains("insert"));
    assert!(dynamic.contains("$e has age 30"));
    assert!(!dynamic.contains("$e has name \"Alice\""));
}

#[test]
fn dynamic_entity_update_query_can_match_by_key() {
    let dynamic = query_builder::build_dynamic_entity_update(
        &person_descriptor(),
        None,
        &person_attrs(),
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("has name \"Alice\""));
    assert!(dynamic.contains("$e has age 30"));
}

#[test]
fn dynamic_entity_update_replaces_multi_value_attributes() {
    let attrs = vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("tag".into(), AttributeValue::String("alpha".into())),
        ("tag".into(), AttributeValue::String("beta".into())),
    ];
    let dynamic =
        query_builder::build_dynamic_entity_update(&tagged_person_descriptor(), None, &attrs, "$e")
            .unwrap();

    assert!(dynamic.contains("try { $e has tag $old_attr_0; }"));
    assert!(dynamic.contains("delete"));
    assert!(dynamic.contains("try { $old_attr_0 of $e; }"));
    assert!(dynamic.contains("$e has tag \"alpha\""));
    assert!(dynamic.contains("$e has tag \"beta\""));
}

#[test]
fn dynamic_entity_aggregate_query_binds_attribute_aggregate() {
    let dynamic = query_builder::build_dynamic_entity_aggregate(
        &person_descriptor(),
        &[Filter::string_eq("name", "Alice")],
        &[count_aggregate(), mean_age_aggregate()],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person, has name \"Alice\""));
    assert!(dynamic.contains("$e has age $agg1"));
    assert!(dynamic.contains("$count = count($e)"));
    assert!(dynamic.contains("$avg_age = mean($agg1)"));
}

#[test]
fn dynamic_entity_aggregate_query_binds_multi_value_attribute_aggregate() {
    let dynamic = query_builder::build_dynamic_entity_aggregate(
        &scored_person_descriptor(),
        &[],
        &[
            DynamicAggregate {
                result_key: "sum_scores".into(),
                function: "sum".into(),
                attr_name: Some("score".into()),
            },
            DynamicAggregate {
                result_key: "avg_scores".into(),
                function: "mean".into(),
                attr_name: Some("score".into()),
            },
        ],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person"));
    assert!(dynamic.contains("$e has score $agg0"));
    assert!(dynamic.contains("$sum_scores = sum($agg0)"));
    assert!(dynamic.contains("$avg_scores = mean($agg0)"));
    assert!(!dynamic.contains("$e has score $agg1"));
}

#[test]
fn dynamic_entity_fetch_query_binds_comparison_filter() {
    let dynamic = query_builder::build_dynamic_entity_fetch(
        &person_descriptor(),
        &[Filter::compare("age", ">", AttributeValue::Long(60))],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa! $t"));
    assert!(dynamic.contains("$e has age $filter0"));
    assert!(dynamic.contains("$filter0 > 60"));
}

#[test]
fn dynamic_entity_expr_fetch_supports_boolean_sort_and_limit() {
    let dynamic = query_builder::build_dynamic_entity_expr_fetch(
        &person_descriptor(),
        &[DynamicExpr::Or {
            exprs: vec![
                DynamicExpr::Compare {
                    attr_name: "name".into(),
                    operator: DynamicComparisonOp::Contains,
                    value: AttributeValue::String("Al".into()),
                },
                DynamicExpr::Compare {
                    attr_name: "age".into(),
                    operator: DynamicComparisonOp::Gt,
                    value: AttributeValue::Long(40),
                },
            ],
        }],
        &[DynamicSort::Attribute {
            attr_name: "age".into(),
            direction: SortDir::Desc,
        }],
        Some(10),
        Some(5),
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa! $t"));
    assert!(dynamic.contains("$dyn_attr0 contains \"Al\""));
    assert!(dynamic.contains("$dyn_attr1 > 40"));
    assert!(dynamic.contains(" or "));
    assert!(dynamic.contains("$e has age $dyn_sort0"));
    assert!(dynamic.contains("sort $dyn_sort0 desc"));
    assert!(dynamic.contains("limit 10"));
    assert!(dynamic.contains("offset 5"));
}

#[test]
fn dynamic_relation_group_by_aggregate_query_groups_relation_attribute() {
    let dynamic = query_builder::build_dynamic_relation_group_by_aggregate(
        &employment_descriptor(),
        &[],
        &["position".into()],
        &[count_aggregate()],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$r isa employment"));
    assert!(dynamic.contains("$r has position $group0"));
    assert!(dynamic.contains("$count = count($r)"));
    assert!(dynamic.contains("groupby $group0"));
}

#[test]
fn dynamic_relation_fetch_query_binds_role_player_filter() {
    let dynamic = query_builder::build_dynamic_relation_fetch_with_role_filters(
        &employment_descriptor(),
        &[],
        &[DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0xperson".into()),
            key: None,
        }],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$r isa $t"));
    assert!(dynamic.contains("employee: $rp0"));
    assert!(dynamic.contains("$rp0 isa person, iid 0xperson"));
}

#[test]
fn dynamic_relation_expr_fetch_binds_role_player_expr_and_sort() {
    let dynamic = query_builder::build_dynamic_relation_expr_fetch(
        &employment_descriptor(),
        &[DynamicExpr::RolePlayer {
            role_name: "employee".into(),
            expr: Box::new(DynamicExpr::Compare {
                attr_name: "age".into(),
                operator: DynamicComparisonOp::Gte,
                value: AttributeValue::Long(30),
            }),
        }],
        &[DynamicSort::RolePlayerAttribute {
            role_name: "employee".into(),
            attr_name: "name".into(),
            direction: SortDir::Asc,
        }],
        Some(3),
        None,
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$r isa $t"));
    assert!(dynamic.contains("$t sub employment"));
    assert!(dynamic.contains("employee: $employee"));
    assert!(dynamic.contains("$employee has age $dyn_attr0"));
    assert!(dynamic.contains("$dyn_attr0 >= 30"));
    assert!(dynamic.contains("$employee has name $dyn_sort0"));
    assert!(dynamic.contains("sort $dyn_sort0 asc"));
    assert!(dynamic.contains("limit 3"));
    assert!(dynamic.contains("\"_role_0_iid\": iid($employee)"));
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

#[test]
fn dynamic_relation_put_query_uses_put_clause() {
    let dynamic = query_builder::build_dynamic_relation_put(
        &employment_descriptor(),
        &employment_attrs("Engineer"),
        &employment_role_players(),
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("match"));
    assert!(dynamic.contains("iid 0xperson"));
    assert!(dynamic.contains("put"));
    assert!(dynamic.contains("$r isa employment, links (employee: $rp0, employer: $rp1)"));
    assert!(dynamic.contains("fetch"));
}

#[test]
fn dynamic_relation_update_query_matches_by_iid() {
    let dynamic = query_builder::build_dynamic_relation_update(
        &employment_descriptor(),
        Some("0xrel"),
        &employment_attrs("Staff Engineer"),
        &[],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("match"));
    assert!(dynamic.contains("iid 0xrel"));
    assert!(dynamic.contains("delete"));
    assert!(dynamic.contains("insert"));
    assert!(dynamic.contains("$r has position \"Staff Engineer\""));
    assert!(!dynamic.contains("employee: $rp0"));
}

#[test]
fn dynamic_relation_update_query_can_match_by_role_players() {
    let dynamic = query_builder::build_dynamic_relation_update(
        &employment_descriptor(),
        None,
        &employment_attrs("Staff Engineer"),
        &employment_role_players(),
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$rp0 isa person, iid 0xperson"));
    assert!(dynamic.contains("$rp1 isa company, iid 0xcompany"));
    assert!(dynamic.contains("$r isa employment (employee: $rp0, employer: $rp1)"));
    assert!(dynamic.contains("$r has position \"Staff Engineer\""));
}

#[test]
fn dynamic_relation_update_replaces_multi_value_attributes() {
    let attrs = vec![
        ("label".into(), AttributeValue::String("primary".into())),
        ("label".into(), AttributeValue::String("secondary".into())),
    ];
    let dynamic = query_builder::build_dynamic_relation_update(
        &tagged_employment_descriptor(),
        Some("0xrel"),
        &attrs,
        &[],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("try { $r has label $old_attr_0; }"));
    assert!(dynamic.contains("try { $old_attr_0 of $r; }"));
    assert!(dynamic.contains("$r has label \"primary\""));
    assert!(dynamic.contains("$r has label \"secondary\""));
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
    assert!(recorded[3].contains("$e;"));
    assert!(!recorded[3].contains("delete\n$e isa"));
}

#[tokio::test]
async fn dynamic_entity_manager_put_returns_iid() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![
        serde_json::json!({"iid": {"value": "0xput"}}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    assert_eq!(manager.put(&person_attrs()).await.unwrap(), "0xput");

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].starts_with("put"));
    assert!(recorded[0].contains("isa person"));
    assert!(recorded[0].contains("fetch"));
}

#[tokio::test]
async fn dynamic_entity_manager_batch_insert_and_put_use_one_transaction() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xput1"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xput0"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xinsert1"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xinsert0"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);
    let items = vec![
        vec![
            ("name".into(), AttributeValue::String("Alice".into())),
            ("age".into(), AttributeValue::Long(30)),
        ],
        vec![
            ("name".into(), AttributeValue::String("Bob".into())),
            ("age".into(), AttributeValue::Long(31)),
        ],
    ];

    assert_eq!(
        manager.insert_many(&items).await.unwrap(),
        vec!["0xinsert0", "0xinsert1"]
    );
    assert_eq!(
        manager.put_many(&items).await.unwrap(),
        vec!["0xput0", "0xput1"]
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].starts_with("insert"));
    assert!(recorded[2].starts_with("put"));
}

#[tokio::test]
async fn dynamic_entity_manager_update_executes_query() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Ok]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    manager
        .update(Some("0xaaa"), &person_attrs())
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("match"));
    assert!(recorded[0].contains("iid 0xaaa"));
    assert!(recorded[0].contains("delete"));
    assert!(recorded[0].contains("insert"));
    assert!(recorded[0].contains("has age"));
    assert!(!recorded[0].contains("$e has name \"Alice\""));
}

#[tokio::test]
async fn dynamic_entity_manager_fetches_by_iid() {
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
        QueryResult::Documents(vec![]),
        QueryResult::Documents(vec![fetch_doc]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let row = manager.get_by_iid("0xaaa").await.unwrap().unwrap();
    assert_eq!(row.iid.as_deref(), Some("0xaaa"));
    assert_eq!(row.type_name.as_deref(), Some("person"));
    assert_eq!(row.attributes, person_attrs());
    assert!(manager.get_by_iid("0xmissing").await.unwrap().is_none());

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("iid 0xaaa"));
    assert!(recorded[0].contains("isa! $t"));
    assert!(recorded[0].contains("sub person"));
}

#[tokio::test]
async fn dynamic_entity_manager_aggregate_executes_reduce_query() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$count": {"value": 2},
        "$avg_age": {"value": 31.5},
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let rows = manager
        .aggregate(&[], &[count_aggregate(), mean_age_aggregate()])
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("$count").unwrap(),
        &serde_json::json!({"value": 2})
    );
    assert_eq!(
        rows[0].get("$avg_age").unwrap(),
        &serde_json::json!({"value": 31.5})
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("reduce"));
    assert!(recorded[0].contains("$avg_age = mean($agg1)"));
}

#[tokio::test]
async fn dynamic_relation_manager_insert_fetch_count_delete() {
    let descriptor = Arc::new(employment_descriptor());
    let relation_attrs = employment_attrs("Engineer");
    let role_players = employment_role_players();
    let fetch_doc = serde_json::json!({
        "_iid": "0xrel",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        },
        "_role_0_iid": "0xperson",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        },
        "_role_1_iid": "0xcompany",
        "_role_1_type": "company",
        "_role_1_attributes": {
            "name": [{"value": "Acme"}]
        }
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
    assert_eq!(
        rows[0].role_players[0].attributes,
        vec![
            ("age".into(), serde_json::json!(30)),
            ("name".into(), serde_json::json!("Alice")),
        ]
    );
    assert_eq!(manager.count().await.unwrap(), 1);
    manager.delete_by_iid("0xrel").await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].contains("links (employee: $rp0, employer: $rp1)"));
    assert!(recorded[1].contains("sub employment"));
    assert!(recorded[1].contains("employee: $rp0"));
    assert!(recorded[1].contains("\"_role_0_attributes\": { $rp0.* }"));
    assert!(recorded[2].contains("reduce"));
    assert!(recorded[3].contains("delete"));
    assert!(recorded[3].contains("$r;"));
    assert!(!recorded[3].contains("delete\n$r isa"));
}

#[tokio::test]
async fn dynamic_relation_manager_put_and_update_execute_queries() {
    let descriptor = Arc::new(employment_descriptor());
    let relation_attrs = employment_attrs("Engineer");
    let role_players = employment_role_players();
    let backend = MockBackend::new(vec![
        QueryResult::Ok,
        QueryResult::Documents(vec![serde_json::json!({"iid": {"value": "0xputrel"}})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager.put(&relation_attrs, &role_players).await.unwrap(),
        "0xputrel"
    );
    manager
        .update(Some("0xputrel"), &employment_attrs("Staff Engineer"), &[])
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].starts_with("match"));
    assert!(recorded[0].contains("\nput"));
    assert!(recorded[0].contains("links (employee: $rp0, employer: $rp1)"));
    assert!(recorded[1].contains("iid 0xputrel"));
    assert!(recorded[1].contains("delete"));
    assert!(recorded[1].contains("insert"));
    assert!(recorded[1].contains("has position \"Staff Engineer\""));
}

#[tokio::test]
async fn dynamic_relation_manager_batch_insert_and_put_use_one_transaction() {
    let descriptor = Arc::new(employment_descriptor());
    let relation_attrs = employment_attrs("Engineer");
    let role_players = employment_role_players();
    let items = vec![
        (relation_attrs.clone(), role_players.clone()),
        (employment_attrs("Manager"), role_players.clone()),
    ];
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xrelput1"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xrelput0"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xrelinsert1"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xrelinsert0"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager.insert_many(&items).await.unwrap(),
        vec!["0xrelinsert0", "0xrelinsert1"]
    );
    assert_eq!(
        manager.put_many(&items).await.unwrap(),
        vec!["0xrelput0", "0xrelput1"]
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].contains("insert"));
    assert!(recorded[2].contains("put"));
}

#[tokio::test]
async fn dynamic_relation_manager_fetches_by_iid() {
    let descriptor = Arc::new(employment_descriptor());
    let relation_attrs = vec![("position".into(), AttributeValue::String("Engineer".into()))];
    let fetch_doc = serde_json::json!({
        "_iid": "0xrel",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        },
        "_role_0_iid": "0xperson",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        },
        "_role_1_iid": "0xcompany",
        "_role_1_type": "company",
        "_role_1_attributes": {
            "name": [{"value": "Acme"}]
        }
    });
    let backend = MockBackend::new(vec![
        QueryResult::Documents(vec![]),
        QueryResult::Documents(vec![fetch_doc]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    let rows = manager.get_by_iid("0xrel").await.unwrap();
    let row = &rows[0];
    assert_eq!(row.iid.as_deref(), Some("0xrel"));
    assert_eq!(row.attributes, relation_attrs);
    assert_eq!(row.role_players.len(), 2);
    assert_eq!(row.role_players[0].player_iid.as_deref(), Some("0xperson"));
    assert!(manager.get_by_iid("0xmissing").await.unwrap().is_empty());

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("iid 0xrel"));
    assert!(recorded[0].contains("employee: $rp0"));
    assert!(recorded[0].contains("\"_role_0_attributes\": { $rp0.* }"));
}

#[tokio::test]
async fn dynamic_relation_manager_group_by_aggregate_executes_reduce_query() {
    let descriptor = Arc::new(employment_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$group0": {"value": "Engineer"},
        "$count": {"value": 2},
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    let rows = manager
        .group_by_aggregate(&[], &["position".into()], &[count_aggregate()])
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("$group0").unwrap(),
        &serde_json::json!({"value": "Engineer"})
    );
    assert_eq!(
        rows[0].get("$count").unwrap(),
        &serde_json::json!({"value": 2})
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("$r has position $group0"));
    assert!(recorded[0].contains("groupby $group0"));
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

#[tokio::test]
async fn dynamic_entity_manager_can_use_shared_transaction_context() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![
        QueryResult::Rows(vec![serde_json::json!({"$count": 1})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xtx"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), descriptor);

    assert_eq!(manager.insert(&person_attrs()).await.unwrap(), "0xtx");
    assert_eq!(manager.count().await.unwrap(), 1);
    tx.commit().await.unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("insert"));
    assert!(recorded[1].contains("reduce"));
}

#[tokio::test]
async fn dynamic_entity_manager_rejects_write_in_read_transaction_context() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx, descriptor);

    assert!(matches!(
        manager.insert(&person_attrs()).await,
        Err(OrmError::Transaction(message)) if message.contains("Write operation")
    ));
}

// ── Phase 1 Gap A: expression-tree-filtered aggregate and group-by ────────────

#[test]
fn entity_expr_aggregate_query_uses_or_filter() {
    let dynamic = query_builder::build_dynamic_entity_expr_aggregate(
        &person_descriptor(),
        &[DynamicExpr::Or {
            exprs: vec![
                DynamicExpr::Compare {
                    attr_name: "name".into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String("Alice".into()),
                },
                DynamicExpr::Compare {
                    attr_name: "name".into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String("Bob".into()),
                },
            ],
        }],
        &[count_aggregate(), mean_age_aggregate()],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person"));
    // OR branch is emitted
    assert!(dynamic.contains(" or "));
    assert!(dynamic.contains("$count = count($e)"));
    assert!(dynamic.contains("$avg_age = mean($agg"));
    assert!(dynamic.contains("reduce"));
}

#[test]
fn entity_expr_aggregate_query_uses_not_filter() {
    let dynamic = query_builder::build_dynamic_entity_expr_aggregate(
        &person_descriptor(),
        &[DynamicExpr::Not {
            expr: Box::new(DynamicExpr::Compare {
                attr_name: "name".into(),
                operator: DynamicComparisonOp::Eq,
                value: AttributeValue::String("Carol".into()),
            }),
        }],
        &[count_aggregate()],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person"));
    assert!(dynamic.contains("not {"));
    assert!(dynamic.contains("$count = count($e)"));
    assert!(dynamic.contains("reduce"));
}

#[test]
fn entity_expr_group_by_aggregate_query_uses_or_filter() {
    let dynamic = query_builder::build_dynamic_entity_expr_group_by_aggregate(
        &person_descriptor(),
        &[DynamicExpr::Or {
            exprs: vec![
                DynamicExpr::Compare {
                    attr_name: "age".into(),
                    operator: DynamicComparisonOp::Lt,
                    value: AttributeValue::Long(30),
                },
                DynamicExpr::Compare {
                    attr_name: "age".into(),
                    operator: DynamicComparisonOp::Gt,
                    value: AttributeValue::Long(50),
                },
            ],
        }],
        &["name".into()],
        &[count_aggregate()],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person"));
    assert!(dynamic.contains(" or "));
    assert!(dynamic.contains("$e has name $group0"));
    assert!(dynamic.contains("$count = count($e)"));
    assert!(dynamic.contains("groupby $group0"));
}

#[test]
fn entity_expr_group_by_aggregate_query_uses_not_filter() {
    let dynamic = query_builder::build_dynamic_entity_expr_group_by_aggregate(
        &person_descriptor(),
        &[DynamicExpr::Not {
            expr: Box::new(DynamicExpr::Compare {
                attr_name: "age".into(),
                operator: DynamicComparisonOp::Lt,
                value: AttributeValue::Long(18),
            }),
        }],
        &["name".into()],
        &[count_aggregate()],
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("$e isa person"));
    assert!(dynamic.contains("not {"));
    assert!(dynamic.contains("$e has name $group0"));
    assert!(dynamic.contains("groupby $group0"));
}

#[tokio::test]
async fn dynamic_entity_manager_aggregate_with_query_executes_reduce_query() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$count": {"value": 2},
        "$avg_age": {"value": 28.0},
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let rows = manager
        .aggregate_with_query(
            &[DynamicExpr::Or {
                exprs: vec![
                    DynamicExpr::Compare {
                        attr_name: "name".into(),
                        operator: DynamicComparisonOp::Eq,
                        value: AttributeValue::String("Alice".into()),
                    },
                    DynamicExpr::Compare {
                        attr_name: "name".into(),
                        operator: DynamicComparisonOp::Eq,
                        value: AttributeValue::String("Bob".into()),
                    },
                ],
            }],
            &[count_aggregate(), mean_age_aggregate()],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("$count").unwrap(),
        &serde_json::json!({"value": 2})
    );
    assert_eq!(
        rows[0].get("$avg_age").unwrap(),
        &serde_json::json!({"value": 28.0})
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains(" or "));
    assert!(recorded[0].contains("reduce"));
    assert!(recorded[0].contains("$avg_age = mean($agg"));
}

#[tokio::test]
async fn dynamic_entity_manager_group_by_aggregate_with_query_executes_reduce_query() {
    let descriptor = Arc::new(person_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![
        serde_json::json!({
            "$group0": {"value": "Alice"},
            "$count": {"value": 1},
        }),
        serde_json::json!({
            "$group0": {"value": "Bob"},
            "$count": {"value": 3},
        }),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let rows = manager
        .group_by_aggregate_with_query(
            &[DynamicExpr::Not {
                expr: Box::new(DynamicExpr::Compare {
                    attr_name: "age".into(),
                    operator: DynamicComparisonOp::Lt,
                    value: AttributeValue::Long(18),
                }),
            }],
            &["name".into()],
            &[count_aggregate()],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("not {"));
    assert!(recorded[0].contains("$e has name $group0"));
    assert!(recorded[0].contains("groupby $group0"));
}

// ── Phase 1 Gap A: relation aggregate_with_query / group_by_aggregate_with_query ──

#[test]
fn relation_expr_aggregate_query_uses_or_filter() {
    let dynamic = query_builder::build_dynamic_relation_expr_aggregate(
        &employment_descriptor(),
        &[DynamicExpr::Or {
            exprs: vec![
                DynamicExpr::Compare {
                    attr_name: "position".into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String("Engineer".into()),
                },
                DynamicExpr::Compare {
                    attr_name: "position".into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String("Manager".into()),
                },
            ],
        }],
        &[count_aggregate()],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$r isa employment"));
    assert!(dynamic.contains(" or "));
    assert!(dynamic.contains("$count = count($r)"));
    assert!(dynamic.contains("reduce"));
}

#[test]
fn relation_expr_group_by_aggregate_query_uses_not_filter() {
    let dynamic = query_builder::build_dynamic_relation_expr_group_by_aggregate(
        &employment_descriptor(),
        &[DynamicExpr::Not {
            expr: Box::new(DynamicExpr::Compare {
                attr_name: "position".into(),
                operator: DynamicComparisonOp::Eq,
                value: AttributeValue::String("Intern".into()),
            }),
        }],
        &["position".into()],
        &[count_aggregate()],
        "$r",
    )
    .unwrap();

    assert!(dynamic.contains("$r isa employment"));
    assert!(dynamic.contains("not {"));
    assert!(dynamic.contains("$r has position $group0"));
    assert!(dynamic.contains("$count = count($r)"));
    assert!(dynamic.contains("groupby $group0"));
}

#[tokio::test]
async fn dynamic_relation_manager_aggregate_with_query_executes_reduce_query() {
    let descriptor = Arc::new(employment_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$count": {"value": 5},
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    let rows = manager
        .aggregate_with_query(
            &[DynamicExpr::Or {
                exprs: vec![
                    DynamicExpr::Compare {
                        attr_name: "position".into(),
                        operator: DynamicComparisonOp::Eq,
                        value: AttributeValue::String("Engineer".into()),
                    },
                    DynamicExpr::Compare {
                        attr_name: "position".into(),
                        operator: DynamicComparisonOp::Eq,
                        value: AttributeValue::String("Manager".into()),
                    },
                ],
            }],
            &[count_aggregate()],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("$count").unwrap(),
        &serde_json::json!({"value": 5})
    );

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains(" or "));
    assert!(recorded[0].contains("$count = count($r)"));
}

#[tokio::test]
async fn dynamic_relation_manager_group_by_aggregate_with_query_executes_reduce_query() {
    let descriptor = Arc::new(employment_descriptor());
    let backend = MockBackend::new(vec![QueryResult::Rows(vec![serde_json::json!({
        "$group0": {"value": "Engineer"},
        "$count": {"value": 3},
    })])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    let rows = manager
        .group_by_aggregate_with_query(
            &[DynamicExpr::Not {
                expr: Box::new(DynamicExpr::Compare {
                    attr_name: "position".into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String("Intern".into()),
                }),
            }],
            &["position".into()],
            &[count_aggregate()],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("not {"));
    assert!(recorded[0].contains("$r has position $group0"));
    assert!(recorded[0].contains("groupby $group0"));
}

// ── Phase 1 Gap B: StartsWith / EndsWith ─────────────────────────────────────

#[test]
fn starts_with_emits_anchored_prefix_like_regex() {
    let dynamic = query_builder::build_dynamic_entity_expr_fetch(
        &person_descriptor(),
        &[DynamicExpr::Compare {
            attr_name: "name".into(),
            operator: DynamicComparisonOp::StartsWith,
            value: AttributeValue::String("Al".into()),
        }],
        &[],
        None,
        None,
        "$e",
    )
    .unwrap();

    // TypeQL like with anchored prefix pattern
    assert!(dynamic.contains("like"));
    assert!(dynamic.contains("^Al.*"));
}

#[test]
fn ends_with_emits_anchored_suffix_like_regex() {
    let dynamic = query_builder::build_dynamic_entity_expr_fetch(
        &person_descriptor(),
        &[DynamicExpr::Compare {
            attr_name: "name".into(),
            operator: DynamicComparisonOp::EndsWith,
            value: AttributeValue::String("ice".into()),
        }],
        &[],
        None,
        None,
        "$e",
    )
    .unwrap();

    assert!(dynamic.contains("like"));
    assert!(dynamic.contains(".*ice$"));
}

#[test]
fn starts_with_escapes_regex_metacharacters_in_literal() {
    let dynamic = query_builder::build_dynamic_entity_expr_fetch(
        &person_descriptor(),
        &[DynamicExpr::Compare {
            attr_name: "name".into(),
            operator: DynamicComparisonOp::StartsWith,
            value: AttributeValue::String("foo.bar".into()),
        }],
        &[],
        None,
        None,
        "$e",
    )
    .unwrap();

    // Two escaping layers stack: regex-escape (`.` -> `\.`) then TypeQL
    // string-literal escape (`\` -> `\\`), so the rendered query text carries
    // a doubled backslash. TypeDB parses it back to the regex `^foo\.bar.*`.
    assert!(dynamic.contains("^foo\\\\.bar.*"));
}

#[test]
fn ends_with_escapes_regex_metacharacters_in_literal() {
    let dynamic = query_builder::build_dynamic_entity_expr_fetch(
        &person_descriptor(),
        &[DynamicExpr::Compare {
            attr_name: "name".into(),
            operator: DynamicComparisonOp::EndsWith,
            value: AttributeValue::String("foo+bar".into()),
        }],
        &[],
        None,
        None,
        "$e",
    )
    .unwrap();

    // Same doubled-backslash rendering as the StartsWith case (`+` -> `\+` -> `\\+`).
    assert!(dynamic.contains(".*foo\\\\+bar$"));
}

#[tokio::test]
async fn dynamic_entity_manager_starts_with_get_with_query_executes() {
    let descriptor = Arc::new(person_descriptor());
    let fetch_doc = serde_json::json!({
        "_iid": "0xaaa",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        }
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![fetch_doc])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let rows = manager
        .get_with_query(
            &[DynamicExpr::Compare {
                attr_name: "name".into(),
                operator: DynamicComparisonOp::StartsWith,
                value: AttributeValue::String("Al".into()),
            }],
            &[],
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].iid.as_deref(), Some("0xaaa"));

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("like"));
    assert!(recorded[0].contains("^Al.*"));
}

#[tokio::test]
async fn dynamic_entity_manager_ends_with_get_with_query_executes() {
    let descriptor = Arc::new(person_descriptor());
    let fetch_doc = serde_json::json!({
        "_iid": "0xbbb",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        }
    });
    let backend = MockBackend::new(vec![QueryResult::Documents(vec![fetch_doc])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    let rows = manager
        .get_with_query(
            &[DynamicExpr::Compare {
                attr_name: "name".into(),
                operator: DynamicComparisonOp::EndsWith,
                value: AttributeValue::String("ice".into()),
            }],
            &[],
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("like"));
    assert!(recorded[0].contains(".*ice$"));
}
