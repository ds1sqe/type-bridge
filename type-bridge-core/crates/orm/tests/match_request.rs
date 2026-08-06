//! Cross-module Phase 1 coverage for the canonical match-request contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use type_bridge_orm::_registry::DescriptorFingerprintRoot;
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attribute(
    field_name: &str,
    attr_name: &str,
    value_type: ValueType,
    annotations: Vec<Annotation>,
    is_optional: bool,
) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.to_owned(),
        attr_name: attr_name.to_owned(),
        value_type,
        annotations,
        is_optional,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn person_descriptor() -> EntityDescriptor {
    EntityDescriptor {
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
            attribute("age", "age", ValueType::Long, vec![], true),
        ],
        doc: None,
        meta: Default::default(),
    }
}

fn company_descriptor() -> EntityDescriptor {
    EntityDescriptor {
        type_name: "company".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![attribute(
            "name",
            "company-name",
            ValueType::String,
            vec![Annotation::Key],
            false,
        )],
        doc: None,
        meta: Default::default(),
    }
}

fn employment_descriptor() -> RelationDescriptor {
    RelationDescriptor {
        type_name: "employment".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            attribute(
                "identifier",
                "employment-id",
                ValueType::String,
                vec![Annotation::Key],
                false,
            ),
            attribute("since", "start-date", ValueType::Date, vec![], false),
        ],
        roles: vec![
            RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec!["person".into()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            },
            RoleDescriptor {
                role_name: "employer".into(),
                player_type_names: vec!["company".into()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            },
        ],
        doc: None,
        meta: Default::default(),
    }
}

fn registry(reverse: bool) -> DescriptorRegistry {
    let registry = DescriptorRegistry::new();
    if reverse {
        registry.register_relation(employment_descriptor()).unwrap();
        registry.register_entity(company_descriptor()).unwrap();
        registry.register_entity(person_descriptor()).unwrap();
    } else {
        registry.register_entity(person_descriptor()).unwrap();
        registry.register_entity(company_descriptor()).unwrap();
        registry.register_relation(employment_descriptor()).unwrap();
    }
    registry
}

#[derive(Clone)]
struct GraphIds {
    person: DescriptorId,
    employment: DescriptorId,
    company: DescriptorId,
    person_name: FieldId,
    person_age: FieldId,
    employment_id: FieldId,
    company_name: FieldId,
    employee: RoleId,
    employer: RoleId,
}

fn graph_ids(registry: &DescriptorRegistry) -> GraphIds {
    let person = registry.descriptor_id("person").unwrap();
    let employment = registry.descriptor_id("employment").unwrap();
    let company = registry.descriptor_id("company").unwrap();
    GraphIds {
        person_name: registry.field_id(&person, "name").unwrap(),
        person_age: registry.field_id(&person, "age").unwrap(),
        employment_id: registry.field_id(&employment, "identifier").unwrap(),
        company_name: registry.field_id(&company, "name").unwrap(),
        employee: registry.role_id(&employment, "employee").unwrap(),
        employer: registry.role_id(&employment, "employer").unwrap(),
        person,
        employment,
        company,
    }
}

fn field(binding: u16, field: &FieldId) -> BoundFieldId {
    BoundFieldId::new(BindingId::new(binding), field.clone())
}

fn order(
    binding: u16,
    field_id: &FieldId,
    direction: SortDirection,
    missing: MissingOrder,
) -> MatchOrder {
    MatchOrder {
        field: field(binding, field_id),
        direction,
        missing,
    }
}

