//! Persistent handle construction and deterministic canonical lowering.

use std::fmt::Debug;
use std::sync::Arc;

use type_bridge_orm::*;

fn attribute(field_name: &str, attr_name: &str, value_type: ValueType) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.to_owned(),
        attr_name: attr_name.to_owned(),
        value_type,
        annotations: Vec::new(),
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                OwnedAttributeDescriptor {
                    annotations: vec![Annotation::Key],
                    ..attribute("name", "person-name", ValueType::String)
                },
                attribute("age", "person-age", ValueType::Long),
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "company".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                annotations: vec![Annotation::Key],
                ..attribute("name", "company-name", ValueType::String)
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attribute(
                "position",
                "employment-position",
                ValueType::String,
            )],
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
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "node".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "leaf-node".into(),
            is_abstract: false,
            parent_type: Some("node".into()),
            owned_attributes: vec![],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "directed-edge".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![
                RoleDescriptor {
                    role_name: "origin".into(),
                    player_type_names: vec!["leaf-node".into()],
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "destination".into(),
                    player_type_names: vec!["leaf-node".into()],
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    let inherited_name = OwnedAttributeDescriptor {
        annotations: vec![Annotation::Key],
        ..attribute("name", "party-name", ValueType::String)
    };
    registry
        .register_entity(EntityDescriptor {
            type_name: "party".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![inherited_name.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "employee".into(),
            is_abstract: false,
            parent_type: Some("party".into()),
            owned_attributes: vec![inherited_name],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    let inherited_participant = RoleDescriptor {
        role_name: "participant".into(),
        player_type_names: vec!["person".into()],
        ..Default::default()
    };
    registry
        .register_relation(RelationDescriptor {
            type_name: "association".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![inherited_participant.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "special-association".into(),
            is_abstract: false,
            parent_type: Some("association".into()),
            owned_attributes: vec![],
            roles: vec![inherited_participant.clone()],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "specialized-association".into(),
            is_abstract: false,
            parent_type: Some("association".into()),
            owned_attributes: vec![],
            roles: vec![RoleDescriptor {
                role_name: "member".into(),
                player_type_names: vec!["person".into()],
                overrides: Some("participant".into()),
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "collaboration".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![inherited_participant],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
}

fn assert_match_code<T: Debug>(result: type_bridge_orm::Result<T>, expected: &str) {
    match result {
        Err(OrmError::Match(error)) => assert_eq!(error.code().as_str(), expected),
        other => panic!("expected match error {expected}, got {other:?}"),
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn registered_bindings_are_session_owned_fresh_and_thread_safe() {
    assert_send_sync::<SessionHandle>();
    assert_send_sync::<BindingHandle>();
    assert_send_sync::<FieldHandle>();
    assert_send_sync::<RoleHandle>();
    assert_send_sync::<PredicateHandle>();
    assert_send_sync::<OrderHandle>();
    assert_send_sync::<SelectionHandle>();
    assert_send_sync::<ShapeHandle>();
    assert_send_sync::<QueryHandle>();

    let session = SessionHandle::new(registry());
    let first = session.exact("person").unwrap();
    let second = session.exact("person").unwrap();
    let polymorphic = session.subtypes("person").unwrap();

    assert_ne!(first, second);
    assert_eq!(first.descriptor_id(), second.descriptor_id());
    assert_eq!(first.match_mode(), MatchMode::Exact);
    assert_eq!(polymorphic.match_mode(), MatchMode::Subtypes);
    assert_eq!(first.thing_kind(), ThingKind::Entity);
    assert_eq!(
        session.exact("employment").unwrap().thing_kind(),
        ThingKind::Relation
    );
    assert_match_code(session.exact("missing"), "unknown_descriptor");
}

#[test]
fn owner_qualified_handles_reject_aliases_and_preserve_inherited_owners() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let employee = session.exact("employee").unwrap();
    let special = session.exact("special-association").unwrap();

    let inherited_name = employee.field_owned_by("party", "name").unwrap();
    assert_eq!(inherited_name.field_id().owner.as_str(), "entity:party");
    assert_match_code(
        person.field_owned_by("company", "name"),
        "cross_owner_field",
    );

    let inherited_role = special.role_owned_by("association", "participant").unwrap();
    assert_eq!(
        inherited_role.role_id().owner.as_str(),
        "relation:association"
    );
    assert_match_code(
        special.role_owned_by("collaboration", "participant"),
        "cross_owner_role",
    );

    let predicate = inherited_role.connects(&person).unwrap().and(
        &inherited_name
            .compare_field(ComparisonOp::Equal, &person.field("name").unwrap())
            .unwrap(),
    );
    let query = session
        .query(session.positional([employee.one(), special.one()]).unwrap())
        .unwrap()
        .add_hidden(person)
        .unwrap()
        .where_predicate(predicate.unwrap())
        .unwrap();
    let validated = query
        .validate_fetch_rows(
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
            RowCardinality::ExactlyOne,
        )
        .unwrap();
    let diagnostic = UnvalidatedMatchRequest::from_request(validated.request().clone()).unwrap();
    let encoded = String::from_utf8(diagnostic.to_canonical_bytes().unwrap()).unwrap();
    assert!(encoded.contains("entity:party"));
    assert!(encoded.contains("relation:association"));
}

#[test]
fn output_shape_arity_is_rejected_during_native_construction() {
    let session = SessionHandle::new(registry());
    assert_match_code(session.positional([]), "empty_output");
    assert_match_code(session.named::<_, String>([]), "empty_output");

    let slots = (0..=MAX_SELECTED_SLOTS)
        .map(|_| session.exact("person").unwrap().one())
        .collect::<Vec<_>>();
    assert_match_code(session.positional(slots), "selection_cap_exceeded");
}

#[test]
fn named_declaration_is_checked_against_native_selection_contracts() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let slots = || {
        vec![
            ("employee".to_owned(), person.one()),
            ("employers".to_owned(), company.collect()),
        ]
    };
    let declarations = || {
        vec![
            ("employee".to_owned(), "person".to_owned(), false),
            ("employers".to_owned(), "company".to_owned(), true),
        ]
    };

    session.named_checked(declarations(), slots()).unwrap();
    assert_match_code(
        session.named_checked(
            vec![("employee".to_owned(), "person".to_owned(), false)],
            slots(),
        ),
        "named_declaration_length_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("person".to_owned(), "person".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_name_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "company".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_descriptor_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "person".to_owned(), true),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "named_declaration_cardinality_mismatch",
    );
    assert_match_code(
        session.named_checked(
            vec![
                ("employee".to_owned(), "missing".to_owned(), false),
                ("employers".to_owned(), "company".to_owned(), true),
            ],
            slots(),
        ),
        "unknown_declared_descriptor",
    );
}

#[test]
fn lowering_assigns_selected_then_hidden_contiguous_binding_ids() {
    let session = SessionHandle::new(registry());
    let selected_company = session.exact("company").unwrap();
    let selected_person = session.subtypes("person").unwrap();
    let hidden_relation = session.exact("employment").unwrap();
    let hidden_second_person = session.exact("person").unwrap();

    let shape = session
        .positional([selected_company.one(), selected_person.one()])
        .unwrap();
    let original = session.query(shape).unwrap();
    let with_relation = original.add_hidden(hidden_relation).unwrap();
    let complete = with_relation.add_hidden(hidden_second_person).unwrap();

    let original_request = original.count_by(&selected_company).unwrap();
    assert_eq!(original_request.plan.bindings.len(), 2);

    let request = complete.count_by(&selected_company).unwrap();
    assert_eq!(request.plan.bindings.len(), 4);
    assert_eq!(
        request
            .plan
            .bindings
            .iter()
            .map(|binding| (binding.id.get(), binding.descriptor.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (0, "entity:company"),
            (1, "entity:person"),
            (2, "relation:employment"),
            (3, "entity:person"),
        ]
    );
    assert_eq!(request.plan.bindings[1].match_mode, MatchMode::Subtypes);
    assert_eq!(request.plan.bindings[3].match_mode, MatchMode::Exact);
}

#[test]
fn immutable_transitions_lower_fields_roles_orders_shapes_and_cross_joins() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let employment = session.exact("employment").unwrap();

    let person_name = person.field("person-name").unwrap();
    let company_name = company.field("name").unwrap();
    let company_order = company_name.order(SortDirection::Ascending, MissingOrder::Last);
    let company_selection = company
        .collect()
        .distinct(true)
        .unwrap()
        .order_by(company_order.clone())
        .unwrap();
    let shape = session
        .named([("employee", person.one()), ("employers", company_selection)])
        .unwrap();
    let base = session.query(shape).unwrap();
    let attached = base.add_hidden(employment.clone()).unwrap();

    let employee_edge = employment
        .role("employee")
        .unwrap()
        .connects(&person)
        .unwrap();
    let employer_edge = employment
        .role("employer")
        .unwrap()
        .connects(&company)
        .unwrap();
    let named_alice =
        person_name.compare_value(ComparisonOp::Equal, AttributeValue::String("Alice".into()));
    let predicate = employee_edge
        .and(&employer_edge)
        .unwrap()
        .and(&named_alice.not())
        .unwrap();
    let filtered = attached.where_predicate(predicate).unwrap();
    let joined = filtered.allow_cross_join(&person, &company).unwrap();

    let request = joined
        .fetch_rows(
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 3,
                limit: 10,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap();

    assert_eq!(base.count_by(&person).unwrap().plan.bindings.len(), 2);
    assert!(attached.count_by(&person).unwrap().plan.predicate.is_none());
    assert_eq!(request.plan.allowed_cross_joins.len(), 1);
    assert_eq!(
        request.plan.allowed_cross_joins.iter().next().unwrap(),
        &BindingPair::new(BindingId::new(0), BindingId::new(1))
    );

    let MatchOperation::FetchRows {
        output,
        order,
        window,
        cardinality,
    } = &request.operation
    else {
        panic!("expected fetch rows")
    };
    assert_eq!(
        *window,
        Window {
            offset: 3,
            limit: 10
        }
    );
    assert_eq!(*cardinality, RowCardinality::BoundedMany);
    assert_eq!(order[0].field.binding, BindingId::new(0));
    assert_eq!(order[0].field.field.name, "name");
    let FetchShape::Named { slots } = output else {
        panic!("expected named output")
    };
    assert_eq!(slots[0].name, "employee");
    assert_eq!(slots[1].name, "employers");
    let FetchSlot::Collect {
        binding,
        distinct,
        order,
    } = &slots[1].slot
    else {
        panic!("expected collection slot")
    };
    assert_eq!(*binding, BindingId::new(1));
    assert!(*distinct);
    assert_eq!(order[0].field.binding, BindingId::new(1));

    let mut role_edges = Vec::new();
    collect_role_edge_ids(request.plan.predicate.as_ref().unwrap(), &mut role_edges);
    assert_eq!(role_edges, vec![0, 1]);
}

fn collect_role_edge_ids(expression: &MatchExpr, ids: &mut Vec<u16>) {
    match expression {
        MatchExpr::RoleEdge { id, .. } => ids.push(id.get()),
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for expression in expressions {
                collect_role_edge_ids(expression, ids);
            }
        }
        MatchExpr::Not { expression } => collect_role_edge_ids(expression, ids),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::Reachable { .. } => {}
    }
}

#[test]
fn duplicate_cross_session_and_unattached_handles_are_rejected() {
    let registry = registry();
    let first = SessionHandle::new(Arc::clone(&registry));
    let second = SessionHandle::new(registry);
    let person = first.exact("person").unwrap();
    let company = first.exact("company").unwrap();
    let foreign_person = second.exact("person").unwrap();

    let duplicate_shape = first.positional([person.one(), person.one()]).unwrap();
    assert_match_code(first.query(duplicate_shape), "duplicate_selection");
    assert_match_code(
        first.positional([person.one(), foreign_person.one()]),
        "cross_session_handle",
    );

    let query = first
        .query(first.positional([person.one()]).unwrap())
        .unwrap();
    assert_match_code(
        query.add_hidden(foreign_person.clone()),
        "cross_session_handle",
    );
    assert_match_code(query.add_hidden(person.clone()), "duplicate_binding");
    assert_match_code(query.count_by(&company), "unattached_binding");
    assert_match_code(
        query.where_predicate(
            company
                .field("name")
                .unwrap()
                .compare_value(ComparisonOp::Equal, AttributeValue::String("Acme".into())),
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.where_predicate(
            company
                .field("name")
                .unwrap()
                .compare_value(ComparisonOp::Equal, AttributeValue::String("Acme".into()))
                .not(),
        ),
        "unattached_binding",
    );
    assert_match_code(
        query.fetch_rows(
            &[company
                .field("name")
                .unwrap()
                .order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 0,
                limit: 5,
            },
            RowCardinality::BoundedMany,
        ),
        "unattached_binding",
    );
    assert_match_code(
        person
            .field("name")
            .unwrap()
            .compare_field(ComparisonOp::Equal, &foreign_person.field("name").unwrap()),
        "cross_session_handle",
    );
}

#[test]
fn bounded_reachability_is_session_owned_bounded_and_subtype_aware() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let other = SessionHandle::new(registry);
    let source = session.subtypes("node").unwrap();
    let target = session.subtypes("node").unwrap();
    let foreign = other.subtypes("node").unwrap();

    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            3,
            2,
        ),
        "reachable_bounds",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            65,
        ),
        "reachable_depth_limit",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &foreign,
            0,
            2,
        ),
        "cross_session_handle",
    );
    assert_match_code(
        session.reachable(
            "directed-edge",
            "origin",
            "destination",
            &session.exact("node").unwrap(),
            &target,
            0,
            2,
        ),
        "incompatible_reachable_endpoint",
    );

    let reachable = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            0,
            3,
        )
        .unwrap();
    let deep = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            64,
        )
        .unwrap();
    let deep_query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(deep.and(&deep).unwrap())
        .unwrap();
    assert_match_code(
        deep_query.validate_count_by(&source),
        "reachable_expansion_limit",
    );
    session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable.and(&reachable).unwrap())
        .unwrap()
        .validate_count_by(&source)
        .expect("nested root conjunction remains valid");
    let query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable)
        .unwrap();
    let validated = query.validate_count_by(&source).unwrap();
    let MatchExpr::Reachable {
        relation,
        role_from,
        role_to,
        source,
        target,
        min_depth,
        max_depth,
    } = validated.request().plan.predicate.as_ref().unwrap()
    else {
        panic!("expected bounded reachability")
    };
    assert_eq!(relation.as_str(), "relation:directed-edge");
    assert_eq!(role_from.name, "origin");
    assert_eq!(role_to.name, "destination");
    assert_eq!((*source, *target), (BindingId::new(0), BindingId::new(1)));
    assert_eq!((*min_depth, *max_depth), (0, 3));
    assert!(
        validated
            .capabilities()
            .contains(Capability::BoundedReachability)
    );
}

#[test]
fn bounded_reachability_is_rejected_under_disjunction_or_negation() {
    let session = SessionHandle::new(registry());
    let source = session.exact("leaf-node").unwrap();
    let target = session.exact("leaf-node").unwrap();
    let reachable = session
        .reachable(
            "directed-edge",
            "origin",
            "destination",
            &source,
            &target,
            1,
            2,
        )
        .unwrap();

    let query = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap();
    assert_match_code(
        query
            .where_predicate(reachable.or(&reachable).unwrap())
            .unwrap()
            .validate_count_by(&source),
        "reachable_not_root",
    );
    assert_match_code(
        query
            .where_predicate(reachable.not())
            .unwrap()
            .validate_count_by(&source),
        "reachable_not_root",
    );
}

#[test]
fn reachable_canonicalizes_an_inherited_role_to_the_exact_child_relation() {
    let session = SessionHandle::new(registry());
    let child_relation = session.exact("special-association").unwrap();
    let inherited = child_relation
        .role_owned_by("association", "participant")
        .expect("ancestor role reference remains effective on the child");
    let source = session.exact("person").unwrap();
    let target = session.exact("person").unwrap();

    let reachable = session
        .reachable(
            "special-association",
            &inherited.role_id().name,
            &inherited.role_id().name,
            &source,
            &target,
            1,
            2,
        )
        .expect("effective inherited role is accepted by name");
    let validated = session
        .query(session.positional([source.one(), target.one()]).unwrap())
        .unwrap()
        .where_predicate(reachable)
        .unwrap()
        .validate_count_by(&source)
        .unwrap();
    let MatchExpr::Reachable {
        relation,
        role_from,
        role_to,
        ..
    } = validated.request().plan.predicate.as_ref().unwrap()
    else {
        panic!("expected bounded reachability");
    };
    assert_eq!(relation.as_str(), "relation:special-association");
    assert_eq!(
        role_from.owner.as_str(),
        "relation:special-association",
        "provider lowering receives the exact child relation-owned role",
    );
    assert_eq!(role_to.owner, role_from.owner);

    assert_match_code(
        session.reachable(
            "specialized-association",
            &inherited.role_id().name,
            &inherited.role_id().name,
            &source,
            &target,
            1,
            2,
        ),
        "unknown_role",
    );
}

#[test]
fn canonical_requests_ignore_process_local_allocation_history() {
    fn build(registry: Arc<DescriptorRegistry>, allocate_noise: bool) -> MatchRequest {
        let session = SessionHandle::new(registry);
        if allocate_noise {
            let _ = session.exact("person").unwrap();
            let _ = session.exact("employment").unwrap();
        }
        let person = session.exact("person").unwrap();
        let company = session.exact("company").unwrap();
        let employment = session.exact("employment").unwrap();
        if allocate_noise {
            let _ = session.exact("company").unwrap();
        }

        let shape = session.positional([person.one(), company.one()]).unwrap();
        let predicate = employment
            .role("employee")
            .unwrap()
            .connects(&person)
            .unwrap()
            .and(
                &employment
                    .role("employer")
                    .unwrap()
                    .connects(&company)
                    .unwrap(),
            )
            .unwrap();
        session
            .query(shape)
            .unwrap()
            .add_hidden(employment)
            .unwrap()
            .where_predicate(predicate)
            .unwrap()
            .page_by(
                &person,
                &[person
                    .field("age")
                    .unwrap()
                    .order(SortDirection::Descending, MissingOrder::Last)],
                Window {
                    offset: 5,
                    limit: 20,
                },
                true,
            )
            .unwrap()
    }

    let registry = registry();
    let first = build(Arc::clone(&registry), false);
    let second = build(registry, true);
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );

    let debug = format!(
        "{:?}",
        SessionHandle::new(Arc::new(DescriptorRegistry::new()))
    );
    assert!(debug.contains("SessionId(..)"));
    assert!(!debug.contains("73657373"));
    let serialized = serde_json::to_string(&first).unwrap();
    assert!(!serialized.contains("SessionId"));
    assert!(!serialized.contains("SessionBindingToken"));
}

