//! Dynamic manager smoke tests using the existing mock backend abstraction.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use common::*;
use type_bridge_orm::_manager::query_builder;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

#[derive(Debug, Default)]
struct RecordingState {
    opens: Vec<TxType>,
    queries: Vec<String>,
    query_modes: Vec<&'static str>,
    commits: usize,
    rollbacks: usize,
    closes: usize,
}

#[test]
fn dynamic_relation_identity_dto_is_owned_and_serializable() {
    let identity = DynamicRelationIdentity {
        iid: "0x1".into(),
        type_name: "employment".into(),
    };
    let encoded = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({"iid":"0x1","type_name":"employment"})
    );
    assert_eq!(
        serde_json::from_value::<DynamicRelationIdentity>(encoded).unwrap(),
        identity
    );
}

#[test]
fn dynamic_relation_identity_query_inventory_is_exact() {
    let descriptor = employment_descriptor();
    let all =
        query_builder::build_dynamic_relation_identity_discovery(&descriptor, None, "$r").unwrap();
    assert!(all.contains("$r isa! $t"));
    assert!(!all.contains("$r isa $t"));
    assert!(all.contains("$t sub employment"));
    assert_eq!(all.matches("\"_iid\": iid($r)").count(), 1);
    assert_eq!(all.matches("\"_type\": label($t)").count(), 1);
    for forbidden in [
        "attributes",
        ".*",
        "links",
        "role_players",
        "employee",
        "employer",
        "$employee",
        "$employer",
    ] {
        assert!(!all.contains(forbidden));
    }
    let one =
        query_builder::build_dynamic_relation_identity_discovery(&descriptor, Some("0x1"), "$r")
            .unwrap();
    assert!(
        one.contains("$r isa! $t")
            && !one.contains("$r isa $t")
            && one.contains("$t sub employment")
    );
    assert!(one.contains("iid 0x1"));
    assert!(!all.contains("iid 0x1"));
    assert_eq!(one.matches("\"_iid\": iid($r)").count(), 1);
    assert_eq!(one.matches("\"_type\": label($t)").count(), 1);
    for forbidden in [
        "attributes",
        ".*",
        "links",
        "role_players",
        "employee",
        "employer",
        "$employee",
        "$employer",
    ] {
        assert!(!one.contains(forbidden));
    }
}

#[test]
fn dynamic_relation_identity_query_rejects_invalid_iid_before_building() {
    let err = query_builder::build_dynamic_relation_identity_discovery(
        &employment_descriptor(),
        Some("bad"),
        "$r",
    )
    .unwrap_err();
    assert!(
        matches!(err, OrmError::QueryExecution(message) if message == "Exact relation operation for employment requires a canonical TypeDB IID")
    );
    let err = query_builder::build_dynamic_relation_delete_by_iid_exact(
        &employment_descriptor(),
        "bad",
        "$r",
    )
    .unwrap_err();
    assert!(
        matches!(err, OrmError::QueryExecution(message) if message == "Exact relation operation for employment requires a canonical TypeDB IID")
    );
}

#[test]
fn dynamic_relation_exact_count_query_contrasts_inclusive() {
    let d = employment_descriptor();
    let inclusive = query_builder::build_dynamic_relation_count(&d, &[], "$r").unwrap();
    let exact = query_builder::build_dynamic_relation_count_exact(&d, "$r").unwrap();
    assert!(inclusive.contains("$r isa employment") && !inclusive.contains("isa!"));
    assert!(
        exact.contains("$r isa! employment")
            && !exact.contains("$r isa employment")
            && exact.matches("$count = count($r)").count() == 1
    );
    assert!(
        !exact.contains("fetch")
            && !exact.contains("employee")
            && !exact.contains("employer")
            && !exact.contains("links")
            && !exact.contains("role_players")
    );
}

#[test]
fn dynamic_relation_exact_scalar_queries_keep_filters_database_side() {
    let expressions = [DynamicExpr::Compare {
        attr_name: "salary".into(),
        operator: DynamicComparisonOp::Gte,
        value: AttributeValue::Long(100),
    }];
    let count = query_builder::build_dynamic_relation_expr_count_exact(
        &employment_descriptor(),
        &expressions,
        "$r",
    )
    .unwrap();
    let exists = query_builder::build_dynamic_relation_expr_exists_exact(
        &employment_descriptor(),
        &expressions,
        "$r",
    )
    .unwrap();

    assert!(count.contains("$r isa! employment"));
    assert!(count.contains("$count = count($r)"));
    assert!(!count.contains("fetch"));
    assert!(exists.contains("$r isa! employment"));
    assert!(exists.contains("limit 1"));
    assert!(exists.contains("\"iid\": iid($r)"));
    assert!(!exists.contains("$count = count($r)"));
    assert!(!exists.contains("attributes"));
}

#[tokio::test]
async fn dynamic_relation_exact_scalar_terminals_do_not_hydrate_models() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 3})])),
        RecordingResponse::Result(QueryResult::Documents(vec![serde_json::json!({
            "malformed-model": true
        })])),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    let expressions = [DynamicExpr::Compare {
        attr_name: "salary".into(),
        operator: DynamicComparisonOp::Gte,
        value: AttributeValue::Long(100),
    }];

    assert_eq!(
        manager.count_exact_with_query(&expressions).await.unwrap(),
        3
    );
    assert!(manager.exists_exact_with_query(&expressions).await.unwrap());
    assert!(!manager.exists_exact_with_query(&expressions).await.unwrap());

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Read, TxType::Read, TxType::Read]);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("isa! employment"))
    );
    assert!(state.queries[0].contains("$count = count($r)"));
    assert!(!state.queries[0].contains("fetch"));
    for query in &state.queries[1..] {
        assert!(query.contains("limit 1"));
        assert!(query.contains("\"iid\": iid($r)"));
        assert!(!query.contains("attributes"));
    }
}

#[tokio::test]
async fn dynamic_relation_exact_first_limits_and_hydrates_only_one_row() {
    let first = serde_json::json!({
        "_iid": "0xabc",
        "_type": "employment",
        "attributes": {"position": [{"value": "Engineer"}]},
        "_role_0_iid": "0x101",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        },
        "_role_1_iid": "0x102",
        "_role_1_type": "company",
        "_role_1_attributes": {"name": [{"value": "Acme"}]}
    });
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![first, serde_json::json!({"malformed": true})]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let row = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()))
        .first_exact_with_query(&[])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row.iid.as_deref(), Some("0xabc"));
    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Read]);
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("isa! employment"));
    assert!(state.queries[0].contains("limit 1"));
}

#[tokio::test]
async fn dynamic_relation_exact_delete_batch_rolls_back_on_second_failure() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Error("second relation delete failed"),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));

    assert!(matches!(
        manager
            .delete_many_by_iid_exact(&["0xaaa".into(), "0xbbb".into()])
            .await,
        Err(OrmError::QueryExecution(message)) if message == "second relation delete failed"
    ));
    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[test]
fn dynamic_relation_exact_delete_query_contrasts_inclusive() {
    let d = employment_descriptor();
    let inclusive = query_builder::build_dynamic_relation_delete_by_iid(&d, "0x1", "$r").unwrap();
    let exact = query_builder::build_dynamic_relation_delete_by_iid_exact(&d, "0x1", "$r").unwrap();
    assert!(inclusive.contains("$r isa employment") && !inclusive.contains("isa!"));
    assert!(inclusive.contains("iid 0x1") && inclusive.contains("delete\n$r"));
    assert!(
        exact.contains("$r isa! employment")
            && exact.contains("iid 0x1")
            && exact.contains("delete\n$r")
    );
    assert!(!exact.contains("$r isa employment"));
}

#[test]
fn dynamic_relation_update_exact_attribute_builder_is_complete_and_strict() {
    let attr =
        |field: &str, name: &str, key: bool, optional: bool, card: Option<(u32, Option<u32>)>| {
            OwnedAttributeDescriptor {
                field_name: field.into(),
                attr_name: name.into(),
                value_type: type_bridge_orm::_attribute::ValueType::String,
                annotations: {
                    let mut a = Vec::new();
                    if key {
                        a.push(Annotation::Key);
                    }
                    if let Some((min, max)) = card {
                        a.push(Annotation::Card(min, max));
                    }
                    a
                },
                is_optional: optional,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }
        };
    let d = RelationDescriptor {
        type_name: "audit-relation".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            attr("id", "relation-id", true, false, None),
            attr("status", "status", false, false, None),
            attr("note", "note", false, true, None),
            attr("tags", "tag", false, true, Some((0, Some(4)))),
        ],
        roles: vec![],
        doc: None,
        meta: Default::default(),
    };
    let inclusive = query_builder::build_dynamic_relation_update(
        &d,
        Some("0x1"),
        &vec![("status".into(), AttributeValue::String("active".into()))],
        &[],
        "$r",
    )
    .unwrap();
    assert!(inclusive.contains("$r isa audit-relation") && !inclusive.contains("isa!"));
    let full = vec![
        ("relation-id".into(), AttributeValue::String("keep".into())),
        ("status".into(), AttributeValue::String("active".into())),
        ("tag".into(), AttributeValue::String("a".into())),
        ("tag".into(), AttributeValue::String("b".into())),
    ];
    let exact = query_builder::build_dynamic_relation_update_exact(&d, "0x1", &full, "$r").unwrap();
    assert!(exact.contains("$r isa! audit-relation") && exact.contains("iid 0x1"));
    assert_eq!(exact.matches("try { $old_attr_0").count(), 1);
    assert_eq!(exact.matches("try { $old_attr_1").count(), 1);
    assert_eq!(exact.matches("try { $old_attr_2").count(), 1);
    assert_eq!(exact.matches("try { $r has status").count(), 1);
    assert_eq!(exact.matches("try { $r has note").count(), 1);
    assert_eq!(exact.matches("try { $r has tag").count(), 1);
    assert_eq!(exact.matches("of $r").count(), 3);
    let tail = exact.split_once("insert\n").map(|(_, t)| t).unwrap_or("");
    assert_eq!(tail.matches("$r has status").count(), 1);
    assert_eq!(tail.matches("$r has tag").count(), 2);
    assert!(!tail.contains("note") && !tail.contains("relation-id"));
    assert!(tail.find("\"a\"").unwrap() < tail.find("\"b\"").unwrap());
    assert!(!exact.contains("relation-id") && !exact.contains("delete\n$r"));
    assert!(!tail.contains("$r isa") && !tail.contains("delete"));
    let omitted = vec![("status".into(), AttributeValue::String("active".into()))];
    let exact_omitted =
        query_builder::build_dynamic_relation_update_exact(&d, "0x1", &omitted, "$r").unwrap();
    let omitted_tail = exact_omitted
        .split_once("insert\n")
        .map(|(_, t)| t)
        .unwrap_or("");
    assert!(omitted_tail.contains("$r has status"));
    assert!(!omitted_tail.contains("note") && !omitted_tail.contains("tag"));
}

#[test]
fn dynamic_relation_update_exact_builder_guards_are_exact() {
    let d = employment_descriptor();
    let expect = |result: Result<String>, message: &str| match result {
        Err(OrmError::QueryExecution(actual)) => assert_eq!(actual, message),
        other => panic!("unexpected {other:?}"),
    };
    let value = AttributeValue::String("x".into());
    expect(
        query_builder::build_dynamic_relation_player_lookup("person", None, None, "$p"),
        "player identity must be exactly IID xor key",
    );
    expect(
        query_builder::build_dynamic_relation_player_lookup(
            "person",
            Some("0x1"),
            Some(("name", &value)),
            "$p",
        ),
        "player identity must be exactly IID xor key",
    );
    expect(
        query_builder::build_dynamic_relation_player_lookup("person", Some("bad"), None, "$p"),
        "player IID must be canonical",
    );
    for ty in ["bad type", "match"] {
        expect(
            query_builder::build_dynamic_relation_player_lookup(ty, Some("0x1"), None, "$p"),
            "unsafe player type label",
        );
    }
    for key in ["", "bad key", "match"] {
        expect(
            query_builder::build_dynamic_relation_player_lookup(
                "person",
                None,
                Some((key, &value)),
                "$p",
            ),
            "unsafe player key label",
        );
    }
    for blank in [
        AttributeValue::String(" ".into()),
        AttributeValue::Date("".into()),
        AttributeValue::DateTime("".into()),
        AttributeValue::DateTimeTZ("".into()),
        AttributeValue::Decimal("".into()),
        AttributeValue::Duration("".into()),
    ] {
        expect(
            query_builder::build_dynamic_relation_player_lookup(
                "person",
                None,
                Some(("name", &blank)),
                "$p",
            ),
            "player key value must be nonblank",
        );
    }
    expect(
        query_builder::build_dynamic_relation_clear_role(&d, "bad", "employee", "$r"),
        "Exact relation operation for employment requires a canonical TypeDB IID",
    );
    let mut bad_type = d.clone();
    bad_type.type_name = "bad type".into();
    expect(
        query_builder::build_dynamic_relation_clear_role(&bad_type, "0x1", "employee", "$r"),
        "unsafe or inactive relation role",
    );
    expect(
        query_builder::build_dynamic_relation_clear_role(&d, "0x1", "inactive", "$r"),
        "unsafe or inactive relation role",
    );
    for role in ["bad role", "match"] {
        let mut dd = d.clone();
        dd.roles[0].role_name = role.into();
        expect(
            query_builder::build_dynamic_relation_clear_role(&dd, "0x1", role, "$r"),
            "unsafe or inactive relation role",
        );
    }
    let tuple = [("person".into(), "0x2".into(), "employee".into())];
    expect(
        query_builder::build_dynamic_relation_attach(&d, "bad", &tuple, "$r"),
        "Exact relation operation for employment requires a canonical TypeDB IID",
    );
    expect(
        query_builder::build_dynamic_relation_attach(&bad_type, "0x1", &tuple, "$r"),
        "empty or unsafe relation attachment",
    );
    expect(
        query_builder::build_dynamic_relation_attach(&d, "0x1", &[], "$r"),
        "empty or unsafe relation attachment",
    );
    for ty in ["bad type", "match"] {
        let t = [(ty.into(), "0x2".into(), "employee".into())];
        expect(
            query_builder::build_dynamic_relation_attach(&d, "0x1", &t, "$r"),
            "unsafe relation attachment",
        );
    }
    expect(
        query_builder::build_dynamic_relation_attach(
            &d,
            "0x1",
            &[("person".into(), "0x2".into(), "inactive".into())],
            "$r",
        ),
        "unsafe relation attachment",
    );
    for role in ["bad role", "match"] {
        let mut dd = d.clone();
        dd.roles[0].role_name = role.into();
        let t = [("person".into(), "0x2".into(), role.into())];
        expect(
            query_builder::build_dynamic_relation_attach(&dd, "0x1", &t, "$r"),
            "unsafe relation attachment",
        );
    }
    let bad_player = [("person".into(), "bad".into(), "employee".into())];
    expect(
        query_builder::build_dynamic_relation_attach(&d, "0x1", &bad_player, "$r"),
        "unsafe relation attachment",
    );
    let lookup_iid =
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x1"), None, "$p")
            .unwrap();
    assert!(
        lookup_iid.contains("$p isa! person")
            && lookup_iid.contains("iid 0x1")
            && lookup_iid.contains("\"iid\": iid($p)")
    );
    let lookup_key = query_builder::build_dynamic_relation_player_lookup(
        "person",
        None,
        Some(("name", &value)),
        "$p",
    )
    .unwrap();
    assert!(
        lookup_key.contains("$p isa! person")
            && lookup_key.contains("has name")
            && lookup_key.contains("\"iid\": iid($p)")
    );
    for forbidden in ["links", "role_players", "_type", ".*"] {
        assert!(!lookup_iid.contains(forbidden) && !lookup_key.contains(forbidden));
    }
    let clear =
        query_builder::build_dynamic_relation_clear_role(&d, "0x1", "employee", "$r").unwrap();
    assert!(
        clear.contains("$r isa! employment") && clear.contains("links") && clear.contains("delete")
    );
    let attach = query_builder::build_dynamic_relation_attach(
        &d,
        "0x1",
        &[("person".into(), "0x2".into(), "employee".into())],
        "$r",
    )
    .unwrap();
    assert!(
        attach.contains("$r isa! employment")
            && attach.contains("$p0 isa! person")
            && attach.contains("$r links")
            && !attach.contains("$r isa employment")
    );
    let tail = attach
        .split_once("insert\n")
        .map(|(_, t)| t)
        .unwrap_or(&attach);
    assert!(tail.contains("$r links") && !tail.contains("$r isa") && !tail.contains("delete"));
}

