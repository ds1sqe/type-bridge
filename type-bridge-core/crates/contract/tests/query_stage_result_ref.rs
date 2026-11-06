#[path = "support/plan.rs"]
#[allow(dead_code)]
mod support;
use support::*;
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::migration_assertion::QueryVariable;
use type_bridge_contract::query_plan::{Expr, Pattern, QueryPlan, Stage};
use type_bridge_contract::query_plan::{InputColumn, InputColumnId, InputRow, QueryOperand};
use type_bridge_contract::query_remote::*;
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteExecutorBinding, RemoteSigningPublicKey, RemoteValue,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

fn source() -> StagedPlan {
    let plan = QueryPlan::new(spec(vec![matched()], vec![0]), &schema()).unwrap();
    StagedPlan::new(&plan, vec![], &schema()).unwrap()
}
fn reference(input: u16, source_plan: u32) -> StagedResultRef {
    StagedResultRef {
        input: InputColumnId::new(input),
        source_plan,
        source_column: 0,
        expected_types: vec![entity("account")],
    }
}
fn target(rows: usize, optional: bool, refs: Vec<StagedResultRef>) -> StagedPlan {
    let width = refs
        .iter()
        .map(|r| usize::from(r.input.get()) + 1)
        .max()
        .unwrap();
    let mut plan = spec(
        vec![Stage::Match {
            patterns: vec![
                isa(0, entity("account")).into(),
                type_bridge_contract::query_plan::QueryPattern::Iid {
                    binding: id(0),
                    iid: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                }
                .into(),
            ],
        }],
        vec![0],
    );
    plan.inputs = (0..width)
        .map(|i| {
            InputColumn::new(
                InputColumnId::new(i as u16),
                QueryVariable::new(format!("input{i}")).unwrap(),
                ValueTypeTag::String,
                optional,
            )
        })
        .collect();
    let plan = QueryPlan::new(plan, &schema()).unwrap();
    StagedPlan::new_with_result_refs(
        &plan,
        vec![InputRow::new(vec![None; width]); rows],
        refs,
        &schema(),
    )
    .unwrap()
}
fn answer(ids: &[&str]) -> StagedAnswer {
    StagedAnswer::Rows {
        rows: ids
            .iter()
            .map(|iid| {
                vec![RemoteValue::Thing {
                    iid: (*iid).into(),
                    type_id: entity("account"),
                }]
            })
            .collect(),
    }
}
fn iid(value: &str) -> Option<CanonicalValue> {
    Some(CanonicalValue::String(
        type_bridge_contract::value::CanonicalString::new(value).unwrap(),
    ))
}

