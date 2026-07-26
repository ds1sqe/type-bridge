use serde_json::{Value, json};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_CANONICAL_BYTES, MAX_CANONICAL_STRING_BYTES};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, HydrationBindingV2, HydrationDescriptorV2, HydrationFieldV2,
    HydrationPlayerV2, HydrationProjectionV2, HydrationRoleV2, ModelQueryV2,
    QUERY_PLAN_CANONICALIZATION_V1, QUERY_PLAN_CANONICALIZATION_V2, QUERY_PLAN_FORMAT_V1,
    QUERY_PLAN_FORMAT_V2, QueryBindingPairV2, QueryComparatorV2, QueryFieldV2, QueryMissingOrderV2,
    QueryModelOutputSlotV2, QueryModelOutputV2, QueryNamedOutputSlotV2, QueryOrderDirectionV2,
    QueryOrderTermV2, QueryOutput, QueryPattern, QueryPatternV2, QueryPlan,
    QueryPlanV2Compatibility, QueryRowCardinalityV2, QueryStableOrderV2, QueryWindowV2, ReadStage,
    decode_query_plan, query_plan_v2_capability_vocabulary,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::temporal::{CanonicalDateTime, CanonicalDuration};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, Cardinality, ValueTypeTag};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding ID"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding ID")
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("type ID")
}

fn semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"query-plan-v2-golden-authority",
    )
    .expect("managed semantics")
}

fn base_parts() -> (
    Vec<AssertionBinding>,
    Vec<ReadStage>,
    QueryOutput,
    ManagedSemanticSchemaFingerprint,
) {
    let bindings = vec![
        binding(0, "person"),
        binding(1, "employment"),
        binding(2, "peer"),
    ];
    let pipeline = vec![ReadStage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            QueryPattern::Isa {
                binding: binding_id(1),
                include_subtypes: true,
                type_id: type_id(TypeKind::Relation, "employment"),
            },
            QueryPattern::Isa {
                binding: binding_id(2),
                include_subtypes: true,
                type_id: type_id(TypeKind::Entity, "person"),
            },
        ],
    }];
    let output = QueryOutput::Rows {
        columns: vec![binding_id(0), binding_id(1)],
    };
    (bindings, pipeline, output, semantics())
}

fn hydration() -> HydrationProjectionV2 {
    let person = type_id(TypeKind::Entity, "person");
    let employment = type_id(TypeKind::Relation, "employment");
    HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(binding_id(1), employment.clone(), vec![employment.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(
                person.clone(),
                vec![
                    HydrationFieldV2::new(
                        "name",
                        vec![person.clone()],
                        AttributeId::new("name").expect("attribute"),
                        ValueTypeTag::String,
                        Cardinality::new(1, Some(1)).expect("cardinality"),
                        false,
                        false,
                        true,
                    ),
                    HydrationFieldV2::new(
                        "tags",
                        vec![person.clone()],
                        AttributeId::new("tag").expect("attribute"),
                        ValueTypeTag::String,
                        Cardinality::new(0, None).expect("cardinality"),
                        true,
                        true,
                        false,
                    ),
                ],
                Vec::new(),
            ),
            HydrationDescriptorV2::new(
                employment.clone(),
                vec![
                    HydrationFieldV2::new(
                        "code",
                        vec![employment.clone()],
                        AttributeId::new("code").expect("attribute"),
                        ValueTypeTag::String,
                        Cardinality::new(1, Some(1)).expect("cardinality"),
                        false,
                        false,
                        true,
                    ),
                    HydrationFieldV2::new(
                        "start_date",
                        vec![employment],
                        AttributeId::new("start-date").expect("attribute"),
                        ValueTypeTag::Date,
                        Cardinality::new(0, Some(1)).expect("cardinality"),
                        false,
                        false,
                        false,
                    ),
                ],
                vec![HydrationRoleV2::new(
                    RoleId::new("employment", "employee").expect("role"),
                    vec![RoleId::new("employment", "employee").expect("role")],
                    vec![HydrationPlayerV2::new(person.clone(), vec![person])],
                    Cardinality::new(1, None).expect("cardinality"),
                    true,
                    true,
                )],
            ),
        ],
    )
}

fn person_name(binding: u16) -> QueryFieldV2 {
    QueryFieldV2::new(
        binding_id(binding),
        type_id(TypeKind::Entity, "person"),
        AttributeId::new("name").expect("attribute"),
        ValueTypeTag::String,
    )
}

fn employment_code(binding: u16) -> QueryFieldV2 {
    QueryFieldV2::new(
        binding_id(binding),
        type_id(TypeKind::Relation, "employment"),
        AttributeId::new("code").expect("attribute"),
        ValueTypeTag::String,
    )
}

fn stable_order(field: QueryFieldV2, tiebreaker: u16) -> QueryStableOrderV2 {
    QueryStableOrderV2::new(
        vec![QueryOrderTermV2::new(
            field,
            QueryOrderDirectionV2::Ascending,
            QueryMissingOrderV2::Reject,
        )],
        vec![binding_id(tiebreaker)],
    )
}

fn rich_output() -> QueryModelOutputV2 {
    QueryModelOutputV2::Named {
        slots: vec![
            QueryNamedOutputSlotV2::new(
                "person",
                QueryModelOutputSlotV2::One {
                    binding: binding_id(0),
                    declared: type_id(TypeKind::Entity, "person"),
                },
            ),
            QueryNamedOutputSlotV2::new(
                "employments",
                QueryModelOutputSlotV2::Collect {
                    binding: binding_id(1),
                    declared: type_id(TypeKind::Relation, "employment"),
                    distinct: true,
                    order: stable_order(employment_code(1), 1),
                },
            ),
        ],
    }
}

fn rich_compatibility() -> QueryPlanV2Compatibility {
    let predicate = QueryPatternV2::And {
        patterns: vec![
            QueryPatternV2::Or {
                patterns: vec![
                    QueryPatternV2::FieldValue {
                        field: person_name(0),
                        comparator: QueryComparatorV2::Contains,
                        value: CanonicalValue::String(
                            CanonicalString::new("Ali").expect("canonical string"),
                        )
                        .into(),
                    },
                    QueryPatternV2::RoleEdge {
                        include_relation_subtypes: true,
                        player: binding_id(0),
                        relation: binding_id(1),
                        relation_type: type_id(TypeKind::Relation, "employment"),
                        role: RoleId::new("employment", "employee").expect("role"),
                    },
                ],
            },
            QueryPatternV2::Reachable {
                min_depth: 0,
                max_depth: 2,
                relation: type_id(TypeKind::Relation, "employment"),
                role_from: RoleId::new("employment", "employee").expect("role"),
                role_to: RoleId::new("employment", "employer").expect("role"),
                source: binding_id(0),
                target: binding_id(2),
            },
        ],
    };
    QueryPlanV2Compatibility::new(
        Some(predicate),
        vec![QueryBindingPairV2::new(binding_id(0), binding_id(2))],
        Some(ModelQueryV2::Page {
            hydration: hydration(),
            include_total: true,
            order: stable_order(person_name(0), 0),
            output: rich_output(),
            root: binding_id(0),
            window: QueryWindowV2::new(10, 25),
        }),
    )
}

fn rich_plan() -> QueryPlan {
    let (bindings, pipeline, output, managed) = base_parts();
    QueryPlan::new_v2_with_functions(
        bindings,
        Vec::new(),
        Vec::new(),
        pipeline,
        output,
        rich_compatibility(),
        managed,
    )
    .expect("rich V2 plan")
}

