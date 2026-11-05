use serde_json::Value;
use type_bridge_core_lib::__internal as tql;
use type_bridge_migration::plan::plan;
use type_bridge_migration::spec::{MigrationGraph, MigrationSpec};

fn same_plan(old: &Value, new: &Value, carried: bool) {
    match (old, new) {
        (Value::Array(old), Value::Array(new)) => {
            assert_eq!(old.len(), new.len());
            for (old, new) in old.iter().zip(new) {
                same_plan(old, new, carried);
            }
        }
        (Value::Object(old), Value::Object(new)) => {
            assert_eq!(
                old.keys().collect::<Vec<_>>(),
                new.keys().collect::<Vec<_>>()
            );
            for (key, old) in old {
                let new = &new[key];
                if matches!(key.as_str(), "forward" | "reverse")
                    && let (Some(old), Some(new)) = (old.as_str(), new.as_str())
                {
                    if carried || old == new {
                        assert_eq!(old, new);
                    } else {
                        assert!(tql::equivalent(old, new).unwrap(), "{old} != {new}");
                    }
                    continue;
                }
                same_plan(old, new, carried);
            }
        }
        _ => assert_eq!(old, new),
    }
}

#[test]
fn released_legacy_plans_keep_their_ast_and_carried_bytes() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/migration_pre_ast.json")).unwrap();
    let mut compared = 0;
    let mut rejected = 0;
    for case in &cases {
        let migration: MigrationSpec = serde_json::from_value(case["migration"].clone()).unwrap();
        let operation = &case["migration"]["operations"][0];
        let carried = operation["kind"] == "run_typeql";
        let result = plan(
            &MigrationGraph {
                migrations: vec![migration],
            },
            &[],
            None,
            None,
        );
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err().to_string(), error);
            rejected += 1;
        } else {
            let mut expected = case["plan"].clone();
            if case["case"] == "annotated_ordered_specialized_role" {
                let forward = expected["to_apply"][0]["steps"][0]["forward"]
                    .as_str()
                    .unwrap();
                assert!(
                    tql::parse(forward).is_err(),
                    "old-invalid role form must be rejected"
                );
                expected["to_apply"][0]["steps"][0]["forward"] =
                    Value::String(forward.replace("as participant @", "as participant[] @"));
            }
            same_plan(
                &expected,
                &serde_json::to_value(result.unwrap()).unwrap(),
                carried,
            );
            compared += 1;
        }
    }
    assert_eq!((compared, rejected), (24, 1));
    println!(
        "legacy migration parity: {compared} plans (23 equivalent, 1 old-invalid correction), {rejected} unchanged rejection"
    );
}

#[test]
fn known_parent_role_ordering_rejects_both_mismatch_directions() {
    use type_bridge_migration::error::MigrationError;
    for parent_ordered in [false, true] {
        for child_ordered in [false, true] {
            let migration: MigrationSpec = serde_json::from_value(serde_json::json!({
                "app_label": "ordering", "name": "initial", "dependencies": [], "reversible": true,
                "operations": [
                    {"kind": "add_relation", "relation": {
                        "type_name": "base", "parent_type": null, "is_abstract": true,
                        "owned_attributes": [], "roles": [{
                            "role_name": "participant", "player_type_names": [],
                            "cardinality": null, "is_abstract": true, "ordered": parent_ordered
                        }]
                    }},
                    {"kind": "add_relation", "relation": {
                        "type_name": "review", "parent_type": "base", "is_abstract": true,
                        "owned_attributes": [], "roles": []
                    }},
                    {"kind": "add_role", "relation_type": "review", "role": {
                        "role_name": "reviewer", "player_type_names": [],
                        "cardinality": null, "is_abstract": true,
                        "ordered": child_ordered, "overrides": "participant"
                    }}
                ]
            }))
            .unwrap();
            let result = plan(
                &MigrationGraph {
                    migrations: vec![migration],
                },
                &[],
                None,
                None,
            );
            if parent_ordered == child_ordered {
                assert!(result.is_ok(), "{result:?}");
            } else {
                assert!(matches!(result, Err(MigrationError::AuthoringInput { .. })));
            }
        }
    }
}

