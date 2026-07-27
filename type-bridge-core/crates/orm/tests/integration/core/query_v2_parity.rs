//! Result parity between the V1 executor and adapted V2 execution.
//!
//! This is the delegation gate: every expressible corpus request runs
//! through the released V1 executor and through adapt -> validate -> lower
//! -> execute on the same live data, and the results must agree exactly —
//! row identities in order, counts, and existence. The V1 side deliberately
//! uses the retained direct executor rather than the production compatibility
//! route so this test cannot compare adapted V2 execution with itself.

use std::collections::BTreeSet;

use crate::common::dynamic_crud::{
    DynamicCrudSchema, company_attrs, relation_attrs, role_players, setup_dynamic_database,
};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_orm::integration_test_support::adapt_match_request_for_live_test;
use type_bridge_orm::*;

struct ParityFixture {
    company_descriptor_id: DescriptorId,
    employee_descriptor_id: DescriptorId,
    employment_descriptor_id: DescriptorId,
    person_descriptor_id: DescriptorId,
    registry: DescriptorRegistry,
}

fn name_field(fixture: &ParityFixture) -> BoundFieldId {
    bound_field(BindingId::new(0), &fixture.person_descriptor_id, "name")
}

fn age_field(fixture: &ParityFixture) -> BoundFieldId {
    bound_field(BindingId::new(0), &fixture.person_descriptor_id, "age")
}

fn person_field(fixture: &ParityFixture, name: &str) -> BoundFieldId {
    bound_field(BindingId::new(0), &fixture.person_descriptor_id, name)
}

fn bound_field(binding: BindingId, descriptor: &DescriptorId, name: &str) -> BoundFieldId {
    BoundFieldId::new(binding, FieldId::new(descriptor.clone(), name))
}