#[tokio::test]
async fn dynamic_relation_update_exact_preflight_zero_io_table() {
    let mut cases: Vec<(RelationDescriptor, DynamicAttributeMap, &str, &str)> = Vec::new();
    cases.push((
        employment_descriptor(),
        vec![],
        "bad",
        "Exact relation operation for employment requires a canonical TypeDB IID",
    ));
    cases.push((
        employment_descriptor(),
        vec![],
        "0x1",
        "employment: exact relation update requires at least one role player",
    ));
    let mut d = employment_descriptor();
    d.type_name = "bad type".into();
    cases.push((d, vec![], "0x1", "bad type: unsafe relation type label"));
    let mut d = employment_descriptor();
    d.owned_attributes[0].attr_name = "bad attr".into();
    cases.push((
        d,
        vec![],
        "0x1",
        "employment: unsafe relation attribute label bad attr",
    ));
    let mut d = employment_descriptor();
    d.roles[0].role_name = "bad role".into();
    cases.push((
        d,
        vec![],
        "0x1",
        "employment: unsafe relation role label bad role",
    ));
    cases.push((
        employment_descriptor(),
        vec![("ghost".into(), AttributeValue::String("x".into()))],
        "0x1",
        "employment: unknown relation attribute ghost",
    ));
    let mut d = employment_descriptor();
    d.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "shared".into(),
        attr_name: "alpha".into(),
        value_type: ValueType::String,
        is_optional: true,
        annotations: vec![],
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    d.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "beta".into(),
        attr_name: "shared".into(),
        value_type: ValueType::Long,
        is_optional: true,
        annotations: vec![],
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    cases.push((
        d,
        vec![("shared".into(), AttributeValue::String("x".into()))],
        "0x1",
        "employment: ambiguous relation attribute shared",
    ));
    cases.push((
        employment_descriptor(),
        vec![("position".into(), AttributeValue::Long(1))],
        "0x1",
        "employment: relation attribute position has wrong value type",
    ));
    let mut d = employment_descriptor();
    d.owned_attributes[0].is_optional = false;
    cases.push((
        d.clone(),
        vec![],
        "0x1",
        "employment: relation attribute position violates minimum cardinality",
    ));
    cases.push((
        d,
        vec![
            ("position".into(), AttributeValue::String("a".into())),
            ("position".into(), AttributeValue::String("b".into())),
        ],
        "0x1",
        "employment: relation attribute position violates maximum cardinality",
    ));
    for attrs in [
        vec![("position".into(), AttributeValue::String("a".into()))],
        vec![
            ("position".into(), AttributeValue::String("a".into())),
            ("position".into(), AttributeValue::String("b".into())),
            ("position".into(), AttributeValue::String("c".into())),
            ("position".into(), AttributeValue::String("d".into())),
        ],
    ] {
        let mut d = employment_descriptor();
        d.owned_attributes[0].is_optional = true;
        d.owned_attributes[0].annotations = vec![Annotation::Card(2, Some(3))];
        let msg = if attrs.len() == 1 {
            "employment: relation attribute position violates minimum cardinality"
        } else {
            "employment: relation attribute position violates maximum cardinality"
        };
        cases.push((d, attrs, "0x1", msg));
    }
    let player = |role: &str| DynamicRolePlayerInput {
        role_name: role.into(),
        player_type_name: "person".into(),
        iid: Some("0x2".into()),
        key: None,
    };
    let mut role_cases: Vec<(RelationDescriptor, Vec<DynamicRolePlayerInput>, &str)> = Vec::new();
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            role_name: "ghost".into(),
            ..player("ghost")
        }],
        "employment: unknown relation role ghost",
    ));
    let mut d = employment_descriptor();
    d.roles[0].cardinality = Some((1, Some(2)));
    role_cases.push((
        d,
        vec![],
        "employment: relation role employee violates cardinality",
    ));
    let mut d = employment_descriptor();
    d.roles[0].cardinality = Some((0, Some(1)));
    role_cases.push((
        d,
        vec![
            player("employee"),
            DynamicRolePlayerInput {
                iid: Some("0x3".into()),
                ..player("employee")
            },
        ],
        "employment: relation role employee violates cardinality",
    ));
    let mut d = employment_descriptor();
    d.roles[0].ordered = true;
    role_cases.push((
        d,
        vec![
            player("employee"),
            DynamicRolePlayerInput {
                iid: Some("0x3".into()),
                ..player("employee")
            },
        ],
        "employment: ordered relation role employee cannot contain multiple players",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            player_type_name: "bad type".into(),
            ..player("employee")
        }],
        "employment: unsafe player type label bad type",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            key: Some(("bad key".into(), AttributeValue::String("x".into()))),
            iid: None,
            ..player("employee")
        }],
        "employment: unsafe player key label bad key",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            key: Some(("".into(), AttributeValue::String("x".into()))),
            iid: None,
            ..player("employee")
        }],
        "employment: unsafe player key label ",
    ));
    for value in [
        AttributeValue::String("".into()),
        AttributeValue::Date("".into()),
        AttributeValue::DateTime("".into()),
        AttributeValue::DateTimeTZ("".into()),
        AttributeValue::Decimal("".into()),
        AttributeValue::Duration("".into()),
    ] {
        role_cases.push((
            employment_descriptor(),
            vec![DynamicRolePlayerInput {
                key: Some(("name".into(), value)),
                iid: None,
                ..player("employee")
            }],
            "employment: player key value must be nonblank",
        ));
    }
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            key: Some(("name".into(), AttributeValue::String("x".into()))),
            ..player("employee")
        }],
        "employment: player identity must be exactly IID xor key",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            iid: None,
            key: None,
            ..player("employee")
        }],
        "employment: player identity must be exactly IID xor key",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![DynamicRolePlayerInput {
            iid: Some("bad".into()),
            ..player("employee")
        }],
        "employment: player IID must be canonical",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![player("employee"), player("employee")],
        "employment: duplicate relation player input",
    ));
    role_cases.push((
        employment_descriptor(),
        vec![
            DynamicRolePlayerInput {
                iid: None,
                key: Some(("name".into(), AttributeValue::String("x".into()))),
                ..player("employee")
            },
            DynamicRolePlayerInput {
                iid: None,
                key: Some(("name".into(), AttributeValue::String("x".into()))),
                ..player("employee")
            },
        ],
        "employment: duplicate relation player input",
    ));
    for (descriptor, attrs, iid, expected) in cases {
        let (backend, state) = RecordingBackend::new(vec![]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(descriptor))
            .update_exact(iid, &attrs, &[])
            .await
            .unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(message) if message == expected));
        let s = state.lock().unwrap();
        assert!(s.opens.is_empty() && s.queries.is_empty() && s.query_modes.is_empty());
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
    }
    for (descriptor, roles, expected) in role_cases {
        let (backend, state) = RecordingBackend::new(vec![]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(descriptor))
            .update_exact("0x1", &Vec::new(), &roles)
            .await
            .unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(message) if message == expected));
        let s = state.lock().unwrap();
        assert!(s.opens.is_empty() && s.queries.is_empty() && s.query_modes.is_empty());
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_identity_boundary_distinct_keys_reach_both_lookups() {
    let mut d = employment_descriptor();
    d.roles.retain(|r| r.role_name == "employee");
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Alice".into()))),
        },
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Bob".into()))),
        },
    ];
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Error("post-resolution sentinel"),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(d))
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, OrmError::QueryExecution(m) if m == "post-resolution sentinel"));
    let s = state.lock().unwrap();
    assert_eq!(s.queries.len(), 3);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy", "legacy", "legacy"]);
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
    assert_eq!(s.closes, 0);
    assert!(s.queries[0].contains("has name") && s.queries[0].contains("Alice"));
    assert!(s.queries[1].contains("has name") && s.queries[1].contains("Bob"));
    for q in &s.queries[..2] {
        assert!(q.contains("isa! person") && q.contains("\"iid\": iid($p)"));
        for forbidden in [
            "links",
            "role_players",
            "$r",
            "employee",
            "employer",
            "delete",
            "insert",
        ] {
            assert!(!q.contains(forbidden));
        }
    }
    assert!(
        s.queries[2].contains("isa! employment")
            && s.queries[2].contains("iid 0x1")
            && s.queries[2].contains("position")
    );
    assert!(!s.queries[2].contains("\"iid\": iid($p)"));
}