fn compatibility_value_plan(
    value: CompatibilityValueV2,
    value_type: ValueTypeTag,
    attribute: &str,
) -> QueryPlan {
    let person = type_id(TypeKind::Entity, "person");
    let (bindings, pipeline, output, managed) = base_parts();
    let attribute = AttributeId::new(attribute).expect("attribute");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![person.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            person.clone(),
            vec![HydrationFieldV2::new(
                "value",
                vec![person.clone()],
                attribute.clone(),
                value_type,
                Cardinality::new(0, Some(1)).expect("cardinality"),
                false,
                false,
                false,
            )],
            Vec::new(),
        )],
    );
    QueryPlan::new_v2_with_functions(
        bindings,
        Vec::new(),
        Vec::new(),
        pipeline,
        output,
        QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::FieldValue {
                field: QueryFieldV2::new(binding_id(0), person.clone(), attribute, value_type),
                comparator: QueryComparatorV2::Equal,
                value,
            }),
            Vec::new(),
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        managed,
    )
    .expect("compatibility value plan")
}

fn reachable_plan(v2: bool, min_depth: u8, max_depth: u8) -> QueryPlan {
    let pipeline = vec![ReadStage::Match {
        patterns: vec![QueryPattern::Reachable {
            min_depth,
            max_depth,
            relation: type_id(TypeKind::Relation, "edge"),
            role_from: RoleId::new("edge", "origin").expect("role"),
            role_to: RoleId::new("edge", "destination").expect("role"),
            source: binding_id(0),
            target: binding_id(1),
        }],
    }];
    let bindings = vec![binding(0, "source"), binding(1, "target")];
    let output = QueryOutput::Rows {
        columns: vec![binding_id(0), binding_id(1)],
    };
    if v2 {
        QueryPlan::new_v2(bindings, Vec::new(), pipeline, output, semantics())
            .expect("V2 reachable plan")
    } else {
        QueryPlan::new(bindings, Vec::new(), pipeline, output, semantics())
            .expect("V1 reachable plan")
    }
}

#[test]
fn v1_serialization_and_fingerprint_metadata_stay_exact() {
    let (bindings, pipeline, output, managed) = base_parts();
    let plan =
        QueryPlan::new(bindings, Vec::new(), pipeline, output, managed).expect("V1 query plan");
    let bytes = plan.canonical_bytes().expect("V1 bytes");
    let value: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(plan.format(), QUERY_PLAN_FORMAT_V1);
    assert!(value.get("compatibility").is_none());
    assert_eq!(
        plan.fingerprint()
            .expect("fingerprint")
            .as_fingerprint()
            .canonicalization()
            .as_str(),
        QUERY_PLAN_CANONICALIZATION_V1,
    );
    assert_eq!(decode_query_plan(&bytes).expect("V1 decode"), plan);
    assert_eq!(
        decode_query_plan(&bytes)
            .expect("V1 decode")
            .canonical_bytes()
            .expect("V1 re-encode"),
        bytes,
    );
}

#[test]
fn reachable_wire_is_frozen_in_v1_and_additive_in_v2() {
    let v1 = reachable_plan(false, 1, 2);
    let v1_bytes = v1.canonical_bytes().expect("V1 bytes");
    let fixture = include_bytes!("fixtures/query-plan-v1-reachable.json");
    let fixture = fixture.strip_suffix(b"\n").unwrap_or(fixture);
    assert_eq!(v1_bytes, fixture, "released V1 reachability bytes changed");
    assert_eq!(decode_query_plan(&v1_bytes).expect("V1 decode"), v1);

    let mut missing_format: Value = serde_json::from_slice(&v1_bytes).expect("JSON");
    missing_format
        .as_object_mut()
        .expect("object")
        .remove("format");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&missing_format).expect("canonical JSON"))
            .expect_err("missing V1 discriminator")
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut forged_v1: Value = serde_json::from_slice(&v1_bytes).expect("JSON");
    forged_v1["pipeline"][0]["patterns"][0]["min_depth"] = json!(1);
    assert!(
        decode_query_plan(&to_canonical_json(&forged_v1).expect("canonical JSON")).is_err(),
        "V1 must reject the additive field"
    );

    let v2 = reachable_plan(true, 0, 0);
    let v2_bytes = v2.canonical_bytes().expect("V2 bytes");
    assert_eq!(
        serde_json::from_slice::<Value>(&v2_bytes).expect("JSON")["pipeline"][0]["patterns"][0]["min_depth"],
        json!(0),
    );
    assert_eq!(decode_query_plan(&v2_bytes).expect("V2 decode"), v2);

    let mut missing_v2: Value = serde_json::from_slice(&v2_bytes).expect("JSON");
    missing_v2["pipeline"][0]["patterns"][0]
        .as_object_mut()
        .expect("pattern object")
        .remove("min_depth");
    assert!(
        decode_query_plan(&to_canonical_json(&missing_v2).expect("canonical JSON")).is_err(),
        "V2 must require its inclusive minimum"
    );

    let error = QueryPlan::new(
        vec![binding(0, "source"), binding(1, "target")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Reachable {
                min_depth: 0,
                max_depth: 0,
                relation: type_id(TypeKind::Relation, "edge"),
                role_from: RoleId::new("edge", "origin").expect("role"),
                role_to: RoleId::new("edge", "destination").expect("role"),
                source: binding_id(0),
                target: binding_id(1),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        semantics(),
    )
    .expect_err("V1 cannot express an inclusive zero-hop branch");
    assert_eq!(error.code().as_str(), "query_plan_v1_reachable_min_depth");
}

#[test]
fn v1_near_ceiling_is_bounded_after_frozen_wire_projection() {
    let build = |tail_bytes: usize| {
        let full = || {
            CanonicalValue::String(
                CanonicalString::new("x".repeat(MAX_CANONICAL_STRING_BYTES))
                    .expect("maximum canonical string"),
            )
        };
        let comparison = |left, right| QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left: type_bridge_contract::query_plan::QueryOperand::Literal { value: left },
            right: type_bridge_contract::query_plan::QueryOperand::Literal { value: right },
        };
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "node"),
            },
            QueryPattern::Reachable {
                min_depth: 1,
                max_depth: 1,
                relation: type_id(TypeKind::Relation, "edge"),
                role_from: RoleId::new("edge", "origin").expect("role"),
                role_to: RoleId::new("edge", "destination").expect("role"),
                source: binding_id(0),
                target: binding_id(0),
            },
        ];
        for _ in 0..7 {
            patterns.push(comparison(full(), full()));
        }
        patterns.push(comparison(
            full(),
            CanonicalValue::String(
                CanonicalString::new("x".repeat(tail_bytes)).expect("tail string"),
            ),
        ));
        QueryPlan::new(
            vec![binding(0, "node")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            semantics(),
        )
        .expect("V1 near-ceiling plan")
    };

    let baseline = build(0).canonical_bytes().expect("baseline frozen bytes");
    let tail_bytes = MAX_CANONICAL_BYTES
        .checked_sub(baseline.len())
        .expect("fifteen MiB of literals leave bounded envelope overhead");
    assert!(
        tail_bytes <= MAX_CANONICAL_STRING_BYTES,
        "the final literal must fit the per-string ceiling",
    );

    let plan = build(tail_bytes);
    let frozen = plan.canonical_bytes().expect("frozen V1 bytes fit exactly");
    assert_eq!(frozen.len(), MAX_CANONICAL_BYTES);
    assert_eq!(
        to_canonical_json(&plan)
            .expect_err("widened min_depth bytes exceed the ceiling")
            .code()
            .as_str(),
        "canonical_json_too_large",
    );
    assert_eq!(decode_query_plan(&frozen).expect("V1 decode"), plan);
    plan.fingerprint()
        .expect("frozen near-ceiling bytes remain fingerprintable");
}

