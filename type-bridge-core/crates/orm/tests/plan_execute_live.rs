//! Opt-in proof that `query-plan/v3` programs execute staged against TypeDB 3.13.6.
#![cfg(feature = "typedb")]
#[path = "support/live_database.rs"]
mod live_database;
#[path = "support/plan_race.rs"]
mod race;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::query_plan::Operation;
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::session::staged::{PlanAnswer, StagedQuery, TransactionOptions};
use type_bridge_orm::{
    ConnectOptions, Database, InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedCreate,
    ProjectedCrudExecutor, ProjectedReference, TxType,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

fn rows(answer: &QueryResult) -> usize {
    match answer {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    }
}

/// Connect to the opt-in live server with a fresh database name, or `None`
/// when the live address is unset or the server cannot carry `given` plans.
async fn live_database(stem: &str) -> Option<Database> {
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").ok()?;
    let name = live_database::name(stem);
    let db = Database::connect_with_options(
        &address,
        &name,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".into())
                .parse()
                .expect("valid TypeDB HTTP port"),
            tls: false,
            server_version: Some(
                std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")
                    .unwrap_or_else(|_| "3.13.6".into())
                    .parse()
                    .unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    // Plan inputs travel as `given` rows (TypeDB 3.12+ over band 9).
    if !db.supports_given_stage() {
        println!("skipped: TypeDB at {address} cannot carry the given stage plans need");
        return None;
    }
    db.create_database().await.unwrap();
    Some(db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_query_purchase_and_write_plans_resolve_at_commit() {
    let Some(db) = live_database("plans").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $i isa item, has item-id "I1", has stock 1, has price 40;
           $a isa account, has account-id "A1", has balance 100;
           $d isa doctor, has name "alice", has on-call true;"#,
        TxType::Write,
    )
    .await
    .unwrap();

    let mut tx = db.transaction(TransactionOptions::default());
    let bought = tx
        .stage_plan(&race::implicit_purchase("O1", "I1", "A1"))
        .unwrap();
    tx.commit().await.unwrap();
    let PlanAnswer::Matched(count) = *bought.get().unwrap() else {
        panic!("purchase must answer a matched count");
    };
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/contracts/sdk_conformance/writes/purchase/expected.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::json!(["matched", count]),
        expected["results"]["program"]["purchase"]
    );

    // Stock is now 0: the same pipeline matches no row and writes nothing.
    let refused = tx.stage_plan(&race::purchase("O2", "I1", "A1")).unwrap();
    let orders = tx
        .stage(StagedQuery::new("match $p isa purchase; select $p;", false))
        .unwrap();
    let stock = tx
        .stage(StagedQuery::new(
            "match $i isa item, has stock 0; $a isa account, has balance 60; select $i, $a;",
            false,
        ))
        .unwrap();
    let stocked = tx.stage_plan(&race::stock_read(Operation::Rows)).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(*refused.get().unwrap(), PlanAnswer::Matched(0));
    assert_eq!(rows(&orders.get().unwrap()), 1);
    assert_eq!(rows(&stock.get().unwrap()), 1);
    assert!(matches!(&*stocked.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));

    let touched = tx.stage_plan(&race::touch_rows()).unwrap();
    let touch = tx.stage_plan(&race::touch()).unwrap();
    let dropped = tx.stage_plan(&race::drop_price()).unwrap();
    let exists = tx.stage_plan(&race::stock_read(Operation::Exists)).unwrap();
    let count = tx.stage_plan(&race::stock_read(Operation::Count)).unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(&*touched.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));
    assert_eq!(*touch.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*dropped.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*exists.get().unwrap(), PlanAnswer::Exists(true));
    assert_eq!(*count.get().unwrap(), PlanAnswer::Count(1));
    let prices = db
        .execute_raw("match $i isa item, has price $p; select $i;", TxType::Read)
        .await
        .unwrap();
    assert_eq!(rows(&prices), 0);
    let on_call = db
        .execute_raw(
            "match $d isa doctor, has on-call true; select $d;",
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&on_call), 1);

    db.delete_database().await.unwrap();
    db.close().unwrap();
}

const PARTY_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  note: { value: string }
entities:
  party:
    abstract: true
    owns:
      identifier: { key: true }
  person:
    sub: party
  company:
    sub: party
relations:
  membership:
    owns:
      note: { card: { min: 0, max: 1 } }
    relates:
      member: { card: { min: 1, max: 2 } }
plays:
  party:
    membership: [member]
"#;

const PARTY_SCHEMA_TYPEQL: &str = "define
attribute identifier, value string; attribute note, value string;
entity party @abstract, owns identifier @key, plays membership:member;
entity person sub party; entity company sub party;
relation membership, relates member @card(1..2), owns note @card(0..1);";

async fn raw_entity_rows(
    db: &Database,
    descriptor: &type_bridge_orm::_descriptor::EntityDescriptor,
    predicate: &str,
) -> Vec<type_bridge_orm::_dynamic::DynamicEntityRow> {
    let query = format!(
        "match $e isa! $t; $t sub person; {predicate} fetch {{ \"_iid\": iid($e), \"_type\": label($t), \"attributes\": {{ $e.* }} }};"
    );
    let QueryResult::Documents(documents) = db.execute_raw(&query, TxType::Read).await.unwrap()
    else {
        panic!("raw entity fetch must return documents");
    };
    documents
        .iter()
        .map(|document| {
            type_bridge_orm::_manager::hydration::hydrate_dynamic_entity(descriptor, document)
                .unwrap()
        })
        .collect()
}

async fn raw_relation_rows(
    db: &Database,
    descriptor: &type_bridge_orm::_descriptor::RelationDescriptor,
    excluded_player: &str,
) -> Vec<type_bridge_orm::_dynamic::DynamicRelationRow> {
    let query = format!(
        "match $r isa! $t; $t sub membership; $r links (member: $p); $p isa! $pt; not {{ $r links (member: $excluded); iid($excluded) == \"{excluded_player}\"; }}; fetch {{ \"_iid\": iid($r), \"_type\": label($t), \"attributes\": {{ $r.* }}, \"_role_0_iid\": iid($p), \"_role_0_type\": label($pt), \"_role_0_attributes\": {{ $p.* }} }};"
    );
    let QueryResult::Documents(documents) = db.execute_raw(&query, TxType::Read).await.unwrap()
    else {
        panic!("raw relation fetch must return documents");
    };
    documents
        .iter()
        .map(|document| {
            type_bridge_orm::_manager::hydration::hydrate_dynamic_relation(descriptor, document)
                .unwrap()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_managers_and_nested_iids_match_raw_builder_results() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::{DynamicComparisonOp, DynamicExpr, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    let Some(db) = live_database("managers").await else {
        return;
    };
    let typeql = PARTY_SCHEMA_TYPEQL.replace(
        "plays membership:member;",
        "plays membership:member, plays employment:employee;",
    ) + " relation employment sub membership, relates employee as member @card(1..2);";
    db.execute_raw(&typeql, TxType::Schema).await.unwrap();
    let yaml = PARTY_SCHEMA_YAML.replace("plays:\n", "  employment:\n    sub: membership\n    relates:\n      employee: { as: member, card: { min: 1, max: 2 } }\nplays:\n")
        .replace("    membership: [member]", "    membership: [member]\n    employment: [employee]");
    let installed = party_projection_for_schema(&yaml);
    let registry = Arc::new(installed.match_registry().unwrap());
    let person = registry.entity("person").unwrap();
    let planned =
        DynamicEntityManager::from_registry(&db, Arc::clone(&registry), Arc::clone(&person));
    let first = planned
        .insert(&vec![(
            "identifier".into(),
            type_bridge_orm::AttributeValue::String("A".into()),
        )])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let second = planned
        .insert(&vec![(
            "identifier".into(),
            type_bridge_orm::AttributeValue::String("B".into()),
        )])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    assert_eq!(planned.count().await.unwrap(), 2);
    let iid_expression = db
        .execute_raw(
            &format!("match $p isa person; iid($p) == \"{first}\"; select $p;"),
            TxType::Read,
        )
        .await;
    assert!(
        iid_expression.is_ok(),
        "native IID expression comparison: {iid_expression:?}"
    );
    let inside_try = db.execute_raw(&format!("match $p isa person; try {{ $p has identifier $key; iid($p) == \"{first}\"; }}; select $p;"), TxType::Read).await;
    assert!(
        inside_try.is_ok(),
        "IID predicate inside Try: {inside_try:?}"
    );
    assert_eq!(
        planned.all().await.unwrap(),
        raw_entity_rows(&db, &person, "").await
    );
    for (expression, predicate) in [
        (
            DynamicExpr::Or {
                exprs: vec![
                    DynamicExpr::Iid { iid: first.clone() },
                    DynamicExpr::Iid {
                        iid: second.clone(),
                    },
                ],
            },
            format!("{{ $e iid {first}; }} or {{ $e iid {second}; }};"),
        ),
        (
            DynamicExpr::Not {
                expr: Box::new(DynamicExpr::Iid { iid: first.clone() }),
            },
            format!("not {{ $e iid {first}; }};"),
        ),
    ] {
        assert_eq!(
            planned
                .get_with_query(std::slice::from_ref(&expression), &[], None, None)
                .await
                .unwrap(),
            raw_entity_rows(&db, &person, &predicate).await
        );
    }
    assert!(
        planned
            .exists_exact_with_query(&[DynamicExpr::Compare {
                attr_name: "identifier".into(),
                operator: DynamicComparisonOp::Eq,
                value: type_bridge_orm::AttributeValue::String("A".into())
            }])
            .await
            .unwrap()
    );
    let membership = registry.relation("membership").unwrap();
    let relations =
        DynamicRelationManager::from_registry(&db, Arc::clone(&registry), Arc::clone(&membership));
    let relation_iid = relations
        .insert(
            &vec![(
                "note".into(),
                type_bridge_orm::AttributeValue::String("joined".into()),
            )],
            &[DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "person".into(),
                iid: Some(first.clone()),
                key: None,
            }],
        )
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let lookup = DynamicExpr::RolePlayer {
        role_name: "member".into(),
        expr: Box::new(DynamicExpr::Iid { iid: first.clone() }),
    };
    let resolved = relations
        .get_exact_with_query(&[lookup], &[], Some(1), None)
        .await
        .expect("keyless relation resolves by role-player IID");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].iid.as_deref(), Some(relation_iid.as_str()));
    let expression = DynamicExpr::Not {
        expr: Box::new(DynamicExpr::RolePlayer {
            role_name: "member".into(),
            expr: Box::new(DynamicExpr::Iid {
                iid: second.clone(),
            }),
        }),
    };
    assert_eq!(
        relations
            .get_with_query(std::slice::from_ref(&expression), &[], None, None)
            .await
            .unwrap(),
        raw_relation_rows(&db, &membership, &second).await
    );
    assert_eq!(
        relations
            .get_by_iid_exact(&relation_iid)
            .await
            .unwrap()
            .len(),
        1
    );
    relations.delete_by_iid_exact(&relation_iid).await.unwrap();
    planned.delete_by_iid_exact(&first).await.unwrap();
    planned.delete_by_iid_exact(&second).await.unwrap();
    assert_eq!(planned.count().await.unwrap(), 0);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_manager_heterogeneous_subtypes_keep_selection_evidence() {
    use std::sync::Arc;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::AttributeValue;
    let Some(db) = live_database("heterogeneous").await else {
        return;
    };
    db.execute_raw(
        &PARTY_SCHEMA_TYPEQL.replace(
            "entity company sub party;",
            "entity company sub party, owns note @card(0..1);",
        ),
        TxType::Schema,
    )
    .await
    .unwrap();
    let installed = party_projection_for_schema(&PARTY_SCHEMA_YAML.replace(
        "  company:\n    sub: party",
        "  company:\n    sub: party\n    owns:\n      note: { card: { min: 0, max: 1 } }",
    ));
    let registry = Arc::new(installed.match_registry().unwrap());
    let mut expected = Vec::new();
    for name in ["person", "company"] {
        let manager = DynamicEntityManager::from_registry(
            &db,
            Arc::clone(&registry),
            registry.entity(name).unwrap(),
        );
        expected.push(
            manager
                .insert(&vec![(
                    "identifier".into(),
                    AttributeValue::String(name.into()),
                )])
                .await
                .unwrap()
                .item()
                .expect("one inserted root")
                .iid
                .clone()
                .unwrap(),
        );
    }
    let manager = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("party").unwrap(),
    );
    let found = manager
        .get_with_query(&[], &[], Some(2), None)
        .await
        .expect("heterogeneous model partitions keep an independent root selection");
    let mut actual = found
        .iter()
        .map(|row| row.iid.clone().unwrap())
        .collect::<Vec<_>>();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn specialized_role_queries_keep_legacy_subtype_contract() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::{DynamicExpr, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let Some(db) = live_database("specialized_contract").await else {
        return;
    };
    let typeql = PARTY_SCHEMA_TYPEQL.replace(
        "plays membership:member;",
        "plays membership:member, plays employment:employee;",
    ) + " relation employment sub membership, relates employee as member @card(1..2);";
    db.execute_raw(&typeql, TxType::Schema).await.unwrap();
    let yaml = PARTY_SCHEMA_YAML.replace("plays:\n", "  employment:\n    sub: membership\n    relates:\n      employee: { as: member, card: { min: 1, max: 2 } }\nplays:\n")
        .replace("    membership: [member]", "    membership: [member]\n    employment: [employee]");
    let installed = party_projection_for_schema(&yaml);
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let person = people
        .insert(&vec![(
            "identifier".into(),
            AttributeValue::String("P".into()),
        )])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let player = |role: &str| DynamicRolePlayerInput {
        role_name: role.into(),
        player_type_name: "person".into(),
        iid: Some(person.clone()),
        key: None,
    };
    let legacy = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let membership = legacy
        .insert(&vec![], &[player("member")])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let employment = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("employment").unwrap(),
    )
    .insert(&vec![], &[player("employee")])
    .await
    .unwrap()
    .item()
    .expect("one inserted root")
    .iid
    .clone()
    .unwrap();
    let planned = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let predicate = DynamicExpr::RolePlayer {
        role_name: "member".into(),
        expr: Box::new(DynamicExpr::Iid {
            iid: person.clone(),
        }),
    };
    let exact = planned
        .get_exact_with_query(std::slice::from_ref(&predicate), &[], None, None)
        .await
        .unwrap();
    let inclusive = planned
        .get_with_query(&[predicate], &[], None, None)
        .await
        .unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].iid.as_deref(), Some(membership.as_str()));
    assert_eq!(
        inclusive
            .iter()
            .map(|row| row.iid.clone().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([membership, employment])
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_returning_deletes_hydrate_before_commit_on_both_execution_paths() {
    let Some(db) = live_database("tb_returning_helpers").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");
    let relation_type = party_type(TypeKind::Relation, "membership");
    for controlled in [false, true] {
        let create = ProjectedCreate::try_new(
            &installed,
            person_type.clone(),
            vec![(
                OwnsFactId::new(
                    person_type.clone(),
                    type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
                )
                .unwrap(),
                vec![party_text(
                    &installed,
                    "identifier",
                    if controlled { "controlled" } else { "direct" },
                )],
            )],
            vec![],
        )
        .unwrap();
        let person = crud
            .insert_entity(&db, &create)
            .await
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        let relation = crud
            .insert_relation(
                &db,
                &membership(
                    &installed,
                    person.try_to_reference(&installed).unwrap(),
                    "snapshot",
                ),
            )
            .await
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        let deleted = if controlled {
            crud.delete_relation_by_iid_returning_controlled(
                &db,
                &relation_type,
                relation.iid(),
                Default::default(),
                Default::default(),
            )
            .await
        } else {
            crud.delete_relation_by_iid_returning(&db, &relation_type, relation.iid())
                .await
        }
        .unwrap()
        .into_items()
        .pop()
        .unwrap();
        assert!(deleted.is_deleted());
        assert_eq!(deleted.fields(), relation.fields());
        let member = RoleId::new("membership", "member").unwrap();
        assert_eq!(deleted.roles()[&member][0].iid(), person.iid());
        assert!(
            crud.get_relation_by_iid(&db, &relation_type, relation.iid())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            crud.get_entity_by_iid(&db, &person_type, person.iid())
                .await
                .unwrap()
                .is_some()
        );
        let deleted = if controlled {
            crud.delete_entity_by_iid_returning_controlled(
                &db,
                &person_type,
                person.iid(),
                Default::default(),
                Default::default(),
            )
            .await
        } else {
            crud.delete_entity_by_iid_returning(&db, &person_type, person.iid())
                .await
        }
        .unwrap()
        .into_items()
        .pop()
        .unwrap();
        assert_eq!(deleted.type_id(), &person_type);
        assert_eq!(deleted.fields(), person.fields());
        assert!(deleted.is_deleted());
        assert!(
            crud.get_entity_by_iid(&db, &person_type, person.iid())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            crud.delete_entity_by_iid_returning(&db, &person_type, person.iid())
                .await
                .unwrap()
                .into_items()
                .pop()
                .is_none()
        );
    }
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_returning_deletes_preserve_subtype_and_relation_players() {
    use std::sync::Arc;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};

    let Some(db) = live_database("tb_returning_dynamic").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");
    let relation_type = party_type(TypeKind::Relation, "membership");
    let create = ProjectedCreate::try_new(
        &installed,
        person_type.clone(),
        vec![(
            OwnsFactId::new(
                person_type.clone(),
                type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
            )
            .unwrap(),
            vec![party_text(&installed, "identifier", "dynamic")],
        )],
        vec![],
    )
    .unwrap();
    let person = crud
        .insert_entity(&db, &create)
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let relation = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                person.try_to_reference(&installed).unwrap(),
                "snapshot",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let relations = DynamicRelationManager::from_registry(
        &db,
        installed.manager_registry().unwrap(),
        Arc::new(
            installed
                .relation_descriptor(&relation_type)
                .unwrap()
                .clone(),
        ),
    );
    let snapshots = relations
        .delete_many_by_iid_exact_returning(&[relation.iid().to_owned()])
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0]["_type"], "membership");
    let players = snapshots[0]["role_players"].as_array().unwrap();
    assert_eq!(players.len(), 1);
    assert_eq!(players[0]["iid"], person.iid());
    assert_eq!(players[0]["attributes"]["identifier"], "dynamic");
    assert!(
        crud.get_relation_by_iid(&db, &relation_type, relation.iid())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        crud.get_entity_by_iid(&db, &person_type, person.iid())
            .await
            .unwrap()
            .is_some()
    );
    let people = DynamicEntityManager::from_registry(
        &db,
        installed.manager_registry().unwrap(),
        Arc::new(installed.entity_descriptor(&person_type).unwrap().clone()),
    );
    let snapshots = people
        .delete_many_by_iid_exact_returning(&[person.iid().to_owned()])
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0]["_type"], "person");
    assert!(
        crud.get_entity_by_iid(&db, &person_type, person.iid())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        people
            .delete_many_by_iid_exact_returning(&[person.iid().to_owned()])
            .await
            .unwrap()
            .is_empty()
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

fn party_projection() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-party.yaml").unwrap(),
        PARTY_SCHEMA_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

fn party_projection_with_authority() -> InstalledRuntimeProjection {
    party_projection_for_schema(PARTY_SCHEMA_YAML)
}

fn party_projection_for_schema(yaml: &str) -> InstalledRuntimeProjection {
    party_projection_for_profile(yaml, "typedb-3.13.6/v1")
}

fn party_projection_for_profile(yaml: &str, profile: &str) -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("authority.yaml").unwrap(), yaml)]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new(profile).unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("manager-live").unwrap(),
        profile,
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    installed.with_schema_authority(&authority)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_exact_updates_clear_optional_entity_and_relation_fields() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let Ok(address) = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS") else {
        return;
    };
    let name = live_database::name("clear");
    let version = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION").unwrap();
    let db = Database::connect_with_options(
        &address,
        &name,
        "admin",
        "password",
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            tls: false,
            server_version: Some(version.parse().unwrap()),
        },
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    db.execute_raw(
        &PARTY_SCHEMA_TYPEQL.replace(
            "entity person sub party;",
            "entity person sub party, owns note @card(0..1);",
        ),
        TxType::Schema,
    )
    .await
    .unwrap();
    let profile = if version.starts_with("3.12.") {
        "typedb-3.12.1/v1"
    } else {
        "typedb-3.13.6/v1"
    };
    let installed = party_projection_for_profile(
        &PARTY_SCHEMA_YAML.replace(
            "  person:\n    sub: party",
            "  person:\n    sub: party\n    owns:\n      note: { card: { min: 0, max: 1 } }",
        ),
        profile,
    );
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let relations = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let iid = people
        .insert(&vec![
            ("identifier".into(), AttributeValue::String("clear".into())),
            ("note".into(), AttributeValue::String("before".into())),
        ])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let player = DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "person".into(),
        iid: Some(iid.clone()),
        key: None,
    };
    let relation = relations
        .insert(
            &vec![("note".into(), AttributeValue::String("before".into()))],
            std::slice::from_ref(&player),
        )
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    people.update_exact(&iid, &vec![]).await.unwrap();
    relations
        .update_exact(&relation, &vec![], &[player])
        .await
        .unwrap();
    let people_after = people.get_by_iid_exact(&iid).await.unwrap().unwrap();
    assert_eq!(
        people_after.attributes,
        vec![("identifier".into(), AttributeValue::String("clear".into()))]
    );
    let relations_after = relations.get_by_iid_exact(&relation).await.unwrap();
    assert_eq!(relations_after.len(), 1);
    assert!(relations_after[0].attributes.is_empty());
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