#[test]
fn removed_role_facts_do_not_constrain_a_later_recreated_relation() {
    let relation = |name, parent, roles| {
        serde_json::json!({
            "kind": "add_relation", "relation": { "type_name": name, "parent_type": parent,
                "is_abstract": true, "owned_attributes": [], "roles": roles }
        })
    };
    let role = |name, ordered, parent| {
        serde_json::json!({
            "role_name": name, "player_type_names": [], "cardinality": null,
            "is_abstract": true, "ordered": ordered, "overrides": parent
        })
    };
    for remove_child_type in [false, true] {
        let remove = if remove_child_type {
            serde_json::json!({"kind": "remove_relation", "type_name": "review"})
        } else {
            serde_json::json!({"kind": "remove_role", "relation_type": "review", "role_name": "reviewer"})
        };
        let remove_parent = if remove_child_type {
            serde_json::json!({"kind": "remove_relation", "type_name": "base"})
        } else {
            serde_json::json!({"kind": "remove_role", "relation_type": "base", "role_name": "participant"})
        };
        let recreate_parent = if remove_child_type {
            relation(
                "base",
                None::<&str>,
                vec![role("participant", true, None::<&str>)],
            )
        } else {
            serde_json::json!({"kind": "add_role", "relation_type": "base", "role": role("participant", true, None::<&str>)})
        };
        let migration: MigrationSpec = serde_json::from_value(serde_json::json!({
            "app_label": "ordering", "name": "recreate", "dependencies": [], "reversible": false,
            "operations": [
                relation("base", None::<&str>, vec![role("participant", false, None::<&str>)]),
                relation("review", Some("base"), vec![role("reviewer", false, Some("participant"))]),
                remove,
                remove_parent,
                recreate_parent
            ]
        })).unwrap();
        assert!(
            plan(
                &MigrationGraph {
                    migrations: vec![migration]
                },
                &[],
                None,
                None
            )
            .is_ok()
        );
    }
}

#[tokio::test]
#[ignore = "requires the isolated TypeDB 3.13.6 worker server"]
async fn ordered_role_specialisation_from_the_new_plan_applies_and_commits() {
    let database = live_database().await;
    let test_database = database.clone();
    let result = tokio::spawn(async move {
        let database = test_database;
        let migration: MigrationSpec = serde_json::from_value(serde_json::json!({
            "app_label": "ordering", "name": "initial", "dependencies": [], "reversible": true,
            "operations": [{"kind": "add_role", "relation_type": "review", "role": {
                "role_name": "reviewer", "player_type_names": [], "cardinality": null,
                "ordered": true, "overrides": "participant"
            }}]
        })).unwrap();
        let planned = plan(&MigrationGraph { migrations: vec![migration] }, &[], None, None).unwrap();
        let forward = &planned.to_apply[0].steps[0].forward;
        assert!(forward.contains("reviewer[] as participant[]"));
        let mut transaction = database.schema_transaction().await.unwrap();
        transaction.query("define relation base @abstract, relates participant[] @abstract; relation review, sub base;").await.unwrap();
        transaction.query(forward).await.unwrap();
        transaction.commit().await.unwrap();
        assert!(database.schema_text().await.unwrap().contains("reviewer[] as participant[]"));
    }).await;
    database.delete_database().await.unwrap();
    result.unwrap();
}

async fn live_database() -> std::sync::Arc<type_bridge_orm::Database> {
    use type_bridge_orm::{ConnectOptions, Database};
    let address = std::env::var("TYPEDB_ADDRESS").unwrap();
    let options = ConnectOptions {
        http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
        ..ConnectOptions::default()
    };
    let name = format!("tb270_06c_migration_ast_{}", uuid::Uuid::new_v4().simple());
    type_bridge_orm::ensure_database_exists(&address, &name, "admin", "password", options)
        .await
        .unwrap();
    std::sync::Arc::new(
        Database::connect_with_options(&address, &name, "admin", "password", options)
            .await
            .unwrap(),
    )
}

async fn direct_count(database: &type_bridge_orm::Database, query: &str) -> u64 {
    use type_bridge_orm::session::backend::QueryResult;
    let mut read = database.read_transaction().await.unwrap();
    let result = read.query(query).await.unwrap();
    read.close().await.unwrap();
    let (QueryResult::Rows(rows) | QueryResult::Documents(rows)) = result else {
        panic!("direct count must return rows")
    };
    assert_eq!(rows.len(), 1);
    let count = &rows[0]["c"];
    count.as_u64().or_else(|| count["value"].as_u64()).unwrap()
}