#[test]
fn ordinary_v2_disjunction_preserves_conjunction_branches_and_binding_intersection() {
    let branches = vec![
        vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            QueryPattern::Isa {
                binding: binding_id(1),
                include_subtypes: false,
                type_id: type_id(TypeKind::Relation, "employment"),
            },
        ],
        vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "contractor"),
            },
            QueryPattern::Not {
                patterns: vec![QueryPattern::Isa {
                    binding: binding_id(2),
                    include_subtypes: false,
                    type_id: type_id(TypeKind::Entity, "banned"),
                }],
            },
        ],
    ];
    let bindings = vec![
        binding(0, "person"),
        binding(1, "employment"),
        binding(2, "banned"),
    ];
    let pipeline = vec![ReadStage::Match {
        patterns: vec![QueryPattern::Or {
            branches: branches.clone(),
        }],
    }];
    let output = QueryOutput::Rows {
        columns: vec![binding_id(0)],
    };
    let plan = QueryPlan::new_v2(
        bindings.clone(),
        Vec::new(),
        pipeline.clone(),
        output,
        semantics(),
    )
    .expect("(A and B) or (C and not D)");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.pattern.disjunction")
    );
    let bytes = plan.canonical_bytes().expect("V2 bytes");
    assert_eq!(decode_query_plan(&bytes).expect("V2 round trip"), plan);
    let wire: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(
        wire["pipeline"][0]["patterns"][0]["branches"]
            .as_array()
            .expect("branches")
            .len(),
        2,
    );

    assert_eq!(
        QueryPlan::new(
            bindings.clone(),
            Vec::new(),
            pipeline.clone(),
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            semantics(),
        )
        .expect_err("V1 ordinary disjunction")
        .code()
        .as_str(),
        "query_plan_v1_disjunction_unsupported",
    );
    assert!(
        QueryPlan::new_v2(
            bindings.clone(),
            Vec::new(),
            pipeline,
            QueryOutput::Rows {
                columns: vec![binding_id(1)],
            },
            semantics(),
        )
        .is_err(),
        "a branch-local binding must not leak into the outer row"
    );
    assert_eq!(
        QueryPlan::new_v2(
            bindings,
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![QueryPattern::Or {
                    branches: vec![Vec::new()],
                }],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            semantics(),
        )
        .expect_err("empty disjunction branch")
        .code()
        .as_str(),
        "query_plan_disjunction_term_limit",
    );
}

#[test]
fn equal_looking_v1_and_v2_plans_have_distinct_format_domains() {
    let (bindings, pipeline, output, managed) = base_parts();
    let v1 = QueryPlan::new(
        bindings.clone(),
        Vec::new(),
        pipeline.clone(),
        output.clone(),
        managed.clone(),
    )
    .expect("V1 plan");
    let v2 = QueryPlan::new_v2(bindings, Vec::new(), pipeline, output, managed).expect("V2 plan");
    assert_eq!(v2.format(), QUERY_PLAN_FORMAT_V2);
    assert!(v2.v2_compatibility().is_some());
    assert_eq!(
        v2.fingerprint()
            .expect("V2 fingerprint")
            .as_fingerprint()
            .canonicalization()
            .as_str(),
        QUERY_PLAN_CANONICALIZATION_V2,
    );
    assert_ne!(
        v1.fingerprint().expect("V1 fingerprint"),
        v2.fingerprint().expect("V2 fingerprint"),
    );
}

#[test]
fn rich_v2_compatibility_round_trips_and_derives_every_used_capability() {
    let plan = rich_plan();
    let bytes = plan.canonical_bytes().expect("V2 bytes");
    assert_eq!(decode_query_plan(&bytes).expect("V2 decode"), plan);
    let capabilities = plan
        .required_capabilities()
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "query.execution.batch-identity-rebind",
        "query.execution.same-snapshot-hydration",
        "query.operation.distinct-count",
        "query.operation.page",
        "query.order.stable-collection",
        "query.order.stable-root",
        "query.output.collect-distinct",
        "query.output.hydrated",
        "query.output.named",
        "query.pattern.disjunction",
        "query.pattern.links-subtypes",
        "query.pattern.reachable",
        "query.pattern.string-operators",
        "query.plan.v2",
        "query.topology.cross-join",
    ] {
        assert!(capabilities.contains(&expected), "missing {expected}");
    }
    assert!(
        plan.required_capabilities()
            .missing_from(&query_plan_v2_capability_vocabulary())
            .is_empty()
    );
}

#[test]
fn v2_decode_dispatch_and_claim_checks_fail_closed() {
    let bytes = rich_plan().canonical_bytes().expect("V2 bytes");
    let mut future_format: Value = serde_json::from_slice(&bytes).expect("JSON");
    future_format["format"] = json!("typebridge.query-plan/v99");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&future_format).expect("canonical JSON"))
            .expect_err("unknown version")
            .code()
            .as_str(),
        "query_plan_format_unsupported",
    );

    let mut missing_contract: Value = serde_json::from_slice(&bytes).expect("JSON");
    missing_contract
        .as_object_mut()
        .expect("object")
        .remove("compatibility");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&missing_contract).expect("canonical JSON"))
            .expect_err("missing required V2 field")
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut unknown_root: Value = serde_json::from_slice(&bytes).expect("JSON");
    unknown_root["future"] = json!(true);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&unknown_root).expect("canonical JSON"))
            .expect_err("unknown root field")
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut unknown_nested: Value = serde_json::from_slice(&bytes).expect("JSON");
    unknown_nested["compatibility"]["future"] = json!(true);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&unknown_nested).expect("canonical JSON"))
            .expect_err("unknown nested field")
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut forged_claim: Value = serde_json::from_slice(&bytes).expect("JSON");
    forged_claim["required_capabilities"] = json!(["query.plan", "query.plan.v2"]);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&forged_claim).expect("canonical JSON"))
            .expect_err("forged syntax claim")
            .code()
            .as_str(),
        "query_plan_capability_claim_mismatch",
    );
}

#[test]
fn v2_compatibility_invalid_edge_shapes_fail_before_encoding() {
    let (bindings, pipeline, output, managed) = base_parts();
    let build = |compatibility| {
        QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            compatibility,
            managed.clone(),
        )
    };

    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            None,
            vec![QueryBindingPairV2::new(binding_id(1), binding_id(1))],
            None,
        ))
        .expect_err("self cross join")
        .code()
        .as_str(),
        "query_plan_v2_cross_join_pair",
    );
    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::FieldValue {
                field: QueryFieldV2::new(
                    binding_id(0),
                    type_id(TypeKind::Entity, "person"),
                    AttributeId::new("age").expect("attribute"),
                    ValueTypeTag::Long,
                ),
                comparator: QueryComparatorV2::Regex,
                value: CanonicalValue::Long(1).into(),
            }),
            Vec::new(),
            None,
        ))
        .expect_err("string operator over a long")
        .code()
        .as_str(),
        "query_plan_v2_string_operator_type",
    );
    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::Or {
                patterns: Vec::new(),
            }),
            Vec::new(),
            None,
        ))
        .expect_err("empty disjunction")
        .code()
        .as_str(),
        "query_plan_v2_boolean_term_limit",
    );
    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::Or {
                patterns: vec![QueryPatternV2::Reachable {
                    min_depth: 0,
                    max_depth: 1,
                    relation: type_id(TypeKind::Relation, "employment"),
                    role_from: RoleId::new("employment", "employee").expect("role"),
                    role_to: RoleId::new("employment", "employer").expect("role"),
                    source: binding_id(0),
                    target: binding_id(2),
                }],
            }),
            Vec::new(),
            None,
        ))
        .expect_err("reachability under disjunction")
        .code()
        .as_str(),
        "query_plan_v2_reachable_not_root_positive",
    );
    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::RoleEdge {
                include_relation_subtypes: false,
                player: binding_id(0),
                relation: binding_id(1),
                relation_type: type_id(TypeKind::Relation, "employment"),
                role: RoleId::new("other-relation", "employee").expect("role"),
            }),
            Vec::new(),
            None,
        ))
        .expect_err("foreign role owner")
        .code()
        .as_str(),
        "query_plan_v2_role_edge_authority",
    );
    assert_eq!(
        build(QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::Reachable {
                min_depth: 1,
                max_depth: 2,
                relation: type_id(TypeKind::Relation, "employment"),
                role_from: RoleId::new("employment", "employee").expect("role"),
                role_to: RoleId::new("other-relation", "employer").expect("role"),
                source: binding_id(0),
                target: binding_id(2),
            }),
            Vec::new(),
            None,
        ))
        .expect_err("foreign reachability role owner")
        .code()
        .as_str(),
        "query_plan_v2_reachable_contract",
    );

    let exactly_one_with_order = QueryPlanV2Compatibility::new(
        None,
        Vec::new(),
        Some(ModelQueryV2::Rows {
            cardinality: QueryRowCardinalityV2::ExactlyOne,
            hydration: hydration(),
            order: Some(stable_order(person_name(0), 0)),
            output: QueryModelOutputV2::Positional {
                slots: vec![QueryModelOutputSlotV2::One {
                    binding: binding_id(0),
                    declared: type_id(TypeKind::Entity, "person"),
                }],
            },
            window: QueryWindowV2::new(0, 1),
        }),
    );
    assert_eq!(
        build(exactly_one_with_order)
            .expect_err("ordered exactly-one")
            .code()
            .as_str(),
        "query_plan_v2_exactly_one_contract",
    );
}

