use super::*;
use crate::_descriptor::{OwnedAttributeDescriptor, RoleDescriptor};
use crate::_entity::Annotation;
use crate::{AttributeValue, ValueType};

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
fn dynamic_relation_update_exact_preflight_zero_io_table() {
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
    let mut role_cases: Vec<(RelationDescriptor, Vec<DynamicRolePlayerInput>, &str)> = vec![
        (
            employment_descriptor(),
            vec![DynamicRolePlayerInput {
                role_name: "ghost".into(),
                ..player("ghost")
            }],
            "employment: unknown relation role ghost",
        ),
        (
            employment_descriptor(),
            vec![DynamicRolePlayerInput {
                player_type_name: "bad type".into(),
                ..player("employee")
            }],
            "employment: unsafe player type label bad type",
        ),
        (
            employment_descriptor(),
            vec![DynamicRolePlayerInput {
                key: Some(("bad key".into(), AttributeValue::String("x".into()))),
                iid: None,
                ..player("employee")
            }],
            "employment: unsafe player key label bad key",
        ),
        (
            employment_descriptor(),
            vec![DynamicRolePlayerInput {
                key: Some(("".into(), AttributeValue::String("x".into()))),
                iid: None,
                ..player("employee")
            }],
            "employment: unsafe player key label ",
        ),
    ];
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
    for (descriptor, attrs, iid, expected) in cases {
        let err = validate_relation_exact_update_input(&descriptor, iid, &attrs, &[]).unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(message) if message == expected));
    }
    for (descriptor, roles, expected) in role_cases {
        let err = validate_relation_exact_update_input(&descriptor, "0x1", &Vec::new(), &roles)
            .unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(message) if message == expected));
    }
}

#[test]
fn dynamic_relation_put_exact_single_preflight_zero_io_table() {
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
        let err = validate_relation_exact_put_input(&d, &a, &p).unwrap_err();
        assert!(matches!(err, OrmError::QueryExecution(message) if message == expected));
    }
    validate_relation_exact_update_input(
        &exact_put_builder_descriptor(),
        "0x1",
        &vec![("title".into(), AttributeValue::String("Engineer".into()))],
        &players(),
    )
    .unwrap();
}

#[test]
fn dynamic_relation_update_exact_preflight_accounts_by_descriptor_index() {
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
    validate_relation_exact_update_input(
        &d,
        "0x1",
        &vec![("alpha".into(), AttributeValue::String("x".into()))],
        &[DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x2".into()),
            key: None,
        }],
    )
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn native_write_holds_the_borrowed_mutation_lease_through_materialization() {
    use crate::session::backend::{BoxFuture, DriverBackend, TransactionOps};
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    struct Backend(Arc<AtomicUsize>);
    struct Tx(Arc<AtomicUsize>);
    impl DriverBackend for Backend {
        fn open_transaction(
            &self,
            _: &str,
            _: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>>> {
            let commits = Arc::clone(&self.0);
            Box::pin(async move { Ok(Box::new(Tx(commits)) as Box<dyn TransactionOps>) })
        }
        fn is_open(&self) -> bool {
            true
        }
    }
    impl TransactionOps for Tx {
        fn query(&mut self, query: &str) -> BoxFuture<'_, Result<QueryResult>> {
            let (iid, name) = if query.contains("Bob") {
                ("0x12", "Bob")
            } else {
                ("0x11", "Alice")
            };
            Box::pin(async move {
                Ok(QueryResult::Documents(vec![
                    serde_json::json!({"_iid":iid,"_type":"person","attributes":{"name":name}}),
                ]))
            })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    let documents = SchemaDocumentSet::parse([(DocumentId::new("mapped-write.yaml").unwrap(), "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person:\n    owns:\n      name: { key: true }\n")]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = crate::InstalledRuntimeProjection::try_new(
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
    let registry = installed.manager_registry().unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    let db = Database::with_backend(Box::new(Backend(Arc::clone(&commits))), "mapped-write");
    for (group, expires) in [(false, false), (false, true), (true, false), (true, true)] {
        let tx = db.transaction_context(TxType::Write).await.unwrap();
        let manager = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            Arc::clone(&registry),
            registry.entity("person").unwrap(),
        )
        .literal_values();
        let map = || {
            let mut commit = Box::pin(tx.commit_sdk());
            assert!(matches!(
                commit
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Pending
            ));
            assert_eq!(commits.load(Ordering::SeqCst), 0);
            if expires {
                let mut advance = Box::pin(tokio::time::advance(std::time::Duration::from_millis(
                    crate::query_execution_limits::DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS + 1,
                )));
                let _ = advance
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()));
                Ok(())
            } else {
                Err(OrmError::Hydration {
                    type_name: "person".into(),
                    message: "materializer refused".into(),
                })
            }
        };
        let error = if group {
            let plans = manager.plans.as_ref().unwrap();
            let build = |name: &str| {
                plans
                    .single_write(
                        "person",
                        WriteKind::Insert,
                        None,
                        &vec![("name".into(), AttributeValue::String(name.into()))],
                        &[],
                    )
                    .unwrap()
            };
            manager
                .target
                .write_plan_group_mapped(
                    crate::plan_builders::bulk_write::WriteProgram::compact(vec![
                        build("Alice"),
                        build("Bob"),
                    ]),
                    |_, _| Ok(()),
                    |rows| {
                        assert_eq!(rows.len(), 2);
                        map()
                    },
                )
                .await
        } else {
            manager
                .write_mapped(
                    WriteKind::Insert,
                    None,
                    &vec![("name".into(), AttributeValue::String("Alice".into()))],
                    |rows| {
                        assert_eq!(rows.count(), 1);
                        map()
                    },
                )
                .await
        }
        .unwrap_err();
        if expires {
            assert!(
                matches!(&error, OrmError::ExecutionDiagnostic(diagnostic) if diagnostic.code().as_str() == "transaction_deadline_exceeded"),
                "{error:?}"
            );
        } else {
            assert!(matches!(error, OrmError::Hydration { .. }));
        }
        assert_eq!(
            tx.lifecycle_state().await,
            crate::session::context::TransactionContextState::RollbackOnly
        );
        assert!(tx.commit_sdk().await.is_err());
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        tx.rollback().await.unwrap();
    }
}