#[tokio::test]
async fn dynamic_relation_update_exact_identity_boundary_convergence_is_post_resolution() {
    let rows = vec![
        ("iid/key", "person", "person"),
        ("same-iid-types", "person", "contractor"),
    ];
    for (kind, first, second) in rows {
        let mut d = employment_descriptor();
        d.roles.retain(|r| r.role_name == "employee");
        let second_input = if kind == "iid/key" {
            DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: second.into(),
                iid: None,
                key: Some(("name".into(), AttributeValue::String("Alice".into()))),
            }
        } else {
            DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: second.into(),
                iid: Some("0x2".into()),
                key: None,
            }
        };
        let players = vec![
            DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: first.into(),
                iid: Some("0x2".into()),
                key: None,
            },
            second_input,
        ];
        let (backend, state) = RecordingBackend::new(vec![
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x2"}),
            ])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":{"value":"0x2"}}),
            ])),
        ]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(d))
            .update_exact("0x1", &Vec::new(), &players)
            .await
            .unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "Player resolution converged on duplicate IID 0x2 for role employee")
        );
        let s = state.lock().unwrap();
        assert_eq!(s.queries.len(), 2);
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.query_modes, vec!["legacy", "legacy"]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
        assert!(s.queries[0].contains(&format!("isa! {first}")));
        assert!(s.queries[1].contains(&format!("isa! {second}")));
        for q in &s.queries {
            assert!(q.contains("\"iid\": iid($p)"));
            for forbidden in ["$r", "links", "role_players", "delete", "insert"] {
                assert!(!q.contains(forbidden));
            }
        }
        if kind == "iid/key" {
            assert!(
                s.queries[0].contains("iid 0x2")
                    && s.queries[1].contains("has name")
                    && s.queries[1].contains("Alice")
            );
        } else {
            assert!(s.queries[0].contains("iid 0x2") && s.queries[1].contains("iid 0x2"));
        }
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_player_answer_matrix_is_relation_rooted_and_pre_mutation() {
    let answers = vec![
        (
            QueryResult::Documents(vec![]),
            "Player resolution returned 0; expected exactly one document",
        ),
        (
            QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x2"}),
                serde_json::json!({"iid":"0x2"}),
            ]),
            "Player resolution returned 2; expected exactly one document",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!(null)]),
            "Player resolution returned a non-object",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({})]),
            "Player resolution omitted its IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid": 2})]),
            "Player resolution returned a nonstring IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid":""})]),
            "Player resolution returned a blank IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid":"bad"})]),
            "Player resolution returned a noncanonical IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid":"0x3"})]),
            "Player resolution returned the wrong IID",
        ),
        (
            QueryResult::Ok,
            "Player resolution returned Ok; expected exactly one document",
        ),
        (
            QueryResult::Rows(vec![serde_json::json!({"iid":"0x2"})]),
            "Player resolution returned Rows; expected exactly one document",
        ),
    ];
    assert_eq!(answers.len(), 10);
    for (answer, expected) in answers {
        let mut d = employment_descriptor();
        d.roles.retain(|r| r.role_name == "employee");
        let player = DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        };
        let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(answer)]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(d))
            .update_exact("0x1", &Vec::new(), &[player])
            .await
            .unwrap_err();
        assert!(
            matches!(err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == expected)
        );
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.queries.len(), 1);
        assert_eq!(s.query_modes, vec!["legacy"]);
        assert!(
            s.queries[0].contains("isa! person")
                && s.queries[0].contains("iid 0x2")
                && s.queries[0].contains("\"iid\": iid($p)")
        );
        for forbidden in [
            "$r",
            "employee",
            "employer",
            "role_players",
            "links",
            "delete",
            "insert",
        ] {
            assert!(!s.queries[0].contains(forbidden));
        }
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_player_answer_finishes_lookups_before_mutation_and_rejects_read_tx()
 {
    let d = employment_descriptor();
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![serde_json::json!({})])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(d.clone()))
        .update_exact("0x1", &Vec::new(), &players)
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Hydration { type_name, message } if type_name == "employment" && message == "Player resolution omitted its IID")
    );
    {
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.queries.len(), 2);
        assert_eq!(s.query_modes, vec!["legacy", "legacy"]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
        assert!(
            s.queries[0].contains("isa! person")
                && s.queries[0].contains("iid 0x2")
                && s.queries[0].contains("\"iid\": iid($p)")
        );
        assert!(
            s.queries[1].contains("isa! company")
                && s.queries[1].contains("iid 0x3")
                && s.queries[1].contains("\"iid\": iid($p)")
        );
        for q in &s.queries {
            for forbidden in [
                "$r",
                "employee",
                "employer",
                "role_players",
                "links",
                "delete",
                "insert",
            ] {
                assert!(!q.contains(forbidden));
            }
        }
    }
    let (read_backend, read_state) = RecordingBackend::new(vec![]);
    let read_db = Database::with_backend(Box::new(read_backend), "testdb");
    let read_tx = read_db.transaction_context(TxType::Read).await.unwrap();
    let manager = DynamicRelationManager::with_transaction(read_tx.clone(), Arc::new(d));
    let player = DynamicRolePlayerInput {
        role_name: "employee".into(),
        player_type_name: "person".into(),
        iid: Some("0x2".into()),
        key: None,
    };
    let err = manager
        .update_exact("0x1", &Vec::new(), &[player])
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Transaction(message) if message == "Cannot execute Write operation in Read transaction")
    );
    let s = read_state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Read]);
    assert!(s.queries.is_empty());
    assert!(s.query_modes.is_empty());
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_update_exact_success_replaces_complete_active_state_in_order() {
    let attr =
        |field: &str, name: &str, key: bool, optional: bool, card: Option<(u32, Option<u32>)>| {
            OwnedAttributeDescriptor {
                field_name: field.into(),
                attr_name: name.into(),
                value_type: ValueType::String,
                annotations: {
                    let mut a = Vec::new();
                    if key {
                        a.push(Annotation::Key);
                    }
                    if let Some((min, max)) = card {
                        a.push(Annotation::Card(min, max));
                    }
                    a
                },
                is_optional: optional,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }
        };
    let role = |name: &str,
                player: &str,
                overrides: Option<&str>,
                cardinality: Option<(u32, Option<u32>)>| RoleDescriptor {
        role_name: name.into(),
        player_type_names: vec![player.into()],
        cardinality,
        overrides: overrides.map(str::to_owned),
        is_abstract: false,
        ordered: false,
        distinct: false,
        plays_cardinality: None,
        doc: None,
        meta: Default::default(),
    };
    let d = RelationDescriptor {
        type_name: "special-employment".into(),
        is_abstract: false,
        parent_type: Some("association".into()),
        owned_attributes: vec![
            attr("id", "relation-id", true, false, None),
            attr("status", "status", false, false, None),
            attr("note", "note", false, true, None),
            attr("tags", "tag", false, true, Some((0, Some(4)))),
        ],
        roles: vec![
            role("observer", "person", None, None),
            role("assignee", "contractor", Some("participant"), None),
            role("reviewer", "person", None, None),
        ],
        doc: None,
        meta: Default::default(),
    };
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "observer".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Alice".into()))),
        },
        DynamicRolePlayerInput {
            role_name: "assignee".into(),
            player_type_name: "contractor".into(),
            iid: Some("0x3".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "observer".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Bob".into()))),
        },
    ];
    let answers = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x3"}}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x4"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
    ];
    let (backend, state) = RecordingBackend::new(answers);
    let db = Database::with_backend(Box::new(backend), "testdb");
    DynamicRelationManager::new(&db, Arc::new(d))
        .update_exact(
            "0x1",
            &vec![
                ("relation-id".into(), AttributeValue::String("keep".into())),
                ("status".into(), AttributeValue::String("active".into())),
                ("tag".into(), AttributeValue::String("a".into())),
                ("tag".into(), AttributeValue::String("b".into())),
            ],
            &players,
        )
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.queries.len(), 8);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 8]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
    assert!(
        s.queries[0].contains("$p isa! person")
            && s.queries[0].contains("has name")
            && s.queries[0].contains("Alice")
            && s.queries[0].contains("\"iid\": iid($p)")
    );
    assert!(
        s.queries[1].contains("$p isa! contractor")
            && s.queries[1].contains("iid 0x3")
            && s.queries[1].contains("\"iid\": iid($p)")
    );
    assert!(
        s.queries[2].contains("$p isa! person")
            && s.queries[2].contains("has name")
            && s.queries[2].contains("Bob")
            && s.queries[2].contains("\"iid\": iid($p)")
    );
    for q in &s.queries[0..3] {
        for forbidden in [
            "$r",
            "observer",
            "assignee",
            "reviewer",
            "participant",
            "role_players",
            "links",
            "delete",
            "insert",
        ] {
            assert!(!q.contains(forbidden));
        }
    }
    assert!(s.queries[3].contains("isa! special-employment") && s.queries[3].contains("iid 0x1"));
    let tail = s.queries[3]
        .split_once("insert\n")
        .map(|(_, t)| t)
        .unwrap_or("");
    assert_eq!(s.queries[3].matches("try { $r has status").count(), 1);
    assert_eq!(s.queries[3].matches("try { $r has note").count(), 1);
    assert_eq!(s.queries[3].matches("try { $r has tag").count(), 1);
    assert_eq!(s.queries[3].matches("try { $old_attr_0").count(), 1);
    assert_eq!(s.queries[3].matches("try { $old_attr_1").count(), 1);
    assert_eq!(s.queries[3].matches("try { $old_attr_2").count(), 1);
    assert_eq!(s.queries[3].matches("of $r").count(), 3);
    assert_eq!(tail.matches("$r has status").count(), 1);
    assert_eq!(tail.matches("$r has tag").count(), 2);
    assert!(
        !tail.contains("note")
            && !tail.contains("relation-id")
            && !s.queries[3].contains("relation-id")
            && !tail.contains("$r isa")
    );
    assert!(tail.find("\"a\"").unwrap() < tail.find("\"b\"").unwrap());
    assert!(!s.queries[3].contains("delete\n$r"));
    for (q, role) in s.queries[4..7]
        .iter()
        .zip(["observer", "assignee", "reviewer"])
    {
        assert!(
            q.contains("isa! special-employment")
                && q.contains("iid 0x1")
                && q.contains(&format!("$r links ({role}: $old);"))
                && q.contains(&format!("delete\nlinks ({role}: $old) of $r;"))
                && !q.contains(";;")
                && !q.contains("participant")
                && !q.contains("delete\n$r")
        );
    }
    let attach = &s.queries[7];
    assert!(!attach.contains(";;"));
    assert!(attach.contains("$r isa! special-employment, iid 0x1;"));
    assert!(
        attach.contains("$p0 isa! person, iid 0x2;")
            && attach.contains("$p1 isa! contractor, iid 0x3;")
            && attach.contains("$p2 isa! person, iid 0x4;")
    );
    let at = attach.split_once("insert\n").map(|(_, t)| t).unwrap_or("");
    assert!(
        at.contains("$r links (observer: $p0, assignee: $p1, observer: $p2);")
            && !at.contains("reviewer")
            && !at.contains("participant")
            && !at.contains("$r isa")
            && !at.contains("delete")
    );
    for q in &s.queries[3..8] {
        assert!(
            q.contains("isa! special-employment")
                && q.contains("iid 0x1")
                && !q.contains("delete\n$r")
        );
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_preflight_accounts_by_descriptor_index() {
    let mut d = employment_descriptor();
    d.owned_attributes = vec![
        OwnedAttributeDescriptor {
            field_name: "alpha".into(),
            attr_name: "shared".into(),
            value_type: ValueType::String,
            is_optional: true,
            annotations: vec![],
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
        OwnedAttributeDescriptor {
            field_name: "beta".into(),
            attr_name: "shared".into(),
            value_type: ValueType::Long,
            is_optional: true,
            annotations: vec![],
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
    ];
    d.roles.retain(|r| r.role_name == "employee");
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    DynamicRelationManager::new(&db, Arc::new(d))
        .update_exact(
            "0x1",
            &vec![("alpha".into(), AttributeValue::String("x".into()))],
            &[DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: "person".into(),
                iid: Some("0x2".into()),
                key: None,
            }],
        )
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.queries.len(), 4);
    assert_eq!(s.query_modes, vec!["legacy", "legacy", "legacy", "legacy"]);
    assert!(s.queries[0].contains("isa! person") && s.queries[0].contains("iid 0x2"));
    assert!(
        s.queries[1].contains("isa! employment")
            && s.queries[1].contains("iid 0x1")
            && s.queries[1].contains("shared")
    );
    assert!(
        s.queries[2].contains("isa! employment")
            && s.queries[2].contains("employee")
            && s.queries[2].contains("delete\nlinks")
    );
    assert!(
        s.queries[3].contains("isa! employment")
            && s.queries[3].contains("employee")
            && s.queries[3].contains("$r links")
    );
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_put_exact_single_preflight_zero_io_table() {
    let attrs = || {
        vec![
            ("external_id".into(), AttributeValue::String("emp-7".into())),
            ("revision".into(), AttributeValue::Long(7)),
            ("title".into(), AttributeValue::String("Engineer".into())),
        ]
    };
    let players = || {
        vec![
            DynamicRolePlayerInput {
                role_name: "employee".into(),
                player_type_name: "person".into(),
                iid: Some("0x2".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "employer".into(),
                player_type_name: "company".into(),
                iid: Some("0x3".into()),
                key: None,
            },
        ]
    };
    let mut cases: Vec<(
        RelationDescriptor,
        DynamicAttributeMap,
        Vec<DynamicRolePlayerInput>,
        &'static str,
    )> = Vec::new();
    let mut a = attrs();
    a.retain(|(n, _)| n != "external_id");
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: relation attribute employment-id violates minimum cardinality",
    ));
    let mut a = attrs();
    a.retain(|(n, _)| n != "revision");
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: relation attribute employment-revision violates minimum cardinality",
    ));
    let mut a = attrs();
    a.retain(|(n, _)| n != "title");
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: relation attribute position violates minimum cardinality",
    ));
    let mut a = attrs();
    a.push(("ghost".into(), AttributeValue::String("x".into())));
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: unknown relation attribute ghost",
    ));
    let mut x = exact_put_builder_descriptor();
    x.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "employment-id".into(),
        attr_name: "shadow-id".into(),
        value_type: ValueType::String,
        is_optional: true,
        annotations: vec![],
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    let mut a = attrs();
    if let Some(v) = a.iter_mut().find(|(n, _)| n == "external_id") {
        v.0 = "employment-id".into();
    }
    cases.push((
        x,
        a,
        players(),
        "employment: ambiguous relation attribute employment-id",
    ));
    let mut a = attrs();
    a.push(("external_id".into(), AttributeValue::String("x".into())));
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: relation attribute employment-id violates maximum cardinality",
    ));
    let mut a = attrs();
    if let Some(v) = a.iter_mut().find(|(n, _)| n == "external_id") {
        v.1 = AttributeValue::Long(9);
    }
    cases.push((
        exact_put_builder_descriptor(),
        a,
        players(),
        "employment: relation attribute employment-id has wrong value type",
    ));
    let mut d = exact_put_builder_descriptor();
    d.roles
        .iter_mut()
        .for_each(|r| r.cardinality = Some((0, Some(1))));
    cases.push((
        d,
        attrs(),
        Vec::new(),
        "employment: exact relation put requires at least one role player",
    ));
    let mut p = players();
    p.push(DynamicRolePlayerInput {
        role_name: "ghost".into(),
        player_type_name: "person".into(),
        iid: Some("0x4".into()),
        key: None,
    });
    cases.push((
        exact_put_builder_descriptor(),
        attrs(),
        p,
        "employment: unknown relation role ghost",
    ));
    let mut p = players();
    p[0].iid = None;
    p[0].key = None;
    cases.push((
        exact_put_builder_descriptor(),
        attrs(),
        p,
        "employment: player identity must be exactly IID xor key",
    ));
    for (d, a, p, expected) in cases {
        let (backend, state) = RecordingBackend::new(vec![]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(d))
            .put_exact(&a, &p)
            .await
            .unwrap_err();
        assert!(matches!(err,OrmError::QueryExecution(m) if m==expected));
        let s = state.lock().unwrap();
        assert!(s.opens.is_empty() && s.queries.is_empty() && s.query_modes.is_empty());
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
    }
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Error(
        "accepted update without keys",
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let attrs = vec![("title".into(), AttributeValue::String("Engineer".into()))];
    let p = players();
    let err = DynamicRelationManager::new(&db, Arc::new(exact_put_builder_descriptor()))
        .update_exact("0x1", &attrs, &p)
        .await
        .unwrap_err();
    assert!(matches!(err,OrmError::QueryExecution(m) if m=="accepted update without keys"));
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.queries.len(), 1);
    assert_eq!(s.query_modes, vec!["legacy"]);
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
    assert_eq!(s.closes, 0);
    let expected =
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
            .unwrap();
    assert_eq!(s.queries[0], expected);
}

#[tokio::test]
async fn dynamic_relation_put_exact_single_miss_inserts_after_strict_resolution() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Alice".into()))),
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let attrs = vec![("position".into(), AttributeValue::String("x".into()))];
    let resolved = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let insert_q = query_builder::build_dynamic_relation_insert_resolved_with_iid(
        &employment_descriptor(),
        &attrs,
        &resolved,
        "$r",
    )
    .unwrap();
    let lookup_a = query_builder::build_dynamic_relation_player_lookup(
        "person",
        None,
        Some(("name", &AttributeValue::String("Alice".into()))),
        "$p",
    )
    .unwrap();
    let lookup_b =
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap();
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x8"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let got = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()))
        .put_exact(&attrs, &players)
        .await
        .unwrap();
    assert_eq!(got, "0x8");
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.queries,
            vec![lookup_a.clone(), lookup_b.clone(), insert_q]
        );
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.query_modes, vec!["legacy"; 3]);
        assert_eq!(s.commits, 1);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
        assert!(
            query_builder::build_dynamic_relation_exact_key_lookup(
                &employment_descriptor(),
                &attrs,
                "$r"
            )
            .unwrap()
            .is_none()
        );
        assert!(
            s.queries[0].contains("isa! person")
                && s.queries[0].contains("has name")
                && s.queries[0].contains("Alice")
                && s.queries[0].contains("\"iid\": iid($p)")
        );
        assert!(
            s.queries[1].contains("isa! company")
                && s.queries[1].contains("iid 0x3")
                && s.queries[1].contains("\"iid\": iid($p)")
        );
        for q in &s.queries[0..2] {
            for forbidden in [
                "$r", "links", "employee", "employer", "insert", "delete", ".*", "_type", "put",
            ] {
                assert!(!q.contains(forbidden));
            }
        }
        assert!(
            s.queries[2].contains("$p0 isa! person, iid 0x2;")
                && s.queries[2].contains("$p1 isa! company, iid 0x3;")
                && s.queries[2].contains("$r isa employment, links (employee: $p0, employer: $p1)")
                && s.queries[2].contains("position")
                && s.queries[2].ends_with("fetch {\n  \"iid\": iid($r)\n};")
        );
        assert!(!s.queries.iter().any(|q| q.contains("put")));
    }
    let d = exact_put_builder_descriptor();
    let attrs2 = vec![
        ("title".into(), AttributeValue::String("Engineer".into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("external_id".into(), AttributeValue::String("emp-7".into())),
    ];
    let key_q = query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs2, "$r")
        .unwrap()
        .unwrap();
    let ins_q = query_builder::build_dynamic_relation_insert_resolved_with_iid(
        &d, &attrs2, &resolved, "$r",
    )
    .unwrap();
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x9"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let got = DynamicRelationManager::new(&db, Arc::new(d.clone()))
        .put_exact(&attrs2, &players)
        .await
        .unwrap();
    assert_eq!(got, "0x9");
    let s = state.lock().unwrap();
    assert_eq!(s.queries[0], key_q);
    assert_eq!(s.queries[1], lookup_a);
    assert_eq!(s.queries[2], lookup_b);
    assert_eq!(s.queries[3], ins_q);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 4]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_put_exact_single_hit_delegates_complete_update() {
    let descriptor = exact_put_builder_descriptor();
    let attrs = vec![
        ("title".into(), AttributeValue::String("Engineer".into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("external_id".into(), AttributeValue::String("emp-7".into())),
    ];
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: None,
            key: Some(("name".into(), AttributeValue::String("Alice".into()))),
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let resolved = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let key_q = query_builder::build_dynamic_relation_exact_key_lookup(&descriptor, &attrs, "$r")
        .unwrap()
        .unwrap();
    let lookup_a = query_builder::build_dynamic_relation_player_lookup(
        "person",
        None,
        Some(("name", &AttributeValue::String("Alice".into()))),
        "$p",
    )
    .unwrap();
    let lookup_b =
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap();
    let update_q =
        query_builder::build_dynamic_relation_update_exact(&descriptor, "0x1", &attrs, "$r")
            .unwrap();
    let clear_a =
        query_builder::build_dynamic_relation_clear_role(&descriptor, "0x1", "employee", "$r")
            .unwrap();
    let clear_b =
        query_builder::build_dynamic_relation_clear_role(&descriptor, "0x1", "employer", "$r")
            .unwrap();
    let attach_q =
        query_builder::build_dynamic_relation_attach(&descriptor, "0x1", &resolved, "$r").unwrap();
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x1"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let got = DynamicRelationManager::new(&db, Arc::new(descriptor.clone()))
        .put_exact(&attrs, &players)
        .await
        .unwrap();
    assert_eq!(got, "0x1");
    let s = state.lock().unwrap();
    assert_eq!(
        s.queries,
        vec![
            key_q, lookup_a, lookup_b, update_q, clear_a, clear_b, attach_q
        ]
    );
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 7]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
    assert!(
        !s.queries.iter().any(|q| q.contains("put")
            || q.contains("delete\n$r")
            || q.contains("$r isa employment"))
    );
}

#[tokio::test]
async fn dynamic_relation_put_exact_single_key_answer_matrix_is_strict() {
    let descriptor = exact_put_builder_descriptor();
    let attrs = vec![
        ("title".into(), AttributeValue::String("Engineer".into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("external_id".into(), AttributeValue::String("emp-7".into())),
    ];
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let expected_query =
        query_builder::build_dynamic_relation_exact_key_lookup(&descriptor, &attrs, "$r")
            .unwrap()
            .unwrap();
    let rows = vec![
        (
            QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x1"}),
                serde_json::json!({"iid":"0x2"}),
            ]),
            "Expected at most one exact key identity, got 2",
        ),
        (
            QueryResult::Documents(vec![serde_json::Value::Null]),
            "Expected JSON object from exact key lookup",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({})]),
            "Exact key lookup omitted its IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid": 1})]),
            "Exact key lookup omitted its IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid": ""})]),
            "Exact key lookup returned a noncanonical IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"iid": "bad"})]),
            "Exact key lookup returned a noncanonical IID",
        ),
        (
            QueryResult::Ok,
            "Expected Documents from exact key lookup, got Ok",
        ),
        (
            QueryResult::Rows(vec![]),
            "Expected Documents from exact key lookup, got Rows",
        ),
    ];
    assert_eq!(rows.len(), 8);
    for (answer, expected) in rows {
        let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(answer)]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(descriptor.clone()))
            .put_exact(&attrs, &players)
            .await
            .unwrap_err();
        match err {
            OrmError::Hydration { type_name, message } => {
                assert_eq!(type_name, "employment");
                assert_eq!(message, expected);
            }
            other => panic!("expected hydration error, got {other:?}"),
        }
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.queries, vec![expected_query.clone()]);
        assert_eq!(s.query_modes, vec!["legacy"]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
    }
}