#[test]
fn hydration_roles_bind_owner_and_declared_to_concrete_player_authority() {
    let bytes = rich_plan().canonical_bytes().expect("V2 bytes");
    let authority: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(
        authority["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"][0]["distinct"],
        json!(false),
    );
    assert_eq!(
        authority["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"][0]["unique"],
        json!(true),
    );

    let mut unordered_distinct_field = authority.clone();
    unordered_distinct_field["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"]
        [1]["ordered"] = json!(false);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&unordered_distinct_field).expect("canonical JSON"))
            .expect_err("distinct unordered field")
            .code()
            .as_str(),
        "query_plan_v2_field_distinct_requires_ordered",
    );

    let mut unordered_distinct_role = authority.clone();
    unordered_distinct_role["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"]
        [0]["ordered"] = json!(false);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&unordered_distinct_role).expect("canonical JSON"))
            .expect_err("distinct unordered role")
            .code()
            .as_str(),
        "query_plan_v2_role_distinct_requires_ordered",
    );

    let mut empty_reference_owners = authority.clone();
    empty_reference_owners["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"]
        [0]["reference_owners"] = json!([]);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&empty_reference_owners).expect("canonical JSON"))
            .expect_err("field without owner authority")
            .code()
            .as_str(),
        "query_plan_v2_hydration_field_owners_not_canonical",
    );

    let mut shadowed_alias = authority.clone();
    shadowed_alias["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"][1]["alias"] =
        json!("name");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&shadowed_alias).expect("canonical JSON"))
            .expect_err("shadowed field alias")
            .code()
            .as_str(),
        "query_plan_v2_hydration_fields_not_canonical",
    );

    let mut duplicate_provider_field = authority.clone();
    duplicate_provider_field["compatibility"]["model_query"]["hydration"]["descriptors"][0]["fields"]
        [1]["attribute"] = json!("name");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&duplicate_provider_field).expect("canonical JSON"))
            .expect_err("duplicate provider field")
            .code()
            .as_str(),
        "query_plan_v2_hydration_fields_not_canonical",
    );

    let mut forged_owner = authority.clone();
    forged_owner["compatibility"]["predicate"]["patterns"][0]["patterns"][0]["field"]["descriptor"]
        ["label"] = json!("forged-person");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&forged_owner).expect("canonical JSON"))
            .expect_err("forged owner")
            .code()
            .as_str(),
        "query_plan_v2_field_authority",
    );

    let mut forged_attribute = authority.clone();
    forged_attribute["compatibility"]["predicate"]["patterns"][0]["patterns"][0]["field"]["attribute"] =
        json!("forged-name");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&forged_attribute).expect("canonical JSON"))
            .expect_err("forged attribute")
            .code()
            .as_str(),
        "query_plan_v2_field_authority",
    );

    let mut forged_type = authority.clone();
    forged_type["compatibility"]["predicate"]["patterns"][0]["patterns"][0]["comparator"] =
        json!("equal");
    forged_type["compatibility"]["predicate"]["patterns"][0]["patterns"][0]["field"]["value_type"] =
        json!("long");
    forged_type["compatibility"]["predicate"]["patterns"][0]["patterns"][0]["value"] =
        json!({"kind":"long","value":"1"});
    assert_eq!(
        decode_query_plan(&to_canonical_json(&forged_type).expect("canonical JSON"))
            .expect_err("forged field type")
            .code()
            .as_str(),
        "query_plan_v2_field_authority",
    );

    let mut empty_role_references = authority.clone();
    empty_role_references["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"]
        [0]["reference_roles"] = json!([]);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&empty_role_references).expect("canonical JSON"))
            .expect_err("role without reference authority")
            .code()
            .as_str(),
        "query_plan_v2_role_references_not_canonical",
    );

    let mut missing_provider_reference = authority.clone();
    missing_provider_reference["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"]
        [0]["reference_roles"] =
        json!([{"declaring_relation":"ancestor-employment","label":"employee"}]);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&missing_provider_reference).expect("canonical JSON"))
            .expect_err("role authority without its concrete provider")
            .code()
            .as_str(),
        "query_plan_v2_role_references_not_canonical",
    );

    let mut different_label_reference = authority.clone();
    different_label_reference["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"]
        [0]["reference_roles"] = json!([
        {"declaring_relation":"employment","label":"employee"},
        {"declaring_relation":"employment","label":"employer"}
    ]);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&different_label_reference).expect("canonical JSON"))
            .expect_err("different-label role alias")
            .code()
            .as_str(),
        "query_plan_v2_role_references_not_canonical",
    );

    let mut foreign_owner = authority.clone();
    foreign_owner["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"][0]["role"]
        ["declaring_relation"] = json!("other-relation");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&foreign_owner).expect("canonical JSON"))
            .expect_err("foreign hydration role owner")
            .code()
            .as_str(),
        "query_plan_v2_hydration_roles_not_canonical",
    );

    let mut missing_concrete = authority.clone();
    missing_concrete["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"][0]["players"]
        [0]["concrete_descriptors"][0]["label"] = json!("unprojected-person");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&missing_concrete).expect("canonical JSON"))
            .expect_err("unprojected concrete role player")
            .code()
            .as_str(),
        "query_plan_v2_missing_role_player_descriptor",
    );

    let mut legacy_shape = authority;
    let role = legacy_shape["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"]
        [0]
    .as_object_mut()
    .expect("role object");
    let players = role.remove("players").expect("players");
    role.insert("player_descriptors".to_owned(), players);
    assert_eq!(
        decode_query_plan(&to_canonical_json(&legacy_shape).expect("canonical JSON"))
            .expect_err("unversioned player shape")
            .code()
            .as_str(),
        "invalid_canonical_value",
    );
}

