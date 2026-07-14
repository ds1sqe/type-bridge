//! Fail-closed request-validation coverage across structure, schema, topology,
//! output shape, ordering, and capability preflight.

use std::collections::BTreeSet;

use type_bridge_orm::*;

fn attribute(
    field: &str,
    schema: &str,
    value_type: ValueType,
    annotations: Vec<Annotation>,
    optional: bool,
) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field.into(),
        attr_name: schema.into(),
        value_type,
        annotations,
        is_optional: optional,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> DescriptorRegistry {
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                attribute(
                    "name",
                    "person-name",
                    ValueType::String,
                    vec![Annotation::Key],
                    false,
                ),
                attribute("age", "person-age", ValueType::Long, vec![], true),
                attribute("active", "active", ValueType::Boolean, vec![], false),
                attribute(
                    "aliases",
                    "person-alias",
                    ValueType::String,
                    vec![Annotation::Card(0, None)],
                    true,
                ),
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
            owned_attributes: vec![attribute(
                "name",
                "company-name",
                ValueType::String,
                vec![Annotation::Unique],
                false,
            )],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "skill".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attribute(
                "label",
                "skill-label",
                ValueType::String,
                vec![],
                false,
            )],
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
                "id",
                "employment-id",
                ValueType::String,
                vec![Annotation::Key],
                false,
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
        .register_relation(RelationDescriptor {
            type_name: "collaboration".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles: vec![RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec!["person".into()],
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
}

fn binding(registry: &DescriptorRegistry, id: u16, name: &str) -> MatchBinding {
    let descriptor = registry.get(name).unwrap();
    MatchBinding {
        id: BindingId::new(id),
        descriptor: registry.descriptor_id(name).unwrap(),
        thing_kind: match descriptor {
            type_bridge_orm::descriptor::TypeDescriptorRef::Entity(_) => ThingKind::Entity,
            type_bridge_orm::descriptor::TypeDescriptorRef::Relation(_) => ThingKind::Relation,
        },
        match_mode: MatchMode::Exact,
    }
}

fn field(registry: &DescriptorRegistry, binding: u16, owner: &str, name: &str) -> BoundFieldId {
    let owner = registry.descriptor_id(owner).unwrap();
    BoundFieldId::new(
        BindingId::new(binding),
        registry.field_id(&owner, name).unwrap(),
    )
}

fn one(binding: u16) -> FetchSlot {
    FetchSlot::One {
        binding: BindingId::new(binding),
    }
}

fn fetch_request(
    _registry: &DescriptorRegistry,
    bindings: Vec<MatchBinding>,
    predicate: Option<MatchExpr>,
    slots: Vec<FetchSlot>,
) -> MatchRequest {
    MatchRequest::v1(
        MatchPlan {
            bindings,
            predicate,
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional { slots },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 10,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    )
}

fn code(result: std::result::Result<ValidatedMatchRequest, MatchError>) -> String {
    result.unwrap_err().code().as_str().to_owned()
}

#[test]
fn raw_ids_versions_and_structural_errors_have_stable_precedence() {
    let registry = registry();
    let mut request = fetch_request(
        &registry,
        vec![binding(&registry, 0, "person")],
        None,
        vec![one(0)],
    );
    request.version = MatchRequestVersion::from_raw(9);
    assert_eq!(
        code(request.validate(&registry)),
        "unsupported_request_version"
    );

    let mut request = fetch_request(
        &registry,
        vec![binding(&registry, 0, "person")],
        None,
        vec![one(0)],
    );
    request.plan.bindings[0].id = BindingId::new(2);
    assert_eq!(
        code(request.validate(&registry)),
        "non_canonical_binding_id"
    );

    let mut request = fetch_request(
        &registry,
        vec![binding(&registry, 0, "person")],
        None,
        vec![],
    );
    assert_eq!(code(request.clone().validate(&registry)), "empty_output");
    if let MatchOperation::FetchRows { window, .. } = &mut request.operation {
        window.limit = 0;
    }
    // Numeric/window checks precede output-shape checks deterministically.
    assert_eq!(code(request.validate(&registry)), "zero_window_limit");

    let mut request = fetch_request(
        &registry,
        vec![binding(&registry, 0, "person")],
        None,
        vec![one(0)],
    );
    if let MatchOperation::FetchRows { window, .. } = &mut request.operation {
        *window = Window {
            offset: u64::MAX,
            limit: 1,
        };
    }
    assert_eq!(code(request.validate(&registry)), "window_overflow");
}

#[test]
fn descriptor_field_operator_and_regex_validation_fail_closed() {
    let registry = registry();
    let person = binding(&registry, 0, "person");
    let mut wrong_kind = person.clone();
    wrong_kind.thing_kind = ThingKind::Relation;
    let request = fetch_request(&registry, vec![wrong_kind], None, vec![one(0)]);
    assert_eq!(
        code(request.validate(&registry)),
        "descriptor_kind_mismatch"
    );

    let mut unknown = person.clone();
    unknown.descriptor = DescriptorId::new("entity:missing");
    let request = fetch_request(&registry, vec![unknown], None, vec![one(0)]);
    assert_eq!(code(request.validate(&registry)), "unknown_descriptor");

    let request = fetch_request(
        &registry,
        vec![person.clone()],
        Some(MatchExpr::FieldValue {
            field: field(&registry, 0, "company", "name"),
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("Acme".into()),
        }),
        vec![one(0)],
    );
    assert_eq!(code(request.validate(&registry)), "cross_owner_field");

    let request = fetch_request(
        &registry,
        vec![person.clone()],
        Some(MatchExpr::FieldValue {
            field: field(&registry, 0, "person", "age"),
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("18".into()),
        }),
        vec![one(0)],
    );
    assert_eq!(code(request.validate(&registry)), "literal_type_mismatch");

    let request = fetch_request(
        &registry,
        vec![person.clone()],
        Some(MatchExpr::FieldValue {
            field: field(&registry, 0, "person", "active"),
            operator: ComparisonOp::GreaterThan,
            value: AttributeValue::Boolean(true),
        }),
        vec![one(0)],
    );
    assert_eq!(
        code(request.validate(&registry)),
        "invalid_operator_for_type"
    );

    let request = fetch_request(
        &registry,
        vec![person],
        Some(MatchExpr::FieldValue {
            field: field(&registry, 0, "person", "name"),
            operator: ComparisonOp::Regex,
            value: AttributeValue::String("(".into()),
        }),
        vec![one(0)],
    );
    assert_eq!(code(request.validate(&registry)), "invalid_regex");
}

#[test]
fn role_player_or_scope_and_positive_connectivity_are_enforced() {
    let registry = registry();
    let person = binding(&registry, 0, "person");
    let employment = binding(&registry, 1, "employment");
    let company = binding(&registry, 2, "company");
    let skill = binding(&registry, 2, "skill");
    let employee_role = registry
        .role_id(&registry.descriptor_id("employment").unwrap(), "employee")
        .unwrap();

    let foreign_employee_role = registry
        .role_id(
            &registry.descriptor_id("collaboration").unwrap(),
            "employee",
        )
        .unwrap();
    let request = fetch_request(
        &registry,
        vec![person.clone(), employment.clone()],
        Some(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: BindingId::new(1),
            role: foreign_employee_role,
            player: BindingId::new(0),
        }),
        vec![one(0), one(1)],
    );
    assert_eq!(code(request.validate(&registry)), "cross_owner_role");

    let bad_player = MatchExpr::RoleEdge {
        id: RoleEdgeId::new(0),
        relation: BindingId::new(1),
        role: employee_role.clone(),
        player: BindingId::new(2),
    };
    let request = fetch_request(
        &registry,
        vec![person.clone(), employment.clone(), skill],
        Some(bad_player),
        vec![one(0), one(1), one(2)],
    );
    assert_eq!(
        code(request.validate(&registry)),
        "incompatible_role_player"
    );

    let partial_or = MatchExpr::Or {
        expressions: vec![
            MatchExpr::FieldValue {
                field: field(&registry, 0, "person", "name"),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Alice".into()),
            },
            MatchExpr::FieldValue {
                field: field(&registry, 2, "company", "name"),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Acme".into()),
            },
        ],
    };
    let request = fetch_request(
        &registry,
        vec![person.clone(), employment.clone(), company.clone()],
        Some(partial_or),
        vec![one(0), one(1), one(2)],
    );
    assert_eq!(code(request.validate(&registry)), "partial_or_binding");

    let not_only = MatchExpr::Not {
        expression: Box::new(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: BindingId::new(1),
            role: employee_role,
            player: BindingId::new(0),
        }),
    };
    let request = fetch_request(
        &registry,
        vec![person, employment, company],
        Some(not_only),
        vec![one(0), one(1), one(2)],
    );
    assert_eq!(code(request.validate(&registry)), "disconnected_plan");
}

#[test]
fn shape_page_order_and_stable_key_rules_are_operation_specific() {
    let registry = registry();
    let person = binding(&registry, 0, "person");
    let company = binding(&registry, 1, "company");
    let mut request = fetch_request(&registry, vec![person.clone()], None, vec![one(0), one(0)]);
    assert_eq!(code(request.validate(&registry)), "duplicate_selection");

    request = fetch_request(
        &registry,
        vec![person.clone()],
        None,
        vec![FetchSlot::Collect {
            binding: BindingId::new(0),
            distinct: false,
            order: vec![],
        }],
    );
    assert_eq!(
        code(request.validate(&registry)),
        "collection_requires_page_root"
    );

    let mut disconnected = fetch_request(
        &registry,
        vec![person.clone(), company.clone()],
        None,
        vec![one(0), one(1)],
    );
    disconnected
        .plan
        .allowed_cross_joins
        .insert(BindingPair::new(BindingId::new(0), BindingId::new(1)));
    if let MatchOperation::FetchRows { order, .. } = &mut disconnected.operation {
        order.push(MatchOrder {
            field: field(&registry, 1, "company", "name"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        });
    }
    let validated = disconnected.validate(&registry).unwrap();
    // Public company order is followed by the missing person identity key.
    assert_eq!(validated.stable_order().terms().len(), 2);
    assert_eq!(
        validated.stable_order().terms()[1].origin(),
        StableOrderOrigin::UniqueTieBreaker
    );

    let mut non_scalar = fetch_request(&registry, vec![person.clone()], None, vec![one(0)]);
    if let MatchOperation::FetchRows { order, .. } = &mut non_scalar.operation {
        order.push(MatchOrder {
            field: field(&registry, 0, "person", "aliases"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        });
    }
    assert_eq!(
        code(non_scalar.validate(&registry)),
        "non_scalar_order_field"
    );

    let mut no_key = fetch_request(
        &registry,
        vec![binding(&registry, 0, "skill")],
        None,
        vec![one(0)],
    );
    assert_eq!(
        code(no_key.clone().validate(&registry)),
        "missing_stable_unique_key"
    );
    if let MatchOperation::FetchRows {
        cardinality,
        window,
        ..
    } = &mut no_key.operation
    {
        *cardinality = RowCardinality::ExactlyOne;
        *window = Window {
            offset: 0,
            limit: 1,
        };
    }
    no_key.validate(&registry).unwrap();

    let unstable_collection = MatchRequest::v1(
        MatchPlan {
            bindings: vec![person.clone(), binding(&registry, 1, "skill")],
            predicate: None,
            allowed_cross_joins: BTreeSet::from([BindingPair::new(
                BindingId::new(0),
                BindingId::new(1),
            )]),
        },
        MatchOperation::PageBy {
            root: BindingId::new(0),
            output: FetchShape::Positional {
                slots: vec![
                    one(0),
                    FetchSlot::Collect {
                        binding: BindingId::new(1),
                        distinct: false,
                        order: vec![],
                    },
                ],
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 10,
            },
            include_total: false,
        },
    );
    assert_eq!(
        code(unstable_collection.validate(&registry)),
        "missing_stable_unique_key"
    );

    let page = MatchRequest::v1(
        MatchPlan {
            bindings: vec![person, company],
            predicate: None,
            allowed_cross_joins: BTreeSet::from([BindingPair::new(
                BindingId::new(0),
                BindingId::new(1),
            )]),
        },
        MatchOperation::PageBy {
            root: BindingId::new(0),
            output: FetchShape::Positional {
                slots: vec![one(0), one(1)],
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 10,
            },
            include_total: false,
        },
    );
    assert_eq!(
        code(page.validate(&registry)),
        "singular_non_root_page_slot"
    );
}