fn comprehensive_predicate(ids: &GraphIds) -> MatchExpr {
    let person_name = field(0, &ids.person_name);
    let person_age = field(0, &ids.person_age);
    let company_name = field(2, &ids.company_name);
    MatchExpr::And {
        expressions: vec![
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Alice".into()),
            },
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::NotEqual,
                value: AttributeValue::String("Mallory".into()),
            },
            MatchExpr::FieldValue {
                field: person_age.clone(),
                operator: ComparisonOp::LessThan,
                value: AttributeValue::Long(70),
            },
            MatchExpr::FieldValue {
                field: person_age.clone(),
                operator: ComparisonOp::LessThanOrEqual,
                value: AttributeValue::Long(65),
            },
            MatchExpr::FieldValue {
                field: person_age.clone(),
                operator: ComparisonOp::GreaterThan,
                value: AttributeValue::Long(17),
            },
            MatchExpr::FieldValue {
                field: person_age,
                operator: ComparisonOp::GreaterThanOrEqual,
                value: AttributeValue::Long(18),
            },
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::Contains,
                value: AttributeValue::String("lic".into()),
            },
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::StartsWith,
                value: AttributeValue::String("Al".into()),
            },
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::EndsWith,
                value: AttributeValue::String("ice".into()),
            },
            MatchExpr::FieldValue {
                field: person_name.clone(),
                operator: ComparisonOp::Regex,
                value: AttributeValue::String("^A.*e$".into()),
            },
            MatchExpr::FieldComparison {
                left: person_name.clone(),
                operator: ComparisonOp::NotEqual,
                right: company_name.clone(),
            },
            MatchExpr::RoleEdge {
                id: RoleEdgeId::new(0),
                relation: BindingId::new(1),
                role: ids.employee.clone(),
                player: BindingId::new(0),
            },
            MatchExpr::RoleEdge {
                id: RoleEdgeId::new(1),
                relation: BindingId::new(1),
                role: ids.employer.clone(),
                player: BindingId::new(2),
            },
            MatchExpr::Or {
                expressions: vec![
                    MatchExpr::FieldValue {
                        field: company_name.clone(),
                        operator: ComparisonOp::Equal,
                        value: AttributeValue::String("Acme".into()),
                    },
                    MatchExpr::FieldValue {
                        field: company_name.clone(),
                        operator: ComparisonOp::Equal,
                        value: AttributeValue::String("Globex".into()),
                    },
                ],
            },
            MatchExpr::Not {
                expression: Box::new(MatchExpr::FieldValue {
                    field: company_name,
                    operator: ComparisonOp::Equal,
                    value: AttributeValue::String("Retired".into()),
                }),
            },
        ],
    }
}