#[test]
fn stable_order_terms_are_provable_from_their_public_response_slots() {
    let (bindings, pipeline, output, managed) = base_parts();
    let build = |model_query| {
        QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            QueryPlanV2Compatibility::new(None, Vec::new(), Some(model_query)),
            managed.clone(),
        )
    };

    build(ModelQueryV2::Rows {
        cardinality: QueryRowCardinalityV2::BoundedMany,
        hydration: hydration(),
        order: Some(stable_order(person_name(0), 0)),
        output: rich_output(),
        window: QueryWindowV2::new(0, 10),
    })
    .expect("collection slots are not part of the selected row identity");

    let duplicate_order_field = QueryStableOrderV2::new(
        vec![
            QueryOrderTermV2::new(
                person_name(0),
                QueryOrderDirectionV2::Ascending,
                QueryMissingOrderV2::Reject,
            ),
            QueryOrderTermV2::new(
                person_name(0),
                QueryOrderDirectionV2::Descending,
                QueryMissingOrderV2::Reject,
            ),
        ],
        vec![binding_id(0)],
    );
    assert_eq!(
        build(ModelQueryV2::Rows {
            cardinality: QueryRowCardinalityV2::BoundedMany,
            hydration: hydration(),
            order: Some(duplicate_order_field),
            output: rich_output(),
            window: QueryWindowV2::new(0, 10),
        })
        .expect_err("duplicate order field")
        .code()
        .as_str(),
        "query_plan_v2_duplicate_order_field",
    );

    let hidden_collection_term = QueryStableOrderV2::new(
        vec![QueryOrderTermV2::new(
            employment_code(1),
            QueryOrderDirectionV2::Ascending,
            QueryMissingOrderV2::Reject,
        )],
        vec![binding_id(0)],
    );
    assert_eq!(
        build(ModelQueryV2::Rows {
            cardinality: QueryRowCardinalityV2::BoundedMany,
            hydration: hydration(),
            order: Some(hidden_collection_term),
            output: rich_output(),
            window: QueryWindowV2::new(0, 10),
        })
        .expect_err("row order through collection slot")
        .code()
        .as_str(),
        "query_plan_v2_order_term_not_exposed",
    );

    let collect_only = QueryModelOutputV2::Positional {
        slots: vec![QueryModelOutputSlotV2::Collect {
            binding: binding_id(1),
            declared: type_id(TypeKind::Relation, "employment"),
            distinct: false,
            order: stable_order(employment_code(1), 1),
        }],
    };
    assert_eq!(
        build(ModelQueryV2::Rows {
            cardinality: QueryRowCardinalityV2::ExactlyOne,
            hydration: hydration(),
            order: None,
            output: collect_only,
            window: QueryWindowV2::new(0, 1),
        })
        .expect_err("row without singular selected identity")
        .code()
        .as_str(),
        "query_plan_v2_missing_selected_identity",
    );
}

#[test]
fn inherited_order_fields_use_closed_multi_owner_authority() {
    let declared = type_id(TypeKind::Entity, "person");
    let concrete = type_id(TypeKind::Entity, "vip-person");
    let owner_a = type_id(TypeKind::Entity, "base-a");
    let owner_b = type_id(TypeKind::Entity, "base-b");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            declared.clone(),
            vec![concrete.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            concrete,
            vec![HydrationFieldV2::new(
                "name",
                vec![owner_a.clone(), owner_b.clone()],
                AttributeId::new("name").expect("attribute"),
                ValueTypeTag::String,
                Cardinality::new(1, Some(1)).expect("cardinality"),
                false,
                false,
                true,
            )],
            Vec::new(),
        )],
    );
    let (bindings, pipeline, output, managed) = base_parts();
    let build = |owner: TypeId| {
        QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            QueryPlanV2Compatibility::new(
                None,
                Vec::new(),
                Some(ModelQueryV2::Rows {
                    cardinality: QueryRowCardinalityV2::BoundedMany,
                    hydration: hydration.clone(),
                    order: Some(stable_order(
                        QueryFieldV2::new(
                            binding_id(0),
                            owner,
                            AttributeId::new("name").expect("attribute"),
                            ValueTypeTag::String,
                        ),
                        0,
                    )),
                    output: QueryModelOutputV2::Positional {
                        slots: vec![QueryModelOutputSlotV2::One {
                            binding: binding_id(0),
                            declared: declared.clone(),
                        }],
                    },
                    window: QueryWindowV2::new(0, 5),
                }),
            ),
            managed.clone(),
        )
    };

    build(owner_a).expect("first inherited owner");
    build(owner_b).expect("second inherited owner");
    assert_eq!(
        build(type_id(TypeKind::Entity, "forged-owner"))
            .expect_err("owner outside the reference authority")
            .code()
            .as_str(),
        "query_plan_v2_order_field_claim",
    );
}

#[test]
fn subtype_predicates_filter_inapplicable_concretes_but_exact_domains_reject() {
    let declared = type_id(TypeKind::Entity, "person");
    let applicable = type_id(TypeKind::Entity, "person-a");
    let shadowed = type_id(TypeKind::Entity, "person-b");
    let attribute = AttributeId::new("name").expect("attribute");
    let projection = |concretes: Vec<TypeId>, descriptors: Vec<HydrationDescriptorV2>| {
        HydrationProjectionV2::new(
            vec![HydrationBindingV2::new(
                binding_id(0),
                declared.clone(),
                concretes,
            )],
            descriptors,
        )
    };
    let applicable_descriptor = HydrationDescriptorV2::new(
        applicable.clone(),
        vec![HydrationFieldV2::new(
            "name",
            vec![declared.clone()],
            attribute.clone(),
            ValueTypeTag::String,
            Cardinality::new(0, Some(1)).expect("cardinality"),
            false,
            false,
            false,
        )],
        Vec::new(),
    );
    let shadowed_descriptor = HydrationDescriptorV2::new(
        shadowed.clone(),
        vec![HydrationFieldV2::new(
            "name",
            vec![shadowed.clone()],
            attribute.clone(),
            ValueTypeTag::String,
            Cardinality::new(0, Some(1)).expect("cardinality"),
            false,
            false,
            false,
        )],
        Vec::new(),
    );
    let (bindings, pipeline, output, managed) = base_parts();
    let build = |hydration| {
        QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            QueryPlanV2Compatibility::new(
                Some(QueryPatternV2::FieldValue {
                    field: QueryFieldV2::new(
                        binding_id(0),
                        declared.clone(),
                        attribute.clone(),
                        ValueTypeTag::String,
                    ),
                    comparator: QueryComparatorV2::Equal,
                    value: CanonicalValue::String(CanonicalString::new("Alice").expect("string"))
                        .into(),
                }),
                Vec::new(),
                Some(ModelQueryV2::Rows {
                    cardinality: QueryRowCardinalityV2::ExactlyOne,
                    hydration,
                    order: None,
                    output: QueryModelOutputV2::Positional {
                        slots: vec![QueryModelOutputSlotV2::One {
                            binding: binding_id(0),
                            declared: declared.clone(),
                        }],
                    },
                    window: QueryWindowV2::new(0, 1),
                }),
            ),
            managed.clone(),
        )
    };

    build(projection(
        vec![applicable.clone(), shadowed.clone()],
        vec![applicable_descriptor, shadowed_descriptor.clone()],
    ))
    .expect("subtype predicate filters a shadowed child");
    assert_eq!(
        build(projection(vec![shadowed], vec![shadowed_descriptor]))
            .expect_err("singleton inapplicable domain")
            .code()
            .as_str(),
        "query_plan_v2_field_authority",
    );
}

