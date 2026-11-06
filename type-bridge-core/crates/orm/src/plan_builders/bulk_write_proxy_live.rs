//! Opt-in live proof for the manager compiler's static IID dependencies.
use super::*;
use crate::_dynamic::{DynamicAttributeMap, DynamicRelationRow, DynamicRolePlayerInput};
use crate::_manager::dynamic::{DynamicEntityManager, DynamicRelationManager};
use crate::match_request::{MatchResult, SlotValue};
use crate::plan_authority::QueryAuthority;
use crate::plan_builders::manager::{ManagerPlans, ManagerWrite};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::session::ProxyEndpoint;
use crate::session::backend::AnswerCancellation;
use crate::{
    AttributeValue, ConnectOptions, Database, InstalledRuntimeProjection, ProxyQueryConnection,
    TxType,
};
use base64::Engine;
use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use type_bridge_contract::{
    fingerprint::SemanticProfileId,
    limits::QueryLimits,
    managed_scope::ManagedScopeId,
    projection::{BindingTarget, ProjectionConfig, ProjectionHandler},
    schema::DocumentId,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const YAML: &str = r#"format: typebridge.schema/v2
attributes:
  proxy-selector: { value: string }
  proxy-tag: { value: string }
  proxy-failure-tag: { value: string }
entities:
  proxy-person:
    owns:
      proxy-selector: { card: { min: 0, max: 1 } }
  proxy-failure:
    owns:
      proxy-failure-tag: { unique: true, card: { min: 0 } }
relations:
  proxy-link:
    owns:
      proxy-tag: { card: { min: 0 } }
    relates:
      member: { card: { min: 0 } }
plays:
  proxy-person:
    proxy-link: [member]
"#;
const TYPEQL: &str = r#"define
attribute proxy-selector, value string;
attribute proxy-tag, value string;
attribute proxy-failure-tag, value string;
entity proxy-person, owns proxy-selector @card(0..1), plays proxy-link:member;
entity proxy-failure, owns proxy-failure-tag @unique @card(0..);
relation proxy-link, owns proxy-tag @card(0..), relates member @card(0..);
"#;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn attributes(name: &str, prefix: &str) -> DynamicAttributeMap {
    (0..300)
        .map(|i| {
            (
                name.into(),
                AttributeValue::String(format!("{prefix}-{i:03}")),
            )
        })
        .collect()
}

fn configured_limits(name: &str) -> Option<QueryLimits> {
    let encoded = std::env::var(name).ok()?;
    let overrides: BTreeMap<String, u64> = serde_json::from_str(&encoded).unwrap();
    Some(
        QueryLimits::from_overrides(
            overrides
                .iter()
                .map(|(name, value)| (name.as_str(), *value)),
        )
        .unwrap(),
    )
}

fn program(
    plans: &ManagerPlans,
    name: &str,
    kind: WriteKind,
    fields: &DynamicAttributeMap,
    players: &[DynamicRolePlayerInput],
) -> WriteProgram {
    match plans
        .single_write_program(name, kind, None, fields, players)
        .unwrap()
    {
        ManagerWrite::One(plan) => WriteProgram::compact(vec![plan]),
        ManagerWrite::Pieces(program) => program,
    }
    .prepare_static()
    .unwrap()
}

fn hydrate(
    plans: &ManagerPlans,
    mut program: WriteProgram,
    answers: Vec<PlanAnswer>,
) -> Vec<DynamicRelationRow> {
    let mut result = Vec::new();
    for answer in answers {
        let step = program
            .next()
            .unwrap()
            .expect("one answer per manager piece");
        program.record_answer(&step, &answer).unwrap();
        if step.complete {
            let PlanAnswer::Documents(documents) = answer else {
                panic!("manager document answer")
            };
            let (request, validated) = crate::match_request::validated_match_result_from_plan(
                &plans.registry,
                step.plan.local(),
                &documents,
                &plans.registry.descriptor_id("proxy-link").unwrap(),
                QueryExecutionResourceLimits::default(),
            )
            .unwrap();
            let MatchResult::Rows { rows } = validated.for_request(&request).unwrap() else {
                panic!("manager rows")
            };
            for row in rows {
                let [SlotValue::One(thing)] = row.slots() else {
                    panic!("one hydrated root")
                };
                result.push(
                    crate::_manager::dynamic::plan_relation_row(&plans.registry, thing).unwrap(),
                );
            }
        }
    }
    assert!(program.next().unwrap().is_none());
    result.sort_by(|a, b| a.iid.cmp(&b.iid));
    result
}