#[test]
fn handle_requests_validate_and_survive_canonical_diagnostics() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let employment = session.exact("employment").unwrap();
    let person_name = person.field("name").unwrap();
    let company_name = company.field("name").unwrap();
    let connected = employment
        .role("employee")
        .unwrap()
        .connects(&person)
        .unwrap()
        .and(
            &employment
                .role("employer")
                .unwrap()
                .connects(&company)
                .unwrap(),
        )
        .unwrap();

    let positional = session
        .query(session.positional([person.one(), company.one()]).unwrap())
        .unwrap()
        .add_hidden(employment.clone())
        .unwrap()
        .where_predicate(connected.clone())
        .unwrap()
        .fetch_rows(
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 0,
                limit: 25,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap();
    let direct = validate_match_request(&registry, positional.clone()).unwrap();
    assert_eq!(direct.request(), &positional);
    assert_eq!(direct.stable_order().terms().len(), 2);

    let diagnostic = UnvalidatedMatchRequest::from_request(positional).unwrap();
    let bytes = diagnostic.to_canonical_bytes().unwrap();
    let decoded = UnvalidatedMatchRequest::from_canonical_bytes(&bytes).unwrap();
    let revalidated = decoded.validate(&registry).unwrap();
    assert_eq!(direct.request(), revalidated.request());
    assert_eq!(
        direct.schema_fingerprint(),
        revalidated.schema_fingerprint()
    );
    assert_eq!(direct.shape_id(), revalidated.shape_id());
    assert_eq!(direct.stable_order(), revalidated.stable_order());
    assert_eq!(direct.capabilities(), revalidated.capabilities());

    let collected_company = company
        .collect()
        .distinct(true)
        .unwrap()
        .order_by(company_name.order(SortDirection::Ascending, MissingOrder::Reject))
        .unwrap();
    let named_page = session
        .query(
            session
                .named([("person", person.one()), ("companies", collected_company)])
                .unwrap(),
        )
        .unwrap()
        .add_hidden(employment)
        .unwrap()
        .where_predicate(connected)
        .unwrap()
        .page_by(
            &person,
            &[person_name.order(SortDirection::Ascending, MissingOrder::Reject)],
            Window {
                offset: 10,
                limit: 10,
            },
            true,
        )
        .unwrap();
    let validated_page = validate_match_request(&registry, named_page.clone()).unwrap();
    assert_eq!(validated_page.request(), &named_page);
    assert!(
        validated_page
            .capabilities()
            .contains(Capability::CollectDistinct)
    );
    assert!(
        validated_page
            .capabilities()
            .contains(Capability::DistinctRootSelection)
    );
}

#[test]
fn every_terminal_only_builds_an_unvalidated_request() {
    let session = SessionHandle::new(registry());
    let person = session.exact("person").unwrap();
    let query = session
        .query(session.positional([person.one()]).unwrap())
        .unwrap();
    let order = [person
        .field("name")
        .unwrap()
        .order(SortDirection::Ascending, MissingOrder::Reject)];
    let window = Window {
        offset: 0,
        limit: 1,
    };

    assert!(matches!(
        query
            .fetch_rows(&order, window, RowCardinality::ExactlyOne)
            .unwrap()
            .operation,
        MatchOperation::FetchRows { .. }
    ));
    assert!(matches!(
        query
            .page_by(&person, &order, window, false)
            .unwrap()
            .operation,
        MatchOperation::PageBy { .. }
    ));
    assert!(matches!(
        query.count_by(&person).unwrap().operation,
        MatchOperation::CountBy { .. }
    ));
    assert!(matches!(
        query.exists_by(&person).unwrap().operation,
        MatchOperation::ExistsBy { .. }
    ));
}