#[test]
fn inherited_role_edges_use_closed_effective_reference_authority() {
    let person = type_id(TypeKind::Entity, "person");
    let declared_relation = type_id(TypeKind::Relation, "employment-child");
    let concrete_relation = type_id(TypeKind::Relation, "employment-leaf");
    let filtered_relation = type_id(TypeKind::Relation, "employment-leaf-z");
    let role_a = RoleId::new("ancestor-a", "employee").expect("role");
    let role_b = RoleId::new("ancestor-b", "employee").expect("role");
    let concrete_role = RoleId::new("employment-leaf", "employee").expect("role");
    let hydration = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(
                binding_id(1),
                declared_relation.clone(),
                vec![concrete_relation.clone(), filtered_relation.clone()],
            ),
        ],
        vec![
            HydrationDescriptorV2::new(person.clone(), Vec::new(), Vec::new()),
            HydrationDescriptorV2::new(
                concrete_relation,
                Vec::new(),
                vec![HydrationRoleV2::new(
                    concrete_role.clone(),
                    vec![role_a.clone(), role_b.clone(), concrete_role.clone()],
                    vec![HydrationPlayerV2::new(person.clone(), vec![person.clone()])],
                    Cardinality::new(1, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
            HydrationDescriptorV2::new(filtered_relation.clone(), Vec::new(), Vec::new()),
        ],
    );
    let (bindings, pipeline, output, managed) = base_parts();
    let build = |role: RoleId, hydration: HydrationProjectionV2| {
        QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            QueryPlanV2Compatibility::new(
                Some(QueryPatternV2::RoleEdge {
                    include_relation_subtypes: true,
                    player: binding_id(0),
                    relation: binding_id(1),
                    relation_type: declared_relation.clone(),
                    role,
                }),
                Vec::new(),
                Some(ModelQueryV2::Rows {
                    cardinality: QueryRowCardinalityV2::ExactlyOne,
                    hydration,
                    order: None,
                    output: QueryModelOutputV2::Positional {
                        slots: vec![QueryModelOutputSlotV2::One {
                            binding: binding_id(0),
                            declared: person.clone(),
                        }],
                    },
                    window: QueryWindowV2::new(0, 1),
                }),
            ),
            managed.clone(),
        )
    };

    build(role_a.clone(), hydration.clone()).expect("first ancestor role reference");
    build(role_b.clone(), hydration.clone()).expect("second ancestor role reference");
    build(concrete_role, hydration.clone()).expect("concrete role reference");
    for rejected in [
        RoleId::new("ancestor-c", "employee").expect("role"),
        RoleId::new("overridden-parent", "employee").expect("role"),
    ] {
        assert_eq!(
            build(rejected, hydration.clone())
                .expect_err("unadmitted or overridden role reference")
                .code()
                .as_str(),
            "query_plan_v2_role_edge_authority",
        );
    }

    let exact_filtered = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(
                binding_id(1),
                declared_relation.clone(),
                vec![filtered_relation.clone()],
            ),
        ],
        vec![
            HydrationDescriptorV2::new(person.clone(), Vec::new(), Vec::new()),
            HydrationDescriptorV2::new(filtered_relation, Vec::new(), Vec::new()),
        ],
    );
    assert_eq!(
        build(role_a, exact_filtered)
            .expect_err("singleton relation without the role")
            .code()
            .as_str(),
        "query_plan_v2_role_edge_authority",
    );
}

#[test]
fn role_player_authority_uses_concrete_subtype_overlap_and_rejects_disjoint_bindings() {
    let person = type_id(TypeKind::Entity, "person");
    let employee = type_id(TypeKind::Entity, "employee");
    let contractor = type_id(TypeKind::Entity, "contractor");
    let employment = type_id(TypeKind::Relation, "employment");
    let role = RoleId::new("employment", "employee").expect("role");
    let build = |declared_player: TypeId, concrete_player: TypeId| {
        let mut descriptors = vec![
            HydrationDescriptorV2::new(employee.clone(), Vec::new(), Vec::new()),
            HydrationDescriptorV2::new(person.clone(), Vec::new(), Vec::new()),
            HydrationDescriptorV2::new(
                employment.clone(),
                Vec::new(),
                vec![HydrationRoleV2::new(
                    role.clone(),
                    vec![role.clone()],
                    vec![HydrationPlayerV2::new(
                        person.clone(),
                        vec![employee.clone(), person.clone()],
                    )],
                    Cardinality::new(1, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
        ];
        if concrete_player == contractor {
            descriptors.push(HydrationDescriptorV2::new(
                contractor.clone(),
                Vec::new(),
                Vec::new(),
            ));
        }
        descriptors.sort_by(|left, right| left.descriptor().cmp(right.descriptor()));
        let hydration = HydrationProjectionV2::new(
            vec![
                HydrationBindingV2::new(
                    binding_id(0),
                    declared_player.clone(),
                    vec![concrete_player],
                ),
                HydrationBindingV2::new(
                    binding_id(1),
                    employment.clone(),
                    vec![employment.clone()],
                ),
            ],
            descriptors,
        );
        QueryPlan::new_v2_with_functions(
            vec![binding(0, "player"), binding(1, "employment")],
            Vec::new(),
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: false,
                        type_id: declared_player.clone(),
                    },
                    QueryPattern::Isa {
                        binding: binding_id(1),
                        include_subtypes: false,
                        type_id: employment.clone(),
                    },
                ],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            QueryPlanV2Compatibility::new(
                Some(QueryPatternV2::RoleEdge {
                    include_relation_subtypes: false,
                    player: binding_id(0),
                    relation: binding_id(1),
                    relation_type: employment.clone(),
                    role: role.clone(),
                }),
                Vec::new(),
                Some(ModelQueryV2::Rows {
                    cardinality: QueryRowCardinalityV2::ExactlyOne,
                    hydration,
                    order: None,
                    output: QueryModelOutputV2::Positional {
                        slots: vec![QueryModelOutputSlotV2::One {
                            binding: binding_id(0),
                            declared: declared_player,
                        }],
                    },
                    window: QueryWindowV2::new(0, 1),
                }),
            ),
            semantics(),
        )
    };

    build(employee.clone(), employee.clone())
        .expect("an exact subtype binding intersects its base-declared role authority");
    let error = build(contractor.clone(), contractor.clone())
        .expect_err("a disjoint concrete binding must not forge role-player authority");
    assert_eq!(error.code().as_str(), "query_plan_v2_role_player_authority");
    assert_eq!(
        error.message(),
        "role player binding has no applicable declared-to-concrete role authority"
    );
}

#[test]
fn role_player_only_descriptors_are_reachable_hydration_authority() {
    let person = type_id(TypeKind::Entity, "person");
    let employment = type_id(TypeKind::Relation, "employment");
    let role = RoleId::new("employment", "employee").expect("role");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(1),
            employment.clone(),
            vec![employment.clone()],
        )],
        vec![
            HydrationDescriptorV2::new(person.clone(), Vec::new(), Vec::new()),
            HydrationDescriptorV2::new(
                employment.clone(),
                Vec::new(),
                vec![HydrationRoleV2::new(
                    role.clone(),
                    vec![role],
                    vec![HydrationPlayerV2::new(person.clone(), vec![person])],
                    Cardinality::new(0, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
        ],
    );
    let (bindings, pipeline, output, managed) = base_parts();
    QueryPlan::new_v2_with_functions(
        bindings,
        Vec::new(),
        Vec::new(),
        pipeline,
        output,
        QueryPlanV2Compatibility::new(
            None,
            Vec::new(),
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(1),
                        declared: employment,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        managed,
    )
    .expect("role-only player descriptors are reachable from relation authority");
}

#[test]
fn relation_player_hydration_is_shallow_unless_independently_bound() {
    let relation_a = type_id(TypeKind::Relation, "relation-a");
    let relation_b = type_id(TypeKind::Relation, "relation-b");
    let relation_c = type_id(TypeKind::Relation, "relation-c");
    let role_a = RoleId::new("relation-a", "next").expect("role");
    let role_b = RoleId::new("relation-b", "next").expect("role");
    let descriptor_a = HydrationDescriptorV2::new(
        relation_a.clone(),
        Vec::new(),
        vec![HydrationRoleV2::new(
            role_a.clone(),
            vec![role_a],
            vec![HydrationPlayerV2::new(
                relation_b.clone(),
                vec![relation_b.clone()],
            )],
            Cardinality::new(0, None).expect("cardinality"),
            false,
            false,
        )],
    );
    let descriptor_b = HydrationDescriptorV2::new(
        relation_b.clone(),
        Vec::new(),
        vec![HydrationRoleV2::new(
            role_b.clone(),
            vec![role_b],
            vec![HydrationPlayerV2::new(
                relation_c.clone(),
                vec![relation_c.clone()],
            )],
            Cardinality::new(0, None).expect("cardinality"),
            false,
            false,
        )],
    );
    let descriptor_c = HydrationDescriptorV2::new(relation_c.clone(), Vec::new(), Vec::new());
    let build = |bindings: Vec<HydrationBindingV2>, descriptors: Vec<HydrationDescriptorV2>| {
        QueryPlan::new_v2_with_functions(
            vec![binding(0, "a"), binding(1, "b")],
            Vec::new(),
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: false,
                        type_id: relation_a.clone(),
                    },
                    QueryPattern::Isa {
                        binding: binding_id(1),
                        include_subtypes: false,
                        type_id: relation_b.clone(),
                    },
                ],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            QueryPlanV2Compatibility::new(
                None,
                Vec::new(),
                Some(ModelQueryV2::DistinctCount {
                    hydration: HydrationProjectionV2::new(bindings, descriptors),
                    root: binding_id(0),
                }),
            ),
            semantics(),
        )
    };

    build(
        vec![HydrationBindingV2::new(
            binding_id(0),
            relation_a.clone(),
            vec![relation_a.clone()],
        )],
        vec![descriptor_a.clone(), descriptor_b.clone()],
    )
    .expect("A binding hydrates B shallowly without requiring C");
    build(
        vec![
            HydrationBindingV2::new(binding_id(0), relation_a.clone(), vec![relation_a.clone()]),
            HydrationBindingV2::new(binding_id(1), relation_b.clone(), vec![relation_b.clone()]),
        ],
        vec![descriptor_a, descriptor_b, descriptor_c],
    )
    .expect("independently bound B admits its direct C player");
}

#[test]
fn provably_empty_binding_and_player_domains_remain_representable() {
    let abstract_person = type_id(TypeKind::Entity, "abstract-person");
    let bindings = vec![binding(0, "person")];
    let pipeline = vec![ReadStage::Match {
        patterns: vec![QueryPattern::Isa {
            binding: binding_id(0),
            include_subtypes: false,
            type_id: abstract_person.clone(),
        }],
    }];
    let output = QueryOutput::Rows {
        columns: vec![binding_id(0)],
    };
    let empty = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            abstract_person.clone(),
            Vec::new(),
        )],
        Vec::new(),
    );
    for model_query in [
        ModelQueryV2::Rows {
            cardinality: QueryRowCardinalityV2::ExactlyOne,
            hydration: empty.clone(),
            order: None,
            output: QueryModelOutputV2::Positional {
                slots: vec![QueryModelOutputSlotV2::One {
                    binding: binding_id(0),
                    declared: abstract_person.clone(),
                }],
            },
            window: QueryWindowV2::new(0, 1),
        },
        ModelQueryV2::DistinctCount {
            hydration: empty.clone(),
            root: binding_id(0),
        },
        ModelQueryV2::DistinctExists {
            hydration: empty,
            root: binding_id(0),
        },
    ] {
        let plan = QueryPlan::new_v2_with_functions(
            bindings.clone(),
            Vec::new(),
            Vec::new(),
            pipeline.clone(),
            output.clone(),
            QueryPlanV2Compatibility::new(None, Vec::new(), Some(model_query)),
            semantics(),
        )
        .expect("provably empty model query");
        let bytes = plan.canonical_bytes().expect("canonical bytes");
        assert_eq!(decode_query_plan(&bytes).expect("round trip"), plan);
    }

    let relation = type_id(TypeKind::Relation, "employment");
    let role = RoleId::new("employment", "employee").expect("role");
    let player_hydration = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), abstract_person.clone(), Vec::new()),
            HydrationBindingV2::new(binding_id(1), relation.clone(), vec![relation.clone()]),
        ],
        vec![HydrationDescriptorV2::new(
            relation.clone(),
            Vec::new(),
            vec![
                HydrationRoleV2::new(
                    role.clone(),
                    vec![role.clone()],
                    vec![HydrationPlayerV2::new(abstract_person.clone(), Vec::new())],
                    Cardinality::new(0, None).expect("cardinality"),
                    false,
                    false,
                ),
                HydrationRoleV2::new(
                    RoleId::new("employment", "vacant").expect("role"),
                    vec![RoleId::new("employment", "vacant").expect("role")],
                    Vec::new(),
                    Cardinality::new(0, None).expect("cardinality"),
                    false,
                    false,
                ),
            ],
        )],
    );
    let plan = QueryPlan::new_v2_with_functions(
        vec![binding(0, "person"), binding(1, "employment")],
        Vec::new(),
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: false,
                    type_id: abstract_person.clone(),
                },
                QueryPattern::Isa {
                    binding: binding_id(1),
                    include_subtypes: false,
                    type_id: relation.clone(),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
        QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::RoleEdge {
                include_relation_subtypes: false,
                player: binding_id(0),
                relation: binding_id(1),
                relation_type: relation.clone(),
                role,
            }),
            Vec::new(),
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration: player_hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(1),
                        declared: relation,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        semantics(),
    )
    .expect("abstract role player and no-player role");
    assert_eq!(
        decode_query_plan(&plan.canonical_bytes().expect("canonical bytes")).expect("round trip"),
        plan
    );
}