fn trace(path: &std::path::Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_transaction(events: &[serde_json::Value], plans: usize, committed: bool) {
    assert_eq!(events.len(), plans + 2, "{events:?}");
    assert_eq!(events[0]["event"], "request");
    for (i, event) in events[1..=plans].iter().enumerate() {
        assert_eq!(event["event"], "plan");
        assert_eq!(event["index"], i);
        assert_eq!(event["id"], events[0]["id"]);
    }
    assert_eq!(events.last().unwrap()["event"], "commit");
    assert_eq!(events.last().unwrap()["id"], events[0]["id"]);
    assert_eq!(events.last().unwrap()["committed"], committed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly configured TypeDB and staged smoke server"]
async fn wide_manager_proxy_preserves_plural_roots_and_rolls_back_last_piece() {
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap();
    let version = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION").unwrap();
    let name = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_DATABASE").unwrap();
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
    db.execute_raw(TYPEQL, TxType::Schema).await.unwrap();
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(DocumentId::new("proxy-wide.yaml").unwrap(), YAML)]).unwrap(),
    )
    .unwrap();
    let profile_name = if version.starts_with("3.12.") {
        "typedb-3.12.1/v1"
    } else {
        "typedb-3.13.6/v1"
    };
    let profile = SemanticProfileId::new(profile_name).unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        ManagedScopeId::new("proxy-wide").unwrap(),
        profile.clone(),
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
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
    .unwrap()
    .with_schema_authority(&authority);
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("proxy-person").unwrap(),
    );
    let selector = vec![(
        "proxy-selector".into(),
        AttributeValue::String("both".into()),
    )];
    for _ in 0..2 {
        assert_eq!(people.insert(&selector).await.unwrap().count(), 1);
    }
    let players = vec![DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "proxy-person".into(),
        iid: None,
        key: Some(selector[0].clone()),
    }];
    let links = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("proxy-link").unwrap(),
    );
    let direct_fields = attributes("proxy-tag", "direct");
    let direct = links.insert(&direct_fields, &players).await.unwrap();
    assert_eq!(direct.count(), 2);
    assert!(
        direct
            .items()
            .iter()
            .all(|row| row.attributes.len() == 300 && row.role_players.len() == 1)
    );

    let port: u16 = std::env::var("SMOKE_PORT").unwrap().parse().unwrap();
    let path = std::path::PathBuf::from(std::env::var_os("SMOKE_TRACE_FILE").unwrap());
    let mut server = Server(
        Command::new(std::env::var_os("TYPE_BRIDGE_V2_SMOKE_SERVER").unwrap())
            .env("SMOKE_TYPEDB_ADDRESS", &address)
            .env(
                "SMOKE_TYPEDB_HTTP_PORT",
                std::env::var("TYPEDB_HTTP_PORT").unwrap(),
            )
            .env("SMOKE_TYPEDB_USERNAME", "admin")
            .env("SMOKE_TYPEDB_PASSWORD", "password")
            .env("SMOKE_DATABASE", &name)
            .env(
                "SMOKE_AUTHORITY_B64",
                base64::engine::general_purpose::STANDARD
                    .encode(type_bridge_schema::encode_schema_authority(&authority)),
            )
            .env("SMOKE_ALLOW_STAGED_WRITES", "true")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "owned smoke server exited"
        );
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cancellation = AnswerCancellation::default();
    let resources = QueryExecutionResourceLimits::default();
    let mut proxy = ProxyQueryConnection::connect(
        &ProxyEndpoint::new(format!("http://127.0.0.1:{port}")),
        None,
        Arc::new(
            QueryAuthority::from_declared_bytes(
                &type_bridge_contract::schema::encode_declared_schema(&declared).unwrap(),
                "proxy-wide",
                profile_name,
            )
            .unwrap(),
        ),
        resources,
        resources,
        &cancellation,
    )
    .await
    .unwrap();
    let requested = configured_limits("TYPE_BRIDGE_WIDE_QUERY_LIMITS").unwrap_or(QueryLimits::PLAN);
    proxy = proxy.with_query_limits(requested).unwrap();
    let server_limits = configured_limits("SMOKE_QUERY_LIMITS").unwrap_or(QueryLimits::PLAN);
    assert_eq!(proxy.query_limits(), requested.min(server_limits));
    let effective = Arc::new(proxy.query_limits());
    let plans = ManagerPlans::new(Arc::clone(&registry)).with_query_limits(Some(&effective));
    let expect_single = std::env::var("TYPE_BRIDGE_WIDE_EXPECT_SINGLE").as_deref() == Ok("true");
    let expect_insert_single = std::env::var("TYPE_BRIDGE_WIDE_EXPECT_INSERT_SINGLE")
        .map_or(expect_single, |value| value == "true");
    let mut prior_iids = Vec::new();
    for (kind, prefix) in [
        (WriteKind::Insert, "proxy-insert"),
        (WriteKind::PartialPut, "proxy-put"),
        (WriteKind::PartialPut, "proxy-put"),
    ] {
        let fields = attributes("proxy-tag", prefix);
        let program = program(&plans, "proxy-link", kind, &fields, &players);
        let entries = program
            .staged_entries(plans.authority().unwrap())
            .unwrap()
            .unwrap();
        let count = entries.len();
        assert_eq!(
            count == 1,
            if kind == WriteKind::Insert {
                expect_insert_single
            } else {
                expect_single
            },
            "wide plan count {count}"
        );
        let before = trace(&path).len();
        let answers = proxy
            .execute_write_stack(
                entries,
                resources,
                &cancellation,
                QueryExecutionDeadline::for_limits(resources),
            )
            .await
            .unwrap();
        let rows = hydrate(&plans, program, answers);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row.attributes.len() == 300 && row.role_players.len() == 1)
        );
        for row in &rows {
            let mut actual_fields = row.attributes.clone();
            actual_fields.sort_by(|a, b| a.1.to_json().as_str().cmp(&b.1.to_json().as_str()));
            assert_eq!(actual_fields, fields);
        }
        let iids: Vec<_> = rows.iter().map(|row| row.iid.clone()).collect();
        if kind == WriteKind::PartialPut && prior_iids.is_empty() {
            prior_iids = iids;
        } else if kind == WriteKind::PartialPut {
            assert_eq!(iids, prior_iids);
        }
        let mut direct_put = links.put(&fields, &players).await.unwrap().items().to_vec();
        direct_put.sort_by(|a, b| a.iid.cmp(&b.iid));
        assert_eq!(rows, direct_put);
        assert_transaction(&trace(&path)[before..], count, true);
        println!(
            "wide proxy insert={}: {count} plans, two roots,300values,one HTTP transaction/commit",
            kind == WriteKind::Insert
        );
    }
    // Retain the default compiler ceiling to force the final-piece failure.
    let plans = ManagerPlans::new(Arc::clone(&registry));
    let failures = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("proxy-failure").unwrap(),
    );
    failures
        .insert(&vec![(
            "proxy-failure-tag".into(),
            AttributeValue::String("failure-299".into()),
        )])
        .await
        .unwrap();
    let program = program(
        &plans,
        "proxy-failure",
        WriteKind::Insert,
        &attributes("proxy-failure-tag", "failure"),
        &[],
    );
    let entries = program
        .staged_entries(plans.authority().unwrap())
        .unwrap()
        .unwrap();
    let count = entries.len();
    assert!(count > 1);
    let conflict = CanonicalValue::String(CanonicalString::new("failure-299").unwrap());
    let conflicting_pieces: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .rows()
                .iter()
                .any(|row| {
                    row.values()
                        .iter()
                        .any(|value| value.as_ref() == Some(&conflict))
                })
                .then_some(index)
        })
        .collect();
    assert_eq!(
        conflicting_pieces,
        [count - 1],
        "conflict first introduced in the last piece"
    );
    let before = trace(&path).len();
    let error = proxy
        .execute_write_stack(
            entries,
            resources,
            &cancellation,
            QueryExecutionDeadline::for_limits(resources),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            error.failure.as_deref(),
            Some(type_bridge_contract::failure::Failure::Rejected { .. })
        ),
        "{error:?}"
    );
    let failure = error.failure.as_deref().unwrap();
    assert_eq!(failure.code(), Some("CNT9"));
    if let type_bridge_contract::failure::Failure::Rejected {
        phase, op_index, ..
    } = failure
    {
        assert!(
            *phase == type_bridge_contract::failure::Phase::Commit
                || *op_index == Some((count - 1) as u32),
            "{failure:?}"
        );
    }
    let remaining = failures.all().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].attributes,
        vec![(
            "proxy-failure-tag".into(),
            AttributeValue::String("failure-299".into())
        )]
    );
    assert_transaction(&trace(&path)[before..], count, false);
    if std::env::var("TYPE_BRIDGE_WIDE_BENCHMARK").as_deref() == Ok("1") {
        benchmark_count(&proxy, &installed, &plans, &path).await;
    }
    drop(server);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test]