fn batch_attrs(id: &str, title: &str) -> DynamicAttributeMap {
    vec![
        ("external_id".into(), AttributeValue::String(id.into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("title".into(), AttributeValue::String(title.into())),
    ]
}
fn batch_players(employee: &str, employer: &str) -> Vec<DynamicRolePlayerInput> {
    vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some(employee.into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some(employer.into()),
            key: None,
        },
    ]
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_preflight_and_empty_are_zero_io() {
    let d = exact_put_builder_descriptor();
    let mut bad = batch_attrs("x", "bad");
    bad.retain(|(n, _)| n != "external_id");
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(d.clone()))
        .put_many_exact(&[
            (batch_attrs("x", "ok"), batch_players("0x2", "0x3")),
            (bad, batch_players("0x4", "0x5")),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::QueryExecution(m) if m == "employment: relation attribute employment-id violates minimum cardinality")
    );
    {
        let s = state.lock().unwrap();
        assert!(
            s.opens.is_empty()
                && s.queries.is_empty()
                && s.query_modes.is_empty()
                && s.commits == 0
                && s.rollbacks == 0
                && s.closes == 0
        );
    }
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    assert!(
        DynamicRelationManager::new(&db, Arc::new(d.clone()))
            .put_many_exact(&[])
            .await
            .unwrap()
            .is_empty()
    );
    {
        let s = state.lock().unwrap();
        assert!(
            s.opens.is_empty()
                && s.queries.is_empty()
                && s.query_modes.is_empty()
                && s.commits == 0
                && s.rollbacks == 0
                && s.closes == 0
        );
    }
    let (backend, state) = RecordingBackend::new(vec![]);
    let read_db = Database::with_backend(Box::new(backend), "testdb");
    let read_tx = read_db.transaction_context(TxType::Read).await.unwrap();
    let read_manager = DynamicRelationManager::with_transaction(read_tx.clone(), Arc::new(d));
    assert!(read_manager.put_many_exact(&[]).await.unwrap().is_empty());
    let err = read_manager
        .put_many_exact(&[(batch_attrs("x", "ok"), batch_players("0x2", "0x3"))])
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Transaction(m) if m == "Cannot execute Write operation in Read transaction")
    );
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Read]);
    assert!(
        s.queries.is_empty()
            && s.query_modes.is_empty()
            && s.commits == 0
            && s.rollbacks == 0
            && s.closes == 0
    );
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_mixed_hit_miss_preserves_order() {
    let d = exact_put_builder_descriptor();
    let attrs0 = batch_attrs("a", "A");
    let attrs1 = batch_attrs("b", "B");
    let p0 = batch_players("0x2", "0x3");
    let p1 = batch_players("0x4", "0x5");
    let r0: Vec<(String, String, String)> = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let r1: Vec<(String, String, String)> = vec![
        ("person".into(), "0x4".into(), "employee".into()),
        ("company".into(), "0x5".into(), "employer".into()),
    ];
    let qs = vec![
        query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs0, "$r")
            .unwrap()
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_update_exact(&d, "0x10", &attrs0, "$r").unwrap(),
        query_builder::build_dynamic_relation_clear_role(&d, "0x10", "employee", "$r").unwrap(),
        query_builder::build_dynamic_relation_clear_role(&d, "0x10", "employer", "$r").unwrap(),
        query_builder::build_dynamic_relation_attach(&d, "0x10", &r0, "$r").unwrap(),
        query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs1, "$r")
            .unwrap()
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x4"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x5"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(&d, &attrs1, &r1, "$r")
            .unwrap(),
    ];
    let mut responses = vec![RecordingResponse::Result(QueryResult::Documents(vec![
        serde_json::json!({"iid":"0x10"}),
    ]))];
    responses.extend((0..2).map(|i| {
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":if i==0{"0x2"}else{"0x3"}}),
        ]))
    }));
    responses.extend((0..4).map(|_| RecordingResponse::Result(QueryResult::Ok)));
    responses.push(RecordingResponse::Result(QueryResult::Documents(vec![])));
    responses.extend((0..2).map(|i| {
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":if i==0{"0x4"}else{"0x5"}}),
        ]))
    }));
    responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
        serde_json::json!({"iid":"0x20"}),
    ])));
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let got = DynamicRelationManager::new(&db, Arc::new(d))
        .put_many_exact(&[(attrs0, p0), (attrs1, p1)])
        .await
        .unwrap();
    assert_eq!(got, vec!["0x10", "0x20"]);
    let s = state.lock().unwrap();
    assert_eq!(s.queries, qs);
    assert!(!s.queries.iter().any(|q| q.contains("put")));
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 11]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_no_key_rows_are_lookup_free() {
    let d = employment_descriptor();
    let a0 = vec![("position".into(), AttributeValue::String("A".into()))];
    let a1 = vec![("position".into(), AttributeValue::String("B".into()))];
    let p0 = batch_players("0x2", "0x3");
    let p1 = batch_players("0x4", "0x5");
    let r0: Vec<(String, String, String)> = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let r1: Vec<(String, String, String)> = vec![
        ("person".into(), "0x4".into(), "employee".into()),
        ("company".into(), "0x5".into(), "employer".into()),
    ];
    let mut responses = Vec::new();
    for iid in ["0x2", "0x4"] {
        responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":iid}),
        ])));
        responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":if iid=="0x2"{"0x3"}else{"0x5"}}),
        ])));
        responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":if iid=="0x2"{"0x31"}else{"0x32"}}),
        ])));
    }
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let m = DynamicRelationManager::new(&db, Arc::new(d.clone()));
    assert!(
        query_builder::build_dynamic_relation_exact_key_lookup(&d, &a0, "$r")
            .unwrap()
            .is_none()
    );
    assert!(
        query_builder::build_dynamic_relation_exact_key_lookup(&d, &a1, "$r")
            .unwrap()
            .is_none()
    );
    let expected_queries = vec![
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(&d, &a0, &r0, "$r").unwrap(),
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x4"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x5"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(&d, &a1, &r1, "$r").unwrap(),
    ];
    let got = m.put_many_exact(&[(a0, p0), (a1, p1)]).await.unwrap();
    assert_eq!(got, vec!["0x31", "0x32"]);
    let s = state.lock().unwrap();
    assert_eq!(s.queries, expected_queries);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert!(!s.queries.iter().any(|q| q.contains("put")));
    assert_eq!(s.query_modes, vec!["legacy"; 6]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_later_row_failures_are_atomic_and_preserve_primary() {
    let d = exact_put_builder_descriptor();
    let cases = [
        ("batch row-1 key failure", 5usize),
        ("batch row-1 update failure", 8),
        ("batch row-1 insert failure", 8),
    ];
    for (failure, expected_count) in cases {
        let attrs0 = batch_attrs("a", "A");
        let attrs1 = batch_attrs("b", "B");
        let p0 = batch_players("0x2", "0x3");
        let p1 = batch_players("0x4", "0x5");
        let r0 = vec![
            ("person".into(), "0x2".into(), "employee".into()),
            ("company".into(), "0x3".into(), "employer".into()),
        ];
        let r1 = vec![
            ("person".into(), "0x4".into(), "employee".into()),
            ("company".into(), "0x5".into(), "employer".into()),
        ];
        let mut expected_queries = vec![
            query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs0, "$r")
                .unwrap()
                .unwrap(),
            query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
                .unwrap(),
            query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
                .unwrap(),
            query_builder::build_dynamic_relation_insert_resolved_with_iid(&d, &attrs0, &r0, "$r")
                .unwrap(),
        ];
        let mut responses = vec![
            RecordingResponse::Result(QueryResult::Documents(vec![])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x2"}),
            ])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x3"}),
            ])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x10"}),
            ])),
        ];
        if failure.contains("key") {
            expected_queries.push(
                query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs1, "$r")
                    .unwrap()
                    .unwrap(),
            );
            responses.push(RecordingResponse::Error(failure));
        } else {
            if failure.contains("update") {
                expected_queries.push(
                    query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs1, "$r")
                        .unwrap()
                        .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_player_lookup(
                        "person",
                        Some("0x4"),
                        None,
                        "$p",
                    )
                    .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_player_lookup(
                        "company",
                        Some("0x5"),
                        None,
                        "$p",
                    )
                    .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_update_exact(&d, "0x20", &attrs1, "$r")
                        .unwrap(),
                );
                responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
                    serde_json::json!({"iid":"0x20"}),
                ])));
            } else {
                expected_queries.push(
                    query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs1, "$r")
                        .unwrap()
                        .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_player_lookup(
                        "person",
                        Some("0x4"),
                        None,
                        "$p",
                    )
                    .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_player_lookup(
                        "company",
                        Some("0x5"),
                        None,
                        "$p",
                    )
                    .unwrap(),
                );
                expected_queries.push(
                    query_builder::build_dynamic_relation_insert_resolved_with_iid(
                        &d, &attrs1, &r1, "$r",
                    )
                    .unwrap(),
                );
                responses.push(RecordingResponse::Result(QueryResult::Documents(vec![])));
            }
            responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x4"}),
            ])));
            responses.push(RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x5"}),
            ])));
            responses.push(RecordingResponse::Error(failure));
        }
        let (backend, state) = if failure.contains("update") {
            RecordingBackend::with_failures(responses, true, false, false)
        } else {
            RecordingBackend::new(responses)
        };
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(d.clone()))
            .put_many_exact(&[(attrs0, p0), (attrs1, p1)])
            .await
            .unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(m) if m == failure));
        let s = state.lock().unwrap();
        assert_eq!(s.queries, expected_queries);
        assert_eq!(s.queries.len(), expected_count);
        assert!(!s.queries.iter().any(|q| q.contains("put")));
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.query_modes, vec!["legacy"; s.queries.len()]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
    }
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_commit_failure_is_not_rolled_back() {
    let d = employment_descriptor();
    let attrs = vec![("position".into(), AttributeValue::String("A".into()))];
    let players = batch_players("0x2", "0x3");
    let resolved = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x30"}),
        ])),
    ];
    let (backend, state) = RecordingBackend::with_failures(responses, true, true, false);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(d.clone()))
        .put_many_exact(&[(attrs.clone(), players)])
        .await
        .unwrap_err();
    assert!(matches!(err, OrmError::Transaction(m) if m == "commit failed"));
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.queries.len(), 3);
    assert_eq!(
        s.queries,
        vec![
            query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
                .unwrap(),
            query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
                .unwrap(),
            query_builder::build_dynamic_relation_insert_resolved_with_iid(
                &d, &attrs, &resolved, "$r"
            )
            .unwrap(),
        ]
    );
    assert!(!s.queries.iter().any(|q| q.contains("put")));
    assert_eq!(s.query_modes, vec!["legacy"; 3]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_canonical_database_preserves_mode() {
    let d = employment_descriptor();
    let attrs0 = vec![("position".into(), AttributeValue::String("A".into()))];
    let attrs1 = vec![("position".into(), AttributeValue::String("B".into()))];
    let p0 = batch_players("0x2", "0x3");
    let p1 = batch_players("0x4", "0x5");
    let resolved0: Vec<(String, String, String)> = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let resolved1: Vec<(String, String, String)> = vec![
        ("person".into(), "0x4".into(), "employee".into()),
        ("company".into(), "0x5".into(), "employer".into()),
    ];
    let responses = ["0x2", "0x3", "0x41", "0x4", "0x5", "0x42"]
        .into_iter()
        .map(|iid| {
            RecordingResponse::Result(QueryResult::Documents(vec![serde_json::json!({"iid":iid})]))
        })
        .collect();
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let got = DynamicRelationManager::new_canonical(&db, Arc::new(d.clone()))
        .put_many_exact(&[(attrs0.clone(), p0), (attrs1.clone(), p1)])
        .await
        .unwrap();
    assert_eq!(got, vec!["0x41", "0x42"]);
    let s = state.lock().unwrap();
    let expected = vec![
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(
            &d, &attrs0, &resolved0, "$r",
        )
        .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x4"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x5"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(
            &d, &attrs1, &resolved1, "$r",
        )
        .unwrap(),
    ];
    assert_eq!(s.queries, expected);
    assert_eq!(s.query_modes, vec!["canonical"; 6]);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
    assert!(!s.queries.iter().any(|q| q.contains("put")));
}

#[tokio::test]
async fn dynamic_relation_put_many_exact_caller_owned_contexts_preserve_ownership() {
    let d = employment_descriptor();
    let attrs0 = vec![("position".into(), AttributeValue::String("A".into()))];
    let attrs1 = vec![("position".into(), AttributeValue::String("B".into()))];
    let p0 = batch_players("0x2", "0x3");
    let p1 = batch_players("0x4", "0x5");
    let resolved0: Vec<(String, String, String)> = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
    ];
    let resolved1: Vec<(String, String, String)> = vec![
        ("person".into(), "0x4".into(), "employee".into()),
        ("company".into(), "0x5".into(), "employer".into()),
    ];
    let responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x51"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x4"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x5"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x52"}),
        ])),
    ];
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let m = DynamicRelationManager::with_transaction(tx.clone(), Arc::new(d.clone()));
    let got = m
        .put_many_exact(&[(attrs0.clone(), p0.clone()), (attrs1.clone(), p1.clone())])
        .await
        .unwrap();
    assert_eq!(got, vec!["0x51", "0x52"]);
    let expected_queries = vec![
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x2"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x3"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(
            &d, &attrs0, &resolved0, "$r",
        )
        .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("person", Some("0x4"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_player_lookup("company", Some("0x5"), None, "$p")
            .unwrap(),
        query_builder::build_dynamic_relation_insert_resolved_with_iid(
            &d, &attrs1, &resolved1, "$r",
        )
        .unwrap(),
    ];
    {
        let s = state.lock().unwrap();
        assert_eq!(s.queries, expected_queries);
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.query_modes, vec!["legacy"; 6]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
        assert!(!s.queries.iter().any(|q| q.contains("put")));
    }
    let responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x61"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x4"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x5"}),
        ])),
        RecordingResponse::Error("caller-owned canonical batch failure"),
    ];
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let m = DynamicRelationManager::with_canonical_transaction(tx.clone(), Arc::new(d.clone()));
    let err = m
        .put_many_exact(&[(attrs0.clone(), p0.clone()), (attrs1.clone(), p1.clone())])
        .await
        .unwrap_err();
    assert!(matches!(err,OrmError::QueryExecution(m) if m=="caller-owned canonical batch failure"));
    {
        let s = state.lock().unwrap();
        assert_eq!(s.queries, expected_queries);
        assert!(!s.queries.iter().any(|q| q.contains("put")));
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.query_modes, vec!["canonical"; 6]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
    }
    tx.rollback().await.unwrap();
    {
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
    }
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    let m = DynamicRelationManager::with_transaction(tx, Arc::new(d));
    let err = m.put_many_exact(&[(attrs0, p0)]).await.unwrap_err();
    assert!(
        matches!(err,OrmError::Transaction(m) if m=="Cannot execute Write operation in Read transaction")
    );
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Read]);
    assert!(
        s.queries.is_empty()
            && s.query_modes.is_empty()
            && s.commits == 0
            && s.rollbacks == 0
            && s.closes == 0
    );
}

#[tokio::test]
async fn dynamic_relation_update_exact_owned_failure_table_rolls_back_once() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let stages = [
        ("attribute", "primary attribute failure"),
        ("employee clear", "primary employee clear failure"),
        ("employer clear", "primary employer clear failure"),
        ("attach", "primary attach failure"),
    ];
    assert_eq!(stages.len(), 4);
    for (fail_index, (stage, message)) in stages.iter().enumerate() {
        let mut responses = vec![
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x2"}),
            ])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x3"}),
            ])),
        ];
        for i in 2..6 {
            responses.push(if i == fail_index + 2 {
                RecordingResponse::Error(message)
            } else {
                RecordingResponse::Result(QueryResult::Ok)
            });
        }
        let (backend, state) = RecordingBackend::new(responses);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let err = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()))
            .update_exact(
                "0x1",
                &vec![("position".into(), AttributeValue::String("x".into()))],
                &players,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(m) if m == *message));
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.queries.len(), fail_index + 3);
        assert_eq!(s.query_modes, vec!["legacy"; fail_index + 3]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 1);
        assert_eq!(s.closes, 0);
        assert!(s.queries[0].contains("person") && s.queries[1].contains("company"));
        assert!(s.queries[fail_index + 2].contains(match *stage {
            "attribute" => "position",
            "employee clear" => "employee",
            "employer clear" => "employer",
            _ => "$r links",
        }));
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_owned_rollback_failure_preserves_primary() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let mut responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
    ];
    responses.extend((0..3).map(|_| RecordingResponse::Result(QueryResult::Ok)));
    responses.push(RecordingResponse::Error("primary attach failure"));
    let (backend, state) = RecordingBackend::with_rollback_error(responses, true);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()))
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await
        .unwrap_err();
    assert!(matches!(err,OrmError::QueryExecution(m) if m=="primary attach failure"));
    let s = state.lock().unwrap();
    assert_eq!(s.queries.len(), 6);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 6]);
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_update_exact_owned_commit_failure_is_not_rolled_back() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let mut responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
    ];
    responses.extend((0..4).map(|_| RecordingResponse::Result(QueryResult::Ok)));
    let (backend, state) = RecordingBackend::with_failures(responses, true, true, false);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let err = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()))
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await
        .unwrap_err();
    assert!(matches!(err,OrmError::Transaction(m) if m=="commit failed"));
    let s = state.lock().unwrap();
    assert_eq!(s.queries.len(), 6);
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.query_modes, vec!["legacy"; 6]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

