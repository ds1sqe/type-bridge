use serde_json::{Value, json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm, QueryInvocation, QueryOperand,
    QueryOperation, QueryOutput, QueryPattern, QueryPlan, ReadStage, decode_query_invocation,
    decode_query_plan,
};
use type_bridge_contract::query_plan_capability_vocabulary;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

fn managed_semantics(seed: &[u8]) -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        seed,
    )
    .expect("managed semantic fingerprint")
}

fn person_isa(binding: u16) -> QueryPattern {
    QueryPattern::Isa {
        binding: binding_id(binding),
        include_subtypes: true,
        type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
    }
}

fn full_pipeline_plan() -> QueryPlan {
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("minimum_age").expect("input name"),
            ValueTypeTag::Long,
            false,
        )],
        vec![
            ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("age").expect("attribute id"),
                        owner: binding_id(0),
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::GreaterOrEqual,
                        left: QueryOperand::Binding {
                            binding: binding_id(1),
                        },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Distinct,
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Descending)],
            },
            ReadStage::Offset { rows: 10 },
            ReadStage::Limit { rows: 5 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(b"query-plan-managed-fixture"),
    )
    .expect("full pipeline plan")
}

#[test]
fn query_plan_capability_vocabulary_is_exact_and_deterministic() {
    let vocabulary = query_plan_capability_vocabulary();
    assert_eq!(vocabulary, query_plan_capability_vocabulary());
    assert_eq!(
        vocabulary
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>(),
        vec![
            "query.function.local",
            "query.input.columns",
            "query.output.documents",
            "query.output.rows",
            "query.pattern.function-call",
            "query.pattern.has",
            "query.pattern.isa",
            "query.pattern.isa-subtypes",
            "query.pattern.links",
            "query.pattern.negation",
            "query.pattern.reachable",
            "query.pattern.try",
            "query.pattern.value",
            "query.plan",
            "query.stage.distinct",
            "query.stage.limit",
            "query.stage.offset",
            "query.stage.reduce",
            "query.stage.require",
            "query.stage.select",
            "query.stage.sort",
        ]
    );
}

#[test]
fn canonical_bytes_round_trip_and_bind_the_fingerprint() {
    let plan = full_pipeline_plan();
    assert_eq!(
        plan.required_capabilities()
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>(),
        vec![
            "query.input.columns",
            "query.output.rows",
            "query.pattern.has",
            "query.pattern.isa",
            "query.pattern.isa-subtypes",
            "query.pattern.value",
            "query.plan",
            "query.stage.distinct",
            "query.stage.limit",
            "query.stage.offset",
            "query.stage.select",
            "query.stage.sort",
        ],
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(bytes, plan.canonical_bytes().expect("repeat bytes"));
    let decoded = decode_query_plan(&bytes).expect("decode");
    assert_eq!(decoded, plan);
    assert_eq!(
        plan.fingerprint().expect("fingerprint"),
        decoded.fingerprint().expect("decoded fingerprint"),
    );

    // A different input value type is a different plan identity.
    let retyped = QueryPlan::new(
        plan.bindings().to_vec(),
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("minimum_age").expect("input name"),
            ValueTypeTag::Double,
            false,
        )],
        plan.pipeline().to_vec(),
        plan.output().clone(),
        plan.managed_semantics().clone(),
    )
    .expect("retyped plan");
    assert_ne!(
        plan.fingerprint().expect("fingerprint"),
        retyped.fingerprint().expect("retyped fingerprint"),
    );
}

#[test]
fn malformed_forged_and_unknown_wire_bytes_fail_closed() {
    let bytes = full_pipeline_plan()
        .canonical_bytes()
        .expect("canonical bytes");

    let mut forged: Value = serde_json::from_slice(&bytes).expect("JSON");
    forged["required_capabilities"] = json!(["query.future", "query.plan"]);
    assert_eq!(
        decode_query_plan(&serde_json::to_vec(&forged).expect("JSON"))
            .expect_err("forged capabilities")
            .code()
            .as_str(),
        "query_plan_capability_claim_mismatch",
    );

    let mut unknown: Value = serde_json::from_slice(&bytes).expect("JSON");
    unknown["pipeline"][0]["patterns"][0]["future"] = json!(true);
    assert!(decode_query_plan(&serde_json::to_vec(&unknown).expect("JSON")).is_err());

    let mut wrong_format: Value = serde_json::from_slice(&bytes).expect("JSON");
    wrong_format["format"] = json!("typebridge.query-plan/v99");
    assert_eq!(
        decode_query_plan(&serde_json::to_vec(&wrong_format).expect("JSON"))
            .expect_err("unsupported format")
            .code()
            .as_str(),
        "query_plan_format_unsupported",
    );
}