#[ignore = "requires an explicitly configured TypeDB"]
async fn partial_relation_update_by_player_keeps_all_roots_when_pieced() {
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap();
    let version = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION").unwrap();
    let name = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_DATABASE").unwrap();
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
    db.execute_raw(TYPEQL, TxType::Schema).await.unwrap();
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(DocumentId::new("proxy-wide.yaml").unwrap(), YAML)]).unwrap(),
    )
    .unwrap();
    let profile_name = if version.starts_with("3.12.") {
        "typedb-3.12.1/v1"
    } else {
        "typedb-3.13.6/v1"
    };
    let profile = SemanticProfileId::new(profile_name).unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        ManagedScopeId::new("proxy-wide").unwrap(),
        profile.clone(),
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
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
    .unwrap()
    .with_schema_authority(&authority);
    let registry = Arc::new(installed.match_registry().unwrap());
    let people = DynamicEntityManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.entity("proxy-person").unwrap(),
    );

    let player = people
        .insert(&vec![(
            "proxy-selector".into(),
            AttributeValue::String("shared-player".into()),
        )])
        .await
        .unwrap();
    let players = vec![DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "proxy-person".into(),
        iid: player.items()[0].iid.clone(),
        key: None,
    }];
    let links = DynamicRelationManager::from_registry(
        &db,
        Arc::clone(&registry),
        registry.relation("proxy-link").unwrap(),
    );
    for _ in 0..2 {
        assert_eq!(links.insert(&vec![], &players).await.unwrap().count(), 1);
    }
    let fields = (0..8)
        .map(|index| {
            (
                "proxy-tag".into(),
                AttributeValue::String(format!("value-{index}")),
            )
        })
        .collect::<DynamicAttributeMap>();
    let mut raised = QueryLimits::PLAN;
    raised.structural.bindings = 1024;
    raised.structural.selected_slots = 1024;
    raised.structural.boolean_terms = 1024;
    let high_db = db.clone().with_query_limits(raised).unwrap();
    let high = DynamicRelationManager::from_registry(
        &high_db,
        Arc::clone(&registry),
        registry.relation("proxy-link").unwrap(),
    );
    let high_limits = Arc::new(raised);
    let high_plans = ManagerPlans::new(Arc::clone(&registry)).with_query_limits(Some(&high_limits));
    assert!(matches!(
        high_plans
            .single_write_program(
                "proxy-link",
                WriteKind::PartialUpdate,
                None,
                &fields,
                &players
            )
            .unwrap(),
        ManagerWrite::One(_)
    ));
    let expected = high.update(None, &fields, &players).await.unwrap();
    let mut limits = QueryLimits::PLAN;
    limits.structural.bindings = 8;
    limits.structural.selected_slots = 8;
    limits.structural.input_rows = 2;
    let low_db = db.clone().with_query_limits(limits).unwrap();
    let low = DynamicRelationManager::from_registry(
        &low_db,
        Arc::clone(&registry),
        registry.relation("proxy-link").unwrap(),
    );
    let low_limits = Arc::new(limits);
    let low_plans = ManagerPlans::new(Arc::clone(&registry)).with_query_limits(Some(&low_limits));
    let ManagerWrite::Pieces(program) = low_plans
        .single_write_program(
            "proxy-link",
            WriteKind::PartialUpdate,
            None,
            &fields,
            &players,
        )
        .unwrap()
    else {
        panic!("forced pieces");
    };
    eprintln!(
        "partial relation update pieces={}",
        program.statement_count()
    );
    assert!(program.statement_count() > 1);
    let actual = low.update(None, &fields, &players).await;
    let stored = links.all().await;
    db.delete_database().await.unwrap();
    db.close().unwrap();
    let actual = actual.unwrap();
    let stored = stored.unwrap();
    assert_eq!(expected.count(), 2);
    assert_eq!(actual.count(), 2);
    assert_eq!(stored.len(), 2);
    for rows in [expected.items(), actual.items(), stored.as_slice()] {
        for row in rows {
            assert_eq!(
                row.attributes.len(),
                fields.len(),
                "{}",
                row.iid.as_deref().unwrap()
            );
            assert!(fields.iter().all(|field| row.attributes.contains(field)));
            assert_eq!(row.role_players.len(), 1);
        }
    }
    let ids = |rows: &[DynamicRelationRow]| {
        let mut ids = rows.iter().map(|row| row.iid.clone()).collect::<Vec<_>>();
        ids.sort();
        ids
    };
    assert_eq!(ids(expected.items()), ids(actual.items()));
    assert_eq!(ids(expected.items()), ids(&stored));
}