fn exact_put_builder_descriptor() -> RelationDescriptor {
    let attr = |field: &str,
                name: &str,
                ty: ValueType,
                key: bool,
                optional: bool,
                card: Option<(u32, Option<u32>)>| OwnedAttributeDescriptor {
        field_name: field.into(),
        attr_name: name.into(),
        value_type: ty,
        annotations: {
            let mut a = Vec::new();
            if key {
                a.push(Annotation::Key);
            }
            if let Some((m, x)) = card {
                a.push(Annotation::Card(m, x));
            }
            a
        },
        is_optional: optional,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    };
    let role = |name: &str, ty: &str, card| RoleDescriptor {
        role_name: name.into(),
        player_type_names: vec![ty.into()],
        cardinality: card,
        overrides: None,
        is_abstract: false,
        ordered: false,
        distinct: false,
        plays_cardinality: None,
        doc: None,
        meta: Default::default(),
    };
    RelationDescriptor {
        type_name: "employment".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            attr(
                "external_id",
                "employment-id",
                ValueType::String,
                true,
                false,
                None,
            ),
            attr(
                "revision",
                "employment-revision",
                ValueType::Long,
                true,
                false,
                None,
            ),
            attr(
                "title",
                "position",
                ValueType::String,
                false,
                false,
                Some((1, None)),
            ),
        ],
        roles: vec![
            role("employee", "person", Some((1, None))),
            role("employer", "company", Some((1, Some(1)))),
        ],
        doc: None,
        meta: Default::default(),
    }
}

#[test]
fn dynamic_relation_exact_put_builder_key_lookup_inventory() {
    let d = exact_put_builder_descriptor();
    let attrs = vec![
        ("title".into(), AttributeValue::String("Engineer".into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("external_id".into(), AttributeValue::String("emp-7".into())),
        ("title".into(), AttributeValue::String("Reviewer".into())),
    ];
    let q = query_builder::build_dynamic_relation_exact_key_lookup(&d, &attrs, "$r")
        .unwrap()
        .unwrap();
    assert_eq!(q.matches("has employment-id \"emp-7\"").count(), 1);
    assert_eq!(q.matches("has employment-revision 7").count(), 1);
    assert!(q.find("has employment-id").unwrap() < q.find("has employment-revision").unwrap());
    assert!(q.contains("$r isa! employment") && !q.contains("$r isa employment"));
    assert_eq!(q.matches("\"iid\": iid($r)").count(), 1);
    for forbidden in [
        "position",
        "Engineer",
        "Reviewer",
        "put",
        "links",
        "employee",
        "employer",
        "$p",
        "$rp",
        ".*",
        "label(",
        "\"_type\"",
        "\"_iid\"",
    ] {
        assert!(!q.contains(forbidden));
    }
    assert!(
        query_builder::build_dynamic_relation_exact_key_lookup(
            &d,
            &vec![
                ("title".into(), AttributeValue::String("Engineer".into())),
                ("title".into(), AttributeValue::String("Reviewer".into()))
            ],
            "$r"
        )
        .unwrap()
        .is_none()
    );
    assert!(!q.contains("put") && !q.contains("links") && !q.contains(".*"));
    assert!(
        query_builder::build_dynamic_relation_exact_key_lookup(&d, &Vec::new(), "$r")
            .unwrap()
            .is_none()
    );
    let mut guards: Vec<(RelationDescriptor, DynamicAttributeMap, &'static str)> = Vec::new();
    let mut bad = d.clone();
    bad.type_name = "bad type".into();
    guards.push((bad, Vec::new(), "bad type: unsafe relation type label"));
    let mut bad = d.clone();
    bad.owned_attributes[0].attr_name = "bad key".into();
    guards.push((
        bad,
        Vec::new(),
        "employment: unsafe relation attribute label bad key",
    ));
    guards.push((
        d.clone(),
        vec![("ghost".into(), AttributeValue::String("x".into()))],
        "employment: unknown relation attribute ghost",
    ));
    let mut amb = d.clone();
    amb.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "employment-id".into(),
        attr_name: "shadow-id".into(),
        value_type: ValueType::String,
        is_optional: true,
        annotations: vec![],
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    guards.push((
        amb,
        vec![("employment-id".into(), AttributeValue::String("x".into()))],
        "employment: ambiguous relation attribute employment-id",
    ));
    guards.push((
        d.clone(),
        vec![
            ("external_id".into(), AttributeValue::String("a".into())),
            ("external_id".into(), AttributeValue::String("b".into())),
        ],
        "employment: relation attribute employment-id violates maximum cardinality",
    ));
    guards.push((
        d.clone(),
        vec![("external_id".into(), AttributeValue::Long(1))],
        "employment: relation attribute employment-id has wrong value type",
    ));
    guards.push((
        d.clone(),
        vec![("title".into(), AttributeValue::Long(1))],
        "employment: relation attribute position has wrong value type",
    ));
    assert_eq!(guards.len(), 7);
    for (descriptor, input, expected) in guards {
        match query_builder::build_dynamic_relation_exact_key_lookup(&descriptor, &input, "$r")
            .unwrap_err()
        {
            OrmError::QueryExecution(actual) => assert_eq!(actual, expected),
            other => panic!("expected QueryExecution({expected:?}), got {other:?}"),
        }
    }
}

#[test]
fn dynamic_relation_exact_put_builder_resolved_insert_inventory() {
    let d = exact_put_builder_descriptor();
    let attrs = vec![
        ("title".into(), AttributeValue::String("Engineer".into())),
        ("revision".into(), AttributeValue::Long(7)),
        ("external_id".into(), AttributeValue::String("emp-7".into())),
        ("title".into(), AttributeValue::String("Reviewer".into())),
    ];
    let resolved = vec![
        ("person".into(), "0x2".into(), "employee".into()),
        ("company".into(), "0x3".into(), "employer".into()),
        ("contractor".into(), "0x4".into(), "employee".into()),
    ];
    let q =
        query_builder::build_dynamic_relation_insert_resolved_with_iid(&d, &attrs, &resolved, "$r")
            .unwrap();
    for fragment in [
        "$p0 isa! person, iid 0x2",
        "$p1 isa! company, iid 0x3",
        "$p2 isa! contractor, iid 0x4",
    ] {
        assert_eq!(q.matches(fragment).count(), 1);
    }
    let prefix = q.split_once("\ninsert\n").expect("insert boundary").0;
    assert!(!prefix.contains("$r"));
    assert_eq!(
        q.matches("$r isa employment, links (employee: $p0, employer: $p1, employee: $p2)")
            .count(),
        1
    );
    assert!(!q.contains("$r isa! employment"));
    for fragment in ["$p0 isa person", "$p1 isa company", "$p2 isa contractor"] {
        assert!(!q.contains(fragment));
    }
    let owned = [
        "has employment-id \"emp-7\"",
        "has employment-revision 7",
        "has position \"Engineer\"",
        "has position \"Reviewer\"",
    ];
    let mut last = 0;
    for fragment in owned {
        assert_eq!(q.matches(fragment).count(), 1);
        let pos = q.find(fragment).unwrap();
        assert!(pos >= last);
        last = pos;
    }
    assert!(q.ends_with("fetch {\n  \"iid\": iid($r)\n};"));
    for forbidden in ["put", ".*", "label(", "\"_type\"", "\"_iid\"", "\"_role\""] {
        assert!(!q.contains(forbidden));
    }
    let p0 = q.find("$p0 isa! person, iid 0x2").unwrap();
    let p1 = q.find("$p1 isa! company, iid 0x3").unwrap();
    let p2 = q.find("$p2 isa! contractor, iid 0x4").unwrap();
    assert!(p0 < p1 && p1 < p2);
    let va = || attrs.clone();
    let vr = || resolved.clone();
    let mut guards = Vec::new();
    let mut x = d.clone();
    x.type_name = "bad type".into();
    guards.push((x, va(), vr(), "bad type: unsafe relation type label"));
    let mut x = d.clone();
    x.is_abstract = true;
    guards.push((
        x,
        va(),
        vr(),
        "employment: exact relation insert requires a concrete descriptor",
    ));
    let mut x = d.clone();
    x.owned_attributes[0].attr_name = "bad attr".into();
    guards.push((
        x,
        va(),
        vr(),
        "employment: unsafe relation attribute label bad attr",
    ));
    let mut x = d.clone();
    x.roles[0].role_name = "bad role".into();
    guards.push((
        x,
        va(),
        vr(),
        "employment: unsafe relation role label bad role",
    ));
    guards.push((
        d.clone(),
        va(),
        Vec::new(),
        "employment: exact relation insert requires at least one role player",
    ));
    let mut r = vr();
    r[0].0 = "bad type".into();
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: unsafe resolved player type label bad type",
    ));
    let mut r = vr();
    r[0].1 = "not-an-iid".into();
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: resolved player IID must be canonical",
    ));
    let mut r = vr();
    r[0].2 = "ghost".into();
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: unknown relation role ghost",
    ));
    let mut r = vr();
    r.push(("contractor".into(), "0x2".into(), "employee".into()));
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: duplicate resolved relation player",
    ));
    let mut r = vr();
    r.retain(|(_, _, role)| role != "employer");
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: relation role employer violates cardinality",
    ));
    let mut r = vr();
    r.push(("company".into(), "0x5".into(), "employer".into()));
    guards.push((
        d.clone(),
        va(),
        r,
        "employment: relation role employer violates cardinality",
    ));
    let mut x = d.clone();
    x.roles[0].ordered = true;
    guards.push((
        x,
        va(),
        vr(),
        "employment: ordered relation role employee cannot contain multiple players",
    ));
    let mut a = va();
    a.retain(|(n, _)| n != "external_id");
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: relation attribute employment-id violates minimum cardinality",
    ));
    let mut a = va();
    a.retain(|(n, _)| n != "title");
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: relation attribute position violates minimum cardinality",
    ));
    let mut a = va();
    a.push(("ghost".into(), AttributeValue::String("x".into())));
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: unknown relation attribute ghost",
    ));
    let mut x = d.clone();
    x.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "employment-id".into(),
        attr_name: "shadow-id".into(),
        value_type: ValueType::String,
        is_optional: true,
        annotations: vec![],
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    let mut a = va();
    if let Some(v) = a.iter_mut().find(|(n, _)| n == "external_id") {
        v.0 = "employment-id".into();
    }
    guards.push((
        x,
        a,
        vr(),
        "employment: ambiguous relation attribute employment-id",
    ));
    let mut a = va();
    a.push(("external_id".into(), AttributeValue::String("x".into())));
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: relation attribute employment-id violates maximum cardinality",
    ));
    let mut a = va();
    if let Some(v) = a.iter_mut().find(|(n, _)| n == "external_id") {
        v.1 = AttributeValue::Long(9);
    }
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: relation attribute employment-id has wrong value type",
    ));
    let mut a = va();
    if let Some(v) = a.iter_mut().find(|(n, _)| n == "title") {
        v.1 = AttributeValue::Long(9);
    }
    guards.push((
        d.clone(),
        a,
        vr(),
        "employment: relation attribute position has wrong value type",
    ));
    assert_eq!(guards.len(), 19);
    for (descriptor, attrs, resolved, expected) in guards {
        match query_builder::build_dynamic_relation_insert_resolved_with_iid(
            &descriptor,
            &attrs,
            &resolved,
            "$r",
        )
        .unwrap_err()
        {
            OrmError::QueryExecution(actual) => assert_eq!(actual, expected),
            other => panic!("expected QueryExecution({expected:?}), got {other:?}"),
        }
    }
}

#[tokio::test]
async fn dynamic_relation_update_exact_mode_and_transaction_canonical_database_success() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let mut responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
    ];
    responses.extend((0..4).map(|_| RecordingResponse::Result(QueryResult::Ok)));
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    DynamicRelationManager::new_canonical(&db, Arc::new(employment_descriptor()))
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.queries.len(), 6);
    assert_eq!(s.query_modes, vec!["canonical"; 6]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
    assert!(
        s.queries[0].contains("$p isa! person, iid 0x2;")
            && s.queries[0].contains("\"iid\": iid($p)")
    );
    assert!(
        s.queries[1].contains("$p isa! company, iid 0x3;")
            && s.queries[1].contains("\"iid\": iid($p)")
    );
    assert!(
        s.queries[2].contains("$r isa! employment, iid 0x1;") && s.queries[2].contains("position")
    );
    assert!(
        s.queries[3].contains("$r links (employee: $old);")
            && s.queries[3].contains("delete\nlinks (employee: $old) of $r;")
            && !s.queries[3].contains(";;")
    );
    assert!(
        s.queries[4].contains("$r links (employer: $old);")
            && s.queries[4].contains("delete\nlinks (employer: $old) of $r;")
            && !s.queries[4].contains(";;")
    );
    assert!(
        s.queries[5].contains("$r isa! employment, iid 0x1;")
            && s.queries[5].contains("$p0 isa! person, iid 0x2;")
            && s.queries[5].contains("$p1 isa! company, iid 0x3;")
    );
    let attach_tail = s.queries[5]
        .split_once("insert\n")
        .map(|(_, t)| t)
        .unwrap_or("");
    assert!(
        attach_tail.contains("$r links (employee: $p0, employer: $p1);")
            && !attach_tail.contains("$r isa")
    );
}

#[tokio::test]
async fn dynamic_relation_update_exact_mode_and_transaction_legacy_write_reuses_context() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let mut responses = vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x2"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":"0x3"}),
        ])),
    ];
    responses.extend((0..4).map(|_| RecordingResponse::Result(QueryResult::Ok)));
    let (backend, state) = RecordingBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    DynamicRelationManager::with_transaction(tx.clone(), Arc::new(employment_descriptor()))
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.opens, vec![TxType::Write]);
    assert_eq!(s.queries.len(), 6);
    assert_eq!(s.query_modes, vec!["legacy"; 6]);
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_update_exact_mode_and_transaction_canonical_write_success_and_failure() {
    let players = vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0x3".into()),
            key: None,
        },
    ];
    let cases = [false, true];
    assert_eq!(cases.len(), 2);
    for fail in cases {
        let mut responses = vec![
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x2"}),
            ])),
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid":"0x3"}),
            ])),
        ];
        responses.push(if fail {
            RecordingResponse::Error("caller-owned canonical failure")
        } else {
            RecordingResponse::Result(QueryResult::Ok)
        });
        responses.extend((0..3).map(|_| RecordingResponse::Result(QueryResult::Ok)));
        let (backend, state) = RecordingBackend::new(responses);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let tx = db.transaction_context(TxType::Write).await.unwrap();
        let result = DynamicRelationManager::with_canonical_transaction(
            tx.clone(),
            Arc::new(employment_descriptor()),
        )
        .update_exact(
            "0x1",
            &vec![("position".into(), AttributeValue::String("x".into()))],
            &players,
        )
        .await;
        if fail {
            assert!(
                matches!(result,Err(OrmError::QueryExecution(m)) if m=="caller-owned canonical failure")
            );
        } else {
            result.unwrap();
        }
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.queries.len(), if fail { 3 } else { 6 });
        assert_eq!(s.query_modes, vec!["canonical"; if fail { 3 } else { 6 }]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert_eq!(s.closes, 0);
        if fail {
            assert!(
                s.queries[2].contains("$r isa! employment, iid 0x1;")
                    && s.queries[2].contains("position")
            );
        }
    }
}