fn plan(ids: &GraphIds) -> MatchPlan {
    MatchPlan {
        bindings: vec![
            MatchBinding {
                id: BindingId::new(0),
                descriptor: ids.person.clone(),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: BindingId::new(1),
                descriptor: ids.employment.clone(),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Subtypes,
            },
            MatchBinding {
                id: BindingId::new(2),
                descriptor: ids.company.clone(),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(comprehensive_predicate(ids)),
        allowed_cross_joins: BTreeSet::from([
            BindingPair::new(BindingId::new(0), BindingId::new(1)),
            BindingPair::new(BindingId::new(0), BindingId::new(2)),
        ]),
    }
}

fn one(binding: u16) -> FetchSlot {
    FetchSlot::One {
        binding: BindingId::new(binding),
    }
}

fn fixture_requests(registry: &DescriptorRegistry) -> Vec<(&'static str, MatchRequest)> {
    let ids = graph_ids(registry);
    let plan = plan(&ids);
    let bare_plan = MatchPlan {
        predicate: None,
        ..plan.clone()
    };
    let positional = FetchShape::Positional {
        slots: vec![one(0), one(1), one(2)],
    };
    vec![
        (
            "fetch-rows.json",
            MatchRequest::v1(
                plan.clone(),
                MatchOperation::FetchRows {
                    output: positional.clone(),
                    order: vec![
                        order(
                            0,
                            &ids.person_name,
                            SortDirection::Ascending,
                            MissingOrder::Reject,
                        ),
                        order(
                            2,
                            &ids.company_name,
                            SortDirection::Descending,
                            MissingOrder::Last,
                        ),
                    ],
                    window: Window {
                        offset: 0,
                        limit: 50,
                    },
                    cardinality: RowCardinality::BoundedMany,
                },
            ),
        ),
        (
            "exactly-one.json",
            MatchRequest::v1(
                bare_plan.clone(),
                MatchOperation::FetchRows {
                    output: positional,
                    order: vec![],
                    window: Window {
                        offset: 0,
                        limit: 1,
                    },
                    cardinality: RowCardinality::ExactlyOne,
                },
            ),
        ),
        (
            "page-by.json",
            MatchRequest::v1(
                plan.clone(),
                MatchOperation::PageBy {
                    root: BindingId::new(0),
                    output: FetchShape::Named {
                        slots: vec![
                            NamedFetchSlot {
                                name: "person".into(),
                                slot: one(0),
                            },
                            NamedFetchSlot {
                                name: "employments".into(),
                                slot: FetchSlot::Collect {
                                    binding: BindingId::new(1),
                                    distinct: false,
                                    order: vec![order(
                                        1,
                                        &ids.employment_id,
                                        SortDirection::Descending,
                                        MissingOrder::Reject,
                                    )],
                                },
                            },
                            NamedFetchSlot {
                                name: "companies".into(),
                                slot: FetchSlot::Collect {
                                    binding: BindingId::new(2),
                                    distinct: true,
                                    order: vec![order(
                                        2,
                                        &ids.company_name,
                                        SortDirection::Ascending,
                                        MissingOrder::Last,
                                    )],
                                },
                            },
                        ],
                    },
                    order: vec![order(
                        0,
                        &ids.person_name,
                        SortDirection::Ascending,
                        MissingOrder::First,
                    )],
                    window: Window {
                        offset: 10,
                        limit: 20,
                    },
                    include_total: true,
                },
            ),
        ),
        (
            "count-by.json",
            MatchRequest::v1(
                bare_plan.clone(),
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            ),
        ),
        (
            "reduce-by.json",
            MatchRequest::v1(
                bare_plan.clone(),
                MatchOperation::ReduceBy {
                    root: BindingId::new(0),
                    group: Some(BindingId::new(2)),
                    reducers: vec![
                        ReduceTerm {
                            reduction: Reduction::Count,
                            input: None,
                        },
                        ReduceTerm {
                            reduction: Reduction::Sum,
                            input: Some(field(0, &ids.person_age)),
                        },
                    ],
                },
            ),
        ),
        (
            "reduce-by-field.json",
            MatchRequest::v1(
                bare_plan.clone(),
                MatchOperation::ReduceByField {
                    root: BindingId::new(0),
                    group: field(0, &ids.person_age),
                    reducers: vec![ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    }],
                },
            ),
        ),
        (
            "reduce-by-fields.json",
            MatchRequest::v1(
                bare_plan.clone(),
                MatchOperation::ReduceByFields {
                    root: BindingId::new(0),
                    groups: vec![field(0, &ids.person_name), field(0, &ids.person_age)],
                    reducers: vec![ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    }],
                },
            ),
        ),
        (
            "exists-by.json",
            MatchRequest::v1(
                bare_plan,
                MatchOperation::ExistsBy {
                    root: BindingId::new(0),
                },
            ),
        ),
    ]
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/match_request")
        .join(name)
}

fn fixture_payload(name: &str) -> String {
    fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|error| panic!("failed to read {name}: {error}"))
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

fn error_code(error: &MatchError) -> &str {
    error.code().as_str()
}

#[test]
fn checked_in_diagnostics_cover_the_complete_public_request_algebra() {
    let registry = registry(false);
    let mut mismatches = Vec::new();

    for (name, request) in fixture_requests(&registry) {
        let diagnostic = UnvalidatedMatchRequest::from_request(request.clone()).unwrap();
        let actual = String::from_utf8(diagnostic.to_canonical_bytes().unwrap()).unwrap();
        let expected = fixture_payload(name);
        if actual != expected {
            mismatches.push(format!("{name}\n{actual}"));
            continue;
        }

        let parsed = UnvalidatedMatchRequest::from_canonical_bytes(expected.as_bytes()).unwrap();
        assert_eq!(parsed.request(), &request);
        let expected_capabilities = CapabilitySet::for_request(&request);
        assert_eq!(parsed.required_capabilities(), &expected_capabilities);
        let roots = request
            .plan
            .bindings
            .iter()
            .map(|binding| {
                DescriptorFingerprintRoot::new(
                    binding.descriptor.clone(),
                    binding.match_mode == MatchMode::Subtypes,
                )
            })
            .collect::<Vec<_>>();
        let expected_fingerprint = registry.request_relevant_fingerprint(&roots).unwrap();
        let validated = parsed
            .validate(&registry)
            .unwrap_or_else(|error| panic!("{name} did not validate: {error}"));
        assert_eq!(validated.request(), &request);
        assert_eq!(validated.capabilities(), &expected_capabilities);
        assert_eq!(validated.schema_fingerprint(), &expected_fingerprint);
        validated.recheck_schema(&registry).unwrap();
    }

    assert!(
        mismatches.is_empty(),
        "diagnostic fixture mismatches:\n{}",
        mismatches.join("\n---\n")
    );
}

