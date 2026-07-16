use serde_json::{Value, json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, OrderDirection, OrderTerm, QueryOperand,
    QueryOutput, QueryPattern, QueryPlan, ReadStage, decode_query_plan,
};
use type_bridge_contract::query_plan_capability_vocabulary;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

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
                        left: QueryOperand::Binding { binding: binding_id(1) },
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
            "query.input.columns",
            "query.output.rows",
            "query.pattern.function-call",
            "query.pattern.has",
            "query.pattern.isa",
            "query.pattern.isa-subtypes",
            "query.pattern.links",
            "query.pattern.negation",
            "query.pattern.value",
            "query.plan",
            "query.stage.distinct",
            "query.stage.limit",
            "query.stage.offset",
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
    let bytes = full_pipeline_plan().canonical_bytes().expect("canonical bytes");

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
    wrong_format["format"] = json!("typebridge.query-plan/v2");
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
    let output = QueryOutput::Rows { columns: vec![binding_id(0)] };
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
            ReadStage::Match { patterns: vec![person_isa(0)] },
            ReadStage::Distinct,
            ReadStage::Select { bindings: vec![binding_id(0)] },
        ])
        .expect_err("stages follow the canonical order")
        .code()
        .as_str(),
        "query_plan_stage_order",
    );
    assert_eq!(
        build(vec![
            ReadStage::Match { patterns: vec![person_isa(0)] },
            ReadStage::Limit { rows: 3 },
        ])
        .expect_err("truncation requires an explicit order")
        .code()
        .as_str(),
        "query_plan_unordered_truncation",
    );
    assert_eq!(
        build(vec![
            ReadStage::Match { patterns: vec![person_isa(0)] },
            ReadStage::Select { bindings: vec![binding_id(1)] },
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
            ReadStage::Select { bindings: vec![binding_id(0)] },
        ],
        QueryOutput::Rows { columns: vec![binding_id(1)] },
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
                    left: QueryOperand::Input { column: InputColumnId::new(0) },
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
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![row(18), row(65)],
    )
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
            ReadStage::Select { bindings: vec![binding_id(0), binding_id(1)] },
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

    let missing_required = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![None])],
    )
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
fn function_calls_are_first_class_and_capability_gated() {
    use type_bridge_contract::id::FunctionId;

    let semantics = managed_semantics(b"query-plan-function-fixture");
    let call = QueryPattern::FunctionCall {
        arguments: vec![QueryOperand::Binding { binding: binding_id(0) }],
        assigned: binding_id(1),
        function: FunctionId::new("person_age").expect("function id"),
    };
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "age_value")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![person_isa(0), call.clone()],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0), binding_id(1)] },
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
            patterns: vec![person_isa(0), QueryPattern::Not { patterns: vec![call] }],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
        semantics,
    )
    .expect_err("negated calls are reserved");
    assert_eq!(nested.code().as_str(), "query_plan_function_in_negation");
}