#[test]
fn pipeline_shape_rules_fail_closed() {
    let semantics = managed_semantics(b"query-plan-shape-fixture");
    let output = QueryOutput::Rows {
        columns: vec![binding_id(0)],
    };
    let build = |pipeline: Vec<ReadStage>| {
        QueryPlan::new(
            vec![binding(0, "person")],
            Vec::new(),
            pipeline,
            output.clone(),
            semantics.clone(),
        )
    };

    assert_eq!(
        build(vec![ReadStage::Distinct])
            .expect_err("match must open the pipeline")
            .code()
            .as_str(),
        "query_plan_match_not_first",
    );
    assert_eq!(
        build(vec![
            ReadStage::Match {
                patterns: vec![person_isa(0)]
            },
            ReadStage::Distinct,
            ReadStage::Select {
                bindings: vec![binding_id(0)]
            },
        ])
        .expect_err("stages follow the canonical order")
        .code()
        .as_str(),
        "query_plan_stage_order",
    );
    assert_eq!(
        build(vec![
            ReadStage::Match {
                patterns: vec![person_isa(0)]
            },
            ReadStage::Limit { rows: 3 },
        ])
        .expect_err("truncation requires an explicit order")
        .code()
        .as_str(),
        "query_plan_unordered_truncation",
    );
    assert_eq!(
        build(vec![
            ReadStage::Match {
                patterns: vec![person_isa(0)]
            },
            ReadStage::Select {
                bindings: vec![binding_id(1)]
            },
        ])
        .expect_err("select admits only declared bindings")
        .code()
        .as_str(),
        "query_plan_stage_unknown_binding",
    );

    // Output must stay inside the selected visible environment.
    let hidden = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("age").expect("attribute id"),
                        owner: binding_id(0),
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
        semantics.clone(),
    );
    assert_eq!(
        hidden
            .expect_err("output projects only visible bindings")
            .code()
            .as_str(),
        "query_plan_output_not_visible",
    );

    // Input references are validated against the declaration set.
    let orphan_input = QueryPlan::new(
        vec![binding(0, "person")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                    right: QueryOperand::Literal {
                        value: CanonicalValue::Long(1),
                    },
                },
            ],
        }],
        output,
        semantics,
    );
    assert_eq!(
        orphan_input
            .expect_err("input operands require a declaration")
            .code()
            .as_str(),
        "query_plan_unknown_input_column",
    );
}

#[test]
fn invocations_bind_the_exact_plan_and_validate_rectangular_batches() {
    use type_bridge_contract::query_plan::{InputRow, QueryInvocation, QueryOperation};

    let plan = full_pipeline_plan();
    let row = |value: i64| InputRow::new(vec![Some(CanonicalValue::Long(value))]);
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![row(18), row(65)])
        .expect("rectangular batch");
    assert!(invocation.binds(&plan).expect("binding check"));
    assert_eq!(invocation.inputs().len(), 2);
    assert_eq!(invocation.operation(), QueryOperation::Rows);

    // A plan edit invalidates the outstanding invocation.
    let edited = QueryPlan::new(
        plan.bindings().to_vec(),
        plan.inputs().to_vec(),
        vec![
            plan.pipeline()[0].clone(),
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
        ],
        plan.output().clone(),
        plan.managed_semantics().clone(),
    )
    .expect("edited plan");
    assert!(!invocation.binds(&edited).expect("edited binding check"));

    // Batch shape failures are named precisely.
    let wrong_arity = QueryInvocation::new(
        &plan,
        QueryOperation::Count,
        vec![InputRow::new(Vec::new())],
    )
    .expect_err("arity mismatch");
    assert_eq!(wrong_arity.code().as_str(), "query_invocation_row_arity");

    let wrong_type = QueryInvocation::new(
        &plan,
        QueryOperation::Exists,
        vec![InputRow::new(vec![Some(CanonicalValue::Boolean(true))])],
    )
    .expect_err("value type mismatch");
    assert_eq!(wrong_type.code().as_str(), "query_invocation_value_type");

    let missing_required =
        QueryInvocation::new(&plan, QueryOperation::Rows, vec![InputRow::new(vec![None])])
            .expect_err("required value missing");
    assert_eq!(
        missing_required.code().as_str(),
        "query_invocation_missing_value"
    );

    let empty = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new())
        .expect_err("declared inputs require a row");
    assert_eq!(empty.code().as_str(), "query_invocation_missing_inputs");
}

