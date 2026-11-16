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
            |mut answers| { let Some(PlanAnswer::Documents(documents)) = answers.pop() else { panic!("documents"); }; crud.hydrate_entity_documents(proxy.execution_identity(), &kind, installed.entity_descriptor(&kind).unwrap(), &documents, ExecutionMode::default()) }).await.unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires configured TypeDB and TYPE_BRIDGE_SMOKE_SERVER"]
async fn required_wide_replacements_are_atomic_direct_and_over_the_real_proxy() {
    use base64::Engine;
    use crate::plan_authority::QueryAuthority;
    use crate::{ConnectOptions, Database, ProxyEndpoint, QueryTarget, TxType};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    struct Server(std::process::Child);
    impl Drop for Server { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    for kind in [TypeKind::Entity, TypeKind::Relation] {
        let model = TypeId::new(kind, if kind == TypeKind::Entity { "wide-item" } else { "wide-edge" }).unwrap();
        let (installed, _, fixture) = single_proxy_fixture_with_projection(model.clone(), 1, 1,
            type_bridge_contract::limits::QueryLimits::PLAN,
            wide_single_projection_for_target_and_minimum(false, BindingTarget::Python, 1));
        let context = type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("single-proxy").unwrap(), SemanticProfileId::new("typedb-3.13.6/v1").unwrap(), type_bridge_schema::schema_authority_capability_vocabulary());
        let authority = type_bridge_schema::build_schema_authority(&fixture.schema, fixture.schema.required_capabilities(), &context).unwrap();
        let name = format!("tb270_f2_required_{kind:?}_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap();
        let db = Database::connect_with_options(&address, &name, "admin", "password", ConnectOptions { tls: false, http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(), server_version: Some("3.13.6".parse().unwrap()) }).await.unwrap();
        db.create_database().await.unwrap();
        // A conflicting value in the final piece must abort the whole replacement.
        db.execute_raw("define attribute item-key, value string; attribute item-tag, value string; entity wide-item, owns item-key @key, owns item-tag @card(1..) @unique, plays wide-edge:member; relation wide-edge, owns item-key @key, owns item-tag @card(1..) @unique, relates member @card(0..4), plays wide-edge:member;", TxType::Schema).await.unwrap();
        let crud = ProjectedCrudExecutor::new(&installed);
        let entity = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let player = crud.insert_entity(&db, &proxy_batch_input(&installed, &entity, 1, "player")).await.unwrap().item().unwrap().iid().to_owned();
        let tag = OwnsFactId::new(model.clone(), AttributeId::new("item-tag").unwrap()).unwrap();
        let make = |count: usize, key: &str, prefix: &str, fail: bool| {
            let base = proxy_batch_input(&installed, &model, 1, key);
            let fields = base.fields().iter().map(|(field, values)| (field.clone(), if field == &tag {
                (0..count).map(|index| ProjectedAttributeValue::try_new(&installed, TypeId::new(TypeKind::Attribute, "item-tag").unwrap(), CanonicalValue::String(CanonicalString::new(if fail && index + 1 == count { "zz-blocked".into() } else { format!("{prefix}-{index}") }).unwrap())).unwrap()).collect()
            } else { values.clone() })).collect();
            let roles = if kind == TypeKind::Relation { vec![(RoleId::new("wide-edge", "member").unwrap(), vec![ProjectedReference::try_new(&installed, entity.clone(), Some(player.clone()), vec![]).unwrap()])] } else { vec![] };
            ProjectedCreate::try_new(&installed, model.clone(), fields, roles).unwrap()
        };
        let seed = make(1, "target", "old", false);
        let iid = if kind == TypeKind::Entity { crud.insert_entity(&db, &seed).await } else { crud.insert_relation(&db, &seed).await }.unwrap().item().unwrap().iid().to_owned();
        let blocked = make(1, "blocked", "unused", true);
        if kind == TypeKind::Entity { crud.insert_entity(&db, &blocked).await } else { crud.insert_relation(&db, &blocked).await }.unwrap();
        let port: u16 = std::env::var("SMOKE_PORT").unwrap().parse().unwrap();
        let server = Server(Command::new(std::env::var_os("TYPE_BRIDGE_SMOKE_SERVER").unwrap())
            .env("SMOKE_TYPEDB_ADDRESS", &address).env("SMOKE_TYPEDB_HTTP_PORT", std::env::var("TYPEDB_HTTP_PORT").unwrap())
            .env("SMOKE_TYPEDB_USERNAME", "admin").env("SMOKE_TYPEDB_PASSWORD", "password").env("SMOKE_DATABASE", &name)
            .env("SMOKE_PORT", port.to_string()).env("SMOKE_ALLOW_STAGED_WRITES", "true")
            .env("SMOKE_QUERY_LIMITS", r#"{"selected_slots":4}"#)
            .env("SMOKE_AUTHORITY_B64", base64::engine::general_purpose::STANDARD.encode(type_bridge_schema::encode_schema_authority(&authority)))
            .stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap());
        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(started.elapsed() < Duration::from_secs(30)); tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let policy = QueryExecutionResourceLimits::default();
        let proxy = crate::ProxyQueryConnection::connect(&ProxyEndpoint::new(format!("http://127.0.0.1:{port}")), None,
            Arc::new(QueryAuthority::from_declared_bytes(&type_bridge_contract::schema::encode_declared_schema(&fixture.schema).unwrap(), "single-proxy", "typedb-3.13.6/v1").unwrap()), policy, policy, &AnswerCancellation::default()).await.unwrap();
        let mut limits = type_bridge_contract::limits::QueryLimits::PLAN; limits.structural.selected_slots = 4;
        let low_db = db.clone().with_query_limits(limits).unwrap();
        for over_proxy in [false, true] {
            for count in [254, 300, 1000] {
                for operation in [ProjectedBatchOperation::Put, ProjectedBatchOperation::Update] {
                    for fail in [false, true] {
                        let before = if kind == TypeKind::Entity { crud.get_entity_by_iid(&db, &model, &iid).await } else { crud.get_relation_by_iid(&db, &model, &iid).await }.unwrap().unwrap();
                        let replacement = make(count, "target", &format!("v-{over_proxy}-{count}-{operation:?}"), fail);
                        let row = if operation == ProjectedBatchOperation::Update { ProjectedBatchRow::Update { iid: iid.clone(), replacement } } else { ProjectedBatchRow::Create(replacement) };
                        let batch = ProjectedBatch::try_new(&installed, model.clone(), operation, vec![row]).unwrap();
                        let target = if over_proxy { QueryTarget::Proxy(&proxy) } else { QueryTarget::Database(&low_db) };
                        let result = ProjectedBatchExecutor::new(&installed).execute_on_target_mapped(target, &batch,
                            ProjectedBatchInvocationControl::capture(Default::default(), Default::default()), (), |_| Ok(())).await;
                        assert_eq!(result.is_err(), fail, "{kind:?} proxy={over_proxy} {operation:?} {count}: {result:?}");
                        let after = if kind == TypeKind::Entity { crud.get_entity_by_iid(&db, &model, &iid).await } else { crud.get_relation_by_iid(&db, &model, &iid).await }.unwrap().unwrap();
                        if fail { assert_eq!(after.fields(), before.fields()); } else { assert_eq!(after.fields()[&tag].len(), count); }
                    }
                }
            }
        }
        drop(server); db.delete_database().await.unwrap(); db.close().unwrap();
    }
}

#[tokio::test]
#[ignore = "requires configured TypeDB"]
async fn dynamic_split_and_batch_updates_refuse_changed_keys_on_the_real_provider() {
    use crate::_manager::DynamicEntityManager;
    let installed = wide_single_projection_for_target_and_minimum(false, BindingTarget::Python, 0);
    let registry = installed.manager_registry().unwrap();
    let name = format!("tb270_f2_dynamic_keys_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let db = Database::connect_with_options(&std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap(), &name, "admin", "password", crate::ConnectOptions { tls: false, http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(), server_version: Some("3.13.6".parse().unwrap()) }).await.unwrap();
    db.create_database().await.unwrap();
    db.execute_raw("define attribute item-key, value string; attribute item-tag, value string; entity wide-item, owns item-key @key, owns item-tag @card(0..), plays wide-edge:member; relation wide-edge, owns item-key @key, owns item-tag @card(0..), relates member @card(0..4), plays wide-edge:member;", TxType::Schema).await.unwrap();
    let kind = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
    let crud = ProjectedCrudExecutor::new(&installed);
    let seed = crud.insert_entity(&db, &proxy_batch_input(&installed, &kind, 1, "original")).await.unwrap();
    let iid = seed.item().unwrap().iid().to_owned();
    let manager = DynamicEntityManager::from_registry(&db, Arc::clone(&registry), registry.entity("wide-item").unwrap());
    for count in [254, 300, 1000] {
        for batch in [false, true] {
            for bad in [false, true] {
                let mut attributes = vec![("item-key".into(), AttributeValue::String(if bad { "changed" } else { "original" }.into()))];
                attributes.extend((0..count).map(|index| ("item-tag".into(), AttributeValue::String(format!("tag-{index}")))));
                let result = if batch { manager.update_many_and_get_exact(&[(iid.clone(), attributes)]).await.map(|_| ()) } else { manager.update_exact(&iid, &attributes).await.map(|_| ()) };
                if bad { assert!(matches!(result, Err(OrmError::Match(ref error)) if error.code().as_str() == "query_plan_key_update"), "{result:?}"); } else { result.unwrap(); }
                let stored = crud.get_entity_by_iid(&db, &kind, &iid).await.unwrap().unwrap();
                assert_eq!(stored.fields()[&OwnsFactId::new(kind.clone(), AttributeId::new("item-key").unwrap()).unwrap()][0].value(), &CanonicalValue::String(CanonicalString::new("original").unwrap()));
                assert_eq!(stored.fields()[&OwnsFactId::new(kind.clone(), AttributeId::new("item-tag").unwrap()).unwrap()].len(), count);
            }
        }
    }
    db.delete_database().await.unwrap(); db.close().unwrap();
}