#[tokio::test]
#[ignore = "requires the isolated TypeDB 3.13.6 worker server"]
async fn native_copy_attribute_counts_match_direct_queries() {
    let database = live_database().await;
    let test_database = database.clone();
    let result = tokio::spawn(async move {
        let database = test_database;
        let mut schema = database.schema_transaction().await.unwrap();
        schema.query("define attribute old-name, value string; attribute new-name, value string; attribute blocked, value boolean; entity person, owns old-name, owns new-name, owns blocked;").await.unwrap();
        schema.commit().await.unwrap();
        let mut write = database.write_transaction().await.unwrap();
        write.query("insert $a isa person, has old-name \"Ada\"; $b isa person, has old-name \"Bob\", has new-name \"Bob\"; $c isa person, has old-name \"Cora\", has blocked true; $d isa person, has old-name \"Dora\", has new-name \"Dora\", has blocked true;").await.unwrap();
        write.commit().await.unwrap();
        let matched = direct_count(&database, "match $x isa person, has old-name $v; reduce $c = count;").await;
        let skipped = direct_count(&database, "match $x isa person, has old-name $v, has new-name $d; reduce $c = count;").await;
        assert_eq!((matched, skipped), (4, 2));
        let migration: MigrationSpec = serde_json::from_value(serde_json::json!({
            "app_label": "filtered", "name": "initial", "dependencies": [], "reversible": true,
            "operations": [{"kind": "copy_attribute", "mode": "isa", "owner": "person", "source": "old-name", "dest": "new-name"}]
        })).unwrap();
        let authority = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("native-copy.typeql").unwrap(),
            "define attribute old-name, value string; attribute new-name, value string; attribute blocked, value boolean; entity person, owns old-name, owns new-name, owns blocked;",
        ).unwrap();
        let planned = plan(&MigrationGraph { migrations: vec![migration] }, &[], None, Some(&authority)).unwrap();
        let step = &planned.to_apply[0].steps[0];
        let carried = serde_json::to_value(step).unwrap();
        let observed = type_bridge_migration::backfill::execute_backfill(&database, step, 0).await.unwrap().unwrap();
        assert_eq!(observed.matched, matched);
        assert_eq!(observed.skipped, skipped);
        assert_eq!(observed.inserted, matched - skipped);
        assert_eq!(serde_json::to_value(step).unwrap(), carried);
    }).await;
    database.delete_database().await.unwrap();
    result.unwrap();
}

#[derive(Default)]
struct LiveRecoveryEvents(std::sync::Mutex<Vec<type_bridge_migration::StepRecoveryEvent>>);

impl type_bridge_migration::StepRecoveryController for LiveRecoveryEvents {
    fn classify<'a>(
        &'a self,
        _db: &'a type_bridge_orm::Database,
        step: &'a type_bridge_migration::CheckedExecutionStep,
    ) -> type_bridge_migration::RecoveryFuture<
        'a,
        type_bridge_migration::Result<type_bridge_migration::StepRecoveryDecision>,
    > {
        use type_bridge_migration::{PendingProof, StepRecoveryDecision, StepRecoveryEventKind};
        let applied = self.0.lock().unwrap().iter().any(|event| {
            event.step.id == step.id && event.kind == StepRecoveryEventKind::Committed
        });
        Box::pin(async move {
            Ok(if applied {
                StepRecoveryDecision::Applied {
                    evidence: Some("observed successful live commit".into()),
                }
            } else {
                StepRecoveryDecision::Pending {
                    proof: PendingProof::NotCommitted,
                }
            })
        })
    }
    fn record_event<'a>(
        &'a self,
        event: type_bridge_migration::StepRecoveryEvent,
    ) -> type_bridge_migration::RecoveryFuture<'a, type_bridge_migration::Result<()>> {
        self.0.lock().unwrap().push(event);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