async fn benchmark_count(
    proxy: &ProxyQueryConnection,
    installed: &InstalledRuntimeProjection,
    plans: &ManagerPlans,
    path: &std::path::Path,
) {
    use crate::plan_builders::manager::{ManagerRead, ReadTerminal};
    use crate::projected_crud::ProjectedCrudInvocationControl;
    use crate::session::staged::{TransactionKind, TransactionOptions};
    use type_bridge_contract::query_remote::RemoteValue;
    use type_bridge_contract::query_remote::{StagedAnswer, StagedPlan};

    let manager = crate::ProjectedCrudExecutor::new(installed);
    let type_id = TypeId::new(TypeKind::Entity, "proxy-person").unwrap();
    let ManagerRead::Scalar { plan } = plans
        .read(
            "proxy-person",
            true,
            &[],
            &[],
            None,
            None,
            &[],
            ReadTerminal::Count,
        )
        .unwrap()
    else {
        panic!("one manager count plan")
    };
    let entries =
        vec![StagedPlan::new(plan.local(), vec![], plans.authority().unwrap().declared()).unwrap()];
    let schema = Arc::new(plans.authority().unwrap().declared().clone());
    let resources = QueryExecutionResourceLimits::default();
    let cancellation = AnswerCancellation::default();
    let before = trace(path).len();
    let mut manager_times = Vec::new();
    let mut manual_times = Vec::new();
    for index in 0..22 {
        let start = Instant::now();
        let count = manager
            .count_on_target(
                crate::QueryTarget::Proxy(proxy),
                &type_id,
                ProjectedCrudInvocationControl::capture(resources, cancellation.clone()),
            )
            .await
            .unwrap();
        let manager_elapsed = start.elapsed();
        assert_eq!(count, 2);
        let start = Instant::now();
        let mut transaction = proxy
            .staged_transaction(
                TransactionOptions::default().with_kind(TransactionKind::Read),
                &cancellation,
            )
            .await
            .unwrap();
        let pending = transaction
            .stage_plan_stack(entries.clone(), Arc::clone(&schema))
            .unwrap();
        transaction.commit().await.unwrap();
        let answer = pending[0].get().unwrap().to_staged(plan.local()).unwrap();
        let manual_elapsed = start.elapsed();
        assert_eq!(
            answer,
            StagedAnswer::Rows {
                rows: vec![vec![RemoteValue::Value {
                    value: CanonicalValue::Long(2)
                }]]
            }
        );
        if index >= 2 {
            manager_times.push(manager_elapsed.as_secs_f64() * 1000.0);
            manual_times.push(manual_elapsed.as_secs_f64() * 1000.0);
        }
    }
    let events = trace(path);
    assert_eq!(events.len() - before, 44 * 3);
    for group in events[before..].chunks_exact(3) {
        assert_transaction(group, 1, true);
        assert_eq!(group[1]["writes"], false);
        assert_eq!(group[2]["kind"], "read");
    }
    for (name, mut times) in [("manager", manager_times), ("manual", manual_times)] {
        times.sort_by(f64::total_cmp);
        println!(
            "proxy-count bench {name}: n=20 median_ms={:.3} mean_ms={:.3}; oneHTTP/oneplan/oneTX",
            (times[9] + times[10]) / 2.0,
            times.iter().sum::<f64>() / 20.0
        );
    }
}