fn person_plan(fixture: &ParityFixture, predicate: Option<MatchExpr>) -> MatchPlan {
    MatchPlan {
        bindings: vec![MatchBinding {
            id: BindingId::new(0),
            descriptor: fixture.person_descriptor_id.clone(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Subtypes,
        }],
        predicate,
        allowed_cross_joins: BTreeSet::new(),
    }
}

fn rows_operation(
    fixture: &ParityFixture,
    direction: SortDirection,
    offset: u64,
    limit: u64,
) -> MatchOperation {
    MatchOperation::FetchRows {
        output: FetchShape::Positional {
            slots: vec![FetchSlot::One {
                binding: BindingId::new(0),
            }],
        },
        order: vec![MatchOrder {
            field: name_field(fixture),
            direction,
            missing: MissingOrder::Reject,
        }],
        window: Window { offset, limit },
        cardinality: RowCardinality::BoundedMany,
    }
}

async fn assert_result_parity(
    db: &Database,
    fixture: &ParityFixture,
    request: MatchRequest,
    case: &str,
) -> MatchResult {
    let validated = validate_match_request(&fixture.registry, request)
        .expect("corpus request passes V1 validation");
    adapt_match_request_for_live_test(&validated, &fixture.registry, StructuralLimits::CANONICAL)
        .unwrap_or_else(|error| panic!("{case} must take the adapted V2 path: {error}"));
    let legacy = db
        .execute_match_v1_legacy_for_live_test(&fixture.registry, &validated)
        .await
        .unwrap_or_else(|error| panic!("direct V1 execution failed for {case}: {error:?}"));
    let adapted = db
        .execute_match(&fixture.registry, &validated)
        .await
        .unwrap_or_else(|error| {
            panic!("production adapted V2 execution failed for {case}: {error:?}")
        });
    assert_eq!(
        legacy.result(),
        adapted.result(),
        "direct V1 and production adapted V2 differ for {case}",
    );
    legacy.result().clone()
}

async fn assert_error_parity(
    db: &Database,
    fixture: &ParityFixture,
    request: MatchRequest,
    case: &str,
) -> MatchError {
    let validated = validate_match_request(&fixture.registry, request)
        .expect("error corpus request passes V1 validation");
    adapt_match_request_for_live_test(&validated, &fixture.registry, StructuralLimits::CANONICAL)
        .unwrap_or_else(|error| panic!("{case} must take the adapted V2 path: {error}"));
    let legacy = db
        .execute_match_v1_legacy_for_live_test(&fixture.registry, &validated)
        .await
        .expect_err("direct V1 execution must reject this cardinality");
    let adapted = db
        .execute_match(&fixture.registry, &validated)
        .await
        .expect_err("production adapted V2 execution must reject this cardinality");
    let (OrmError::Match(legacy), OrmError::Match(adapted)) = (legacy, adapted) else {
        panic!("{case} must return the released Match error variant");
    };
    assert_eq!(
        legacy, adapted,
        "direct V1 and production adapted V2 diagnostics differ for {case}",
    );
    legacy
}

fn row_iids(result: &MatchResult) -> Vec<Vec<String>> {
    let MatchResult::Rows { rows } = result else {
        panic!("expected rows: {result:?}");
    };
    rows.iter()
        .map(|row| {
            row.slots()
                .iter()
                .flat_map(|slot| match slot {
                    SlotValue::One(thing) => vec![thing.concept_id().as_str().to_owned()],
                    SlotValue::Many(things) => things
                        .iter()
                        .map(|thing| thing.concept_id().as_str().to_owned())
                        .collect(),
                })
                .collect()
        })
        .collect()
}

fn row_count(result: &MatchResult) -> usize {
    let MatchResult::Rows { rows } = result else {
        panic!("expected rows: {result:?}");
    };
    rows.len()
}

fn thing_name(thing: &HydratedThing) -> Option<&str> {
    thing
        .attributes()
        .iter()
        .find(|attribute| attribute.field().name == "name")
        .and_then(|attribute| attribute.values().first())
        .and_then(|value| match value {
            AttributeValue::String(value) => Some(value.as_str()),
            _ => None,
        })
}

fn relation_plan(fixture: &ParityFixture) -> MatchPlan {
    let person = BindingId::new(0);
    let employment = BindingId::new(1);
    let company = BindingId::new(2);
    MatchPlan {
        bindings: vec![
            MatchBinding {
                id: person,
                descriptor: fixture.person_descriptor_id.clone(),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Subtypes,
            },
            MatchBinding {
                id: employment,
                descriptor: fixture.employment_descriptor_id.clone(),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Subtypes,
            },
            MatchBinding {
                id: company,
                descriptor: fixture.company_descriptor_id.clone(),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::And {
            expressions: vec![
                MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(0),
                    relation: employment,
                    role: fixture
                        .registry
                        .role_id(&fixture.employment_descriptor_id, "employee")
                        .expect("employee role"),
                    player: person,
                },
                MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(1),
                    relation: employment,
                    role: fixture
                        .registry
                        .role_id(&fixture.employment_descriptor_id, "employer")
                        .expect("employer role"),
                    player: company,
                },
            ],
        }),
        allowed_cross_joins: BTreeSet::new(),
    }
}