#[test]
fn canonical_invocation_wire_rebuilds_and_rejects_forgery_or_unknown_fields() {
    use type_bridge_contract::codec::to_canonical_json;

    let plan = full_pipeline_plan();
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::Long(18))])],
    )
    .expect("invocation");
    let bytes = to_canonical_json(&invocation).expect("canonical invocation");
    let decoded = decode_query_invocation(&plan, &bytes).expect("trusted reconstruction");
    assert_eq!(decoded, invocation);

    let mut forged = serde_json::from_slice::<Value>(&bytes).expect("invocation value");
    forged["plan_fingerprint"]["digest"] = json!("00".repeat(32));
    assert_eq!(
        decode_query_invocation(
            &plan,
            &to_canonical_json(&forged).expect("canonical forgery"),
        )
        .expect_err("forged plan binding")
        .code()
        .as_str(),
        "query_invocation_plan_fingerprint_mismatch",
    );

    let mut unknown = serde_json::from_slice::<Value>(&bytes).expect("invocation value");
    unknown["extra"] = json!(true);
    assert!(
        decode_query_invocation(
            &plan,
            &to_canonical_json(&unknown).expect("canonical unknown field"),
        )
        .is_err(),
        "unknown invocation fields fail closed",
    );
}

#[test]
fn complete_invocation_wire_ceiling_is_attainable_and_exact() {
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::limits::{MAX_INPUT_BYTES, MAX_QUERY_INVOCATION_BYTES};

    let source = full_pipeline_plan();
    let plan = QueryPlan::new(
        source.bindings().to_vec(),
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("supplied_text").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        source.pipeline().to_vec(),
        source.output().clone(),
        source.managed_semantics().clone(),
    )
    .expect("string-input plan");

    let base_chunk = "x".repeat((MAX_INPUT_BYTES / 5).saturating_sub(128));
    let mut chunks = vec![base_chunk; 5];
    let build_rows = |chunks: &[String]| {
        chunks
            .iter()
            .map(|chunk| {
                InputRow::new(vec![Some(CanonicalValue::String(
                    CanonicalString::new(chunk.clone()).expect("bounded string"),
                ))])
            })
            .collect::<Vec<_>>()
    };
    let initial_size = serde_json::to_vec(&build_rows(&chunks))
        .expect("input rows")
        .len();
    assert!(initial_size < MAX_INPUT_BYTES);
    chunks
        .last_mut()
        .expect("last input chunk")
        .push_str(&"x".repeat(MAX_INPUT_BYTES - initial_size));
    let rows = build_rows(&chunks);
    assert_eq!(
        serde_json::to_vec(&rows).expect("exact input rows").len(),
        MAX_INPUT_BYTES,
    );

    let invocation = QueryInvocation::new(&plan, QueryOperation::Exists, rows)
        .expect("maximum-size input batch");
    let bytes = to_canonical_json(&invocation).expect("maximum-size invocation");
    assert_eq!(bytes.len(), MAX_QUERY_INVOCATION_BYTES);
    assert_eq!(
        decode_query_invocation(&plan, &bytes).expect("maximum invocation round trip"),
        invocation,
    );
}

#[test]
fn function_calls_are_first_class_and_capability_gated() {
    use type_bridge_contract::id::FunctionId;

    let semantics = managed_semantics(b"query-plan-function-fixture");
    let call = QueryPattern::FunctionCall {
        arguments: vec![QueryOperand::Binding {
            binding: binding_id(0),
        }],
        assigned: binding_id(1),
        function: FunctionId::new("person_age").expect("function id"),
    };
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age_value")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![person_isa(0), call.clone()],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        semantics.clone(),
    )
    .expect("function-call plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.pattern.function-call"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decode"), plan);

    // Function calls stay out of negations in the first vocabulary.
    let nested = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age_value")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Not {
                    patterns: vec![call],
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        semantics,
    )
    .expect_err("negated calls are reserved");
    assert_eq!(nested.code().as_str(), "query_plan_function_in_negation");
}