fn party_type(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn party_text(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: &str,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        party_type(TypeKind::Attribute, attribute),
        CanonicalValue::String(CanonicalString::new(value).unwrap()),
    )
    .unwrap()
}

fn membership(
    installed: &InstalledRuntimeProjection,
    reference: ProjectedReference,
    note: &str,
) -> ProjectedCreate {
    let membership = party_type(TypeKind::Relation, "membership");
    let note_field = OwnsFactId::new(
        membership.clone(),
        type_bridge_contract::id::AttributeId::new("note").unwrap(),
    )
    .unwrap();
    ProjectedCreate::try_new(
        installed,
        membership,
        vec![(note_field, vec![party_text(installed, "note", note)])],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
    )
    .unwrap()
}

fn key_reference(
    installed: &InstalledRuntimeProjection,
    label: &str,
    key: &str,
) -> ProjectedReference {
    let referenced = party_type(TypeKind::Entity, label);
    let party = party_type(TypeKind::Entity, "party");
    let field = OwnsFactId::new(
        party,
        type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
    )
    .unwrap();
    let field = installed.projection().models()[&referenced]
        .query_tokens()
        .fields()
        .values()
        .find(|token| token.declaring_id() == &field)
        .map(|token| token.id().clone())
        .unwrap();
    ProjectedReference::try_new(
        installed,
        referenced,
        None,
        vec![(field, party_text(installed, "identifier", key))],
    )
    .unwrap()
}