#[tokio::test]
async fn dynamic_relation_identity_discovery_rejects_wrong_and_duplicate_iids() {
    let (backend, _) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x2","_type":"employment"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    assert!(
        matches!(manager.discover_by_iid("0x1").await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Relation identity discovery returned the wrong IID")
    );
    assert!(
        matches!(manager.discover_by_iid("0x1").await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Expected 0 or 1 relation identity for IID lookup, got 2")
    );
}

#[tokio::test]
async fn dynamic_relation_exact_preflight_is_zero_io() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    for result in [
        manager.discover_by_iid("bad").await.map(|_| ()),
        manager.delete_by_iid_exact("bad").await,
    ] {
        assert!(
            matches!(result, Err(OrmError::QueryExecution(message)) if message == "Exact relation operation for employment requires a canonical TypeDB IID")
        );
    }
    let s = state.lock().unwrap();
    assert!(s.opens.is_empty() && s.queries.is_empty() && s.query_modes.is_empty());
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 0);
    assert_eq!(s.closes, 0);
}

#[tokio::test]
async fn dynamic_relation_new_methods_use_legacy_database_lifecycle() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
        RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 7})])),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    assert_eq!(
        manager.discover_all().await.unwrap(),
        vec![DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        }]
    );
    assert_eq!(
        manager.discover_by_iid("0x1").await.unwrap(),
        Some(DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        })
    );
    assert_eq!(manager.count_exact().await.unwrap(), 7);
    manager.delete_by_iid_exact("0x1").await.unwrap();
    let s = state.lock().unwrap();
    assert_eq!(
        s.opens,
        vec![TxType::Read, TxType::Read, TxType::Read, TxType::Write]
    );
    assert_eq!(s.query_modes, vec!["legacy", "legacy", "legacy", "legacy"]);
    assert_eq!(s.commits, 1);
    assert_eq!(s.rollbacks, 0);
    assert!(s.queries[0].contains("isa! $t") && s.queries[0].contains("_iid"));
    assert!(s.queries[1].contains("isa! $t") && s.queries[1].contains("iid 0x1"));
    assert!(
        s.queries[2].contains("isa! employment") && s.queries[2].contains("$count = count($r)")
    );
    assert!(
        s.queries[3].contains("isa! employment")
            && s.queries[3].contains("iid 0x1")
            && s.queries[3].contains("delete")
    );
}

#[tokio::test]
async fn dynamic_relation_new_methods_reuse_canonical_write_transaction() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
        RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 7})])),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicRelationManager::with_canonical_transaction(
        tx.clone(),
        Arc::new(employment_descriptor()),
    );
    assert_eq!(
        manager.discover_all().await.unwrap(),
        vec![DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        }]
    );
    assert_eq!(
        manager.discover_by_iid("0x1").await.unwrap(),
        Some(DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        })
    );
    assert_eq!(manager.count_exact().await.unwrap(), 7);
    manager.delete_by_iid_exact("0x1").await.unwrap();
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.query_modes,
            vec!["canonical", "canonical", "canonical", "canonical"]
        );
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
        assert!(s.queries[0].contains("isa! $t") && s.queries[1].contains("iid 0x1"));
        assert!(s.queries[2].contains("$count = count($r)") && s.queries[3].contains("delete"));
        assert!(s.queries[0].contains("isa! $t"));
        assert!(s.queries[1].contains("isa! $t") && s.queries[1].contains("iid 0x1"));
        assert!(
            s.queries[2].contains("isa! employment") && s.queries[2].contains("$count = count($r)")
        );
        assert!(
            s.queries[3].contains("isa! employment")
                && s.queries[3].contains("iid 0x1")
                && s.queries[3].contains("delete")
        );
    }
    tx.rollback().await.unwrap();
    assert_eq!(state.lock().unwrap().rollbacks, 1);
}

#[test]
fn dynamic_relation_identity_hydration_accepts_exact_direct_and_wrapped() {
    use type_bridge_orm::_manager::hydration::hydrate_dynamic_relation_identity;
    let direct = serde_json::json!({"_iid":"0x1","_type":"employment"});
    let wrapped = serde_json::json!({"_iid":{"value":"0x2"},"_type":{"value":"contract"}});
    assert_eq!(
        hydrate_dynamic_relation_identity("employment", &direct).unwrap(),
        DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        }
    );
    assert_eq!(
        hydrate_dynamic_relation_identity("employment", &wrapped).unwrap(),
        DynamicRelationIdentity {
            iid: "0x2".into(),
            type_name: "contract".into()
        }
    );
}

#[test]
fn dynamic_relation_identity_hydration_rejects_exact_malformed_matrix() {
    use type_bridge_orm::_manager::hydration::hydrate_dynamic_relation_identity;
    let cases = [
        (
            serde_json::json!(null),
            "Expected JSON object for relation identity discovery",
        ),
        (
            serde_json::json!({}),
            "Relation identity discovery omitted its IID",
        ),
        (
            serde_json::json!({"_iid":"", "_type":"employment"}),
            "Relation identity discovery returned a blank IID",
        ),
        (
            serde_json::json!({"_iid":1, "_type":"employment"}),
            "Relation identity discovery returned a nonstring IID",
        ),
        (
            serde_json::json!({"_iid":{"value":1}, "_type":"employment"}),
            "Relation identity discovery returned a nonstring IID",
        ),
        (
            serde_json::json!({"_iid":"0x1"}),
            "Relation identity discovery omitted its concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":""}),
            "Relation identity discovery returned a blank concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":1}),
            "Relation identity discovery returned a nonstring concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":{"value":1}}),
            "Relation identity discovery returned a nonstring concrete type",
        ),
        (
            serde_json::json!({"_iid":"bad", "_type":"employment"}),
            "Relation identity discovery returned a noncanonical IID",
        ),
    ];
    for (doc, message) in cases {
        assert!(
            matches!(hydrate_dynamic_relation_identity("employment", &doc), Err(OrmError::Hydration { type_name, message: actual }) if type_name == "employment" && actual == message)
        );
    }
}

#[tokio::test]
async fn dynamic_relation_identity_discovery_preserves_exact_order_and_cardinality() {
    let docs = vec![
        serde_json::json!({"_iid":"0x2","_type":"employment"}),
        serde_json::json!({"_iid":"0x1","_type":"contract"}),
    ];
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(docs)),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"employment"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    assert_eq!(
        manager.discover_all().await.unwrap(),
        vec![
            DynamicRelationIdentity {
                iid: "0x2".into(),
                type_name: "employment".into()
            },
            DynamicRelationIdentity {
                iid: "0x1".into(),
                type_name: "contract".into()
            }
        ]
    );
    assert_eq!(manager.discover_by_iid("0x9").await.unwrap(), None);
    let found = manager.discover_by_iid("0x1").await.unwrap();
    assert_eq!(
        found,
        Some(DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        })
    );
    assert_eq!(
        state.lock().unwrap().query_modes,
        vec!["legacy", "legacy", "legacy"]
    );
}

#[tokio::test]
async fn dynamic_relation_identity_discovery_rejects_answer_kinds_on_both_routes() {
    let (backend, _) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Rows(vec![])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Rows(vec![])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    assert!(
        matches!(manager.discover_all().await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Expected Documents from relation identity discovery, got Ok")
    );
    assert!(
        matches!(manager.discover_all().await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Expected Documents from relation identity discovery, got Rows")
    );
    assert!(
        matches!(manager.discover_by_iid("0x1").await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Expected Documents from relation identity discovery, got Ok")
    );
    assert!(
        matches!(manager.discover_by_iid("0x1").await, Err(OrmError::Hydration { type_name, message }) if type_name == "employment" && message == "Expected Documents from relation identity discovery, got Rows")
    );
}

enum RecordingResponse {
    Result(QueryResult),
    Error(&'static str),
}

struct RecordingBackend {
    responses: Arc<Mutex<VecDeque<RecordingResponse>>>,
    state: Arc<Mutex<RecordingState>>,
    rollback_error: bool,
    commit_error: bool,
    close_error: bool,
}

impl RecordingBackend {
    fn new(responses: Vec<RecordingResponse>) -> (Self, Arc<Mutex<RecordingState>>) {
        Self::with_rollback_error(responses, false)
    }

    fn with_rollback_error(
        responses: Vec<RecordingResponse>,
        rollback_error: bool,
    ) -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                rollback_error,
                commit_error: false,
                close_error: false,
            },
            state,
        )
    }

    fn with_failures(
        responses: Vec<RecordingResponse>,
        rollback_error: bool,
        commit_error: bool,
        close_error: bool,
    ) -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                rollback_error,
                commit_error,
                close_error,
            },
            state,
        )
    }
}

impl DriverBackend for RecordingBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opens.push(tx_type);
        let transaction = RecordingTransaction {
            responses: Arc::clone(&self.responses),
            state: Arc::clone(&self.state),
            rollback_error: self.rollback_error,
            commit_error: self.commit_error,
            close_error: self.close_error,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct RecordingTransaction {
    responses: Arc<Mutex<VecDeque<RecordingResponse>>>,
    state: Arc<Mutex<RecordingState>>,
    rollback_error: bool,
    commit_error: bool,
    close_error: bool,
}

impl TransactionOps for RecordingTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.state.lock().unwrap().query_modes.push("legacy");
        self.state.lock().unwrap().queries.push(typeql.to_string());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecordingResponse::Result(QueryResult::Ok));
        Box::pin(async move {
            match response {
                RecordingResponse::Result(result) => Ok(result),
                RecordingResponse::Error(message) => {
                    Err(OrmError::QueryExecution(message.to_string()))
                }
            }
        })
    }

    fn query_canonical(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.state.lock().unwrap().query_modes.push("canonical");
        self.state.lock().unwrap().queries.push(typeql.to_string());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecordingResponse::Result(QueryResult::Ok));
        Box::pin(async move {
            match response {
                RecordingResponse::Result(result) => Ok(result),
                RecordingResponse::Error(message) => {
                    Err(OrmError::QueryExecution(message.to_string()))
                }
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().commits += 1;
        let fail = self.commit_error;
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction("commit failed".into()))
            } else {
                Ok(())
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().rollbacks += 1;
        let rollback_error = self.rollback_error;
        Box::pin(async move {
            if rollback_error {
                Err(OrmError::Transaction("recording rollback failed".into()))
            } else {
                Ok(())
            }
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().closes += 1;
        let fail = self.close_error;
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction("close failed".into()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn dynamic_entity_legacy_and_canonical_selection_are_distinct() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let descriptor = Arc::new(person_descriptor());
    DynamicEntityManager::new(&db, Arc::clone(&descriptor))
        .all()
        .await
        .unwrap();
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    DynamicEntityManager::with_canonical_transaction(tx, descriptor)
        .all()
        .await
        .unwrap();
    assert_eq!(
        state.lock().unwrap().query_modes,
        vec!["legacy", "canonical"]
    );
}

#[tokio::test]
async fn dynamic_relation_canonical_transaction_read_keeps_canonical_mode() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    DynamicRelationManager::with_canonical_transaction(tx, Arc::new(employment_descriptor()))
        .all()
        .await
        .unwrap();
    assert_eq!(state.lock().unwrap().query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn dynamic_entity_canonical_database_read_close_failure_is_reported() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Result(QueryResult::Documents(vec![]))],
        false,
        false,
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()));
    let error = manager.all().await.unwrap_err();
    assert!(matches!(error, OrmError::Transaction(message) if message == "close failed"));
    assert_eq!(state.lock().unwrap().query_modes, vec!["canonical"]);
}

fn assert_canonical_mode_and_counts(
    state: &Arc<Mutex<RecordingState>>,
    opens: &[TxType],
    commits: usize,
    rollbacks: usize,
    closes: usize,
) {
    let s = state.lock().unwrap();
    assert_eq!(s.opens, opens);
    assert_eq!(s.query_modes, vec!["canonical"]);
    assert_eq!(s.commits, commits);
    assert_eq!(s.rollbacks, rollbacks);
    assert_eq!(s.closes, closes);
}

#[tokio::test]
async fn dynamic_canonical_database_read_success_has_one_close() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Result(QueryResult::Documents(vec![]))],
        false,
        false,
        false,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .all()
        .await
        .unwrap();
    assert_canonical_mode_and_counts(&state, &[TxType::Read], 0, 0, 1);
}

#[tokio::test]
async fn dynamic_canonical_database_read_error_close_failure_keeps_query_error() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Error("read failed")],
        false,
        false,
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let error = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .all()
        .await
        .unwrap_err();
    assert!(matches!(error, OrmError::QueryExecution(message) if message == "read failed"));
    assert_canonical_mode_and_counts(&state, &[TxType::Read], 0, 0, 1);
}

#[tokio::test]
async fn dynamic_canonical_database_write_success_commits_and_closes() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({
                "iid": {"value": "0x1"}
            }),
        ]))],
        false,
        false,
        false,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .insert(&person_attrs())
        .await
        .unwrap();
    assert_canonical_mode_and_counts(&state, &[TxType::Write], 1, 0, 1);
}

#[tokio::test]
async fn dynamic_canonical_database_write_error_cleanup_keeps_query_error() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Error("write failed")],
        true,
        false,
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let error = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .insert(&person_attrs())
        .await
        .unwrap_err();
    assert!(matches!(error, OrmError::QueryExecution(message) if message == "write failed"));
    assert_canonical_mode_and_counts(&state, &[TxType::Write], 0, 1, 1);
}

#[tokio::test]
async fn dynamic_canonical_database_commit_error_close_failure_keeps_commit_error() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({
                "iid": {"value": "0x1"}
            }),
        ]))],
        false,
        true,
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let error = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .insert(&person_attrs())
        .await
        .unwrap_err();
    assert!(matches!(error, OrmError::Transaction(message) if message == "commit failed"));
    assert_canonical_mode_and_counts(&state, &[TxType::Write], 1, 0, 1);
}

#[tokio::test]
async fn dynamic_canonical_database_successful_write_close_failure_is_reported() {
    let (backend, state) = RecordingBackend::with_failures(
        vec![RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({
                "iid": {"value": "0x1"}
            }),
        ]))],
        false,
        false,
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let error = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()))
        .insert(&person_attrs())
        .await
        .unwrap_err();
    assert!(matches!(error, OrmError::Transaction(message) if message == "close failed"));
    assert_canonical_mode_and_counts(&state, &[TxType::Write], 1, 0, 1);
}

#[tokio::test]
async fn dynamic_entity_canonical_batch_keeps_nested_queries_canonical() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x1"}}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x1"}}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new_canonical(&db, Arc::new(person_descriptor()));
    manager
        .insert_many(&[person_attrs(), person_attrs()])
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.query_modes, vec!["canonical", "canonical"]);
    assert_eq!(s.commits, 1);
}

#[tokio::test]
async fn dynamic_relation_canonical_insert_many_keeps_nested_queries_canonical() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x1"}}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x2"}}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new_canonical(&db, Arc::new(employment_descriptor()));
    let attrs = employment_attrs("Engineer");
    let roles = employment_role_players();
    manager
        .insert_many(&[(attrs.clone(), roles.clone()), (attrs, roles)])
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.query_modes, vec!["canonical", "canonical"]);
    assert_eq!(s.commits, 1);
}

#[tokio::test]
async fn dynamic_relation_legacy_insert_many_keeps_nested_queries_legacy() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x1"}}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid":{"value":"0x2"}}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, Arc::new(employment_descriptor()));
    let attrs = employment_attrs("Engineer");
    let roles = employment_role_players();
    manager
        .insert_many(&[(attrs.clone(), roles.clone()), (attrs, roles)])
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.query_modes, vec!["legacy", "legacy"]);
    assert_eq!(s.commits, 1);
}