#[test]
fn reduce_stages_group_assign_and_reject_unsound_shapes() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

    let has_age = QueryPattern::Has {
        attribute: binding_id(1),
        attribute_id: AttributeId::new("age").expect("attribute id"),
        owner: binding_id(0),
    };
    let reduce_plan =
        |assignments: Vec<ReduceAssignment>, groups: Vec<BindingId>, columns: Vec<BindingId>| {
            QueryPlan::new(
                vec![
                    binding(0, "person"),
                    binding(1, "age"),
                    binding(2, "age_sum"),
                ],
                Vec::new(),
                vec![
                    ReadStage::Match {
                        patterns: vec![person_isa(0), has_age.clone()],
                    },
                    ReadStage::Reduce {
                        assignments,
                        groups,
                    },
                ],
                QueryOutput::Rows { columns },
                managed_semantics(b"query-plan-reduce-fixture"),
            )
        };

    // A grouped sum round-trips and derives the reduce capability.
    let plan = reduce_plan(
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Sum,
            Some(binding_id(1)),
        )],
        vec![binding_id(0)],
        vec![binding_id(0), binding_id(2)],
    )
    .expect("grouped reduce plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.stage.reduce"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);

    // The reduced-away binding is no longer projectable.
    let error = reduce_plan(
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Sum,
            Some(binding_id(1)),
        )],
        vec![binding_id(0)],
        vec![binding_id(0), binding_id(1)],
    )
    .expect_err("projecting a reduced binding");
    assert_eq!(error.code().as_str(), "query_plan_output_not_visible");

    // A pattern-bound binding cannot receive a reducer result.
    let error = reduce_plan(
        vec![ReduceAssignment::new(
            binding_id(1),
            Reducer::Sum,
            Some(binding_id(1)),
        )],
        vec![binding_id(0)],
        vec![binding_id(0), binding_id(1)],
    )
    .expect_err("assigning onto a pattern binding");
    assert_eq!(error.code().as_str(), "query_plan_reduce_assigned_bound");

    // Reducers undefined on empty streams need group keys.
    let error = reduce_plan(
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Max,
            Some(binding_id(1)),
        )],
        Vec::new(),
        vec![binding_id(2)],
    )
    .expect_err("global max");
    assert_eq!(error.code().as_str(), "query_plan_reduce_requires_groups");

    // Every reducer except count consumes an input binding.
    let error = reduce_plan(
        vec![ReduceAssignment::new(binding_id(2), Reducer::Sum, None)],
        vec![binding_id(0)],
        vec![binding_id(0), binding_id(2)],
    )
    .expect_err("sum without input");
    assert_eq!(error.code().as_str(), "query_plan_reduce_missing_input");

    // A bare global count stays total and needs no input.
    reduce_plan(
        vec![ReduceAssignment::new(binding_id(2), Reducer::Count, None)],
        Vec::new(),
        vec![binding_id(2)],
    )
    .expect("global count plan");
}