fn relation_order(fixture: &ParityFixture) -> Vec<MatchOrder> {
    vec![
        MatchOrder {
            field: name_field(fixture),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        },
        MatchOrder {
            field: bound_field(BindingId::new(2), &fixture.company_descriptor_id, "name"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        },
    ]
}

fn named_link_output() -> FetchShape {
    FetchShape::Named {
        slots: vec![
            NamedFetchSlot {
                name: "person".into(),
                slot: FetchSlot::One {
                    binding: BindingId::new(0),
                },
            },
            NamedFetchSlot {
                name: "company".into(),
                slot: FetchSlot::One {
                    binding: BindingId::new(2),
                },
            },
        ],
    }
}

fn named_relation_output() -> FetchShape {
    FetchShape::Named {
        slots: vec![
            NamedFetchSlot {
                name: "person".into(),
                slot: FetchSlot::One {
                    binding: BindingId::new(0),
                },
            },
            NamedFetchSlot {
                name: "employment".into(),
                slot: FetchSlot::One {
                    binding: BindingId::new(1),
                },
            },
            NamedFetchSlot {
                name: "company".into(),
                slot: FetchSlot::One {
                    binding: BindingId::new(2),
                },
            },
        ],
    }
}

fn exact_one_request(fixture: &ParityFixture, predicate: Option<MatchExpr>) -> MatchRequest {
    MatchRequest::v1(
        person_plan(fixture, predicate),
        MatchOperation::FetchRows {
            output: FetchShape::Named {
                slots: vec![NamedFetchSlot {
                    name: "person".into(),
                    slot: FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                }],
            },
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::ExactlyOne,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn parity_person_attrs(
    schema: &DynamicCrudSchema,
    name: &str,
    age: i64,
    score: f64,
    active: bool,
    birthday: &str,
    login_at: &str,
    seen_at: &str,
    balance: &str,
    session_length: &str,
) -> DynamicAttributeMap {
    vec![
        (
            schema.name_attr.clone(),
            AttributeValue::String(name.into()),
        ),
        (schema.age_attr.clone(), AttributeValue::Long(age)),
        (schema.score_attr.clone(), AttributeValue::Double(score)),
        (schema.active_attr.clone(), AttributeValue::Boolean(active)),
        (
            schema.birthday_attr.clone(),
            AttributeValue::Date(birthday.into()),
        ),
        (
            schema.login_at_attr.clone(),
            AttributeValue::DateTime(login_at.into()),
        ),
        (
            schema.seen_at_attr.clone(),
            AttributeValue::DateTimeTZ(seen_at.into()),
        ),
        (
            schema.balance_attr.clone(),
            AttributeValue::Decimal(balance.into()),
        ),
        (
            schema.session_length_attr.clone(),
            AttributeValue::Duration(session_length.into()),
        ),
    ]
}

#[tokio::test]
async fn v1_and_adapted_v2_execution_agree_on_the_expressible_corpus() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("query-v2-parity").await;
    let employee_type = format!("{}-employee", schema.person_type);
    db.execute_raw(
        &format!("define entity {employee_type} sub {};", schema.person_type),
        TxType::Schema,
    )
    .await
    .expect("parity employee subtype");

    let person_descriptor = schema.person_descriptor();
    let mut employee_descriptor = person_descriptor.as_ref().clone();
    employee_descriptor.type_name = employee_type.clone();
    employee_descriptor.parent_type = Some(schema.person_type.clone());
    let company_descriptor = schema.company_descriptor();
    let mut employment_descriptor = schema.employment_descriptor().as_ref().clone();
    for role in &mut employment_descriptor.roles {
        // The dynamic schema has no relates-side @card annotation.
        role.cardinality = None;
    }

    let person_manager = DynamicEntityManager::new(&db, person_descriptor.clone());
    let employee_manager =
        DynamicEntityManager::new(&db, std::sync::Arc::new(employee_descriptor.clone()));
    let company_manager = DynamicEntityManager::new(&db, company_descriptor.clone());
    let employment_manager =
        DynamicRelationManager::new(&db, std::sync::Arc::new(employment_descriptor.clone()));

    let ada_iid = person_manager
        .insert(&parity_person_attrs(
            &schema,
            "Ada",
            30,
            91.25,
            true,
            "1990-01-02",
            "2026-05-27T10:30:00",
            "2026-05-27T10:30:00+00:00",
            "1234.56",
            "PT2H30M",
        ))
        .await
        .expect("Ada parity row");
    let grace_iid = person_manager
        .insert(&parity_person_attrs(
            &schema,
            "Grace",
            40,
            88.5,
            false,
            "1985-06-15",
            "2026-05-28T11:00:00",
            "2026-05-28T11:00:00+01:00",
            "999.00",
            "PT1H",
        ))
        .await
        .expect("Grace parity row");
    person_manager
        .insert(&parity_person_attrs(
            &schema,
            "Alan",
            25,
            95.0,
            true,
            "2000-12-31",
            "2026-05-26T09:15:00",
            "2026-05-26T09:15:00-04:00",
            "42.10",
            "P1DT30M",
        ))
        .await
        .expect("Alan parity row");
    let eve_iid = employee_manager
        .insert(&parity_person_attrs(
            &schema,
            "Eve",
            35,
            90.0,
            true,
            "1995-03-20",
            "2026-05-29T12:45:00",
            "2026-05-29T12:45:00Z",
            "500.25",
            "PT45M",
        ))
        .await
        .expect("Eve employee parity row");
    let acme_iid = company_manager
        .insert(&company_attrs("Acme"))
        .await
        .expect("Acme parity row");
    let beta_iid = company_manager
        .insert(&company_attrs("Beta"))
        .await
        .expect("Beta parity row");
    for (person, company, since) in [
        (ada_iid.clone(), acme_iid.clone(), "2020-01-01"),
        (ada_iid.clone(), beta_iid.clone(), "2021-02-02"),
        (ada_iid, acme_iid.clone(), "2023-05-05"),
        (grace_iid, acme_iid, "2019-03-03"),
        (eve_iid, beta_iid, "2022-04-04"),
    ] {
        employment_manager
            .insert(
                &relation_attrs(since),
                &role_players(&schema, person, company),
            )
            .await
            .expect("employment parity row");
    }

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(person_descriptor.as_ref().clone())
        .expect("register person descriptor");
    registry
        .register_entity(employee_descriptor)
        .expect("register employee descriptor");
    registry
        .register_entity(company_descriptor.as_ref().clone())
        .expect("register company descriptor");
    registry
        .register_relation(employment_descriptor)
        .expect("register employment descriptor");
    let person_descriptor_id = registry
        .descriptor_id(&schema.person_type)
        .expect("person descriptor id");
    let employee_descriptor_id = registry
        .descriptor_id(&employee_type)
        .expect("employee descriptor id");
    let company_descriptor_id = registry
        .descriptor_id(&schema.company_type)
        .expect("company descriptor id");
    let employment_descriptor_id = registry
        .descriptor_id(&schema.employment_type)
        .expect("employment descriptor id");
    let fixture = ParityFixture {
        company_descriptor_id,
        employee_descriptor_id,
        employment_descriptor_id,
        person_descriptor_id,
        registry,
    };

    // Ordered, filtered, windowed rows agree in identity and order.
    let filtered = |direction, offset, limit| {
        MatchRequest::v1(
            person_plan(
                &fixture,
                Some(MatchExpr::FieldValue {
                    field: name_field(&fixture),
                    operator: ComparisonOp::GreaterThanOrEqual,
                    value: AttributeValue::String("Al".into()),
                }),
            ),
            rows_operation(&fixture, direction, offset, limit),
        )
    };
    for (case, request) in [
        (
            "ascending filtered rows",
            filtered(SortDirection::Ascending, 0, 10),
        ),
        (
            "descending filtered rows",
            filtered(SortDirection::Descending, 0, 10),
        ),
        (
            "windowed filtered rows",
            filtered(SortDirection::Ascending, 1, 1),
        ),
    ] {
        let result = assert_result_parity(&db, &fixture, request, case).await;
        assert!(!row_iids(&result).is_empty(), "{case} must be non-trivial");
    }

    // Every released scalar domain crosses the adapter in a live predicate
    // and returns complete hydration, including the driver's released
    // temporal, duration, and decimal lexical spellings.
    let scalar_cases = vec![
        (
            "string equal",
            "name",
            ComparisonOp::Equal,
            AttributeValue::String("Ada".into()),
            1,
        ),
        (
            "string not equal",
            "name",
            ComparisonOp::NotEqual,
            AttributeValue::String("Ada".into()),
            3,
        ),
        (
            "string contains",
            "name",
            ComparisonOp::Contains,
            AttributeValue::String("a".into()),
            3,
        ),
        (
            "string starts with",
            "name",
            ComparisonOp::StartsWith,
            AttributeValue::String("A".into()),
            2,
        ),
        (
            "string ends with",
            "name",
            ComparisonOp::EndsWith,
            AttributeValue::String("e".into()),
            2,
        ),
        (
            "string regex",
            "name",
            ComparisonOp::Regex,
            AttributeValue::String("^A.*a$".into()),
            1,
        ),
        (
            "long less than",
            "age",
            ComparisonOp::LessThan,
            AttributeValue::Long(30),
            1,
        ),
        (
            "long less than or equal",
            "age",
            ComparisonOp::LessThanOrEqual,
            AttributeValue::Long(30),
            2,
        ),
        (
            "long greater than",
            "age",
            ComparisonOp::GreaterThan,
            AttributeValue::Long(30),
            2,
        ),
        (
            "long greater than or equal",
            "age",
            ComparisonOp::GreaterThanOrEqual,
            AttributeValue::Long(30),
            3,
        ),
        (
            "double equality",
            "score",
            ComparisonOp::Equal,
            AttributeValue::Double(90.0),
            1,
        ),
        (
            "boolean equality",
            "active",
            ComparisonOp::Equal,
            AttributeValue::Boolean(false),
            1,
        ),
        (
            "date comparison",
            "birthday",
            ComparisonOp::LessThan,
            AttributeValue::Date("1990-01-01".into()),
            1,
        ),
        (
            "datetime equality",
            "login_at",
            ComparisonOp::Equal,
            AttributeValue::DateTime("2026-05-27T10:30:00".into()),
            1,
        ),
        (
            "datetime-tz equality",
            "seen_at",
            ComparisonOp::Equal,
            AttributeValue::DateTimeTZ("2026-05-28T11:00:00+01:00".into()),
            1,
        ),
        (
            "decimal released suffix and scale",
            "balance",
            ComparisonOp::Equal,
            AttributeValue::Decimal("999.000dec".into()),
            1,
        ),
        (
            "duration equality",
            "session_length",
            ComparisonOp::Equal,
            AttributeValue::Duration("PT1H".into()),
            1,
        ),
    ];
    for (case, field, operator, value, expected) in scalar_cases {
        let request = MatchRequest::v1(
            person_plan(
                &fixture,
                Some(MatchExpr::FieldValue {
                    field: person_field(&fixture, field),
                    operator,
                    value,
                }),
            ),
            rows_operation(&fixture, SortDirection::Ascending, 0, 10),
        );
        let result = assert_result_parity(&db, &fixture, request, case).await;
        assert_eq!(row_count(&result), expected, "{case} fixture cardinality");
    }

    // Nested And/Or/Not trees preserve both semantics and source structure.
    let negated = MatchRequest::v1(
        person_plan(
            &fixture,
            Some(MatchExpr::Or {
                expressions: vec![
                    MatchExpr::And {
                        expressions: vec![
                            MatchExpr::FieldValue {
                                field: age_field(&fixture),
                                operator: ComparisonOp::GreaterThan,
                                value: AttributeValue::Long(24),
                            },
                            MatchExpr::Not {
                                expression: Box::new(MatchExpr::FieldValue {
                                    field: name_field(&fixture),
                                    operator: ComparisonOp::Equal,
                                    value: AttributeValue::String("Ada".into()),
                                }),
                            },
                        ],
                    },
                    MatchExpr::FieldValue {
                        field: name_field(&fixture),
                        operator: ComparisonOp::Equal,
                        value: AttributeValue::String("Ada".into()),
                    },
                ],
            }),
        ),
        rows_operation(&fixture, SortDirection::Ascending, 0, 10),
    );
    let negated = assert_result_parity(&db, &fixture, negated, "nested numeric negation").await;
    assert_eq!(
        row_iids(&negated).len(),
        4,
        "nested tree covers every fixture person exactly once",
    );

    // Exact and subtype-inclusive targets retain concrete descriptor evidence.
    let subtype_rows = assert_result_parity(
        &db,
        &fixture,
        MatchRequest::v1(
            person_plan(&fixture, None),
            rows_operation(&fixture, SortDirection::Ascending, 0, 10),
        ),
        "subtype-inclusive hydrated rows",
    )
    .await;
    let MatchResult::Rows { rows } = &subtype_rows else {
        panic!("expected subtype rows");
    };
    let eve = rows
        .iter()
        .filter_map(|row| match &row.slots()[0] {
            SlotValue::One(thing) if thing_name(thing) == Some("Eve") => Some(thing),
            _ => None,
        })
        .next()
        .expect("subtype row");
    assert_eq!(eve.declared_descriptor(), &fixture.person_descriptor_id);
    assert_eq!(eve.concrete_descriptor(), &fixture.employee_descriptor_id);

    let mut exact_plan = person_plan(&fixture, None);
    exact_plan.bindings[0].match_mode = MatchMode::Exact;
    let exact_rows = assert_result_parity(
        &db,
        &fixture,
        MatchRequest::v1(
            exact_plan,
            rows_operation(&fixture, SortDirection::Ascending, 0, 10),
        ),
        "exact base target excludes subtype",
    )
    .await;
    assert_eq!(row_count(&exact_rows), 3);

    // Explicit cross joins and same-domain field comparisons are independent
    // compatibility vocabulary, so exercise both shapes.
    let person = BindingId::new(0);
    let company = BindingId::new(1);
    let cross_bindings = vec![
        MatchBinding {
            id: person,
            descriptor: fixture.person_descriptor_id.clone(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        },
        MatchBinding {
            id: company,
            descriptor: fixture.company_descriptor_id.clone(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        },
    ];
    let cross_output = FetchShape::Named {
        slots: vec![
            NamedFetchSlot {
                name: "person".into(),
                slot: FetchSlot::One { binding: person },
            },
            NamedFetchSlot {
                name: "company".into(),
                slot: FetchSlot::One { binding: company },
            },
        ],
    };
    let cross_order = vec![
        MatchOrder {
            field: name_field(&fixture),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        },
        MatchOrder {
            field: bound_field(company, &fixture.company_descriptor_id, "name"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        },
    ];
    let cross_join = MatchRequest::v1(
        MatchPlan {
            bindings: cross_bindings.clone(),
            predicate: None,
            allowed_cross_joins: BTreeSet::from([BindingPair::new(person, company)]),
        },
        MatchOperation::FetchRows {
            output: cross_output.clone(),
            order: cross_order.clone(),
            window: Window {
                offset: 0,
                limit: 20,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    );
    let cross_join = assert_result_parity(
        &db,
        &fixture,
        cross_join,
        "explicit person-company cross join",
    )
    .await;
    assert_eq!(row_count(&cross_join), 6);

    let field_comparison = MatchRequest::v1(
        MatchPlan {
            bindings: cross_bindings,
            predicate: Some(MatchExpr::FieldComparison {
                left: name_field(&fixture),
                operator: ComparisonOp::LessThan,
                right: bound_field(company, &fixture.company_descriptor_id, "name"),
            }),
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: cross_output,
            order: cross_order,
            window: Window {
                offset: 0,
                limit: 20,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    );
    let field_comparison = assert_result_parity(
        &db,
        &fixture,
        field_comparison,
        "cross-binding field comparison",
    )
    .await;
    assert!(!row_iids(&field_comparison).is_empty());

    // Subtype-inclusive relation role edges, named output, and stable
    // multi-binding ordering agree exactly.
    let relation_rows = MatchRequest::v1(
        relation_plan(&fixture),
        MatchOperation::FetchRows {
            output: named_link_output(),
            order: relation_order(&fixture),
            window: Window {
                offset: 0,
                limit: 20,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    );
    let relation_rows = assert_result_parity(
        &db,
        &fixture,
        relation_rows,
        "subtype role edges and named relation hydration",
    )
    .await;
    assert_eq!(row_count(&relation_rows), 4);

    // Exactly-one relation selection also verifies complete relation-role and
    // role-player hydration without inventing a key for the released
    // descriptor's multi-valued `since` field.
    let mut one_relation_plan = relation_plan(&fixture);
    let Some(MatchExpr::And { expressions }) = &mut one_relation_plan.predicate else {
        panic!("relation fixture predicate must be a conjunction");
    };
    expressions.push(MatchExpr::FieldValue {
        field: bound_field(
            BindingId::new(1),
            &fixture.employment_descriptor_id,
            "since",
        ),
        operator: ComparisonOp::Equal,
        value: AttributeValue::Date("2020-01-01".into()),
    });
    let one_relation = assert_result_parity(
        &db,
        &fixture,
        MatchRequest::v1(
            one_relation_plan,
            MatchOperation::FetchRows {
                output: named_relation_output(),
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
        "exactly-one relation role hydration",
    )
    .await;
    let MatchResult::Rows { rows } = &one_relation else {
        panic!("expected relation row");
    };
    let SlotValue::One(relation) = &rows[0].slots()[1] else {
        panic!("expected selected relation");
    };
    assert_eq!(relation.roles().len(), 2);

    // Reachability is existential: duplicate Ada -> Acme proof relations must
    // still produce one distinct endpoint pair.
    let reachable = MatchRequest::v1(
        MatchPlan {
            bindings: vec![
                MatchBinding {
                    id: person,
                    descriptor: fixture.person_descriptor_id.clone(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Subtypes,
                },
                MatchBinding {
                    id: company,
                    descriptor: fixture.company_descriptor_id.clone(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                },
            ],
            predicate: Some(MatchExpr::Reachable {
                relation: fixture.employment_descriptor_id.clone(),
                role_from: fixture
                    .registry
                    .role_id(&fixture.employment_descriptor_id, "employee")
                    .expect("employee role"),
                role_to: fixture
                    .registry
                    .role_id(&fixture.employment_descriptor_id, "employer")
                    .expect("employer role"),
                source: person,
                target: company,
                min_depth: 1,
                max_depth: 1,
            }),
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![
                    FetchSlot::One { binding: person },
                    FetchSlot::One { binding: company },
                ],
            },
            order: vec![
                MatchOrder {
                    field: name_field(&fixture),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                },
                MatchOrder {
                    field: bound_field(company, &fixture.company_descriptor_id, "name"),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                },
            ],
            window: Window {
                offset: 0,
                limit: 20,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    );
    let reachable =
        assert_result_parity(&db, &fixture, reachable, "bounded reachable endpoint pairs").await;
    assert_eq!(row_count(&reachable), 4);

    // Page by distinct person identity while preserving collection
    // multiplicity/distinctness and same-snapshot totals.
    for (distinct, expected_ada_companies) in [(false, 3), (true, 2)] {
        let page = MatchRequest::v1(
            relation_plan(&fixture),
            MatchOperation::PageBy {
                root: person,
                output: FetchShape::Named {
                    slots: vec![
                        NamedFetchSlot {
                            name: "person".into(),
                            slot: FetchSlot::One { binding: person },
                        },
                        NamedFetchSlot {
                            name: "companies".into(),
                            slot: FetchSlot::Collect {
                                binding: BindingId::new(2),
                                distinct,
                                order: vec![MatchOrder {
                                    field: bound_field(
                                        BindingId::new(2),
                                        &fixture.company_descriptor_id,
                                        "name",
                                    ),
                                    direction: SortDirection::Ascending,
                                    missing: MissingOrder::Reject,
                                }],
                            },
                        },
                    ],
                },
                order: vec![MatchOrder {
                    field: name_field(&fixture),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                }],
                window: Window {
                    offset: 0,
                    limit: 10,
                },
                include_total: true,
            },
        );
        let page = assert_result_parity(
            &db,
            &fixture,
            page,
            if distinct {
                "named page with distinct collection"
            } else {
                "named page with multiplicity-preserving collection"
            },
        )
        .await;
        let MatchResult::Page { entries, total, .. } = page else {
            panic!("expected page");
        };
        assert_eq!(total, Some(3));
        let ada = entries
            .iter()
            .find(|entry| match &entry.slots()[0] {
                SlotValue::One(thing) => thing_name(thing) == Some("Ada"),
                SlotValue::Many(_) => false,
            })
            .expect("Ada page entry");
        let SlotValue::Many(companies) = &ada.slots()[1] else {
            panic!("expected company collection");
        };
        assert_eq!(companies.len(), expected_ada_companies);
    }

    // Exactly-one returns one row or the same complete released diagnostic.
    let one = exact_one_request(
        &fixture,
        Some(MatchExpr::FieldValue {
            field: name_field(&fixture),
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("Ada".into()),
        }),
    );
    let one = assert_result_parity(&db, &fixture, one, "exactly-one success").await;
    assert_eq!(row_count(&one), 1);
    let no_result = assert_error_parity(
        &db,
        &fixture,
        exact_one_request(
            &fixture,
            Some(MatchExpr::FieldValue {
                field: name_field(&fixture),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Nobody".into()),
            }),
        ),
        "exactly-one empty",
    )
    .await;
    assert_eq!(no_result.code().as_str(), "no_result");
    let not_unique = assert_error_parity(
        &db,
        &fixture,
        exact_one_request(&fixture, None),
        "exactly-one multiple",
    )
    .await;
    assert_eq!(not_unique.code().as_str(), "not_unique");

    // Count and exists agree with V1 distinct-root semantics, including a
    // joined graph where duplicate relations do not inflate the root count.
    let count_request = MatchRequest::v1(
        person_plan(&fixture, None),
        MatchOperation::CountBy {
            root: BindingId::new(0),
        },
    );
    let count = assert_result_parity(&db, &fixture, count_request, "subtype-inclusive count").await;
    let MatchResult::Count { value: count, .. } = count else {
        panic!("expected count");
    };
    assert_eq!(count, 4);

    let joined_count = assert_result_parity(
        &db,
        &fixture,
        MatchRequest::v1(
            relation_plan(&fixture),
            MatchOperation::CountBy { root: person },
        ),
        "joined distinct-root count",
    )
    .await;
    let MatchResult::Count {
        value: joined_count,
        ..
    } = joined_count
    else {
        panic!("expected joined count");
    };
    assert_eq!(joined_count, 3);

    let exists = |value: &str| {
        MatchRequest::v1(
            person_plan(
                &fixture,
                Some(MatchExpr::FieldValue {
                    field: name_field(&fixture),
                    operator: ComparisonOp::Equal,
                    value: AttributeValue::String(value.into()),
                }),
            ),
            MatchOperation::ExistsBy {
                root: BindingId::new(0),
            },
        )
    };
    for (value, expected) in [("Grace", true), ("Nobody", false)] {
        let result =
            assert_result_parity(&db, &fixture, exists(value), &format!("exists {value}")).await;
        let MatchResult::Exists { value: actual, .. } = result else {
            panic!("expected exists");
        };
        assert_eq!(actual, expected, "exists for {value:?}");
    }
}