/// Relation inserts resolve exact, polymorphic and IID references inside the
/// write's own pipeline; a missing reference writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_relation_inserts_resolve_references_inside_the_write() {
    let Some(db) = live_database("tb_plan_references").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada"; $c isa company, has identifier "acme";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let member = RoleId::new("membership", "member").unwrap();
    let membership_type = party_type(TypeKind::Relation, "membership");

    // Exact: one candidate type.
    let exact = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "person", "ada"),
                "exact",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    // Hydrated from the write's own fetch, equal to a separate read.
    let read = crud
        .get_relation_by_iid(&db, &membership_type, exact.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), exact.fields());
    assert_eq!(read.roles(), exact.roles());
    let ada = &exact.roles()[&member][0];
    assert_eq!(ada.type_id(), &party_type(TypeKind::Entity, "person"));

    // Polymorphic: the abstract `party` resolves through a disjunction.
    let polymorphic = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "party", "acme"),
                "polymorphic",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(
        polymorphic.roles()[&member][0].type_id(),
        &party_type(TypeKind::Entity, "company")
    );

    // By IID, through the abstract type as well.
    let by_iid = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "party"),
        Some(ada.iid().to_owned()),
        vec![],
    )
    .unwrap();
    let again = crud
        .insert_relation(&db, &membership(&installed, by_iid, "iid"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(again.roles()[&member][0].iid(), ada.iid());

    // Repeated references follow TypeQL semantics and report the actual player set.
    let party_iid = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "party"),
        Some(ada.iid().to_owned()),
        vec![],
    )
    .unwrap();
    let doubled = ProjectedCreate::try_new(
        &installed,
        membership_type.clone(),
        vec![],
        vec![(
            member.clone(),
            vec![party_iid, key_reference(&installed, "party", "ada")],
        )],
    )
    .unwrap();
    let duplicate = crud.insert_relation(&db, &doubled).await.unwrap();
    assert_eq!(duplicate.count(), 1);
    assert_eq!(duplicate.item().unwrap().roles()[&member].len(), 1);
    assert_eq!(
        duplicate.item().unwrap().roles()[&member][0].iid(),
        ada.iid()
    );

    // Missing: the write matches no row, so nothing is stored.
    let missing = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "party", "nobody"),
                "missing",
            ),
        )
        .await
        .unwrap();
    assert_eq!(missing.count(), 0);
    assert_eq!(
        crud.count_relations(&db, &membership_type).await.unwrap(),
        4
    );

    db.delete_database().await.unwrap();
    db.close().unwrap();
}

const PERSON_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  email: { value: string }
  nickname: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      email: { card: { min: 0, max: 1 } }
      nickname: { card: { min: 0 } }
  employee:
    sub: person
"#;

const PERSON_SCHEMA_TYPEQL: &str = "define
attribute identifier, value string; attribute email, value string;
attribute nickname, value string;
entity person, owns identifier @key, owns email @card(0..1), owns nickname @card(0..);
entity employee sub person;";

fn person_projection() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-person.yaml").unwrap(),
        PERSON_SCHEMA_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

fn person_field(name: &str) -> OwnsFactId {
    OwnsFactId::new(
        party_type(TypeKind::Entity, "person"),
        type_bridge_contract::id::AttributeId::new(name).unwrap(),
    )
    .unwrap()
}

fn person(
    installed: &InstalledRuntimeProjection,
    key: &str,
    email: Option<&str>,
) -> ProjectedCreate {
    let mut fields = vec![(
        person_field("identifier"),
        vec![party_text(installed, "identifier", key)],
    )];
    if let Some(email) = email {
        fields.push((
            person_field("email"),
            vec![party_text(installed, "email", email)],
        ));
    }
    ProjectedCreate::try_new(
        installed,
        party_type(TypeKind::Entity, "person"),
        fields,
        vec![],
    )
    .unwrap()
}