#[test]
fn try_blocks_export_optional_bindings_and_reject_unsound_shapes() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

    let semantics = managed_semantics(b"query-plan-try-fixture");
    let has_age = |owner: u16, attribute: u16| QueryPattern::Has {
        attribute: binding_id(attribute),
        attribute_id: AttributeId::new("age").expect("attribute id"),
        owner: binding_id(owner),
    };
    let build = |pipeline: Vec<ReadStage>, columns: Vec<BindingId>| {
        QueryPlan::new(
            vec![
                binding(0, "person"),
                binding(1, "age"),
                binding(2, "result"),
            ],
            Vec::new(),
            pipeline,
            QueryOutput::Rows { columns },
            semantics.clone(),
        )
    };
    let try_match = ReadStage::Match {
        patterns: vec![
            person_isa(0),
            QueryPattern::Try {
                patterns: vec![has_age(0, 1)],
            },
        ],
    };

    // Optional bindings project and round-trip under the try capability.
    let plan = build(vec![try_match.clone()], vec![binding_id(0), binding_id(1)])
        .expect("optional projection plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.pattern.try"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);

    // Count and sum skip absence; they may consume optional bindings.
    build(
        vec![
            try_match.clone(),
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(2),
                    Reducer::Count,
                    Some(binding_id(1)),
                )],
                groups: vec![binding_id(0)],
            },
        ],
        vec![binding_id(0), binding_id(2)],
    )
    .expect("count over an optional binding");

    // Mean can observe a group whose optional input never matched.
    let error = build(
        vec![
            try_match.clone(),
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(2),
                    Reducer::Mean,
                    Some(binding_id(1)),
                )],
                groups: vec![binding_id(0)],
            },
        ],
        vec![binding_id(0), binding_id(2)],
    )
    .expect_err("mean over an optional binding");
    assert_eq!(error.code().as_str(), "query_plan_reduce_optional_input");

    // Absence has no defined sort position.
    let error = build(
        vec![
            try_match.clone(),
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        vec![binding_id(0)],
    )
    .expect_err("sorting an optional binding");
    assert_eq!(error.code().as_str(), "query_plan_stage_unknown_binding");

    // Requiring an optional binding is reserved (provider contract unproven).
    let error = build(
        vec![
            try_match.clone(),
            ReadStage::Require {
                bindings: vec![binding_id(1)],
            },
        ],
        vec![binding_id(0)],
    )
    .expect_err("requiring an optional binding");
    assert_eq!(
        error.code().as_str(),
        "query_plan_require_optional_reserved"
    );

    // Grouping keys stay mandatory.
    let error = build(
        vec![
            try_match.clone(),
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(binding_id(2), Reducer::Count, None)],
                groups: vec![binding_id(1)],
            },
        ],
        vec![binding_id(1), binding_id(2)],
    )
    .expect_err("grouping by an optional binding");
    assert_eq!(error.code().as_str(), "query_plan_stage_unknown_binding");

    // One optional binding cannot be claimed by two try bodies.
    let error = build(
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Try {
                    patterns: vec![has_age(0, 1)],
                },
                QueryPattern::Try {
                    patterns: vec![has_age(0, 1)],
                },
            ],
        }],
        vec![binding_id(0), binding_id(1)],
    )
    .expect_err("two try bodies sharing a local");
    assert_eq!(error.code().as_str(), "query_plan_try_binding_shared");

    // A negation-local witness cannot poison the mandatory environment of a
    // try export. In particular, absence has no sort position and therefore
    // cannot define a stable window boundary.
    let error = build(
        vec![
            ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::Not {
                        patterns: vec![has_age(0, 1)],
                    },
                    QueryPattern::Try {
                        patterns: vec![has_age(0, 1)],
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 1 },
        ],
        vec![binding_id(0)],
    )
    .expect_err("negation-local witness reused as a sorted try export");
    assert_eq!(error.code().as_str(), "query_plan_try_binding_shared");

    // The same poisoning must not hide duplicate ownership by two try bodies.
    let error = build(
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Not {
                    patterns: vec![has_age(0, 1)],
                },
                QueryPattern::Try {
                    patterns: vec![has_age(0, 1)],
                },
                QueryPattern::Try {
                    patterns: vec![has_age(0, 1)],
                },
            ],
        }],
        vec![binding_id(0)],
    )
    .expect_err("negation-local witness reused by two try bodies");
    assert_eq!(error.code().as_str(), "query_plan_try_binding_shared");

    // Try blocks stay in the root conjunction with a flat first vocabulary.
    let error = build(
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Not {
                    patterns: vec![QueryPattern::Try {
                        patterns: vec![has_age(0, 1)],
                    }],
                },
            ],
        }],
        vec![binding_id(0)],
    )
    .expect_err("try nested in a negation");
    assert_eq!(error.code().as_str(), "query_plan_try_not_root");

    let error = build(
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Try {
                    patterns: vec![QueryPattern::Not {
                        patterns: vec![has_age(0, 1)],
                    }],
                },
            ],
        }],
        vec![binding_id(0)],
    )
    .expect_err("negation nested in a try");
    assert_eq!(error.code().as_str(), "query_plan_try_body_unsupported");
}