#[test]
fn relation_role_players_keep_scalar_and_edge_authority() {
    let outer = type_id(TypeKind::Relation, "employment");
    let player = type_id(TypeKind::Relation, "assignment");
    let role = RoleId::new("employment", "assignment").expect("role");
    let player_code = QueryFieldV2::new(
        binding_id(1),
        player.clone(),
        AttributeId::new("code").expect("attribute"),
        ValueTypeTag::String,
    );
    let hydration = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), outer.clone(), vec![outer.clone()]),
            HydrationBindingV2::new(binding_id(1), player.clone(), vec![player.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(
                player.clone(),
                vec![HydrationFieldV2::new(
                    "code",
                    vec![player.clone()],
                    AttributeId::new("code").expect("attribute"),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                )],
                Vec::new(),
            ),
            HydrationDescriptorV2::new(
                outer.clone(),
                Vec::new(),
                vec![HydrationRoleV2::new(
                    role.clone(),
                    vec![role.clone()],
                    vec![HydrationPlayerV2::new(player.clone(), vec![player.clone()])],
                    Cardinality::new(0, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
        ],
    );
    let bindings = vec![binding(0, "employment"), binding(1, "assignment")];
    let pipeline = vec![ReadStage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: outer.clone(),
            },
            QueryPattern::Isa {
                binding: binding_id(1),
                include_subtypes: false,
                type_id: player.clone(),
            },
        ],
    }];
    let plan = QueryPlan::new_v2_with_functions(
        bindings,
        Vec::new(),
        Vec::new(),
        pipeline,
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        QueryPlanV2Compatibility::new(
            Some(QueryPatternV2::And {
                patterns: vec![
                    QueryPatternV2::FieldValue {
                        field: player_code,
                        comparator: QueryComparatorV2::Equal,
                        value: CanonicalValue::String(
                            CanonicalString::new("A-1").expect("canonical string"),
                        )
                        .into(),
                    },
                    QueryPatternV2::RoleEdge {
                        include_relation_subtypes: false,
                        player: binding_id(1),
                        relation: binding_id(0),
                        relation_type: outer,
                        role,
                    },
                ],
            }),
            Vec::new(),
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(1),
                        declared: player,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        semantics(),
    )
    .expect("relation role player with scalar field authority");
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("round trip"), plan);

    let mut mixed_kind: Value = serde_json::from_slice(&bytes).expect("JSON");
    mixed_kind["compatibility"]["model_query"]["hydration"]["descriptors"][1]["roles"][0]["players"]
        [0]["declared_descriptor"]["kind"] = json!("entity");
    assert_eq!(
        decode_query_plan(&to_canonical_json(&mixed_kind).expect("canonical JSON"))
            .expect_err("mixed-kind role player authority")
            .code()
            .as_str(),
        "query_plan_v2_role_player_concretes_not_canonical",
    );
}