#[tokio::test]
async fn dynamic_entity_transaction_insert_rejects_zero_iid_documents() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), Arc::new(person_descriptor()));
    let err = manager.insert(&person_attrs()).await.unwrap_err();
    let OrmError::Hydration { type_name, message } = err else {
        panic!("wrong error")
    };
    assert_eq!(type_name, "person");
    assert_eq!(message, "Insert returned 0 documents; expected exactly one");
    {
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(
            s.queries,
            vec![
                query_builder::build_dynamic_entity_insert_with_iid(
                    &person_descriptor(),
                    &person_attrs(),
                    "$e"
                )
                .unwrap()
            ]
        );
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
    }
    tx.rollback().await.unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_transaction_insert_rejects_multiple_iid_documents() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![
            serde_json::json!({"iid":"0xa1"}),
            serde_json::json!({"iid":"0xa2"}),
        ]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), Arc::new(person_descriptor()));
    let err = manager.insert(&person_attrs()).await.unwrap_err();
    let OrmError::Hydration { type_name, message } = err else {
        panic!("wrong error")
    };
    assert_eq!(type_name, "person");
    assert_eq!(message, "Insert returned 2 documents; expected exactly one");
    {
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(
            s.queries,
            vec![
                query_builder::build_dynamic_entity_insert_with_iid(
                    &person_descriptor(),
                    &person_attrs(),
                    "$e"
                )
                .unwrap()
            ]
        );
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
    }
    tx.rollback().await.unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_transaction_insert_rejects_noncanonical_iid() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![serde_json::json!({"iid":"not-an-iid"})]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), Arc::new(person_descriptor()));
    let err = manager.insert(&person_attrs()).await.unwrap_err();
    let OrmError::Hydration { type_name, message } = err else {
        panic!("wrong error")
    };
    assert_eq!(type_name, "person");
    assert_eq!(message, "Insert returned noncanonical IID");
    {
        let s = state.lock().unwrap();
        assert_eq!(s.opens, vec![TxType::Write]);
        assert_eq!(
            s.queries,
            vec![
                query_builder::build_dynamic_entity_insert_with_iid(
                    &person_descriptor(),
                    &person_attrs(),
                    "$e"
                )
                .unwrap()
            ]
        );
        assert_eq!(s.commits, 0);
        assert_eq!(s.rollbacks, 0);
    }
    tx.rollback().await.unwrap();
    let s = state.lock().unwrap();
    assert_eq!(s.commits, 0);
    assert_eq!(s.rollbacks, 1);
}

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
                doc: None,
                meta: Default::default(),
            },
            OwnedAttributeDescriptor {
                field_name: "age".into(),
                attr_name: "age".into(),
                value_type: ValueType::Long,
                annotations: vec![],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            },
        ],
        doc: None,
        meta: Default::default(),
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
            doc: None,
            meta: Default::default(),
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
        doc: None,
        meta: Default::default(),
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
        doc: None,
        meta: Default::default(),
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
        doc: None,
        meta: Default::default(),
    });
    descriptor
}

fn replacement_person_descriptor() -> EntityDescriptor {
    let mut descriptor = person_descriptor();
    descriptor.owned_attributes.extend([
        OwnedAttributeDescriptor {
            field_name: "nickname".into(),
            attr_name: "nickname".into(),
            value_type: ValueType::String,
            annotations: vec![],
            is_optional: true,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
        OwnedAttributeDescriptor {
            field_name: "labels".into(),
            attr_name: "label".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Card(0, Some(4))],
            is_optional: true,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
    ]);
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
        doc: None,
        meta: Default::default(),
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
fn dynamic_entity_exact_count_query_contrasts_inclusive_target() {
    let inclusive =
        query_builder::build_dynamic_entity_count(&person_descriptor(), &[], "$e").unwrap();
    let exact =
        query_builder::build_dynamic_entity_count_exact(&person_descriptor(), "$e").unwrap();

    assert!(inclusive.contains("$e isa person"));
    assert!(!inclusive.contains("$e isa! person"));
    assert!(exact.contains("$e isa! person"));
    assert!(!exact.contains("$e isa person"));
    assert!(exact.contains("$count = count($e)"));
}

#[test]
fn dynamic_entity_exact_scalar_queries_keep_filters_database_side() {
    let expressions = [DynamicExpr::Compare {
        attr_name: "age".into(),
        operator: DynamicComparisonOp::Gte,
        value: AttributeValue::Long(18),
    }];
    let count = query_builder::build_dynamic_entity_expr_count_exact(
        &person_descriptor(),
        &expressions,
        "$e",
    )
    .unwrap();
    let exists = query_builder::build_dynamic_entity_expr_exists_exact(
        &person_descriptor(),
        &expressions,
        "$e",
    )
    .unwrap();

    assert!(count.contains("$e isa! person"));
    assert!(count.contains("$count = count($e)"));
    assert!(!count.contains("fetch"));
    assert!(exists.contains("$e isa! person"));
    assert!(exists.contains("limit 1"));
    assert!(exists.contains("\"iid\": iid($e)"));
    assert!(!exists.contains("$count = count($e)"));
    assert!(!exists.contains("attributes"));
}

#[test]
fn dynamic_entity_exact_update_query_contrasts_inclusive_target() {
    let inclusive = query_builder::build_dynamic_entity_update(
        &person_descriptor(),
        Some("0xaaa"),
        &person_attrs(),
        "$e",
    )
    .unwrap();
    let exact = query_builder::build_dynamic_entity_update_exact(
        &person_descriptor(),
        "0xaaa",
        &person_attrs(),
        "$e",
    )
    .unwrap();

    assert!(inclusive.contains("$e isa person"));
    assert!(!inclusive.contains("$e isa! person"));
    assert!(exact.contains("$e isa! person"));
    assert!(!exact.contains("$e isa person"));
    assert!(exact.contains("iid 0xaaa"));
}

#[test]
fn dynamic_entity_exact_replacement_clears_omitted_optional_and_replaces_multivalue() {
    let descriptor = replacement_person_descriptor();
    let attributes = vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("age".into(), AttributeValue::Long(31)),
        ("labels".into(), AttributeValue::String("new-a".into())),
        ("labels".into(), AttributeValue::String("new-b".into())),
    ];
    let query =
        query_builder::build_dynamic_entity_update_exact(&descriptor, "0xaaa", &attributes, "$e")
            .unwrap();
    assert!(query.contains("$e isa! person"));
    assert!(query.contains("iid 0xaaa"));
    assert!(query.contains("$e has age $old_attr_0"));
    assert!(query.contains("$e has nickname $old_attr_1"));
    assert!(query.contains("$e has label $old_attr_2"));
    assert!(query.contains("try { $old_attr_0 of $e; }"));
    assert!(query.contains("try { $old_attr_1 of $e; }"));
    assert!(query.contains("try { $old_attr_2 of $e; }"));
    assert!(query.contains("$e has age 31"));
    assert!(
        query.contains("new-a") && query.contains("new-b"),
        "{query}"
    );
    assert!(query.contains("$e has label \"new-a\"") && query.contains("$e has label \"new-b\""));
    assert!(!query.contains("old_attr_3"));
    assert!(!query.contains("$e has name $old_attr"));
    assert!(!query.contains("$e has name \"Alice\""));
}

#[tokio::test]
async fn dynamic_entity_put_exact_existing_key_without_non_key_runs_delete_only_replacement() {
    let attributes = vec![("name".into(), AttributeValue::String("Alice".into()))];
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xaaa"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(replacement_person_descriptor()));

    assert_eq!(manager.put_exact(&attributes).await.unwrap(), "0xaaa");
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 1);
    assert!(state.queries[1].contains("delete"));
    assert!(!state.queries[1].contains("insert"));
    assert!(state.queries[1].contains("$e has age $old_attr_0"));
    assert!(state.queries[1].contains("$e has nickname $old_attr_1"));
    assert!(state.queries[1].contains("$e has label $old_attr_2"));
    assert!(state.queries[1].contains("try { $old_attr_0 of $e; }"));
    assert!(state.queries[1].contains("try { $old_attr_1 of $e; }"));
    assert!(state.queries[1].contains("try { $old_attr_2 of $e; }"));
    assert!(!state.queries[1].contains("has name"));
}

#[tokio::test]
async fn dynamic_entity_key_only_exact_update_is_zero_provider_operation() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut descriptor = person_descriptor();
    descriptor
        .owned_attributes
        .retain(OwnedAttributeDescriptor::is_key);
    let manager = DynamicEntityManager::new(&db, Arc::new(descriptor));
    let attributes = vec![("name".into(), AttributeValue::String("Alice".into()))];

    manager.update_exact("0xaaa", &attributes).await.unwrap();
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert!(state.query_modes.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn dynamic_entity_exact_unknown_input_is_rejected_before_provider_io() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let mut descriptor = person_descriptor();
    descriptor
        .owned_attributes
        .retain(OwnedAttributeDescriptor::is_key);
    let manager = DynamicEntityManager::new(&db, Arc::new(descriptor));
    let attributes = vec![("unknown".into(), AttributeValue::String("x".into()))];

    assert!(matches!(
        manager.update_exact("0xaaa", &attributes).await,
        Err(OrmError::QueryExecution(message))
            if message == "Dynamic exact update for person references unknown attribute unknown"
    ));
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[test]
fn dynamic_entity_legacy_patch_update_omits_absent_optional_ownership() {
    let attributes = vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("age".into(), AttributeValue::Long(31)),
    ];
    let query = query_builder::build_dynamic_entity_update(
        &replacement_person_descriptor(),
        Some("0xaaa"),
        &attributes,
        "$e",
    )
    .unwrap();
    assert!(query.contains("$e has age 31"));
    assert!(!query.contains("nickname"));
    assert!(!query.contains("label"));
}

#[test]
fn dynamic_entity_exact_delete_query_contrasts_inclusive_target() {
    let inclusive =
        query_builder::build_dynamic_entity_delete_by_iid(&person_descriptor(), "0xaaa", "$e")
            .unwrap();
    let exact = query_builder::build_dynamic_entity_delete_by_iid_exact(
        &person_descriptor(),
        "0xaaa",
        "$e",
    )
    .unwrap();

    assert!(inclusive.contains("$e isa person"));
    assert!(!inclusive.contains("$e isa! person"));
    assert!(exact.contains("$e isa! person"));
    assert!(!exact.contains("$e isa person"));
    assert!(exact.contains("iid 0xaaa"));
}

#[test]
fn dynamic_entity_identity_discovery_fetches_only_iid_and_concrete_type() {
    let all =
        query_builder::build_dynamic_entity_identity_discovery(&person_descriptor(), None, "$e")
            .unwrap();
    let by_iid = query_builder::build_dynamic_entity_identity_discovery(
        &person_descriptor(),
        Some("0xabc"),
        "$e",
    )
    .unwrap();

    for query in [&all, &by_iid] {
        assert!(query.contains("$e isa! $t"));
        assert!(query.contains("$t sub person"));
        assert!(query.contains("\"_iid\": iid($e)"));
        assert!(query.contains("\"_type\": label($t)"));
        assert!(!query.contains("attributes"));
        assert!(!query.contains(".*"));
    }
    assert!(!all.contains("iid 0xabc"));
    assert!(by_iid.contains("iid 0xabc"));
}

#[test]
fn dynamic_entity_exact_key_lookup_is_strict_and_never_uses_put() {
    let query = query_builder::build_dynamic_entity_exact_key_lookup(
        &person_descriptor(),
        &person_attrs(),
        "$e",
    )
    .unwrap()
    .unwrap();

    assert!(query.contains("$e isa! person"));
    assert!(query.contains("has name \"Alice\""));
    assert!(!query.contains("\nput "));
    assert!(!query.starts_with("put"));
    assert!(!query.contains("attributes"));
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
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa7"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    assert_eq!(manager.insert(&person_attrs()).await.unwrap(), "0xa7");
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
        serde_json::json!({"iid": {"value": "0xa8"}}),
    ])]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, descriptor);

    assert_eq!(manager.put(&person_attrs()).await.unwrap(), "0xa8");

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
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa9"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xaa"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xab"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xac"})]),
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
        vec!["0xac", "0xab"]
    );
    assert_eq!(
        manager.put_many(&items).await.unwrap(),
        vec!["0xaa", "0xa9"]
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
async fn dynamic_entity_manager_exact_count_update_delete_use_strict_targets() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 2})])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(manager.count_exact().await.unwrap(), 2);
    manager
        .update_exact("0xaaa", &person_attrs())
        .await
        .unwrap();
    manager.delete_by_iid_exact("0xaaa").await.unwrap();

    let state = state.lock().unwrap();
    assert_eq!(
        state.opens,
        vec![TxType::Read, TxType::Write, TxType::Write]
    );
    assert_eq!(state.commits, 2);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.queries.len(), 3);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("isa! person"))
    );
    assert!(state.queries[1].contains("iid 0xaaa"));
    assert!(state.queries[2].contains("iid 0xaaa"));
}

#[tokio::test]
async fn dynamic_entity_exact_scalar_terminals_do_not_hydrate_models() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 2})])),
        RecordingResponse::Result(QueryResult::Documents(vec![serde_json::json!({
            "malformed-model": true
        })])),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));
    let expressions = [DynamicExpr::Compare {
        attr_name: "age".into(),
        operator: DynamicComparisonOp::Gte,
        value: AttributeValue::Long(18),
    }];

    assert_eq!(
        manager.count_exact_with_query(&expressions).await.unwrap(),
        2
    );
    assert!(manager.exists_exact_with_query(&expressions).await.unwrap());
    assert!(!manager.exists_exact_with_query(&expressions).await.unwrap());

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Read, TxType::Read, TxType::Read]);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("isa! person"))
    );
    assert!(state.queries[0].contains("$count = count($e)"));
    assert!(!state.queries[0].contains("fetch"));
    for query in &state.queries[1..] {
        assert!(query.contains("limit 1"));
        assert!(query.contains("\"iid\": iid($e)"));
        assert!(!query.contains("attributes"));
    }
}

#[tokio::test]
async fn dynamic_entity_exact_first_limits_and_hydrates_only_one_row() {
    let first = serde_json::json!({
        "_iid": "0xaaa",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        }
    });
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![first, serde_json::json!({"malformed": true})]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let row = DynamicEntityManager::new(&db, Arc::new(person_descriptor()))
        .first_exact_with_query(&[])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row.iid.as_deref(), Some("0xaaa"));
    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Read]);
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("isa! person"));
    assert!(state.queries[0].contains("limit 1"));
}

#[tokio::test]
async fn dynamic_entity_exact_update_batch_rolls_back_on_missing_second_row() {
    let first = serde_json::json!({
        "_iid": "0xaaa",
        "_type": "person",
        "attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 31}]
        }
    });
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Documents(vec![first])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));
    let items = vec![
        ("0xaaa".into(), person_attrs()),
        ("0xbbb".into(), person_attrs()),
    ];

    assert!(matches!(
        manager.update_many_and_get_exact(&items).await,
        Err(OrmError::NotFound(message)) if message.contains("0xbbb")
    ));
    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.queries.len(), 4);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_exact_delete_batch_rolls_back_on_second_failure() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Error("second delete failed"),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager
            .delete_many_by_iid_exact(&["0xaaa".into(), "0xbbb".into()])
            .await,
        Err(OrmError::QueryExecution(message)) if message == "second delete failed"
    ));
    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_exact_mutations_reject_invalid_iid_before_provider_io() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager.update_exact("person-1", &person_attrs()).await,
        Err(OrmError::QueryExecution(message)) if message.contains("canonical TypeDB IID")
    ));
    assert!(matches!(
        manager.delete_by_iid_exact("").await,
        Err(OrmError::QueryExecution(message)) if message.contains("canonical TypeDB IID")
    ));

    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn dynamic_entity_put_exact_updates_existing_exact_row_and_preserves_iid() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xaaa"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(manager.put_exact(&person_attrs()).await.unwrap(), "0xaaa");

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.queries.len(), 2);
    assert!(state.queries[0].contains("$e isa! person"));
    assert!(state.queries[0].contains("has name \"Alice\""));
    assert!(state.queries[1].contains("$e isa! person"));
    assert!(state.queries[1].contains("iid 0xaaa"));
    assert!(!state.queries.iter().any(|query| query.starts_with("put")));
}

#[tokio::test]
async fn dynamic_entity_put_exact_rejects_nonunique_exact_identity_and_rolls_back() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xaaa"}),
            serde_json::json!({"iid": "0xbbb"}),
        ]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager.put_exact(&person_attrs()).await,
        Err(OrmError::Hydration { message, .. }) if message.contains("got 2")
    ));

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.queries.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_put_exact_missing_row_inserts_requested_type() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xbbb"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(manager.put_exact(&person_attrs()).await.unwrap(), "0xbbb");

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.queries.len(), 2);
    assert!(state.queries[0].contains("$e isa! person"));
    assert!(state.queries[1].starts_with("insert"));
    assert!(state.queries[1].contains("$e isa person"));
}