#[test]
fn document_outputs_fetch_typed_fields_and_reject_unsound_shapes() {
    use type_bridge_contract::query_plan::{DocumentField, DocumentSource};

    let semantics = managed_semantics(b"query-plan-document-fixture");
    let key = |name: &str| QueryVariable::new(name).expect("document key");
    let build = |patterns: Vec<QueryPattern>, fields: Vec<DocumentField>| {
        QueryPlan::new(
            vec![binding(0, "person"), binding(1, "age")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Documents { fields },
            semantics.clone(),
        )
    };
    let has_age = QueryPattern::Has {
        attribute: binding_id(1),
        attribute_id: AttributeId::new("age").expect("attribute id"),
        owner: binding_id(0),
    };

    // Scalar and list fields round-trip under the documents capability.
    let plan = build(
        vec![person_isa(0), has_age.clone()],
        vec![
            DocumentField::new(
                key("age"),
                DocumentSource::Binding {
                    binding: binding_id(1),
                },
            ),
            DocumentField::new(
                key("names"),
                DocumentSource::AttributeList {
                    attribute: AttributeId::new("name").expect("attribute id"),
                    owner: binding_id(0),
                },
            ),
        ],
    )
    .expect("document plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.output.documents"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);

    // One key cannot be fetched twice.
    let error = build(
        vec![person_isa(0), has_age.clone()],
        vec![
            DocumentField::new(
                key("age"),
                DocumentSource::Binding {
                    binding: binding_id(1),
                },
            ),
            DocumentField::new(
                key("age"),
                DocumentSource::Binding {
                    binding: binding_id(1),
                },
            ),
        ],
    )
    .expect_err("duplicate document key");
    assert_eq!(error.code().as_str(), "query_plan_duplicate_output_column");

    // Attribute lists reach through mandatory owners only.
    let error = build(
        vec![
            person_isa(0),
            QueryPattern::Try {
                patterns: vec![has_age],
            },
        ],
        vec![DocumentField::new(
            key("names"),
            DocumentSource::AttributeList {
                attribute: AttributeId::new("name").expect("attribute id"),
                owner: binding_id(1),
            },
        )],
    )
    .expect_err("optional list owner");
    assert_eq!(error.code().as_str(), "query_plan_output_not_visible");
}

#[test]
fn local_functions_declare_total_reducers_and_reject_unsound_shapes() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{
        LocalFunction, LocalReturn, QueryOperand as Operand, Reducer,
    };

    let semantics = managed_semantics(b"query-plan-local-fn-fixture");
    let local_body = || {
        vec![QueryPattern::Has {
            attribute: binding_id(1),
            attribute_id: AttributeId::new("age").expect("attribute id"),
            owner: binding_id(0),
        }]
    };
    let local = |returns: LocalReturn| {
        LocalFunction::new(
            FunctionId::new("age_count_of").expect("function id"),
            vec![binding(0, "subject"), binding(1, "age")],
            vec![Label::new("person").expect("label")],
            local_body(),
            returns,
        )
    };
    let build = |functions: Vec<LocalFunction>| {
        QueryPlan::new_with_functions(
            vec![binding(0, "person"), binding(1, "age_count")],
            functions,
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::FunctionCall {
                        arguments: vec![Operand::Binding {
                            binding: binding_id(0),
                        }],
                        assigned: binding_id(1),
                        function: FunctionId::new("age_count_of").expect("function id"),
                    },
                ],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            semantics.clone(),
        )
    };

    // A total count declaration round-trips under the local capability.
    let plan = build(vec![local(LocalReturn::new(
        Reducer::Count,
        binding_id(1),
        ValueTypeTag::Long,
    ))])
    .expect("local function plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.function.local"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);

    // Reducers that can observe an empty body stream are reserved.
    let error = build(vec![local(LocalReturn::new(
        Reducer::Max,
        binding_id(1),
        ValueTypeTag::Long,
    ))])
    .expect_err("partial local reducer");
    assert_eq!(
        error.code().as_str(),
        "query_plan_local_function_return_partial",
    );

    // A count declares a long result, nothing else.
    let error = build(vec![local(LocalReturn::new(
        Reducer::Count,
        binding_id(1),
        ValueTypeTag::Double,
    ))])
    .expect_err("mistyped local return");
    assert_eq!(
        error.code().as_str(),
        "query_plan_local_function_return_type"
    );

    // Two local functions cannot share a name.
    let error = build(vec![
        local(LocalReturn::new(
            Reducer::Count,
            binding_id(1),
            ValueTypeTag::Long,
        )),
        local(LocalReturn::new(
            Reducer::Count,
            binding_id(1),
            ValueTypeTag::Long,
        )),
    ])
    .expect_err("duplicate local name");
    assert_eq!(error.code().as_str(), "query_plan_duplicate_local_function");
}