#[test]
fn compatibility_values_preserve_released_lexicals_and_fail_closed() {
    let marker = "\"; delete $x; # \\\n";
    let large = format!("{}{}", "x".repeat(MAX_CANONICAL_STRING_BYTES + 1), marker);
    let released =
        CompatibilityValueV2::released_string(large.clone()).expect("released long string");
    assert_eq!(released.released_text().as_deref(), Some(large.as_str()));
    assert_eq!(released.released_chunks().expect("chunks").len(), 2);
    let plan = compatibility_value_plan(released.clone(), ValueTypeTag::String, "name");
    let bytes = plan
        .canonical_bytes()
        .expect("bounded released string plan");
    assert_eq!(decode_query_plan(&bytes).expect("round trip"), plan);

    let mut noncanonical: Value = serde_json::from_slice(&bytes).expect("JSON");
    let chunks = noncanonical["compatibility"]["predicate"]["value"]["chunks"]
        .as_array_mut()
        .expect("chunks");
    let mut first = chunks[0].as_str().expect("first").to_owned();
    let moved = first.pop().expect("byte");
    let second = format!("{moved}{}", chunks[1].as_str().expect("second"));
    chunks[0] = json!(first);
    chunks[1] = json!(second);
    assert!(
        decode_query_plan(&to_canonical_json(&noncanonical).expect("canonical JSON")).is_err(),
        "alternate chunk boundaries must fail closed"
    );

    for (value, value_type, attribute) in [
        (
            CompatibilityValueV2::released_datetime("2024-01-01T12:30")
                .expect("short released clock"),
            ValueTypeTag::DateTime,
            "local-time",
        ),
        (
            CompatibilityValueV2::released_datetime("2024-01-01T12:30:00.1234567891")
                .expect("released high precision"),
            ValueTypeTag::DateTime,
            "precise-time",
        ),
        (
            CompatibilityValueV2::released_datetime_tz("2024-01-01T12:30Z")
                .expect("short released zoned clock"),
            ValueTypeTag::DateTimeTz,
            "zoned-time",
        ),
        (
            CompatibilityValueV2::released_duration("P1Y").expect("released year duration"),
            ValueTypeTag::Duration,
            "year-duration",
        ),
        (
            CompatibilityValueV2::released_duration("PT1H").expect("released hour duration"),
            ValueTypeTag::Duration,
            "hour-duration",
        ),
        (
            CompatibilityValueV2::released_decimal("00123.4500dec")
                .expect("released driver decimal"),
            ValueTypeTag::Decimal,
            "decimal-value",
        ),
    ] {
        let plan = compatibility_value_plan(value, value_type, attribute);
        let bytes = plan.canonical_bytes().expect("released lexical plan");
        assert_eq!(decode_query_plan(&bytes).expect("round trip"), plan);
    }

    let short =
        CompatibilityValueV2::released_datetime("2024-01-01T12:30").expect("released datetime");
    let canonical = CompatibilityValueV2::canonical(CanonicalValue::DateTime(
        "2024-01-01T12:30:00"
            .parse::<CanonicalDateTime>()
            .expect("canonical datetime"),
    ));
    assert_eq!(
        short.semantic_cmp_same_domain(&canonical),
        Some(std::cmp::Ordering::Equal)
    );
    let year = CompatibilityValueV2::released_duration("P1Y").expect("year");
    let month = CompatibilityValueV2::canonical(CanonicalValue::Duration(
        "P1M".parse::<CanonicalDuration>().expect("month"),
    ));
    assert_eq!(
        year.semantic_cmp_same_domain(&month),
        Some(std::cmp::Ordering::Greater)
    );
    let released_decimal =
        CompatibilityValueV2::released_decimal("00123.4500dec").expect("released decimal");
    let canonical_decimal = CompatibilityValueV2::canonical(CanonicalValue::Decimal(
        type_bridge_contract::value::DecimalValue::new("123.45").expect("canonical decimal"),
    ));
    assert_eq!(
        released_decimal.semantic_cmp_same_domain(&canonical_decimal),
        Some(std::cmp::Ordering::Equal),
    );

    for invalid in [
        CompatibilityValueV2::released_datetime("2024-01-01T12:30; delete $x;"),
        CompatibilityValueV2::released_duration("P1Y; delete $x;"),
    ] {
        assert!(invalid.is_err(), "temporal markers must not enter the wire");
    }
    assert!(
        CompatibilityValueV2::released_datetime("2024-01-01T12:30:00").is_err(),
        "ordinary canonical values have exactly one representation"
    );
    assert!(
        CompatibilityValueV2::released_decimal("123.45").is_err(),
        "canonical decimal values have exactly one representation",
    );

    let ceiling = CompatibilityValueV2::released_string("x".repeat(MAX_CANONICAL_BYTES))
        .expect("raw ceiling");
    let ceiling_plan = compatibility_value_plan(ceiling, ValueTypeTag::String, "ceiling");
    assert!(
        ceiling_plan.canonical_bytes().is_err(),
        "plan framing overhead must enforce the 16 MiB artifact ceiling"
    );
    assert!(
        CompatibilityValueV2::released_string("x".repeat(MAX_CANONICAL_BYTES + 1)).is_err(),
        "a compatibility scalar cannot exceed its owning artifact ceiling"
    );
}

#[test]
fn forged_released_datetime_with_out_of_range_seconds_fails_decode() {
    let plan = compatibility_value_plan(
        CompatibilityValueV2::released_datetime("2024-01-01T12:34")
            .expect("released two-component clock"),
        ValueTypeTag::DateTime,
        "local-time",
    );
    let mut wire: Value =
        serde_json::from_slice(&plan.canonical_bytes().expect("canonical plan")).expect("JSON");
    wire["compatibility"]["predicate"]["value"]["chunks"] = json!(["2024-01-01T12:34:99"]);
    let hostile = to_canonical_json(&wire).expect("canonical hostile wire");
    assert!(
        decode_query_plan(&hostile).is_err(),
        "untrusted V2 wire must not claim a released datetime V1 rejects",
    );
}

#[test]
fn v2_capability_vocabulary_is_closed_and_deterministic() {
    let vocabulary = query_plan_v2_capability_vocabulary();
    let actual = vocabulary
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual, {
        let mut expected = vec![
            "query.execution.batch-identity-rebind",
            "query.execution.same-snapshot-hydration",
            "query.function.local",
            "query.input.columns",
            "query.operation.distinct-count",
            "query.operation.distinct-exists",
            "query.operation.exactly-one",
            "query.operation.page",
            "query.order.stable-collection",
            "query.order.stable-root",
            "query.order.stable-selected",
            "query.output.collect",
            "query.output.collect-distinct",
            "query.output.documents",
            "query.output.hydrated",
            "query.output.named",
            "query.output.rows",
            "query.pattern.disjunction",
            "query.pattern.function-call",
            "query.pattern.has",
            "query.pattern.isa",
            "query.pattern.isa-subtypes",
            "query.pattern.links",
            "query.pattern.links-subtypes",
            "query.pattern.negation",
            "query.pattern.reachable",
            "query.pattern.string-operators",
            "query.pattern.try",
            "query.pattern.value",
            "query.plan",
            "query.plan.v2",
            "query.stage.distinct",
            "query.stage.limit",
            "query.stage.offset",
            "query.stage.reduce",
            "query.stage.require",
            "query.stage.select",
            "query.stage.sort",
            "query.topology.cross-join",
        ];
        expected.sort_unstable();
        expected
    });
}

#[test]
fn v2_rich_plan_matches_independent_byte_golden() {
    let plan = rich_plan();
    let bytes = plan.canonical_bytes().expect("V2 bytes");
    let fixture = include_bytes!("fixtures/query-plan-v2-rich.json");
    let fixture = fixture.strip_suffix(b"\n").unwrap_or(fixture);
    assert_eq!(bytes, fixture, "V2 canonical bytes changed",);
    assert_eq!(
        plan.fingerprint()
            .expect("fingerprint")
            .as_fingerprint()
            .digest()
            .to_hex(),
        "7aa23d2534766c4b700b938fa3c1caed47e85448e15d2778de49002991ad0c3b",
    );
}