#[tokio::test]
async fn dynamic_entity_put_exact_excludes_same_key_subtype_and_never_mutates_it() {
    let (backend, state) = RecordingBackend::new(vec![
        // A subtype-only inclusive match is absent from the strict lookup.
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xbbb"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(manager.put_exact(&person_attrs()).await.unwrap(), "0xbbb");

    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 2);
    assert!(state.queries[0].contains("$e isa! person"));
    assert!(!state.queries[0].contains("$e isa person"));
    assert!(state.queries[1].starts_with("insert"));
    assert!(!state.queries.iter().any(|query| query.contains("update")));
    assert!(!state.queries.iter().any(|query| query.starts_with("put")));
}

#[tokio::test]
async fn dynamic_entity_delete_exact_absent_target_is_a_strict_noop() {
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(QueryResult::Ok)]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    manager.delete_by_iid_exact("0xaaa").await.unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("$e isa! person"));
    assert!(state.queries[0].contains("iid 0xaaa"));
}

#[tokio::test]
async fn dynamic_entity_put_exact_without_usable_key_inserts_without_lookup() {
    let attributes = vec![("age".into(), AttributeValue::Long(30))];
    let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xaaa"})]),
    )]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(manager.put_exact(&attributes).await.unwrap(), "0xaaa");

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].starts_with("insert"));
}

#[tokio::test]
async fn dynamic_entity_put_many_exact_preserves_order_in_one_transaction() {
    let alice = person_attrs();
    let bob = vec![
        ("name".into(), AttributeValue::String("Bob".into())),
        ("age".into(), AttributeValue::Long(31)),
    ];
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xaaa"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xbbb"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(
        manager.put_many_exact(&[alice, bob]).await.unwrap(),
        vec!["0xaaa", "0xbbb"]
    );

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.queries.len(), 4);
    assert!(state.queries[0].contains("has name \"Alice\""));
    assert!(state.queries[1].contains("iid 0xaaa"));
    assert!(state.queries[2].contains("has name \"Bob\""));
    assert!(state.queries[3].starts_with("insert"));
}

#[tokio::test]
async fn dynamic_entity_put_many_exact_rolls_back_atomically_and_preserves_primary_error() {
    let alice = person_attrs();
    let bob = vec![
        ("name".into(), AttributeValue::String("Bob".into())),
        ("age".into(), AttributeValue::Long(31)),
    ];
    let (backend, state) = RecordingBackend::with_rollback_error(
        vec![
            RecordingResponse::Result(QueryResult::Documents(vec![
                serde_json::json!({"iid": "0xaaa"}),
            ])),
            RecordingResponse::Result(QueryResult::Ok),
            RecordingResponse::Error("primary exact-put failure"),
        ],
        true,
    );
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager.put_many_exact(&[alice, bob]).await,
        Err(OrmError::QueryExecution(message)) if message == "primary exact-put failure"
    ));

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Write]);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.queries.len(), 3);
}

#[tokio::test]
async fn dynamic_entity_put_many_exact_empty_input_performs_zero_provider_operations() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(manager.put_many_exact(&[]).await.unwrap().is_empty());

    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn dynamic_entity_transaction_bound_put_exact_does_not_auto_commit() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": "0xaaa"}),
        ])),
        RecordingResponse::Result(QueryResult::Ok),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), Arc::new(person_descriptor()));

    assert_eq!(manager.put_exact(&person_attrs()).await.unwrap(), "0xaaa");
    {
        let state = state.lock().unwrap();
        assert_eq!(state.opens, vec![TxType::Write]);
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 0);
        assert_eq!(state.queries.len(), 2);
    }
    tx.rollback().await.unwrap();
    assert_eq!(state.lock().unwrap().rollbacks, 1);
}

#[tokio::test]
async fn dynamic_entity_identity_discovery_preserves_rows_and_handles_by_iid_cardinality() {
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid": "0xaaa", "_type": "employee"}),
            serde_json::json!({
                "_iid": {"value": "0xbbb"},
                "_type": {"value": "contractor"}
            }),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid": "0xccc", "_type": "employee"}),
        ])),
        RecordingResponse::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid": "0xddd", "_type": "employee"}),
            serde_json::json!({"_iid": "0xddd", "_type": "employee"}),
        ])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert_eq!(
        manager.discover_all().await.unwrap(),
        vec![
            DynamicEntityIdentity {
                iid: "0xaaa".into(),
                type_name: "employee".into(),
            },
            DynamicEntityIdentity {
                iid: "0xbbb".into(),
                type_name: "contractor".into(),
            },
        ]
    );
    assert!(manager.discover_by_iid("0x0").await.unwrap().is_none());
    assert_eq!(
        manager.discover_by_iid("0xccc").await.unwrap(),
        Some(DynamicEntityIdentity {
            iid: "0xccc".into(),
            type_name: "employee".into(),
        })
    );
    assert!(matches!(
        manager.discover_by_iid("0xddd").await,
        Err(OrmError::Hydration { message, .. }) if message.contains("got 2")
    ));

    let state = state.lock().unwrap();
    assert_eq!(state.opens, vec![TxType::Read; 4]);
    assert_eq!(state.queries.len(), 4);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("$e isa! $t") && !query.contains("attributes"))
    );
}

#[tokio::test]
async fn dynamic_entity_identity_discovery_rejects_every_malformed_shape() {
    let cases = vec![
        (
            QueryResult::Documents(vec![serde_json::json!({"_type": "employee"})]),
            "omitted its IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({
                "_iid": "",
                "_type": "employee"
            })]),
            "noncanonical IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({
                "_iid": "employee-1",
                "_type": "employee"
            })]),
            "noncanonical IID",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({"_iid": "0xaaa"})]),
            "omitted its concrete type",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!({
                "_iid": "0xaaa",
                "_type": " "
            })]),
            "blank concrete type",
        ),
        (
            QueryResult::Documents(vec![serde_json::json!(["not", "a", "document"])]),
            "Expected JSON object",
        ),
        (QueryResult::Rows(vec![]), "got Rows"),
        (QueryResult::Ok, "got Ok"),
    ];

    for (result, expected) in cases {
        let backend = MockBackend::new(vec![result]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));
        assert!(
            matches!(
                manager.discover_all().await,
                Err(OrmError::Hydration { message, .. }) if message.contains(expected)
            ),
            "expected discovery failure containing {expected:?}"
        );
    }
}

#[tokio::test]
async fn dynamic_entity_identity_discovery_rejects_invalid_input_before_provider_io() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicEntityManager::new(&db, Arc::new(person_descriptor()));

    assert!(matches!(
        manager.discover_by_iid("not-an-iid").await,
        Err(OrmError::QueryExecution(message)) if message.contains("canonical TypeDB IID")
    ));

    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
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
        "_iid": "0xabc",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        },
        "_role_0_iid": "0x101",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        },
        "_role_1_iid": "0x102",
        "_role_1_type": "company",
        "_role_1_attributes": {
            "name": [{"value": "Acme"}]
        }
    });
    let backend = MockBackend::new(vec![
        QueryResult::Ok,
        QueryResult::Rows(vec![serde_json::json!({"$count": 1})]),
        QueryResult::Documents(vec![fetch_doc]),
        QueryResult::Documents(vec![serde_json::json!({"iid": {"value": "0xad"}})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager
            .insert(&relation_attrs, &role_players)
            .await
            .unwrap(),
        "0xad"
    );
    let rows = manager.all().await.unwrap();
    assert_eq!(rows[0].iid.as_deref(), Some("0xabc"));
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
    manager.delete_by_iid("0xabc").await.unwrap();

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
        QueryResult::Documents(vec![serde_json::json!({"iid": {"value": "0xa2"}})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager.put(&relation_attrs, &role_players).await.unwrap(),
        "0xa2"
    );
    manager
        .update(Some("0xa2"), &employment_attrs("Staff Engineer"), &[])
        .await
        .unwrap();

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].starts_with("match"));
    assert!(recorded[0].contains("\nput"));
    assert!(recorded[0].contains("links (employee: $rp0, employer: $rp1)"));
    assert!(recorded[1].contains("iid 0xa2"));
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
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa3"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa4"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa5"})]),
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xa6"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);

    assert_eq!(
        manager.insert_many(&items).await.unwrap(),
        vec!["0xa6", "0xa5"]
    );
    assert_eq!(
        manager.put_many(&items).await.unwrap(),
        vec!["0xa4", "0xa3"]
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
        "_iid": "0xabc",
        "_type": "employment",
        "attributes": {
            "position": [{"value": "Engineer"}]
        },
        "_role_0_iid": "0x101",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "name": [{"value": "Alice"}],
            "age": [{"value": 30}]
        },
        "_role_1_iid": "0x102",
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

    let rows = manager.get_by_iid("0xabc").await.unwrap();
    let row = &rows[0];
    assert_eq!(row.iid.as_deref(), Some("0xabc"));
    assert_eq!(row.attributes, relation_attrs);
    assert_eq!(row.role_players.len(), 2);
    assert_eq!(row.role_players[0].player_iid.as_deref(), Some("0x101"));
    assert!(manager.get_by_iid("0xmissing").await.unwrap().is_empty());

    let recorded = queries.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].contains("iid 0xabc"));
    assert!(recorded[0].contains("employee: $rp0"));
    assert!(recorded[0].contains("\"_role_0_attributes\": { $rp0.* }"));
}

#[tokio::test]
async fn dynamic_relation_read_routes_converge_on_one_coalesced_row() {
    let descriptor = Arc::new(employment_descriptor());
    let document = |employee: &str, employer: &str| {
        serde_json::json!({
            "_iid": "0xabc", "_type": "employment",
            "attributes": {"position": [{"value": "Engineer"}]},
            "_role_0_iid": employee, "_role_0_type": "person",
            "_role_0_attributes": {"name": [{"value": "Alice"}]},
            "_role_1_iid": employer, "_role_1_type": "company",
            "_role_1_attributes": {"name": [{"value": "Acme"}]}
        })
    };
    let corpus = vec![
        document("0x101", "0x201"),
        document("0x102", "0x201"),
        document("0x101", "0x202"),
        document("0x102", "0x202"),
    ];
    let responses = (0..9)
        .map(|_| QueryResult::Documents(corpus.clone()))
        .collect();
    let backend = MockBackend::new(responses);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let manager = DynamicRelationManager::new(&db, descriptor);
    let assert_row = |rows: Vec<DynamicRelationRow>| {
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].iid.as_deref(), Some("0xabc"));
        assert_eq!(rows[0].role_players.len(), 4);
        assert_eq!(
            rows[0]
                .role_players
                .iter()
                .map(|player| player.player_iid.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("0x101"), Some("0x102"), Some("0x201"), Some("0x202")]
        );
    };

    assert_row(manager.get(&[]).await.unwrap());
    assert_row(manager.get_exact(&[]).await.unwrap());
    assert_row(manager.get_with_query(&[], &[], None, None).await.unwrap());
    assert_row(manager.get_with_role_filters(&[], &[]).await.unwrap());
    assert_row(manager.get_by_iid("0xabc").await.unwrap());
    assert_row(manager.get_by_iid_exact("0xabc").await.unwrap());
    assert_row(manager.all().await.unwrap());
    assert_row(manager.all_exact().await.unwrap());
    let row = manager.get_one(&[]).await.unwrap();
    assert_eq!(row.iid.as_deref(), Some("0xabc"));
}

#[tokio::test]
async fn dynamic_relation_legacy_database_and_canonical_transaction_reads_share_coalescer() {
    let descriptor = Arc::new(employment_descriptor());
    let doc = serde_json::json!({
        "_iid":"0xabc", "_type":"employment",
        "attributes":{"position":[{"value":"Engineer"}]},
        "_role_0_iid":"0x101", "_role_0_type":"person", "_role_0_attributes":{"name":[{"value":"Alice"}]},
        "_role_1_iid":"0x201", "_role_1_type":"company", "_role_1_attributes":{"name":[{"value":"Acme"}]}
    });
    let mut doc2 = doc.clone();
    doc2["_role_0_iid"] = serde_json::json!("0x102");
    let (backend, state) = RecordingBackend::new(vec![
        RecordingResponse::Result(QueryResult::Documents(vec![doc.clone(), doc2.clone()])),
        RecordingResponse::Result(QueryResult::Documents(vec![doc2, doc])),
    ]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let legacy = DynamicRelationManager::new(&db, Arc::clone(&descriptor))
        .all()
        .await
        .unwrap();
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    let canonical = DynamicRelationManager::with_canonical_transaction(tx, descriptor)
        .all()
        .await
        .unwrap();
    assert_eq!(legacy, canonical);
    assert_eq!(
        legacy[0]
            .role_players
            .iter()
            .map(|p| p.player_iid.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("0x101"), Some("0x102"), Some("0x201")]
    );
    assert_eq!(
        state.lock().unwrap().query_modes,
        vec!["legacy", "canonical"]
    );
}

#[tokio::test]
async fn dynamic_relation_by_iid_boundaries_and_answer_kinds_are_exact() {
    let descriptor = Arc::new(employment_descriptor());
    let cartesian_doc = |employee: &str, employer: &str| {
        serde_json::json!({
            "_iid": "0xabc", "_type": "employment",
            "attributes": {"position": [{"value": "Engineer"}]},
            "_role_0_iid": employee, "_role_0_type": "person",
            "_role_0_attributes": {"name": [{"value": "Alice"}]},
            "_role_1_iid": employer, "_role_1_type": "company",
            "_role_1_attributes": {"name": [{"value": "Acme"}]}
        })
    };
    let empty_backend = MockBackend::new(vec![QueryResult::Documents(vec![])]);
    let empty_db = Database::with_backend(Box::new(empty_backend), "testdb");
    let empty = DynamicRelationManager::new(&empty_db, Arc::clone(&descriptor))
        .get_by_iid("0xabc")
        .await
        .unwrap();
    assert!(empty.is_empty());

    let docs = vec![
        cartesian_doc("0x101", "0x201"),
        cartesian_doc("0x102", "0x201"),
    ];
    let duplicate_backend = MockBackend::new(vec![QueryResult::Documents(docs)]);
    let duplicate_db = Database::with_backend(Box::new(duplicate_backend), "testdb");
    let duplicate = DynamicRelationManager::new(&duplicate_db, Arc::clone(&descriptor))
        .get_by_iid("0xabc")
        .await
        .unwrap();
    assert_eq!(duplicate.len(), 1);
    assert_eq!(duplicate[0].role_players.len(), 3);

    let mut other = cartesian_doc("0x101", "0x201");
    other["_iid"] = serde_json::json!("0xdef");
    let multi_backend = MockBackend::new(vec![QueryResult::Documents(vec![
        cartesian_doc("0x101", "0x201"),
        other,
    ])]);
    let multi_db = Database::with_backend(Box::new(multi_backend), "testdb");
    let err = DynamicRelationManager::new(&multi_db, Arc::clone(&descriptor))
        .get_by_iid("0xabc")
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Hydration { message, .. } if message.contains("multiple logical"))
    );

    let wrong_backend = MockBackend::new(vec![QueryResult::Documents(vec![cartesian_doc(
        "0x101", "0x201",
    )])]);
    let wrong_db = Database::with_backend(Box::new(wrong_backend), "testdb");
    let err = DynamicRelationManager::new(&wrong_db, Arc::clone(&descriptor))
        .get_by_iid("0xdef")
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Hydration { message, .. } if message == "IID lookup returned a different relation IID")
    );

    let ok_backend = MockBackend::new(vec![QueryResult::Ok]);
    let ok_db = Database::with_backend(Box::new(ok_backend), "testdb");
    assert!(
        DynamicRelationManager::new(&ok_db, Arc::clone(&descriptor))
            .all()
            .await
            .unwrap()
            .is_empty()
    );
    let rows_backend = MockBackend::new(vec![QueryResult::Rows(vec![])]);
    let rows_db = Database::with_backend(Box::new(rows_backend), "testdb");
    let err = DynamicRelationManager::new(&rows_db, descriptor)
        .all()
        .await
        .unwrap_err();
    assert!(
        matches!(err, OrmError::Hydration { message, .. } if message.contains("Expected Documents"))
    );
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
        QueryResult::Documents(vec![serde_json::json!({"iid": "0xae"})]),
    ]);
    let queries = Arc::clone(&backend.queries);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    let manager = DynamicEntityManager::with_transaction(tx.clone(), descriptor);

    assert_eq!(manager.insert(&person_attrs()).await.unwrap(), "0xae");
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