#[test]
fn bounded_reachability_requires_a_finite_root_bound() {
    use type_bridge_contract::id::RoleId;

    let semantics = managed_semantics(b"query-plan-reachable-fixture");
    let reachable = |max_depth: u8| QueryPattern::Reachable {
        min_depth: 1,
        max_depth,
        relation: TypeId::new(TypeKind::Relation, "edge").expect("type id"),
        role_from: RoleId::new("edge", "origin").expect("role"),
        role_to: RoleId::new("edge", "destination").expect("role"),
        source: binding_id(0),
        target: binding_id(1),
    };
    let build = |patterns: Vec<QueryPattern>| {
        QueryPlan::new(
            vec![binding(0, "source"), binding(1, "target")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            semantics.clone(),
        )
    };

    // A bounded pattern round-trips under the reachability capability.
    let plan = build(vec![reachable(3)]).expect("reachability plan");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.pattern.reachable"),
    );
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);

    // The bound is mandatory: zero hops is not a V1 reachability question.
    let error = build(vec![reachable(0)]).expect_err("zero bound");
    assert_eq!(error.code().as_str(), "query_plan_reachable_depth");

    // The bound stays within the structural depth ceiling.
    let error = build(vec![reachable(u8::MAX)]).expect_err("unbounded depth");
    assert_eq!(error.code().as_str(), "query_plan_reachable_depth");

    // Reachability stays in the root conjunction.
    let error = build(vec![
        person_isa(0),
        QueryPattern::Not {
            patterns: vec![reachable(2)],
        },
    ])
    .expect_err("reachability in a negation");
    assert_eq!(error.code().as_str(), "query_plan_reachable_not_root");

    // Expansion is charged, not the single spelled pattern: each bound of d
    // unrolls into d(d+1)/2 hop clauses, so a depth-64 pattern alone charges
    // 2,080 nodes and three of them cross the 4,096-node ceiling that a
    // pattern-count check would never see.
    let plan = build(vec![reachable(64)]).expect("one depth-64 pattern fits the node budget");
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decoded plan"), plan);
    let error = build(vec![reachable(64), reachable(64), reachable(64)])
        .expect_err("stacked deep reachability must exhaust the node budget");
    assert_eq!(
        error.code().as_str(),
        "query_plan_reachable_expansion_limit"
    );
}

#[test]
fn optional_input_columns_admit_only_typed_values_or_explicit_absence() {
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("maybe_age").expect("input name"),
            ValueTypeTag::Long,
            true,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("age").expect("attribute id"),
                    owner: binding_id(0),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::GreaterOrEqual,
                    left: QueryOperand::Binding {
                        binding: binding_id(1),
                    },
                    right: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed_semantics(b"optional-input-fixture"),
    )
    .expect("optional input plan");

    QueryInvocation::new(&plan, QueryOperation::Rows, vec![InputRow::new(vec![None])])
        .expect("an optional input admits explicit absence");
    QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::Long(18))])],
    )
    .expect("an optional input still admits its exact declared type");
    let error = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::Boolean(true))])],
    )
    .expect_err("optional does not weaken the declared scalar type");
    assert_eq!(error.code().as_str(), "query_invocation_value_type");
}

#[test]
fn caller_limits_recheck_the_whole_plan_structure() {
    use type_bridge_contract::limits::StructuralLimits;

    let plan = full_pipeline_plan();
    plan.check_structural_limits(StructuralLimits::CANONICAL)
        .expect("canonical limits accept the canonical plan");

    for (case, apply, code) in [
        (
            "boolean terms",
            (|limits: &mut StructuralLimits| limits.boolean_terms = 2) as fn(&mut StructuralLimits),
            "query_plan_pattern_limit",
        ),
        (
            "predicate nodes",
            |limits: &mut StructuralLimits| limits.predicate_nodes = 2,
            "query_plan_pattern_node_limit",
        ),
        (
            "output names",
            |limits: &mut StructuralLimits| limits.output_name_bytes = 3,
            "query_plan_name_limit",
        ),
        (
            "selected slots",
            |limits: &mut StructuralLimits| limits.selected_slots = 1,
            "query_plan_output_limit",
        ),
        (
            "sort terms",
            |limits: &mut StructuralLimits| limits.order_terms = 0,
            "query_plan_sort_term_limit",
        ),
    ] {
        let mut limits = StructuralLimits::CANONICAL;
        apply(&mut limits);
        let error = plan
            .check_structural_limits(limits)
            .expect_err("stricter limits must reject");
        assert_eq!(error.code().as_str(), code, "{case}");
    }
}

#[test]
fn caller_name_limits_cover_document_keys_and_local_bindings() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::query_plan::{
        DocumentField, DocumentSource, LocalFunction, LocalReturn, Reducer,
    };

    let document_plan = QueryPlan::new(
        vec![binding(0, "p"), binding(1, "a")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("age").expect("attribute id"),
                    owner: binding_id(0),
                },
            ],
        }],
        QueryOutput::Documents {
            fields: vec![DocumentField::new(
                QueryVariable::new("age").expect("document key"),
                DocumentSource::Binding {
                    binding: binding_id(1),
                },
            )],
        },
        managed_semantics(b"document-name-limit-fixture"),
    )
    .expect("canonical document plan");

    let local_plan = QueryPlan::new_with_functions(
        vec![binding(0, "p"), binding(1, "c")],
        vec![LocalFunction::new(
            FunctionId::new("age_count_of").expect("function id"),
            vec![binding(0, "subject"), binding(1, "age")],
            vec![Label::new("person").expect("label")],
            vec![QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: AttributeId::new("age").expect("attribute id"),
                owner: binding_id(0),
            }],
            LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
        )],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::FunctionCall {
                    arguments: vec![QueryOperand::Binding {
                        binding: binding_id(0),
                    }],
                    assigned: binding_id(1),
                    function: FunctionId::new("age_count_of").expect("function id"),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(b"local-name-limit-fixture"),
    )
    .expect("canonical local-function plan");

    let mut limits = StructuralLimits::CANONICAL;
    limits.output_name_bytes = 1;
    for (case, plan) in [
        ("document key", &document_plan),
        ("local binding", &local_plan),
    ] {
        let error = plan
            .check_structural_limits(limits)
            .expect_err("tightened name limit must cover every name");
        assert_eq!(error.code().as_str(), "query_plan_name_limit", "{case}");
    }
}