fn texts(thing: &type_bridge_orm::ProjectedThing, field: &str) -> Vec<String> {
    thing
        .fields()
        .get(
            &OwnsFactId::new(
                thing.type_id().clone(),
                type_bridge_contract::id::AttributeId::new(field).unwrap(),
            )
            .unwrap(),
        )
        .map(|values| {
            values
                .iter()
                .map(|value| match value.value() {
                    CanonicalValue::String(text) => text.as_str().to_owned(),
                    other => panic!("expected text, got {other:?}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A put is one plan: it creates a new key, replaces every non-key field of
/// an existing one (omitted fields are cleared, as the helper always did),
/// and two puts racing on one new key serialize on its `@key` lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_put_is_one_plan_with_replace_semantics_and_key_races() {
    let Some(db) = live_database("tb_plan_put").await else {
        return;
    };
    db.execute_raw(PERSON_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = person_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");

    // New key: inserted.
    let created = crud
        .put_entity(&db, &person(&installed, "ada", Some("a@example.test")))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(texts(&created, "email"), ["a@example.test"]);
    db.execute_raw(
        r#"match $p isa person, has identifier "ada"; insert $p has nickname "A", has nickname "Ad";"#,
        TxType::Write,
    )
    .await
    .unwrap();

    // Existing key, different fields: updated in place; the omitted
    // multi-valued nickname is cleared.
    let updated = crud
        .put_entity(&db, &person(&installed, "ada", Some("b@example.test")))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(updated.iid(), created.iid());
    assert_eq!(texts(&updated, "email"), ["b@example.test"]);
    let read = crud
        .get_entity_by_iid(&db, &person_type, updated.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), updated.fields());
    assert!(texts(&updated, "nickname").is_empty());

    // Partial attribute set (key only): the stored email is cleared, not kept.
    let partial = crud
        .put_entity(&db, &person(&installed, "ada", None))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(partial.iid(), created.iid());
    assert!(texts(&partial, "email").is_empty());
    assert_eq!(crud.count_entities(&db, &person_type).await.unwrap(), 1);

    // Two puts of one new key in overlapping transactions: the first commit
    // wins, the second is rejected and stores nothing.
    let first = db.transaction_context(TxType::Write).await.unwrap();
    let second = db.transaction_context(TxType::Write).await.unwrap();
    crud.put_entity_in_transaction(&first, &person(&installed, "bob", Some("1@example.test")))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    crud.put_entity_in_transaction(&second, &person(&installed, "bob", Some("2@example.test")))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    first.commit_classified().await.unwrap();
    assert!(second.commit_classified().await.is_err());
    assert_eq!(crud.count_entities(&db, &person_type).await.unwrap(), 2);
    let bob = db
        .execute_raw(
            r#"match $p isa person, has identifier "bob", has email "1@example.test"; select $p;"#,
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&bob), 1);

    // A subtype instance holding the key is not a match for the exact
    // type: the put inserts, collides on the key, and leaves it untouched.
    db.execute_raw(
        r#"insert $e isa employee, has identifier "emp", has email "e@example.test";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    assert!(
        crud.put_entity(&db, &person(&installed, "emp", Some("x@example.test")))
            .await
            .is_err()
    );
    let employee = db
        .execute_raw(
            r#"match $e isa! employee, has identifier "emp", has email "e@example.test"; select $e;"#,
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&employee), 1);

    db.delete_database().await.unwrap();
    db.close().unwrap();
}

fn person_with(
    installed: &InstalledRuntimeProjection,
    key: &str,
    email: Option<&str>,
    nicknames: &[&str],
) -> ProjectedCreate {
    let mut fields = vec![(
        person_field("identifier"),
        vec![party_text(installed, "identifier", key)],
    )];
    if let Some(email) = email {
        fields.push((
            person_field("email"),
            vec![party_text(installed, "email", email)],
        ));
    }
    fields.push((
        person_field("nickname"),
        nicknames
            .iter()
            .map(|nickname| party_text(installed, "nickname", nickname))
            .collect(),
    ));
    ProjectedCreate::try_new(
        installed,
        party_type(TypeKind::Entity, "person"),
        fields,
        vec![],
    )
    .unwrap()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// Insert, update, put and delete are one plan each, with the helpers'
/// complete-replacement semantics, multi-valued fields and relation roles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_single_writes_are_one_plan_each() {
    let Some(db) = live_database("tb_plan_single").await else {
        return;
    };
    db.execute_raw(PERSON_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = person_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");

    let inserted = crud
        .insert_entity(
            &db,
            &person_with(&installed, "ada", Some("a@x.test"), &["x", "y"]),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(sorted(texts(&inserted, "nickname")), ["x", "y"]);
    let updated = crud
        .update_entity(
            &db,
            inserted.iid(),
            &person_with(&installed, "ada", Some("b@x.test"), &["z"]),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(texts(&updated, "email"), ["b@x.test"]);
    assert_eq!(texts(&updated, "nickname"), ["z"]);
    let read = crud
        .get_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), updated.fields());
    // A put giving a multi-valued field replaces its values in one plan.
    let put = crud
        .put_entity(&db, &person_with(&installed, "ada", None, &["p", "q"]))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(put.iid(), inserted.iid());
    assert_eq!(sorted(texts(&put, "nickname")), ["p", "q"]);
    assert!(texts(&put, "email").is_empty());
    crud.delete_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap();
    assert!(
        crud.get_entity_by_iid(&db, &person_type, inserted.iid())
            .await
            .unwrap()
            .is_none()
    );
    // Deleting an absent thing is not an error.
    crud.delete_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap();
    // In a transaction the delete lands at commit; absent is not an error.
    let staged = crud
        .insert_entity(&db, &person_with(&installed, "bob", None, &[]))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    for _ in 0..2 {
        crud.delete_entity_by_iid_in_transaction(&tx, &person_type, staged.iid())
            .await
            .unwrap();
    }
    assert!(
        crud.get_entity_by_iid(&db, &person_type, staged.iid())
            .await
            .unwrap()
            .is_some()
    );
    tx.commit_classified().await.unwrap();
    assert!(
        crud.get_entity_by_iid(&db, &person_type, staged.iid())
            .await
            .unwrap()
            .is_none()
    );
    db.delete_database().await.unwrap();

    let Some(db) = live_database("tb_plan_single_rel").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada"; $c isa company, has identifier "acme";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let member = RoleId::new("membership", "member").unwrap();
    let membership_type = party_type(TypeKind::Relation, "membership");
    let relation = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "person", "ada"),
                "first",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    // Relation update clears the role and attaches the new player.
    let moved = crud
        .update_relation(
            &db,
            relation.iid(),
            &membership(
                &installed,
                key_reference(&installed, "party", "acme"),
                "second",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(moved.iid(), relation.iid());
    assert_eq!(moved.roles()[&member].len(), 1);
    assert_eq!(
        moved.roles()[&member][0].type_id(),
        &party_type(TypeKind::Entity, "company")
    );
    let read = crud
        .get_relation_by_iid(&db, &membership_type, relation.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), moved.fields());
    assert_eq!(read.roles(), moved.roles());
    for _ in 0..2 {
        crud.delete_relation_by_iid(&db, &membership_type, relation.iid())
            .await
            .unwrap();
    }
    assert_eq!(
        crud.count_relations(&db, &membership_type).await.unwrap(),
        0
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

const TAGGING_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  tagging:
    relates:
      target: { card: { min: 0, max: 2 } }
plays:
  person:
    tagging: [target]
"#;

/// Relation updates with several players stay one document set; an update
/// with no players runs the released path, which refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_relation_updates_with_many_players_and_none() {
    let Some(db) = live_database("tb_plan_rel_update").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada"; $c isa company, has identifier "acme";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let member = RoleId::new("membership", "member").unwrap();
    let two = |installed: &InstalledRuntimeProjection, note: &str| {
        let mut create = membership(installed, key_reference(installed, "person", "ada"), note);
        create = ProjectedCreate::try_new(
            installed,
            party_type(TypeKind::Relation, "membership"),
            create
                .fields()
                .iter()
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect(),
            vec![(
                member.clone(),
                vec![
                    key_reference(installed, "person", "ada"),
                    key_reference(installed, "party", "acme"),
                ],
            )],
        )
        .unwrap();
        create
    };
    let relation = crud
        .insert_relation(&db, &two(&installed, "a"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(relation.roles()[&member].len(), 2);
    // Two old players times two new ones: still one relation document set.
    let updated = crud
        .update_relation(&db, relation.iid(), &two(&installed, "b"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(updated.roles()[&member].len(), 2);
    db.delete_database().await.unwrap();

    let Some(db) = live_database("tb_plan_rel_empty").await else {
        return;
    };
    db.execute_raw(
        "define attribute identifier, value string; \
         entity person, owns identifier @key, plays tagging:target; \
         relation tagging, relates target @card(0..2);",
        TxType::Schema,
    )
    .await
    .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-tagging.yaml").unwrap(),
        TAGGING_SCHEMA_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let crud = ProjectedCrudExecutor::new(&installed);
    let tagging = party_type(TypeKind::Relation, "tagging");
    let target = RoleId::new("tagging", "target").unwrap();
    let ada = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "person"),
        None,
        vec![(
            person_field("identifier"),
            party_text(&installed, "identifier", "ada"),
        )],
    )
    .unwrap();
    let _tagged = crud
        .insert_relation(
            &db,
            &ProjectedCreate::try_new(
                &installed,
                tagging.clone(),
                vec![],
                vec![(target, vec![ada])],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let refused =
        ProjectedCreate::try_new(&installed, tagging.clone(), vec![], vec![]).unwrap_err();
    assert_eq!(refused.code().as_str(), "relation_requires_player");
    assert_eq!(crud.count_relations(&db, &tagging).await.unwrap(), 1);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_documents_resolve_and_enforce_answer_limits_before_commit() {
    use type_bridge_orm::session::backend::BoundedAnswerLimits;
    let Some(db) = live_database("tb_fetch_documents").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let plan = race::insert_doctor_fetch("alice");
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let answer = pending.get().unwrap();
    let PlanAnswer::Documents(documents) = answer.as_ref() else {
        panic!("{answer:?}");
    };
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["_type"], "doctor");
    assert_eq!(
        documents[0]["attributes"]["name"],
        serde_json::json!("alice")
    );
    let wire = answer.to_staged(&plan).unwrap();
    assert_eq!(PlanAnswer::from_staged(&wire, &plan).unwrap(), *answer);
    for (max_items, max_bytes, code) in [
        (0, 4096, "processed_item_limit"),
        (10, 1, "response_byte_limit"),
    ] {
        let mut tx = db.transaction(TransactionOptions::default().with_answer_limits(
            BoundedAnswerLimits {
                max_items,
                max_bytes,
                ..Default::default()
            },
        ));
        tx.stage_plan(&race::touch_fetch()).unwrap();
        assert_eq!(
            tx.commit().await.unwrap_err().diagnostic().code().as_str(),
            code
        );
    }
    db.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_snapshots_return_pre_images_and_rollback_with_the_stack() {
    use type_bridge_orm::session::backend::BoundedAnswerLimits;
    let Some(db) = live_database("tb_delete_snapshots").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        "insert $d isa doctor, has name \"alice\", has on-call true;",
        TxType::Write,
    )
    .await
    .unwrap();
    let plan = race::delete_doctors_returning();
    let mut tight = db.transaction(TransactionOptions::default().with_answer_limits(
        BoundedAnswerLimits {
            max_items: 0,
            ..Default::default()
        },
    ));
    tight.stage_plan(&plan).unwrap();
    assert_eq!(
        tight
            .commit()
            .await
            .unwrap_err()
            .diagnostic()
            .code()
            .as_str(),
        "processed_item_limit"
    );
    let mut failed = db.transaction(TransactionOptions::default());
    let pending = failed.stage_plan(&plan).unwrap();
    failed
        .stage(StagedQuery::new(
            "match $d isa nonexistent_snapshot_type;",
            false,
        ))
        .unwrap();
    assert!(failed.commit().await.is_err());
    assert!(pending.get().is_err());
    assert_eq!(
        rows(
            &db.execute_raw("match $d isa doctor; select $d;", TxType::Read)
                .await
                .unwrap()
        ),
        1
    );
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let answer = pending.get().unwrap();
    let PlanAnswer::Documents(documents) = answer.as_ref() else {
        panic!("{answer:?}");
    };
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["_type"], "doctor");
    assert_eq!(documents[0]["attributes"]["name"], "alice");
    assert_eq!(documents[0]["attributes"]["on-call"], true);
    assert_eq!(
        rows(
            &db.execute_raw("match $d isa doctor; select $d;", TxType::Read)
                .await
                .unwrap()
        ),
        0
    );
    let empty = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        empty.get().unwrap().as_ref(),
        &PlanAnswer::Documents(vec![])
    );
    db.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_snapshots_preserve_optional_roles_and_refuse_missing_required_evidence() {
    let Some(db) = live_database("tb_delete_snapshot_roles").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw("insert $i isa item, has item-id \"I1\", has stock 1; $p isa purchase, links (item: $i), has order-id \"O1\";", TxType::Write).await.unwrap();
    let mut required = db.transaction(TransactionOptions::default());
    required
        .stage_plan(&race::delete_purchases_returning(true))
        .unwrap();
    assert!(required.commit().await.is_err());
    assert_eq!(
        rows(
            &db.execute_raw("match $p isa purchase; select $p;", TxType::Read)
                .await
                .unwrap()
        ),
        1
    );
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx
        .stage_plan(&race::delete_purchases_returning(false))
        .unwrap();
    tx.commit().await.unwrap();
    let answer = pending.get().unwrap();
    let PlanAnswer::Documents(documents) = answer.as_ref() else {
        panic!("{answer:?}");
    };
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["_type"], "purchase");
    assert_eq!(documents[0]["attributes"]["order-id"], "O1");
    assert!(
        documents[0]
            .get("_role_0_iid")
            .is_none_or(serde_json::Value::is_null)
    );
    assert_eq!(documents[0]["_role_1_type"], "item");
    assert_eq!(documents[0]["_role_1_attributes"]["item-id"], "I1");
    assert_eq!(
        rows(
            &db.execute_raw("match $p isa purchase; select $p;", TxType::Read)
                .await
                .unwrap()
        ),
        0
    );
    assert_eq!(
        rows(
            &db.execute_raw("match $i isa item; select $i;", TxType::Read)
                .await
                .unwrap()
        ),
        1
    );
    db.delete_database().await.unwrap();
}

const NATIVE_BATCH_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
  score: { value: integer }
entities:
  person:
    owns:
      identifier: { key: true }
      tag: { card: { min: 0 } }
      score: { card: { min: 0 } }
relations:
  pair:
    owns:
      identifier: { key: true }
    relates:
      left: { card: { min: 0, max: 4 } }
      right: { card: { min: 0, max: 4 } }
plays:
  person:
    pair: [left, right]
"#;

fn native_batch_projection() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("native-batch.yaml").unwrap(),
        NATIVE_BATCH_YAML,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_batches_keep_key_identity_replace_players_and_collect_without_cartesian_rows() {
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::{
        DocumentRole, DocumentScalar, Expr, Pattern, PlanSpec, QueryPlan, Stage, ThingDocument,
    };
    use type_bridge_contract::query_plan::{QueryOutput, QueryPattern};
    use type_bridge_orm::{
        ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchOperation as Op,
        ProjectedBatchResult, ProjectedBatchRow as Row, ProjectedThing,
    };
    let Some(db) = live_database("native_batches").await else {
        return;
    };
    db.execute_raw("define attribute identifier, value string; attribute tag, value string; attribute score, value integer; entity person, owns identifier @key, owns tag @card(0..), owns score @card(0..), plays pair:left, plays pair:right; relation pair, owns identifier @key, relates left @card(0..4), relates right @card(0..4);", TxType::Schema).await.unwrap();
    let installed = native_batch_projection();
    assert_schema_less_typed_read_refuses(&db, &installed).await;
    let executor = ProjectedBatchExecutor::new(&installed);
    let person = party_type(TypeKind::Entity, "person");
    let pair = party_type(TypeKind::Relation, "pair");
    let id_field = |owner: &TypeId| {
        OwnsFactId::new(
            owner.clone(),
            type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
        )
        .unwrap()
    };
    let create_person = |key: &str| {
        ProjectedCreate::try_new(
            &installed,
            person.clone(),
            vec![(
                id_field(&person),
                vec![party_text(&installed, "identifier", key)],
            )],
            vec![],
        )
        .unwrap()
    };
    let batch = |model: &TypeId, operation, rows| {
        ProjectedBatch::try_new(&installed, model.clone(), operation, rows).unwrap()
    };
    let things = |result| {
        let ProjectedBatchResult::Things(things) = result else {
            panic!("expected things")
        };
        assert_eq!(
            things.input_indexes(),
            (0..things.count()).collect::<Vec<_>>()
        );
        things.into_items()
    };
    let people = things(
        executor
            .execute(
                &db,
                &batch(
                    &person,
                    Op::Insert,
                    (0..6)
                        .map(|index| Row::Create(create_person(&format!("p{index}"))))
                        .collect(),
                ),
                type_bridge_orm::ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(people.len(), 6);
    let create_pair = |key: &str, left: &[usize], right: &[usize]| {
        ProjectedCreate::try_new(
            &installed,
            pair.clone(),
            vec![(
                id_field(&pair),
                vec![party_text(&installed, "identifier", key)],
            )],
            [("left", left), ("right", right)]
                .into_iter()
                .map(|(name, indices)| {
                    (
                        RoleId::new("pair", name).unwrap(),
                        indices
                            .iter()
                            .map(|index| people[*index].try_to_reference(&installed).unwrap())
                            .collect(),
                    )
                })
                .collect(),
        )
        .unwrap()
    };
    let pairs = things(
        executor
            .execute(
                &db,
                &batch(
                    &pair,
                    Op::Insert,
                    vec![
                        Row::Create(create_pair("A", &[0, 1, 2], &[3, 4, 5])),
                        Row::Create(create_pair("empty", &[0], &[])),
                    ],
                ),
                type_bridge_orm::ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(pairs.len(), 2);
    let old_iid = pairs[0].iid().to_owned();
    let left = RoleId::new("pair", "left").unwrap();
    let right = RoleId::new("pair", "right").unwrap();
    assert_eq!(pairs[0].roles()[&left].len(), 3);
    assert_eq!(pairs[0].roles()[&right].len(), 3);
    assert!(pairs[1].roles().get(&right).is_none_or(Vec::is_empty));
    // The same terminal shape used by the batch emits one root and charges six player documents.
    let root = BindingId::new(0).unwrap();
    let ordinal = BindingId::new(1).unwrap();
    let mut document = ThingDocument::new(
        root,
        vec![
            DocumentRole::collected(left.clone()),
            DocumentRole::collected(right.clone()),
        ],
    );
    document.scalars.push(DocumentScalar {
        key: QueryVariable::new("ordinal").unwrap(),
        binding: ordinal,
    });
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(
            DocumentId::new("native-batch.yaml").unwrap(),
            NATIVE_BATCH_YAML,
        )])
        .unwrap(),
    )
    .unwrap();
    let plan = QueryPlan::new(
        PlanSpec {
            bindings: vec![
                AssertionBinding::new(root, QueryVariable::new("pair").unwrap()),
                AssertionBinding::new(ordinal, QueryVariable::new("ordinal").unwrap()),
            ],
            inputs: vec![],
            given_rows: None,
            operation: Operation::Rows,
            pipeline: vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Isa {
                            binding: root,
                            type_id: pair.clone(),
                            include_subtypes: false,
                        }
                        .into(),
                        Pattern::Iid {
                            binding: root,
                            iid: old_iid.clone(),
                        },
                        Pattern::Let {
                            binding: ordinal,
                            expr: Expr::Operand(
                                type_bridge_contract::query_plan::QueryOperand::Literal {
                                    value: CanonicalValue::Long(0),
                                },
                            ),
                        },
                    ],
                },
                Stage::Fetch(document),
            ],
            output: QueryOutput::Rows { columns: vec![] },
            managed_semantics:
                type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint::compute(
                    SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                    b"batch-live",
                )
                .unwrap(),
            functions: vec![],
        },
        &declared,
    )
    .unwrap();
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let answer = pending.get().unwrap();
    let PlanAnswer::Documents(documents) = answer.as_ref() else {
        panic!("documents")
    };
    assert_eq!(documents.len(), 1);
    let wire = answer.to_staged(&plan).unwrap();
    assert_eq!(wire.item_count(), 7);
    assert_eq!(wire.resource_counts(), [7, 7, 13, 6]);
    println!("collected role items: 7 (1 root + 6 players); Cartesian triples: 9 root documents");
    let mut tight = db.transaction(TransactionOptions::default().with_answer_limits(
        type_bridge_orm::session::backend::BoundedAnswerLimits {
            max_items: 6,
            ..Default::default()
        },
    ));
    tight.stage_plan(&plan).unwrap();
    assert_eq!(
        tight
            .commit()
            .await
            .unwrap_err()
            .diagnostic()
            .code()
            .as_str(),
        "processed_item_limit"
    );
    let put = things(
        executor
            .execute(
                &db,
                &batch(
                    &pair,
                    Op::Put,
                    vec![
                        Row::Create(create_pair("A", &[5], &[0])),
                        Row::Create(create_pair("new", &[1], &[2])),
                    ],
                ),
                type_bridge_orm::ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(put[0].iid(), old_iid);
    assert_eq!(put[0].roles()[&left].len(), 1);
    assert_eq!(put[0].roles()[&left][0].iid(), people[5].iid());
    assert_eq!(put[0].roles()[&right][0].iid(), people[0].iid());
    assert_eq!(put[1].roles()[&left][0].iid(), people[1].iid());
    // Clearing two multi-valued fields must keep one root between deletes and permit key replacement.
    let mut replacement = vec![(
        id_field(&person),
        vec![party_text(&installed, "identifier", "changed")],
    )];
    for (attribute, values) in [
        ("tag", vec!["a", "b", "c"]),
        ("score", vec!["1", "2", "3", "4"]),
    ] {
        let values = values
            .into_iter()
            .map(|value| {
                if attribute == "tag" {
                    party_text(&installed, attribute, value)
                } else {
                    ProjectedAttributeValue::try_new(
                        &installed,
                        party_type(TypeKind::Attribute, attribute),
                        CanonicalValue::Long(value.parse().unwrap()),
                    )
                    .unwrap()
                }
            })
            .collect();
        replacement.push((
            OwnsFactId::new(
                person.clone(),
                type_bridge_contract::id::AttributeId::new(attribute).unwrap(),
            )
            .unwrap(),
            values,
        ));
    }
    let target = people[0].iid().to_owned();
    let update = |create| {
        batch(
            &person,
            Op::Update,
            vec![Row::Update {
                iid: target.clone(),
                replacement: create,
            }],
        )
    };
    let changed = things(
        executor
            .execute(
                &db,
                &update(
                    ProjectedCreate::try_new(&installed, person.clone(), replacement, vec![])
                        .unwrap(),
                ),
                type_bridge_orm::ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(changed[0].iid(), target);
    let cleared: Vec<ProjectedThing> = things(
        executor
            .execute(
                &db,
                &update(create_person("changed-again")),
                type_bridge_orm::ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(cleared[0].fields().len(), 3);
    for attribute in ["tag", "score"] {
        let field = OwnsFactId::new(
            person.clone(),
            type_bridge_contract::id::AttributeId::new(attribute).unwrap(),
        )
        .unwrap();
        assert!(
            cleared[0].fields()[&field].is_empty(),
            "cleared {attribute}"
        );
    }
    assert_eq!(rows(&db.execute_raw("match $p isa person, has identifier \"changed-again\"; { $p has tag $v; } or { $p has score $v; }; select $p;", TxType::Read).await.unwrap()), 0);
    assert_schema_less_typed_read_refuses(&db, &installed).await;
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

async fn assert_schema_less_typed_read_refuses(
    db: &Database,
    installed: &InstalledRuntimeProjection,
) {
    use std::sync::Arc;
    use type_bridge_orm::match_request::SessionHandle;
    let registry = Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap();
    let request = query.validate_count_by(&person).unwrap();
    let type_bridge_orm::OrmError::Match(error) =
        db.execute_match(&registry, &request).await.unwrap_err()
    else {
        panic!("expected typed refusal")
    };
    assert_eq!(error.code().as_str(), "schema_authority_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playerless_relations_do_not_survive_commit() {
    let Some(db) = live_database("playerless").await else {
        return;
    };
    db.execute_raw(
        "define relation optional-pair, relates left @card(0..), relates right @card(0..);",
        TxType::Schema,
    )
    .await
    .unwrap();
    let inserted = db
        .execute_raw("insert $r isa optional-pair;", TxType::Write)
        .await;
    let persisted = rows(
        &db.execute_raw("match $r isa optional-pair; select $r;", TxType::Read)
            .await
            .unwrap(),
    );
    assert_eq!(
        persisted, 0,
        "native playerless creation requires a capability if this profile can persist it"
    );
    println!(
        "playerless relation: insert_refused={}, persisted_after_commit={persisted}",
        inserted.is_err()
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn negated_given_role_iid_preserves_the_provider_connection() {
    let Some(db) = live_database("negated_given_iid").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(r#"insert $i isa item, has item-id "I1"; $a isa account, has account-id "A1"; $p isa purchase, links (buyer: $a, item: $i), has order-id "O1";"#, TxType::Write).await.unwrap();
    let QueryResult::Documents(accounts) = db
        .execute_raw(
            r#"match $a isa account; fetch {"iid":iid($a)};"#,
            TxType::Read,
        )
        .await
        .unwrap()
    else {
        panic!("account identity");
    };
    let iid = accounts[0]["iid"].as_str().unwrap();
    let mut tx = db.transaction(TransactionOptions::default());
    for (negative, optional, expected) in [(false, false, 1), (false, true, 1), (true, false, 0)] {
        let staged = tx.stage_plan(&race::scoped_given_iid(iid, negative, optional));
        if negative
            && !type_bridge_contract::query_plan::supports_negated_given_iid(
                db.server_version()
                    .as_ref()
                    .and_then(type_bridge_core_lib::version::semantic_profile_id),
            )
        {
            let Err(type_bridge_orm::session::staged::PendingError::InvalidPlan(diagnostic)) =
                staged
            else {
                panic!("unsupported negated given IID must refuse before dispatch");
            };
            assert_eq!(
                diagnostic.code().as_str(),
                "query_plan_negated_given_iid_unsupported"
            );
            continue;
        }
        let pending = staged.unwrap();
        tx.commit().await.unwrap();
        assert!(
            matches!(&*pending.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == expected)
        );
    }
    assert_eq!(
        rows(
            &db.execute_raw("match $p isa purchase; select $p;", TxType::Read)
                .await
                .unwrap()
        ),
        1
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[derive(Debug)]
struct HandwrittenParty {
    iid: Option<String>,
    concrete: String,
    identifier: String,
}
impl type_bridge_orm::_entity::TypeBridgeEntity for HandwrittenParty {
    const TYPE_NAME: &'static str = "party";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        vec![(
            "identifier",
            type_bridge_orm::AttributeValue::String(self.identifier.clone()),
        )]
    }
    fn from_document(
        document: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, type_bridge_orm::OrmError> {
        Ok(Self {
            iid: None,
            concrete: document["_type"].as_str().unwrap().to_owned(),
            identifier: document["identifier"].as_str().unwrap().to_owned(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handwritten_returning_delete_dispatches_exact_subtypes_and_preserves_relation_players() {
    use std::sync::Arc;
    use type_bridge_orm::_manager::{EntityManager, RelationManager};
    let Some(db) = live_database("handwritten_returning").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = party_projection_with_authority();
    let registry = Arc::new(installed.match_registry().unwrap());
    let crud = ProjectedCrudExecutor::new(&installed);
    let mut parties = Vec::new();
    for (name, key) in [("person", "p"), ("company", "c")] {
        let kind = party_type(TypeKind::Entity, name);
        let create = ProjectedCreate::try_new(
            &installed,
            kind.clone(),
            vec![(
                OwnsFactId::new(
                    kind,
                    type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
                )
                .unwrap(),
                vec![party_text(&installed, "identifier", key)],
            )],
            vec![],
        )
        .unwrap();
        parties.push(
            crud.insert_entity(&db, &create)
                .await
                .unwrap()
                .into_items()
                .pop()
                .expect("one written root"),
        );
    }
    let relation = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                parties[0].try_to_reference(&installed).unwrap(),
                "preimage",
            ),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    #[derive(Debug)]
    struct MembershipSnapshot {
        iid: Option<String>,
        players: Vec<String>,
    }
    impl type_bridge_orm::_relation::TypeBridgeRelation for MembershipSnapshot {
        const TYPE_NAME: &'static str = "membership";
        fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
            &[]
        }
        fn role_info() -> &'static [type_bridge_orm::_relation::RoleInfo] {
            &[]
        }
        fn iid(&self) -> Option<&str> {
            self.iid.as_deref()
        }
        fn set_iid(&mut self, iid: String) {
            self.iid = Some(iid);
        }
        fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
            vec![]
        }
        fn to_role_player_refs(&self) -> Vec<type_bridge_orm::_relation::RolePlayerRef> {
            vec![]
        }
        fn from_document(
            document: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<Self, type_bridge_orm::OrmError> {
            Ok(Self {
                iid: None,
                players: document["role_players"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|player| player["player_iid"].as_str().unwrap().to_owned())
                    .collect(),
            })
        }
    }
    let relations =
        RelationManager::<MembershipSnapshot>::from_registry(&db, Arc::clone(&registry)).unwrap();
    let snapshots = relations
        .delete_many_returning(&[MembershipSnapshot {
            iid: Some(relation.iid().into()),
            players: vec![],
        }])
        .await
        .unwrap();
    assert_eq!(snapshots[0].iid.as_deref(), Some(relation.iid()));
    assert_eq!(snapshots[0].players, [parties[0].iid()]);
    let manager = EntityManager::<HandwrittenParty>::from_registry(&db, registry).unwrap();
    let input = parties
        .iter()
        .map(|party| HandwrittenParty {
            iid: Some(party.iid().into()),
            concrete: String::new(),
            identifier: String::new(),
        })
        .collect::<Vec<_>>();
    let deleted = manager.delete_many_returning(&input).await.unwrap();
    assert_eq!(
        deleted
            .iter()
            .map(|party| party.concrete.as_str())
            .collect::<Vec<_>>(),
        ["person", "company"]
    );
    assert_eq!(
        deleted
            .iter()
            .map(|party| party.identifier.as_str())
            .collect::<Vec<_>>(),
        ["p", "c"]
    );
    assert_eq!(
        rows(
            &db.execute_raw("match $p isa party; select $p;", TxType::Read)
                .await
                .unwrap()
        ),
        0
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn native_batches_preserve_integer_keys_and_model_ownership_constraints() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_orm::session::backend::AnswerCancellation;
    use type_bridge_orm::{
        ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchInvocationControl,
        ProjectedBatchOperation, ProjectedBatchResult, ProjectedBatchRow,
        QueryExecutionResourceLimits,
    };
    let Some(db) = live_database("integer_key_batch").await else {
        return;
    };
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("models.yaml").unwrap(),
        include_str!("../../../../tests/contracts/sdk_conformance/models/schema.yaml"),
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("integer-key-batch").unwrap(),
        profile,
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    let installed = installed.with_schema_authority(&authority);
    db.execute_raw(
        include_str!("../../../../tests/contracts/sdk_conformance/models/provider-3.12.1.tql"),
        TxType::Schema,
    )
    .await
    .unwrap();
    let create = |model: &TypeId, fields: Vec<(&str, CanonicalValue)>| {
        ProjectedCreate::try_new(
            &installed,
            model.clone(),
            fields
                .into_iter()
                .map(|(label, value)| {
                    (
                        OwnsFactId::new(model.clone(), AttributeId::new(label).unwrap()).unwrap(),
                        vec![
                            ProjectedAttributeValue::try_new(
                                &installed,
                                party_type(TypeKind::Attribute, label),
                                value,
                            )
                            .unwrap(),
                        ],
                    )
                })
                .collect(),
            vec![],
        )
        .unwrap()
    };
    let batch = |model: &TypeId, operation, rows| {
        ProjectedBatch::try_new(&installed, model.clone(), operation, rows).unwrap()
    };
    let control = || {
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    };
    let things = |result| {
        let ProjectedBatchResult::Things(things) = result else {
            panic!("model batch result");
        };
        assert_eq!(
            things.input_indexes(),
            (0..things.count()).collect::<Vec<_>>()
        );
        things.into_items()
    };
    let text = |value: &str| CanonicalValue::String(CanonicalString::new(value).unwrap());
    let executor = ProjectedBatchExecutor::new(&installed);
    let robot = party_type(TypeKind::Entity, "robot");
    let input = create(
        &robot,
        vec![
            ("robot_id", CanonicalValue::Long(100)),
            ("val_constrained", CanonicalValue::Long(10)),
        ],
    );
    let inserted = things(
        executor
            .execute(
                &db,
                &batch(
                    &robot,
                    ProjectedBatchOperation::Insert,
                    vec![ProjectedBatchRow::Create(input)],
                ),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(inserted.len(), 1);
    let iid = inserted[0].iid().to_owned();
    let replacement = create(
        &robot,
        vec![
            ("robot_id", CanonicalValue::Long(100)),
            ("val_constrained", CanonicalValue::Long(11)),
            ("nickname", text("Ada")),
        ],
    );
    let updated = things(
        executor
            .execute(
                &db,
                &batch(
                    &robot,
                    ProjectedBatchOperation::Update,
                    vec![ProjectedBatchRow::Update {
                        iid: iid.clone(),
                        replacement,
                    }],
                ),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(updated[0].iid(), iid);
    assert_eq!(texts(&updated[0], "nickname"), ["Ada"]);
    let replacement = create(
        &robot,
        vec![
            ("robot_id", CanonicalValue::Long(100)),
            ("val_constrained", CanonicalValue::Long(12)),
        ],
    );
    let replaced = things(
        executor
            .execute(
                &db,
                &batch(
                    &robot,
                    ProjectedBatchOperation::Put,
                    vec![ProjectedBatchRow::Create(replacement)],
                ),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(replaced[0].iid(), iid);
    assert!(texts(&replaced[0], "nickname").is_empty());
    assert_eq!(
        ProjectedCrudExecutor::new(&installed)
            .count_entities(&db, &robot)
            .await
            .unwrap(),
        1
    );

    let employee = party_type(TypeKind::Entity, "employee");
    let input = create(
        &employee,
        vec![
            ("identifier", text("native-employee")),
            ("party_name", text("Native")),
            ("rank", CanonicalValue::Long(1)),
        ],
    );
    let inserted = things(
        executor
            .execute(
                &db,
                &batch(
                    &employee,
                    ProjectedBatchOperation::Put,
                    vec![ProjectedBatchRow::Create(input)],
                ),
                control(),
            )
            .await
            .unwrap(),
    );
    let iid = inserted[0].iid().to_owned();
    let replacement = create(
        &employee,
        vec![
            ("identifier", text("native-employee")),
            ("party_name", text("Replaced")),
            ("rank", CanonicalValue::Long(2)),
        ],
    );
    let replaced = things(
        executor
            .execute(
                &db,
                &batch(
                    &employee,
                    ProjectedBatchOperation::Put,
                    vec![ProjectedBatchRow::Create(replacement)],
                ),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(replaced[0].iid(), iid);
    assert_eq!(texts(&replaced[0], "party_name"), ["Replaced"]);
    assert_eq!(
        ProjectedCrudExecutor::new(&installed)
            .count_entities(&db, &employee)
            .await
            .unwrap(),
        1
    );
    db.delete_database().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_relation_updates_follow_typeql_player_and_root_matching() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let Some(db) = live_database("missing_update_players").await else {
        return;
    };
    db.execute_raw(
        &PARTY_SCHEMA_TYPEQL
            .replace(
                "owns note @card(0..1)",
                "owns identifier @key, owns note @card(0..1)",
            )
            .replace(
                "entity person sub party;",
                "entity person sub party, owns note @card(0..1);",
            ),
        TxType::Schema,
    )
    .await
    .unwrap();
    let installed = party_projection_for_schema(
        &PARTY_SCHEMA_YAML
            .replace(
                "      note: { card: { min: 0, max: 1 } }",
                "      identifier: { key: true }\n      note: { card: { min: 0, max: 1 } }",
            )
            .replace(
                "  person:\n    sub: party",
                "  person:\n    sub: party\n    owns:\n      note: { card: { min: 0, max: 1 } }",
            ),
    );
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let person = people
        .insert(&vec![
            (
                "identifier".into(),
                AttributeValue::String("existing".into()),
            ),
            ("note".into(), AttributeValue::String("shared".into())),
        ])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    people
        .insert(&vec![
            ("identifier".into(), AttributeValue::String("other".into())),
            ("note".into(), AttributeValue::String("shared".into())),
        ])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let relations = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let valid = DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "person".into(),
        iid: Some(person),
        key: None,
    };
    let note = |value: &str| vec![("note".into(), AttributeValue::String(value.into()))];
    let mut initial = note("before");
    initial.push((
        "identifier".into(),
        AttributeValue::String("relation".into()),
    ));
    let iid = relations
        .insert(&initial, std::slice::from_ref(&valid))
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let missing_iid = DynamicRolePlayerInput {
        iid: Some("0x00".into()),
        ..valid.clone()
    };
    let missing_key = DynamicRolePlayerInput {
        iid: None,
        key: Some(("identifier".into(), AttributeValue::String("absent".into()))),
        ..valid.clone()
    };
    // Each write is one TypeQL statement: an unresolved player or root
    // matches nothing (count 0, relation unchanged); a non-unique key fans
    // out to every match; a duplicate alias of one player links it once.
    // Steps run in order, so each expected state follows from the last.
    macro_rules! check {
        ($label:expr, $call:expr, $count:expr, $players:expr, $note:expr) => {{
            let written = $call.await.unwrap();
            assert_eq!(written.count(), $count, "{} count", $label);
            let row = relations.get_by_iid_exact(&iid).await.unwrap();
            assert_eq!(row.len(), 1, "{} rows", $label);
            assert_eq!(row[0].role_players.len(), $players, "{} players", $label);
            assert!(
                row[0].attributes.contains(&note($note)[0]),
                "{} note: {:?}",
                $label,
                row[0].attributes
            );
        }};
    }
    for player in [missing_iid, missing_key] {
        let player = std::slice::from_ref(&player);
        check!(
            "missing update",
            relations.update(Some(&iid), &note("after"), player),
            0,
            1,
            "before"
        );
        check!(
            "missing update_exact",
            relations.update_exact(&iid, &note("after"), player),
            0,
            1,
            "before"
        );
        check!(
            "missing players-only update",
            relations.update(Some(&iid), &vec![], player),
            0,
            1,
            "before"
        );
        check!(
            "missing put_exact",
            relations.put_exact(&initial, player),
            0,
            1,
            "before"
        );
    }
    let ambiguous = vec![DynamicRolePlayerInput {
        iid: None,
        key: Some(("note".into(), AttributeValue::String("shared".into()))),
        ..valid.clone()
    }];
    let alias = vec![
        valid.clone(),
        DynamicRolePlayerInput {
            iid: None,
            key: Some((
                "identifier".into(),
                AttributeValue::String("existing".into()),
            )),
            ..valid.clone()
        },
    ];
    check!(
        "ambiguous update",
        relations.update(Some(&iid), &note("after"), &ambiguous),
        1,
        1,
        "after"
    );
    check!(
        "ambiguous update_exact",
        relations.update_exact(&iid, &note("after"), &ambiguous),
        1,
        2,
        "after"
    );
    check!(
        "ambiguous put_exact",
        relations.put_exact(&initial, &ambiguous),
        1,
        2,
        "before"
    );
    check!(
        "alias update",
        relations.update(Some(&iid), &note("after"), &alias),
        1,
        2,
        "after"
    );
    check!(
        "alias update_exact",
        relations.update_exact(&iid, &note("after"), &alias),
        1,
        1,
        "after"
    );
    check!(
        "alias put_exact",
        relations.put_exact(&initial, &alias),
        1,
        1,
        "before"
    );
    let valid_player = std::slice::from_ref(&valid);
    for attributes in [note("after"), vec![]] {
        check!(
            "missing root update",
            relations.update(Some("0x00"), &attributes, valid_player),
            0,
            1,
            "before"
        );
    }
    check!(
        "players-only update",
        relations.update(Some(&iid), &vec![], valid_player),
        1,
        1,
        "before"
    );
    check!(
        "missing root update_exact",
        relations.update_exact("0x00", &note("after"), valid_player),
        0,
        1,
        "before"
    );
    let row = relations.get_by_iid_exact(&iid).await.unwrap();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].role_players.len(), 1);
    assert_eq!(
        row[0].role_players[0].player_iid.as_deref(),
        valid.iid.as_deref()
    );
    assert!(row[0].attributes.contains(&note("before")[0]));
    assert!(row[0].attributes.contains(&(
        "identifier".into(),
        AttributeValue::String("relation".into())
    )));
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_bulk_insert_splits_six_megabytes_and_rolls_back_later_chunk_failure() {
    use std::sync::Arc;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::AttributeValue;
    let Some(db) = live_database("large_manager_insert").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = party_projection_with_authority();
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let items = (0..200)
        .map(|index| {
            vec![(
                "identifier".into(),
                AttributeValue::String(format!("{index:04}{}", "x".repeat(30_000))),
            )]
        })
        .collect::<Vec<_>>();
    let mut duplicates = items.clone();
    duplicates[199] = duplicates[0].clone();
    assert!(people.insert_many(&duplicates).await.is_err());
    assert_eq!(
        people.count().await.unwrap(),
        0,
        "a later failed chunk must roll back earlier writes"
    );
    let ids = people.insert_many(&items).await.unwrap();
    assert_eq!(ids.len(), 200);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        200
    );
    assert_eq!(people.count().await.unwrap(), 200);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_relation_update_emits_one_root_after_two_values_and_three_players() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::{AttributeValue, QueryExecutionResourceLimits};
    let Some(db) = live_database("update_root_cardinality").await else {
        return;
    };
    db.execute_raw(
        &PARTY_SCHEMA_TYPEQL
            .replace("relates member @card(1..2)", "relates member @card(1..3)")
            .replace("owns note @card(0..1)", "owns note @card(0..2)"),
        TxType::Schema,
    )
    .await
    .unwrap();
    let yaml = PARTY_SCHEMA_YAML
        .replace(
            "member: { card: { min: 1, max: 2 } }",
            "member: { card: { min: 1, max: 3 } }",
        )
        .replace(
            "note: { card: { min: 0, max: 1 } }",
            "note: { card: { min: 0, max: 2 } }",
        );
    let installed = party_projection_for_schema(&yaml);
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let mut players = Vec::new();
    for index in 0..3 {
        let iid = people
            .insert(&vec![(
                "identifier".into(),
                AttributeValue::String(index.to_string()),
            )])
            .await
            .unwrap()
            .item()
            .expect("one inserted root")
            .iid
            .clone()
            .unwrap();
        players.push(DynamicRolePlayerInput {
            role_name: "member".into(),
            player_type_name: "person".into(),
            iid: Some(iid),
            key: None,
        });
    }
    let relations = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let notes = |a: &str, b: &str| {
        vec![
            ("note".into(), AttributeValue::String(a.into())),
            ("note".into(), AttributeValue::String(b.into())),
        ]
    };
    let iid = relations
        .insert(&notes("old1", "old2"), &players)
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let tx = db
        .transaction_context_with_answer_limits(
            TxType::Write,
            // The hydrated answer is 1 relation + 3 players; a 2 x 3 cross
            // product of values and players would exceed this limit.
            QueryExecutionResourceLimits {
                items: 4,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let manager = DynamicRelationManager::with_registry_transaction(
        tx.clone(),
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let updated = manager
        .update_exact(&iid, &notes("new1", "new2"), &players)
        .await
        .unwrap();
    assert_eq!(updated.count(), 1);
    tx.commit_sdk().await.unwrap();
    let rows = db
        .execute_raw(
            &format!("match $r iid {iid}, has note $n; select $r, $n;"),
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(self::rows(&rows), 2);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_relation_update_without_iid_keeps_multiple_root_semantics() {
    use std::sync::Arc;
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let Some(db) = live_database("multi_root_update").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = party_projection_with_authority();
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    );
    let iid = people
        .insert(&vec![(
            "identifier".into(),
            AttributeValue::String("shared".into()),
        )])
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let player = DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "person".into(),
        iid: Some(iid),
        key: None,
    };
    let relations = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("membership").unwrap(),
    );
    let note = |value: &str| vec![("note".into(), AttributeValue::String(value.into()))];
    let first = relations
        .insert(&note("first"), std::slice::from_ref(&player))
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    let second = relations
        .insert(&note("second"), std::slice::from_ref(&player))
        .await
        .unwrap()
        .item()
        .expect("one inserted root")
        .iid
        .clone()
        .unwrap();
    relations
        .update(None, &note("updated"), &[player])
        .await
        .unwrap();
    for iid in [first, second] {
        let rows = relations.get_by_iid_exact(&iid).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attributes, note("updated"));
    }
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn native_control_authority_reads_the_frozen_schema_in_one_transaction() {
    let Some(db) = live_database("control_plans").await else {
        return;
    };
    db.execute_raw(
        type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL,
        TxType::Schema,
    )
    .await
    .unwrap();
    db.execute_raw(
        "insert $control isa typebridge-internal-migration-control, has typebridge-internal-control-scope \"native-control\", has typebridge-internal-lease-fence \"1\", has typebridge-internal-lease-state \"free\";",
        TxType::Write,
    ).await.unwrap();
    let mut tx = db.read_transaction().await.unwrap();
    type_bridge_orm::plan_authority::verify_managed_query_control_scope(
        &mut tx,
        &type_bridge_contract::managed_scope::ManagedScopeId::new("native-control").unwrap(),
        Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        &type_bridge_orm::AnswerCancellation::default(),
    )
    .await
    .unwrap();
    tx.close().await.unwrap();
    db.delete_database().await.unwrap();
}

const KEYLESS_PUT_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  code: { value: string }
  title: { value: string }
  note: { value: string }
entities:
  member:
    owns:
      code: { key: true }
  tag:
    owns:
      title: { card: { min: 0, max: 1 } }
      note: { card: { min: 0 } }
  subtag:
    sub: tag
relations:
  tagging:
    owns:
      note: { card: { min: 0 } }
    relates:
      tagged: { card: { min: 0, max: 4 } }
plays:
  member:
    tagging: [tagged]
"#;

const KEYLESS_PUT_TYPEQL: &str = "define attribute code, value string; attribute title, value string; attribute note, value string; entity member, owns code @key, plays tagging:tagged; entity tag, owns title @card(0..1), owns note @card(0..); entity subtag sub tag; relation tagging, owns note @card(0..), relates tagged @card(0..4);";

/// Keyless put is TypeQL put on the exact type: match the whole given
/// pattern or insert it, never update; several matches refuse the call
/// before commit, including earlier rows of the same batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyless_put_is_typeql_put_on_the_exact_type_live() {
    use type_bridge_orm::{
        ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchInvocationControl,
        ProjectedBatchOperation as Op, ProjectedBatchResult, ProjectedBatchRow as Row,
    };
    let Some(db) = live_database("keyless_put").await else {
        return;
    };
    db.execute_raw(KEYLESS_PUT_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("keyless-put.yaml").unwrap(),
        KEYLESS_PUT_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let crud = ProjectedCrudExecutor::new(&installed);
    let batches = ProjectedBatchExecutor::new(&installed);
    let tag = party_type(TypeKind::Entity, "tag");
    let tagging = party_type(TypeKind::Relation, "tagging");
    let field = |owner: &TypeId, name: &str| {
        installed.projection().models()[owner]
            .query_tokens()
            .fields()
            .values()
            .find(|token| token.declaring_id().attribute().label().as_str() == name)
            .map(|token| token.id().clone())
            .unwrap()
    };
    let label = |value: &str| {
        ProjectedCreate::try_new(
            &installed,
            tag.clone(),
            vec![(
                field(&tag, "title"),
                vec![party_text(&installed, "title", value)],
            )],
            vec![],
        )
        .unwrap()
    };
    let count = |query: &'static str| {
        let db = &db;
        async move {
            match db.execute_raw(query, TxType::Read).await.unwrap() {
                QueryResult::Rows(rows) => rows.len(),
                other => panic!("expected rows, got {other:?}"),
            }
        }
    };
    let put_many = |rows: Vec<ProjectedCreate>| {
        let batch = ProjectedBatch::try_new(
            &installed,
            tag.clone(),
            Op::Put,
            rows.into_iter().map(Row::Create).collect(),
        )
        .unwrap();
        let batches = &batches;
        let db = &db;
        async move {
            batches
                .execute(
                    db,
                    &batch,
                    ProjectedBatchInvocationControl::capture(
                        Default::default(),
                        Default::default(),
                    ),
                )
                .await
        }
    };
    let iids = |result: ProjectedBatchResult| {
        let ProjectedBatchResult::Things(things) = result else {
            panic!("expected things")
        };
        things
            .into_items()
            .into_iter()
            .map(|thing| thing.iid().to_owned())
            .collect::<Vec<_>>()
    };

    // Run twice: one instance, the same IID.
    let first = crud.put_entity(&db, &label("red")).await.unwrap();
    let second = crud.put_entity(&db, &label("red")).await.unwrap();
    assert_eq!(first.item().unwrap().iid(), second.item().unwrap().iid());
    assert_eq!(
        count("match $t isa tag, has title \"red\"; select $t;").await,
        1
    );

    // Identical rows chain in row order and return the same IID.
    let written = iids(
        put_many(vec![label("blue"), label("blue"), label("red")])
            .await
            .unwrap(),
    );
    assert_eq!(written.len(), 3);
    assert_eq!(written[0], written[1]);
    assert_eq!(written[2], first.item().unwrap().iid());
    assert_eq!(
        count("match $t isa tag, has title \"blue\"; select $t;").await,
        1
    );

    // A subset pattern matches an instance with more values; nothing updates.
    db.execute_raw(
        "insert $t isa tag, has title \"green\", has note \"kept\";",
        TxType::Write,
    )
    .await
    .unwrap();
    let green = crud.put_entity(&db, &label("green")).await.unwrap();
    assert_eq!(
        green.item().unwrap().fields()[&field(&tag, "note")].len(),
        1
    );
    assert_eq!(
        count("match $t isa tag, has title \"green\"; select $t;").await,
        1
    );

    // Exact type: a subtype instance is not matched; a new tag is inserted.
    db.execute_raw("insert $t isa subtag, has title \"sub\";", TxType::Write)
        .await
        .unwrap();
    let exact = crud.put_entity(&db, &label("sub")).await.unwrap();
    assert_eq!(exact.item().unwrap().type_id(), &tag);
    assert_eq!(
        count("match $t isa! tag, has title \"sub\"; select $t;").await,
        1
    );
    assert_eq!(
        count("match $t isa subtag, has title \"sub\"; select $t;").await,
        1
    );

    // Ambiguity refuses the call before commit, earlier rows included.
    db.execute_raw(
        "insert $a isa tag, has title \"dup\"; $b isa tag, has title \"dup\";",
        TxType::Write,
    )
    .await
    .unwrap();
    let single = crud.put_entity(&db, &label("dup")).await.unwrap_err();
    assert_eq!(single.code().as_str(), "put_ambiguous_match");
    let many = put_many(vec![label("before"), label("dup"), label("after")])
        .await
        .unwrap_err();
    assert_eq!(many.code().as_str(), "put_ambiguous_match");
    assert_eq!(
        many.category(),
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::InvalidInput
    );
    assert_eq!(
        count("match $t isa tag, has title \"before\"; select $t;").await,
        0
    );
    assert_eq!(
        count("match $t isa tag, has title \"after\"; select $t;").await,
        0
    );
    assert_eq!(
        count("match $t isa tag, has title \"dup\"; select $t;").await,
        2
    );

    // Relations: the pattern holds every given field and player.
    db.execute_raw("insert $m isa member, has code \"m1\";", TxType::Write)
        .await
        .unwrap();
    let member = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "member"),
        None,
        vec![(
            field(&party_type(TypeKind::Entity, "member"), "code"),
            party_text(&installed, "code", "m1"),
        )],
    )
    .unwrap();
    let tagged = ProjectedCreate::try_new(
        &installed,
        tagging.clone(),
        vec![],
        vec![(RoleId::new("tagging", "tagged").unwrap(), vec![member])],
    )
    .unwrap();
    let one = crud.put_relation(&db, &tagged).await.unwrap();
    let two = crud.put_relation(&db, &tagged).await.unwrap();
    assert_eq!(one.item().unwrap().iid(), two.item().unwrap().iid());
    assert_eq!(count("match $r isa tagging; select $r;").await, 1);
    let batch = ProjectedBatch::try_new(
        &installed,
        tagging.clone(),
        Op::Put,
        vec![Row::Create(tagged.clone()), Row::Create(tagged)],
    )
    .unwrap();
    let relations = iids(
        batches
            .execute(
                &db,
                &batch,
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default()),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        relations,
        [one.item().unwrap().iid(), one.item().unwrap().iid()]
    );
    assert_eq!(count("match $r isa tagging; select $r;").await, 1);

    // Distinct rows share one put stage: 1000 rows, one commit, the models
    // in call-row order. An ambiguous row among them refuses the whole call.
    let titles = (0..1000)
        .map(|row| format!("wide-{row}"))
        .collect::<Vec<_>>();
    let mut rows = titles.iter().map(|title| label(title)).collect::<Vec<_>>();
    rows[500] = label("dup");
    let error = put_many(rows).await.unwrap_err();
    assert_eq!(error.code().as_str(), "put_ambiguous_match");
    let wide = "match $t isa tag, has title $v; $v like \"^wide-\"; select $t;";
    assert_eq!(count(wide).await, 0);
    let ProjectedBatchResult::Things(things) =
        put_many(titles.iter().map(|title| label(title)).collect())
            .await
            .unwrap()
    else {
        panic!("expected things")
    };
    let written = things
        .items()
        .iter()
        .map(
            |thing| match thing.fields()[&field(&tag, "title")][0].value() {
                CanonicalValue::String(title) => title.as_str().to_owned(),
                other => panic!("{other:?}"),
            },
        )
        .collect::<Vec<_>>();
    assert_eq!(written, titles);
    assert_eq!(count(wide).await, 1000);
    db.delete_database().await.unwrap();
}
