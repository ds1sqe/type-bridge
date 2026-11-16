// Opt-in live proof and local before/after measurements for key guards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires configured TypeDB and TYPE_BRIDGE_SMOKE_SERVER"]
async fn live_proxy_key_guards_refuse_atomically_and_measure_added_reads() {
    use base64::Engine;
    use crate::plan_authority::QueryAuthority;
    use crate::{ConnectOptions, Database, ProxyEndpoint, QueryTarget, TxType};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap();
    let version = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION").unwrap();
    assert_eq!(version, "3.13.6");
    let name = format!("tb270_f2_proxy_keys_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let db = Database::connect_with_options(&address, &name, "admin", "password", ConnectOptions {
        http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(), tls: false, server_version: Some(version.parse().unwrap()),
    }).await.unwrap();
    db.create_database().await.unwrap();
    db.execute_raw("define attribute item-key, value string; attribute item-tag, value string; entity wide-item, owns item-key @key, owns item-tag @card(0..), plays wide-edge:member; relation wide-edge, owns item-key @key, owns item-tag @card(0..), relates member @card(0..4), plays wide-edge:member;", TxType::Schema).await.unwrap();
    let kind = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
    let (installed, _, fixture) = single_proxy_fixture(kind.clone(), 3, 1);
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("single-proxy").unwrap(), SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        type_bridge_schema::schema_authority_capability_vocabulary());
    let verified = type_bridge_schema::build_schema_authority(&fixture.schema, fixture.schema.required_capabilities(), &context).unwrap();
    let port: u16 = std::env::var("SMOKE_PORT").unwrap().parse().unwrap();
    let server = Server(Command::new(std::env::var_os("TYPE_BRIDGE_SMOKE_SERVER").unwrap())
        .env("SMOKE_TYPEDB_ADDRESS", &address).env("SMOKE_TYPEDB_HTTP_PORT", std::env::var("TYPEDB_HTTP_PORT").unwrap())
        .env("SMOKE_TYPEDB_USERNAME", "admin").env("SMOKE_TYPEDB_PASSWORD", "password").env("SMOKE_DATABASE", &name)
        .env("SMOKE_PORT", port.to_string()).env("SMOKE_ALLOW_STAGED_WRITES", "true")
        .env("SMOKE_AUTHORITY_B64", base64::engine::general_purpose::STANDARD.encode(type_bridge_schema::encode_schema_authority(&verified)))
        .stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap());
    let started = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(started.elapsed() < Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let policy = QueryExecutionResourceLimits::default();
    let proxy = crate::ProxyQueryConnection::connect(&ProxyEndpoint::new(format!("http://127.0.0.1:{port}")), None,
        Arc::new(QueryAuthority::from_declared_bytes(&type_bridge_contract::schema::encode_declared_schema(&fixture.schema).unwrap(), "single-proxy", "typedb-3.13.6/v1").unwrap()),
        policy, policy, &AnswerCancellation::default()).await.unwrap();
    let crud = ProjectedCrudExecutor::new(&installed);
    let mut iids = Vec::new();
    for key in ["first", "second", "third"] {
        let result = crud.insert_entity(&db, &proxy_batch_input(&installed, &kind, 1, key)).await.unwrap();
        iids.push(result.item().unwrap().iid().to_owned());
    }
    let control = || ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
    let replacement = proxy_batch_input(&installed, &kind, 1, "first");
    let single = crud.single_controlled_write_batch(&kind, ProjectedBatchOperation::Update,
        ProjectedBatchRow::Update { iid: iids[0].clone(), replacement: replacement.clone() }, &control()).unwrap();
    let authority = installed.plan_authority().unwrap();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        let write = crud.lower_plan_write(&replacement).unwrap();
        let old = update(&authority, ValueTransport::Given, ThingUpdate {
            type_id: &kind, selection: UpdateSelection::Iid(&iids[0]), keys: &[],
            cleared_fields: &write.cleared_fields, fields: &write.fields, cleared_roles: &[], players: &[], document_roles: vec![],
        }).unwrap();
        let entry = type_bridge_contract::query_remote::StagedPlan::new(old.local(), old.local().given_rows().unwrap().to_vec(), authority.declared()).unwrap();
        proxy.execute_write_stack_mapped(vec![entry], policy, &AnswerCancellation::default(), QueryExecutionDeadline::for_limits(policy),
            |mut answers| { let Some(PlanAnswer::Documents(documents)) = answers.pop() else { panic!("documents"); }; crud.hydrate_entity_documents(proxy.execution_identity(), &kind, &installed.entity_descriptor(&kind).unwrap(), &documents, ExecutionMode::default()) }).await.unwrap();
        before.push(started.elapsed());
        let started = Instant::now();
        crud.execute_single_write_on_target(QueryTarget::Proxy(&proxy), &single, control(), Ok).await.unwrap();
        after.push(started.elapsed());
    }
    before.sort(); after.sort();
    println!("PROXY KEYED UPDATE MEDIAN baseline={:?} guarded={:?}", before[4], after[4]);
    let batch = |wrong: bool, missing: bool| ProjectedBatch::try_new(&installed, kind.clone(), ProjectedBatchOperation::Update,
        ["first", "second", "third"].iter().enumerate().map(|(index, key)| {
            let mut replacement = proxy_batch_input(&installed, &kind, 1, if wrong && index == 1 { "wrong-second" } else { key });
            if index == 0 && (wrong || missing) {
                let fields = replacement.fields().iter().map(|(field, values)| (field.clone(), if field.attribute().label().as_str() == "item-tag" {
                    vec![ProjectedAttributeValue::try_new(&installed, TypeId::new(TypeKind::Attribute, "item-tag").unwrap(), CanonicalValue::String(CanonicalString::new("must-not-write").unwrap())).unwrap()]
                } else { values.clone() })).collect();
                replacement = ProjectedCreate::try_new(&installed, kind.clone(), fields, vec![]).unwrap();
            }
            ProjectedBatchRow::Update {
                iid: if missing && index == 1 { "0x00".into() } else { iids[index].clone() }, replacement,
            }
        }).collect()).unwrap();
    let batches = ProjectedBatchExecutor::new(&installed);
    let valid = batch(false, false);
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        let bytes = crate::projected_batch_executor::inline_batch_plan_bytes_for_test(&installed, &valid, proxy.execution_identity(), false).unwrap();
        let old = type_bridge_contract::query_plan::decode_query_plan(&bytes, authority.declared()).unwrap();
        let entries = vec![type_bridge_contract::query_remote::StagedPlan::new(&old, old.given_rows().unwrap().to_vec(), authority.declared()).unwrap()];
        proxy.execute_write_stack(entries, policy, &AnswerCancellation::default(), QueryExecutionDeadline::for_limits(policy)).await.unwrap();
        before.push(started.elapsed());
        let started = Instant::now();
        batches.execute_on_target_mapped(QueryTarget::Proxy(&proxy), &valid, control(), (), |_| Ok(())).await.unwrap();
        after.push(started.elapsed());
    }
    before.sort(); after.sort();
    println!("PROXY UPDATE_MANY MEDIAN baseline={:?} guarded={:?}", before[4], after[4]);
    for (wrong, missing) in [(true, false), (false, true)] {
        let error = batches.execute_on_target_mapped(QueryTarget::Proxy(&proxy), &batch(wrong, missing), control(), (), |_| Ok(())).await.unwrap_err().into_diagnostic(SdkProviderOperation::Write);
        assert_eq!(error.code().as_str(), "query_plan_key_update");
        assert_eq!(error.message().as_str(), "IID not found or key differs");
        assert!(error.path().contains(&SdkDiagnosticPathSegment::Index(1)));
        for (index, key) in ["first", "second", "third"].iter().enumerate() {
            let stored = crud.get_entity_by_iid(&db, &kind, &iids[index]).await.unwrap().unwrap();
            assert_eq!(stored.fields()[&OwnsFactId::new(kind.clone(), AttributeId::new("item-key").unwrap()).unwrap()][0].value(), &CanonicalValue::String(CanonicalString::new(*key).unwrap()));
            assert_eq!(stored.fields()[&OwnsFactId::new(kind.clone(), AttributeId::new("item-tag").unwrap()).unwrap()][0].value(), &CanonicalValue::String(CanonicalString::new("tag-0").unwrap()));
        }
    }
    drop(proxy);
    drop(server);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}