#[test]
fn invocation_row_and_byte_budgets_are_independent_of_output_slots() {
    use type_bridge_contract::limits::{MAX_INPUT_BYTES, MAX_INPUT_ROWS};

    let plan = full_pipeline_plan();
    let too_many_rows = (0..=MAX_INPUT_ROWS)
        .map(|_| InputRow::new(vec![Some(CanonicalValue::Long(1))]))
        .collect();
    assert_eq!(
        QueryInvocation::new(&plan, QueryOperation::Rows, too_many_rows)
            .expect_err("input rows have their own ceiling")
            .code()
            .as_str(),
        "query_invocation_row_limit",
    );

    // Input bytes are charged before per-cell type validation. Five bounded
    // strings exceed the aggregate 4 MiB invocation budget while remaining
    // far below the canonical string ceiling individually.
    let chunk = "x".repeat((MAX_INPUT_BYTES / 5) + 32);
    let too_many_bytes = (0..5)
        .map(|_| {
            InputRow::new(vec![Some(CanonicalValue::String(
                CanonicalString::new(chunk.clone()).expect("bounded input string"),
            ))])
        })
        .collect();
    assert_eq!(
        QueryInvocation::new(&plan, QueryOperation::Rows, too_many_bytes)
            .expect_err("input bytes have their own ceiling")
            .code()
            .as_str(),
        "query_invocation_input_byte_limit",
    );
}

#[test]
fn predicate_nodes_charge_one_aggregate_budget_across_local_functions() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, Reducer};

    // Root conjunction: two nodes. Local body: one node. A per-function
    // reset would accept a three-node plan under a two-node budget; the
    // aggregate budget must reject it.
    let plan = QueryPlan::new_with_functions(
        vec![binding(0, "person"), binding(1, "age_count")],
        vec![LocalFunction::new(
            FunctionId::new("age_count_of").expect("function id"),
            vec![binding(0, "subject"), binding(1, "age")],
            vec![Label::new("person").expect("label")],
            vec![QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: AttributeId::new("age").expect("attribute id"),
                owner: binding_id(0),
            }],
            LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
        )],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::FunctionCall {
                    arguments: vec![QueryOperand::Binding {
                        binding: binding_id(0),
                    }],
                    assigned: binding_id(1),
                    function: FunctionId::new("age_count_of").expect("function id"),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(b"aggregate-node-budget-fixture"),
    )
    .expect("three-node plan under canonical limits");

    let mut two_nodes = StructuralLimits::CANONICAL;
    two_nodes.predicate_nodes = 2;
    assert_eq!(
        plan.check_structural_limits(two_nodes)
            .expect_err("aggregate budget spans functions")
            .code()
            .as_str(),
        "query_plan_pattern_node_limit"
    );

    let mut three_nodes = StructuralLimits::CANONICAL;
    three_nodes.predicate_nodes = 3;
    plan.check_structural_limits(three_nodes)
        .expect("exact aggregate fits");
}

#[test]
fn typeql_reserved_labels_cannot_enter_plan_or_local_function_contracts() {
    use type_bridge_contract::id::{FunctionId, Label};

    assert_eq!(
        TypeId::new(TypeKind::Entity, "isa")
            .expect_err("reserved type label")
            .code()
            .as_str(),
        "malformed_id",
    );
    assert_eq!(
        FunctionId::new("match")
            .expect_err("reserved schema or local function name")
            .code()
            .as_str(),
        "malformed_id",
    );
    assert_eq!(
        Label::new("return")
            .expect_err("reserved local-function parameter type")
            .code()
            .as_str(),
        "malformed_id",
    );
}