#[test]
fn iid_refs_broadcast_zip_and_allow_absence_only_for_optional_inputs() {
    let target = target(3, false, vec![reference(0, 0), reference(1, 0)]);
    validate_staged_result_refs(&[source(), target.clone()], &schema()).unwrap();
    let bound = target
        .bind_results(&[answer(&["0x10"])], &schema())
        .unwrap();
    assert_eq!(
        bound.given_rows().unwrap(),
        vec![InputRow::new(vec![iid("0x10"), iid("0x10")]); 3]
    );
    let bound = target
        .bind_results(&[answer(&["0x10", "0x20", "0x30"])], &schema())
        .unwrap();
    assert_eq!(
        bound.given_rows().unwrap(),
        ["0x10", "0x20", "0x30"].map(|v| InputRow::new(vec![iid(v), iid(v)]))
    );
    assert_eq!(
        target
            .bind_results(&[answer(&["0x10", "0x20"])], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_rows"
    );
    assert_eq!(
        target
            .bind_results(&[answer(&[])], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_missing"
    );
    let optional = crate::target(2, true, vec![reference(0, 0)]);
    assert_eq!(
        optional
            .bind_results(&[answer(&[])], &schema())
            .unwrap()
            .given_rows()
            .unwrap(),
        vec![InputRow::new(vec![None]); 2]
    );
    assert_eq!(
        target.bound_plan(&schema()).unwrap_err().code().as_str(),
        "query_stage_result_ref_unresolved"
    );
}

#[test]
fn one_fixed_template_inherits_all_rows_of_one_iid_source() {
    let template = target(1, false, vec![reference(0, 0), reference(1, 0)]);
    validate_staged_result_refs(&[source(), template.clone()], &schema()).unwrap();
    let bound = template
        .bind_results(&[answer(&["0x30", "0x10"])], &schema())
        .unwrap();
    assert_eq!(
        bound.given_rows().unwrap(),
        ["0x30", "0x10"].map(|v| InputRow::new(vec![iid(v), iid(v)]))
    );
    let fixed = StagedPlan::new_with_result_refs(
        template.plan(),
        vec![InputRow::new(vec![None, iid("0x40")])],
        vec![reference(0, 0)],
        &schema(),
    )
    .unwrap();
    let bound = fixed
        .bind_results(&[answer(&["0x30", "0x10"])], &schema())
        .unwrap();
    assert_eq!(
        bound.given_rows().unwrap(),
        ["0x30", "0x10"].map(|v| InputRow::new(vec![iid(v), iid("0x40")]))
    );
    let too_many = answer(&vec![
        "0x10";
        template.plan().limits().structural.input_rows + 1
    ]);
    assert_eq!(
        template
            .bind_results(&[too_many], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_invocation_row_limit"
    );
}

#[test]
fn iid_ref_graph_refuses_forward_scalar_and_incompatible_columns() {
    let forward = target(1, false, vec![reference(0, 1)]);
    assert_eq!(
        validate_staged_result_refs(&[source(), forward], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_order"
    );
    let mut outside = reference(0, 0);
    outside.source_column = 99;
    assert_eq!(
        validate_staged_result_refs(&[source(), target(1, false, vec![outside])], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_column"
    );
    let count = source()
        .plan()
        .with_operation(
            type_bridge_contract::query_plan::Operation::Count,
            &schema(),
        )
        .unwrap();
    let count = StagedPlan::new(&count, vec![], &schema()).unwrap();
    assert_eq!(
        validate_staged_result_refs(&[count, target(1, false, vec![reference(0, 0)])], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_column"
    );
    let mut wrong_type = reference(0, 0);
    wrong_type.expected_types = vec![entity("item")];
    assert_eq!(
        validate_staged_result_refs(&[source(), target(1, false, vec![wrong_type])], &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_stage_result_ref_type"
    );
    let scalar = QueryPlan::new(
        spec(
            vec![Stage::Match {
                patterns: vec![Pattern::Let {
                    binding: id(0),
                    expr: Expr::Operand(literal(1)),
                }],
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap();
    let scalar = StagedPlan::new(&scalar, vec![], &schema()).unwrap();
    assert_eq!(
        validate_staged_result_refs(
            &[scalar, target(1, false, vec![reference(0, 0)])],
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_stage_result_ref_column"
    );
}

#[test]
fn iid_ref_templates_refuse_different_sources_and_authored_cells() {
    let template = target(1, false, vec![reference(0, 0), reference(1, 0)]);
    assert_eq!(
        StagedPlan::new_with_result_refs(
            template.plan(),
            template.rows().to_vec(),
            vec![reference(0, 0), reference(1, 1)],
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_stage_result_ref_sources"
    );
    assert_eq!(
        StagedPlan::new_with_result_refs(
            template.plan(),
            vec![InputRow::new(vec![iid("0x10"), None])],
            template.result_refs().to_vec(),
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_stage_result_ref_template"
    );
}

#[test]
fn result_refs_survive_canonical_request_bytes_without_runtime_iids() {
    let plans = vec![source(), target(2, false, vec![reference(0, 0)])];
    let mut caps = type_bridge_contract::capability::CapabilitySet::new();
    for plan in &plans {
        for cap in plan.plan().required_capabilities().iter() {
            caps.insert(cap.clone());
        }
    }
    for cap in [CAP_QUERY_REMOTE_STAGED, CAP_QUERY_STAGE_RESULT_REF] {
        caps.insert(CapabilityId::new(cap).unwrap());
    }
    let ad = RemoteCapabilities::new(
        caps,
        RemoteExecutorBinding::new("result-ref-executor", "result-ref-epoch").unwrap(),
        RemoteSigningPublicKey::from_bytes([7; 32]),
    )
    .with_query_limits(type_bridge_contract::limits::QueryLimits::PLAN)
    .unwrap();
    let request = StagedRequest::new(
        StagedRequestSpec {
            nonce: "result-ref-nonce-00001".into(),
            kind: None,
            limits: StagedLimits::default(),
            plans,
            prepared_at_unix_ms: 1_000_000,
        },
        &schema(),
        &ad,
    )
    .unwrap();
    let missing_capabilities = ad
        .capabilities()
        .iter()
        .filter(|cap| cap.as_str() != CAP_QUERY_STAGE_RESULT_REF)
        .cloned()
        .collect();
    let missing =
        RemoteCapabilities::new(missing_capabilities, ad.executor().clone(), ad.reply_key())
            .with_query_limits(ad.query_limits())
            .unwrap();
    assert_eq!(
        StagedRequest::new(
            StagedRequestSpec {
                nonce: "result-ref-nonce-00001".into(),
                kind: None,
                limits: StagedLimits::default(),
                plans: request.plans().to_vec(),
                prepared_at_unix_ms: 1_000_000,
            },
            &schema(),
            &missing
        )
        .unwrap_err()
        .code()
        .as_str(),
        "unsupported_required_capability"
    );
    let bytes = request.encode().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote");
    for (name, fixture) in [
        ("advertisement-result-ref.json", ad.encode().unwrap()),
        ("request-result-ref.json", bytes.clone()),
    ] {
        let path = root.join(name);
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_V3_FIXTURES").is_some() {
            std::fs::write(&path, &fixture).unwrap();
        }
        assert_eq!(std::fs::read(path).unwrap(), fixture);
    }
    let decoded = StagedRequest::decode(&bytes, &schema(), &ad).unwrap();
    assert_eq!(decoded.encode().unwrap(), bytes);
    assert_eq!(decoded.plans()[1].result_refs(), &[reference(0, 0)]);
    assert!(decoded.plans()[1].plan().given_rows().is_none());
    assert_eq!(
        decoded.plans()[1].rows(),
        vec![InputRow::new(vec![None]); 2]
    );
    assert!(!String::from_utf8(bytes).unwrap().contains("0x10"));
}

#[test]
fn runtime_iid_refs_refuse_incompatible_answers_before_binding() {
    let target = target(1, false, vec![reference(0, 0)]);
    for answer in [
        StagedAnswer::Count { value: 1 },
        StagedAnswer::Rows {
            rows: vec![vec![RemoteValue::Value {
                value: CanonicalValue::Long(1),
            }]],
        },
        StagedAnswer::Rows {
            rows: vec![vec![RemoteValue::Thing {
                iid: "0x10".into(),
                type_id: entity("item"),
            }]],
        },
    ] {
        assert_eq!(
            target
                .bind_results(&[answer], &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_stage_result_ref_answer"
        );
    }
    let absent = StagedAnswer::Rows {
        rows: vec![vec![RemoteValue::Absent]],
    };
    let failure = target
        .bind_results(std::slice::from_ref(&absent), &schema())
        .unwrap_err();
    assert_eq!(failure.code().as_str(), "query_stage_result_ref_missing");
    assert_eq!(
        failure.category(),
        type_bridge_contract::diagnostic::DiagnosticCategory::Integrity
    );
    let optional = crate::target(1, true, vec![reference(0, 0)]);
    assert_eq!(
        optional
            .bind_results(&[absent], &schema())
            .unwrap()
            .given_rows()
            .unwrap(),
        [InputRow::new(vec![None])]
    );
}

#[test]
fn iid_ref_types_use_the_final_selected_domain_and_accept_subtypes() {
    let schema = schema_with_subtype_role();
    let child = type_bridge_contract::id::TypeId::new(
        type_bridge_contract::id::TypeKind::Relation,
        "child-membership",
    )
    .unwrap();
    let source = QueryPlan::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![
                        type_bridge_contract::query_plan::QueryPattern::Isa {
                            binding: id(0),
                            type_id: relation(),
                            include_subtypes: true,
                        }
                        .into(),
                    ],
                },
                Stage::Select {
                    bindings: vec![id(0)],
                },
                Stage::Distinct,
                Stage::Match {
                    patterns: vec![isa(0, child.clone()).into()],
                },
            ],
            vec![0],
        ),
        &schema,
    )
    .unwrap();
    let source = StagedPlan::new(&source, vec![], &schema).unwrap();
    let mut target_spec = spec(
        vec![Stage::Match {
            patterns: vec![
                isa(0, child.clone()).into(),
                type_bridge_contract::query_plan::QueryPattern::Iid {
                    binding: id(0),
                    iid: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                }
                .into(),
            ],
        }],
        vec![0],
    );
    target_spec.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("target").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    let plan = QueryPlan::new(target_spec, &schema).unwrap();
    for expected_type in [child.clone(), relation()] {
        let reference = StagedResultRef {
            input: InputColumnId::new(0),
            source_plan: 0,
            source_column: 0,
            expected_types: vec![expected_type],
        };
        let target = StagedPlan::new_with_result_refs(
            &plan,
            vec![InputRow::new(vec![None])],
            vec![reference],
            &schema,
        )
        .unwrap();
        validate_staged_result_refs(&[source.clone(), target.clone()], &schema).unwrap();
        let answer = StagedAnswer::Rows {
            rows: vec![vec![RemoteValue::Thing {
                iid: "0x10".into(),
                type_id: child.clone(),
            }]],
        };
        assert_eq!(
            target
                .bind_results(&[answer], &schema)
                .unwrap()
                .given_rows()
                .unwrap(),
            [InputRow::new(vec![iid("0x10")])]
        );
    }
}

#[test]
fn iid_refs_accept_a_canonical_union_of_unrelated_player_types() {
    let mut plan = spec(
        vec![Stage::Match {
            patterns: vec![
                type_bridge_contract::query_plan::QueryPattern::Or {
                    branches: vec![
                        vec![isa(0, entity("account"))],
                        vec![isa(0, entity("item"))],
                    ],
                }
                .into(),
            ],
        }],
        vec![0],
    );
    let source = QueryPlan::new(plan.clone(), &schema()).unwrap();
    let source = StagedPlan::new(&source, vec![], &schema()).unwrap();
    plan.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("target").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    let Stage::Match { patterns } = &mut plan.pipeline[0] else {
        unreachable!()
    };
    patterns.push(
        type_bridge_contract::query_plan::QueryPattern::Iid {
            binding: id(0),
            iid: QueryOperand::Input {
                column: InputColumnId::new(0),
            },
        }
        .into(),
    );
    let plan = QueryPlan::new(plan, &schema()).unwrap();
    let mut reference = reference(0, 0);
    reference.expected_types = vec![entity("item"), entity("account"), entity("item")];
    let target = StagedPlan::new_with_result_refs(
        &plan,
        vec![InputRow::new(vec![None]); 2],
        vec![reference],
        &schema(),
    )
    .unwrap();
    assert_eq!(
        target.result_refs()[0].expected_types,
        [entity("account"), entity("item")]
    );
    validate_staged_result_refs(&[source, target.clone()], &schema()).unwrap();
    let rows = StagedAnswer::Rows {
        rows: vec![
            vec![RemoteValue::Thing {
                iid: "0x10".into(),
                type_id: entity("account"),
            }],
            vec![RemoteValue::Thing {
                iid: "0x20".into(),
                type_id: entity("item"),
            }],
        ],
    };
    assert_eq!(
        target
            .bind_results(&[rows], &schema())
            .unwrap()
            .given_rows()
            .unwrap(),
        [
            InputRow::new(vec![iid("0x10")]),
            InputRow::new(vec![iid("0x20")])
        ]
    );
}

#[test]
fn shared_builder_authors_the_same_iid_reference_as_the_contract_constructor() {
    use type_bridge_contract::query_plan::PlanBuilder;
    let mut builder = PlanBuilder::new();
    let input = builder.input("target_iid", ValueTypeTag::String, false);
    let root = builder.exact(entity("account"));
    builder.by_iid_input(root, input).returning(vec![root]);
    let semantics = source().plan().managed_semantics().clone();
    let rows = vec![InputRow::new(vec![None]); 3];
    let refs = vec![reference(0, 0)];
    let authored = builder
        .build_staged(&schema(), semantics.clone(), rows.clone(), refs.clone())
        .unwrap();
    let ordinary = builder.build(&schema(), semantics).unwrap();
    let manual = StagedPlan::new_with_result_refs(&ordinary, rows, refs, &schema()).unwrap();
    assert_eq!(
        authored.plan().canonical_bytes().unwrap(),
        manual.plan().canonical_bytes().unwrap()
    );
    assert_eq!(authored.rows(), manual.rows());
    assert_eq!(authored.result_refs(), manual.result_refs());
    validate_staged_result_refs(&[source(), authored.clone()], &schema()).unwrap();
    let bound = authored
        .bind_results(&[answer(&["0x10"])], &schema())
        .unwrap();
    assert_eq!(
        bound.given_rows().unwrap(),
        vec![InputRow::new(vec![iid("0x10")]); 3]
    );
}