#[ignore = "requires assigned TypeDB 3.14.0 server"]
async fn native_and_raw_steps_share_a_migration_and_recovery_skips_committed_work() {
    use type_bridge_migration::{
        MigrationAction, OperationSpec, RecoveryPlanStatus, StepExecutionOutcome,
    };
    let database = live_database().await;
    let test_database = database.clone();
    let result = tokio::spawn(async move {
        let database = test_database;
        let schema_text = "define attribute old-name, value string; attribute new-name, value string; entity person, owns old-name, owns new-name;";
        let mut transaction = database.schema_transaction().await.unwrap();
        transaction.query(schema_text).await.unwrap();
        transaction.commit().await.unwrap();
        let mut transaction = database.write_transaction().await.unwrap();
        transaction.query("insert $x isa person, has old-name \"Ada\";").await.unwrap();
        transaction.commit().await.unwrap();
        let authority = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("mixed-native-raw.typeql").unwrap(), schema_text).unwrap();
        let mut graph = MigrationGraph { migrations: vec![MigrationSpec {
            app_label: "mixed".into(), name: "0001_data".into(), dependencies: vec![],
            checksum: Some("mixed-native-raw".into()), source_sha256: None, reversible: false,
            operations: vec![OperationSpec::CopyAttribute { mode: Some("isa".into()), owner: Some("person".into()),
                source: Some("old-name".into()), dest: Some("new-name".into()), filter: None,
                forward: None, reverse: None }],
        }] };
        let planned = plan(&graph, &[], None, Some(&authority)).unwrap();
        let native = planned.to_apply[0].steps[0].data.as_ref().unwrap().program(MigrationAction::Apply).unwrap();
        graph.migrations[0].operations = vec![
            OperationSpec::RunPlan { forward: serde_json::to_value(native).unwrap(), reverse: None },
            OperationSpec::RunTypeql { forward: "# deliberately raw\ninsert $x isa person, has old-name \"Raw\";".into(), reverse: None },
        ];
        let plan = type_bridge_migration::plan_recovery(&graph, &[], None, Some(&authority)).unwrap();
        let controller = LiveRecoveryEvents::default();
        let first = type_bridge_migration::execute_recovery_plan(&database, &plan, &controller).await;
        assert_eq!(first.status, RecoveryPlanStatus::Succeeded, "{first:?}");
        assert!(first.migrations[0].steps.iter().all(|step| matches!(step.outcome, StepExecutionOutcome::Committed { backfill: None })));
        assert_eq!(controller.0.lock().unwrap().len(), 4);
        assert_eq!(direct_count(&database, "match $x isa person; reduce $c = count;").await, 2);
        assert_eq!(direct_count(&database, "match $x isa person, has new-name \"Ada\"; reduce $c = count;").await, 1);
        let recovered = type_bridge_migration::execute_recovery_plan(&database, &plan, &controller).await;
        assert_eq!(recovered.status, RecoveryPlanStatus::Succeeded, "{recovered:?}");
        assert!(recovered.migrations[0].steps.iter().all(|step| matches!(step.outcome, StepExecutionOutcome::Applied { .. })));
        assert_eq!(controller.0.lock().unwrap().len(), 4);
        assert_eq!(direct_count(&database, "match $x isa person; reduce $c = count;").await, 2);
    }).await;
    database.delete_database().await.unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires assigned TypeDB 3.14.0 server"]
async fn native_copy_matches_an_abstract_source_owned_by_an_owner_subtype() {
    let database = live_database().await;
    let test_database = database.clone();
    let result = tokio::spawn(async move {
        let database = test_database;
        let schema_text = "define attribute old-name @abstract, value string; attribute specialised, sub old-name; attribute new-name, value string; entity person, owns new-name; entity employee, sub person, owns specialised;";
        database.execute_raw(schema_text, type_bridge_orm::TxType::Schema).await.unwrap();
        database.execute_raw("insert $e isa employee, has specialised \"Ada\";", type_bridge_orm::TxType::Write).await.unwrap();
        let authority = type_bridge_schema_compat::typeql_to_declared(type_bridge_contract::schema::DocumentId::new("abstract-copy-live.typeql").unwrap(), schema_text).unwrap();
        let migration: MigrationSpec = serde_json::from_value(serde_json::json!({
            "app_label":"copy", "name":"abstract_source", "dependencies":[], "reversible":true,
            "operations":[{"kind":"copy_attribute", "owner":"person", "source":"old-name", "dest":"new-name", "mode":"isa"}]
        })).unwrap();
        let planned = plan(&MigrationGraph { migrations: vec![migration] }, &[], None, Some(&authority)).unwrap();
        let step = &planned.to_apply[0].steps[0];
        let observed = type_bridge_migration::backfill::execute_backfill(&database, step, 0).await.unwrap().unwrap();
        assert_eq!((observed.matched, observed.inserted, observed.skipped), (1,1,0));
        assert_eq!(direct_count(&database, "match $e isa employee, has new-name \"Ada\"; reduce $c = count;").await, 1);
    }).await;
    database.delete_database().await.unwrap();
    result.unwrap();
}