#[test]
fn shuffled_registry_insertion_preserves_ids_fingerprints_and_diagnostics() {
    let forward = registry(false);
    let reverse = registry(true);

    assert_eq!(
        forward.identity_snapshot().unwrap(),
        reverse.identity_snapshot().unwrap()
    );
    assert_eq!(
        forward.schema_fingerprint().unwrap(),
        reverse.schema_fingerprint().unwrap()
    );

    let forward_fingerprint = forward
        .request_relevant_fingerprint(&[
            DescriptorFingerprintRoot::new(forward.descriptor_id("person").unwrap(), false),
            DescriptorFingerprintRoot::new(forward.descriptor_id("employment").unwrap(), true),
            DescriptorFingerprintRoot::new(forward.descriptor_id("company").unwrap(), false),
        ])
        .unwrap();
    let reverse_fingerprint = reverse
        .request_relevant_fingerprint(&[
            DescriptorFingerprintRoot::new(reverse.descriptor_id("person").unwrap(), false),
            DescriptorFingerprintRoot::new(reverse.descriptor_id("employment").unwrap(), true),
            DescriptorFingerprintRoot::new(reverse.descriptor_id("company").unwrap(), false),
        ])
        .unwrap();
    assert_eq!(forward_fingerprint, reverse_fingerprint);

    let forward_bytes = fixture_requests(&forward)
        .into_iter()
        .map(|(name, request)| {
            (
                name,
                UnvalidatedMatchRequest::from_request(request)
                    .unwrap()
                    .to_canonical_bytes()
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let reverse_bytes = fixture_requests(&reverse)
        .into_iter()
        .map(|(name, request)| {
            (
                name,
                UnvalidatedMatchRequest::from_request(request)
                    .unwrap()
                    .to_canonical_bytes()
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(forward_bytes, reverse_bytes);
}

#[test]
fn parsed_diagnostic_is_only_revalidation_preparation() {
    let registry = registry(false);
    let (_, request) = fixture_requests(&registry).remove(0);
    let bytes = UnvalidatedMatchRequest::from_request(request.clone())
        .unwrap()
        .to_canonical_bytes()
        .unwrap();
    let parsed = UnvalidatedMatchRequest::from_canonical_bytes(&bytes).unwrap();
    let (unvalidated, capabilities) = parsed.into_parts();

    assert_eq!(unvalidated, request);
    assert_eq!(capabilities, CapabilitySet::for_request(&unvalidated));
    let roots = unvalidated
        .plan
        .bindings
        .iter()
        .map(|binding| {
            DescriptorFingerprintRoot::new(
                binding.descriptor.clone(),
                binding.match_mode == MatchMode::Subtypes,
            )
        })
        .collect::<Vec<_>>();
    let fingerprint = registry.request_relevant_fingerprint(&roots).unwrap();
    assert!(fingerprint.as_str().starts_with("schema-sha256-v1:"));
    assert_eq!(fingerprint.as_str().len(), "schema-sha256-v1:".len() + 64);

    let validated = UnvalidatedMatchRequest::from_canonical_bytes(&bytes)
        .unwrap()
        .validate(&registry)
        .unwrap();
    assert_eq!(validated.request(), &request);
    assert_eq!(validated.schema_fingerprint(), &fingerprint);
    assert_eq!(validated.capabilities(), &capabilities);
    validated.recheck_schema(&registry).unwrap();
}

#[test]
fn diagnostic_errors_and_versions_fail_closed_through_public_api() {
    let registry = registry(false);
    let (_, request) = fixture_requests(&registry).remove(3);
    let canonical = UnvalidatedMatchRequest::from_request(request)
        .unwrap()
        .to_canonical_bytes()
        .unwrap();
    let canonical = String::from_utf8(canonical).unwrap();

    let cases = [
        (
            canonical.replacen(r#""diagnostic_version":1"#, r#""diagnostic_version":77"#, 1),
            "unsupported_diagnostic_version",
        ),
        (
            canonical.replacen(r#""request":{"version":1"#, r#""request":{"version":77"#, 1),
            "unsupported_request_version",
        ),
        (format!("{canonical}\n"), "non_canonical_diagnostic"),
        (
            canonical.replacen("DISTINCT_ROOT_COUNT", "DISTINCT_ROOT_EXISTS", 1),
            "capability_set_mismatch",
        ),
    ];
    for (bytes, expected_code) in cases {
        let error = UnvalidatedMatchRequest::from_canonical_bytes(bytes.as_bytes()).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(error_code(&error), expected_code);
        assert!(!error.path().is_empty());
    }

    let malformed = UnvalidatedMatchRequest::from_canonical_bytes(b"{").unwrap_err();
    assert_eq!(error_code(&malformed), "malformed_diagnostic");
    let oversized = vec![b' '; MAX_DIAGNOSTIC_BYTES + 1];
    let too_large = UnvalidatedMatchRequest::from_canonical_bytes(&oversized).unwrap_err();
    assert_eq!(too_large.category(), MatchErrorCategory::ResourceLimit);
    assert_eq!(error_code(&too_large), "diagnostic_too_large");
}

fn request_with_distinct_output_slots(
    registry: &DescriptorRegistry,
    slot_count: usize,
) -> MatchRequest {
    let descriptor = registry.descriptor_id("person").unwrap();
    let binding_id = |index: usize| BindingId::new(u16::try_from(index).unwrap());
    MatchRequest::v1(
        MatchPlan {
            bindings: (0..slot_count)
                .map(|index| MatchBinding {
                    id: binding_id(index),
                    descriptor: descriptor.clone(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                })
                .collect(),
            predicate: None,
            allowed_cross_joins: (1..slot_count)
                .map(|index| BindingPair::new(binding_id(0), binding_id(index)))
                .collect(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: (0..slot_count)
                    .map(|index| FetchSlot::One {
                        binding: binding_id(index),
                    })
                    .collect(),
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::ExactlyOne,
        },
    )
}

#[test]
fn structural_limits_pin_boundaries_and_validator_enforcement() {
    let limits = StructuralLimits::CANONICAL;
    assert!(limits.allows_selected_slots(MAX_SELECTED_SLOTS));
    assert!(!limits.allows_selected_slots(MAX_SELECTED_SLOTS + 1));
    assert!(limits.allows_bindings(MAX_BINDINGS));
    assert!(!limits.allows_bindings(MAX_BINDINGS + 1));
    assert!(limits.allows_predicate_nodes(MAX_PREDICATE_NODES));
    assert!(!limits.allows_predicate_nodes(MAX_PREDICATE_NODES + 1));
    assert!(limits.allows_predicate_depth(MAX_PREDICATE_DEPTH));
    assert!(!limits.allows_predicate_depth(MAX_PREDICATE_DEPTH + 1));
    assert!(limits.allows_diagnostic_bytes(MAX_DIAGNOSTIC_BYTES));
    assert!(!limits.allows_diagnostic_bytes(MAX_DIAGNOSTIC_BYTES + 1));

    let registry = registry(false);
    let at_limit = request_with_distinct_output_slots(&registry, MAX_SELECTED_SLOTS);
    let validated = UnvalidatedMatchRequest::from_request(at_limit)
        .unwrap()
        .validate(&registry)
        .unwrap();
    validated.recheck_schema(&registry).unwrap();

    let over_limit = request_with_distinct_output_slots(&registry, MAX_SELECTED_SLOTS + 1);
    let diagnostic = UnvalidatedMatchRequest::from_request(over_limit.clone()).unwrap();
    assert_eq!(diagnostic.request(), &over_limit);
    let error = diagnostic.validate(&registry).unwrap_err();
    assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
    assert_eq!(error_code(&error), "selection_cap_exceeded");
    assert_eq!(error.path().segments(), &[MatchErrorPathSegment::Output]);
}
