#[path = "support/plan_v3_examples.rs"]
mod examples;

#[path = "support/plan_v3.rs"]
mod support;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use support::*;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::query_plan::v3::{
    DeleteItem, QueryPlanV3, Set, Stage, decode_query_plan_v3,
};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOutput, QueryPlan, ReadStage, ReduceAssignment, Reducer,
    decode_query_plan,
};

fn stage_plans() -> BTreeMap<&'static str, QueryPlanV3> {
    let mut plans = BTreeMap::new();
    let schema = schema();
    for (name, stage) in [
        (
            "insert",
            Stage::Insert {
                things: vec![thing(1)],
            },
        ),
        (
            "put",
            Stage::Put {
                things: vec![thing(1)],
            },
        ),
        (
            "update",
            Stage::Update {
                sets: vec![Set::Field {
                    owner: id(0),
                    field: field("score"),
                    operand: literal(2),
                }],
            },
        ),
        (
            "delete_has",
            Stage::Delete {
                items: vec![DeleteItem::Has {
                    owner: id(0),
                    field: field("score"),
                }],
            },
        ),
        (
            "delete_thing",
            Stage::Delete {
                items: vec![DeleteItem::Thing { binding: id(0) }],
            },
        ),
    ] {
        let output = if name == "delete_thing" {
            vec![]
        } else if name == "insert" || name == "put" {
            vec![1]
        } else {
            vec![0]
        };
        plans.insert(
            name,
            QueryPlanV3::new(spec(vec![matched(), stage], output), &schema).unwrap(),
        );
    }
    for (name, stage) in [
        (
            "link",
            Stage::Update {
                sets: vec![Set::Link {
                    relation: id(1),
                    role: role(),
                    player: id(0),
                }],
            },
        ),
        (
            "delete_links",
            Stage::Delete {
                items: vec![DeleteItem::Links {
                    relation: id(1),
                    role: role(),
                    player: id(0),
                }],
            },
        ),
    ] {
        plans.insert(
            name,
            QueryPlanV3::new(
                spec(
                    vec![
                        Stage::Match {
                            patterns: vec![pat(0, entity("account")), pat(1, relation())],
                        },
                        stage,
                    ],
                    vec![0, 1],
                ),
                &schema,
            )
            .unwrap(),
        );
    }
    plans.insert(
        "insert_first",
        QueryPlanV3::new(
            spec(
                vec![Stage::Insert {
                    things: vec![thing(0)],
                }],
                vec![0],
            ),
            &schema,
        )
        .unwrap(),
    );
    plans.insert(
        "read_tail",
        QueryPlanV3::new(
            spec(
                vec![
                    matched(),
                    Stage::Select {
                        bindings: vec![id(0)],
                    },
                    Stage::Require {
                        bindings: vec![id(0)],
                    },
                    Stage::Distinct,
                    Stage::Reduce {
                        assignments: vec![ReduceAssignment::new(id(2), Reducer::Count, None)],
                        groups: vec![id(0)],
                    },
                    Stage::Sort {
                        terms: vec![OrderTerm::new(id(0), OrderDirection::Ascending)],
                    },
                    Stage::Offset { rows: 1 },
                    Stage::Limit { rows: 10 },
                ],
                vec![0, 2],
            ),
            &schema,
        )
        .unwrap(),
    );
    plans
}

#[test]
fn v3_stages_have_stable_golden_bytes_and_round_trip() {
    let schema = schema();
    let mut fixtures = BTreeMap::<&str, Value>::new();
    for (name, plan) in stage_plans() {
        let bytes = plan.canonical_bytes().unwrap();
        assert_eq!(bytes, plan.canonical_bytes().unwrap());
        let decoded = decode_query_plan_v3(&bytes, &schema).unwrap();
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes);
        assert_eq!(decoded.fingerprint().unwrap(), plan.fingerprint().unwrap());
        fixtures.insert(name,json!({"plan":serde_json::from_slice::<Value>(&bytes).unwrap(),"fingerprint":plan.fingerprint().unwrap()}));
    }
    let bytes = to_canonical_json(&fixtures).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/query-plan-v3-stages.json");
    if std::env::var_os("TYPE_BRIDGE_UPDATE_PLAN_V3_FIXTURES").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn v2_lift_does_not_change_v2_bytes_or_fingerprint() {
    let old = QueryPlan::new_v2(
        vec![binding(0, "account")],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![isa(0, entity("account"))],
        }],
        QueryOutput::Rows {
            columns: vec![id(0)],
        },
        semantics(),
    )
    .unwrap();
    let bytes = old.canonical_bytes().unwrap();
    let fingerprint = old.fingerprint().unwrap();
    let decoded = decode_query_plan(&bytes).unwrap();
    let lifted = decoded.to_v3(&schema()).unwrap();
    assert_eq!(old.canonical_bytes().unwrap(), bytes);
    assert_eq!(old.fingerprint().unwrap(), fingerprint);
    assert_ne!(lifted.fingerprint().unwrap(), fingerprint);
    assert_eq!(
        decode_query_plan_v3(&lifted.canonical_bytes().unwrap(), &schema()).unwrap(),
        lifted
    );
    assert!(decode_query_plan(&lifted.canonical_bytes().unwrap()).is_err());
}

#[test]
fn write_validation_rejects_unbound_abstract_wrong_schema_key_and_deleted_targets() {
    let schema = schema();
    let mut abstract_thing = thing(1);
    abstract_thing.type_id = entity("abstract_account");
    let mut bad_owns = field("score");
    bad_owns = type_bridge_contract::schema::OwnsFactId::new(
        entity("other"),
        bad_owns.attribute().clone(),
    )
    .unwrap();
    for (pipeline, code) in [
        (
            vec![
                matched(),
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(1),
                        field: field("score"),
                        operand: literal(1),
                    }],
                },
            ],
            "query_plan_write_unbound_target",
        ),
        (
            vec![
                matched(),
                Stage::Insert {
                    things: vec![abstract_thing],
                },
            ],
            "query_plan_abstract_insert",
        ),
        (
            vec![
                matched(),
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(0),
                        field: bad_owns,
                        operand: literal(1),
                    }],
                },
            ],
            "query_plan_undeclared_owns",
        ),
        (
            vec![
                matched(),
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(0),
                        field: field("id"),
                        operand: string("private-key"),
                    }],
                },
            ],
            "query_plan_key_update",
        ),
        (
            vec![
                matched(),
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(0),
                        field: field("score"),
                        operand: literal(1),
                    }],
                },
            ],
            "query_plan_write_unbound_target",
        ),
        (
            vec![
                matched(),
                Stage::Select {
                    bindings: vec![id(0)],
                },
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(0),
                        field: field("score"),
                        operand: literal(1),
                    }],
                },
            ],
            "query_plan_stage_order",
        ),
    ] {
        let error = QueryPlanV3::new(spec(pipeline, vec![]), &schema).unwrap_err();
        assert_eq!(error.code().as_str(), code);
        assert!(!format!("{error:?}").contains("private-key"));
    }
}

#[test]
fn write_roles_require_declared_plays_and_relates() {
    let mut input = spec(
        vec![
            Stage::Match {
                patterns: vec![pat(0, entity("other")), pat(1, relation())],
            },
            Stage::Update {
                sets: vec![Set::Link {
                    relation: id(1),
                    role: role(),
                    player: id(0),
                }],
            },
        ],
        vec![1],
    );
    assert_eq!(
        QueryPlanV3::new(input.clone(), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_undeclared_plays"
    );
    if let Stage::Update { sets } = &mut input.pipeline[1] {
        sets[0] = Set::Link {
            relation: id(1),
            role: type_bridge_contract::id::RoleId::new("membership", "missing").unwrap(),
            player: id(0),
        };
    }
    assert_eq!(
        QueryPlanV3::new(input, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_undeclared_relates"
    );
}

#[test]
fn decode_rejects_unknown_fields_unknown_stages_and_forged_capabilities() {
    let plan = stage_plans().remove("update").unwrap();
    for field in ["extra", "pipeline", "required_capabilities"] {
        let mut value = serde_json::to_value(&plan).unwrap();
        match field {
            "pipeline" => value["pipeline"][1]["kind"] = json!("future_write"),
            "required_capabilities" => value[field] = json!([]),
            _ => value[field] = json!("private-value"),
        }
        let error =
            decode_query_plan_v3(&to_canonical_json(&value).unwrap(), &schema()).unwrap_err();
        assert!(!format!("{error:?}").contains("private-value"));
    }
}

#[test]
fn deletion_ends_lifetime_for_projection_and_read_tail() {
    let delete = Stage::Delete {
        items: vec![DeleteItem::Thing { binding: id(0) }],
    };
    assert_eq!(
        QueryPlanV3::new(spec(vec![matched(), delete.clone()], vec![0]), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_output_not_visible"
    );
    assert_eq!(
        QueryPlanV3::new(
            spec(
                vec![
                    matched(),
                    delete,
                    Stage::Select {
                        bindings: vec![id(0)]
                    }
                ],
                vec![]
            ),
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_plan_stage_unknown_binding"
    );
}

#[test]
fn write_and_read_stages_share_caller_selected_limits() {
    use type_bridge_contract::diagnostic::DiagnosticCategory;
    use type_bridge_contract::limits::QueryLimits;
    let mut limits = QueryLimits::CANONICAL;
    limits.structural.predicate_nodes = 1;
    let error = QueryPlanV3::new_with_limits(
        spec(
            vec![Stage::Insert {
                things: vec![thing(0)],
            }],
            vec![0],
        ),
        &schema(),
        limits,
    )
    .unwrap_err();
    assert_eq!(error.category(), DiagnosticCategory::ResourceLimit);
}

#[test]
fn inherited_owns_and_plays_are_valid_but_inherited_keys_are_immutable() {
    let matched = Stage::Match {
        patterns: vec![pat(0, entity("child_account")), pat(1, relation())],
    };
    let update = Stage::Update {
        sets: vec![
            Set::Field {
                owner: id(0),
                field: field("score"),
                operand: literal(2),
            },
            Set::Link {
                relation: id(1),
                role: role(),
                player: id(0),
            },
        ],
    };
    QueryPlanV3::new(spec(vec![matched.clone(), update], vec![0]), &schema()).unwrap();
    let update_key = Stage::Update {
        sets: vec![Set::Field {
            owner: id(0),
            field: field("id"),
            operand: string("private-key"),
        }],
    };
    assert_eq!(
        QueryPlanV3::new(spec(vec![matched, update_key], vec![0]), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_key_update"
    );
}

#[test]
fn race_operations_and_every_expression_have_stable_goldens() {
    let mut fixtures = BTreeMap::new();
    for (name, plan) in examples::plans() {
        let bytes = plan.canonical_bytes().unwrap();
        assert_eq!(
            decode_query_plan_v3(&bytes, &schema())
                .unwrap()
                .canonical_bytes()
                .unwrap(),
            bytes
        );
        assert_eq!(plan.canonical_bytes().unwrap(), bytes);
        assert!(
            plan.required_capabilities()
                .missing_from(
                    &type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary()
                )
                .is_empty()
        );
        fixtures.insert(name,json!({"plan":serde_json::from_slice::<Value>(&bytes).unwrap(),"fingerprint":plan.fingerprint().unwrap()}));
    }
    let bytes = to_canonical_json(&fixtures).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/query-plan-v3-expressions.json");
    if std::env::var_os("TYPE_BRIDGE_UPDATE_PLAN_V3_FIXTURES").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn numeric_promotion_matches_the_typedb_3_13_6_table() {
    use type_bridge_contract::query_plan::v3::{ArithOp, Expr};
    use type_bridge_contract::value::ValueTypeTag::{Decimal, Double, Long};
    let operands = [literal(2), examples::decimal(), examples::double()];
    let ring = [
        [Long, Decimal, Double],
        [Decimal, Decimal, Double],
        [Double, Double, Double],
    ];
    let modulo = [
        [Long, Double, Double],
        [Double, Double, Double],
        [Double, Double, Double],
    ];
    for (i, left) in operands.iter().enumerate() {
        for (j, right) in operands.iter().enumerate() {
            for (op, expected) in [
                (ArithOp::Add, ring[i][j]),
                (ArithOp::Sub, ring[i][j]),
                (ArithOp::Mul, ring[i][j]),
                (ArithOp::Div, Double),
                (ArithOp::Pow, Double),
                (ArithOp::Mod, modulo[i][j]),
            ] {
                let expr = Expr::Arith {
                    op,
                    lhs: Box::new(Expr::Operand(left.clone())),
                    rhs: Box::new(Expr::Operand(right.clone())),
                };
                assert_eq!(
                    expr.value_type(&BTreeMap::new(), &[]).unwrap(),
                    expected,
                    "{op:?}: {i}/{j}"
                );
            }
        }
    }
}

#[test]
fn wrong_typed_or_wrong_arity_builtins_are_value_free_errors() {
    use type_bridge_contract::query_plan::v3::{ArithOp, Builtin, Expr, Pattern};
    let secret = "private-expression-value";
    for expr in [
        Expr::Arith {
            op: ArithOp::Add,
            lhs: Box::new(Expr::Operand(string(secret))),
            rhs: Box::new(Expr::Operand(literal(1))),
        },
        Expr::Call {
            func: Builtin::Ceil,
            args: vec![Expr::Operand(literal(1))],
        },
        Expr::Call {
            func: Builtin::Length,
            args: vec![Expr::Operand(literal(1))],
        },
        Expr::Call {
            func: Builtin::Abs,
            args: vec![],
        },
        Expr::Call {
            func: Builtin::Max,
            args: vec![Expr::Operand(literal(1))],
        },
        Expr::Call {
            func: Builtin::Min,
            args: vec![Expr::Operand(literal(1)), Expr::Operand(examples::double())],
        },
    ] {
        let error = QueryPlanV3::new(
            spec(
                vec![Stage::Match {
                    patterns: vec![Pattern::Let {
                        binding: id(0),
                        expr,
                    }],
                }],
                vec![0],
            ),
            &schema(),
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains(secret));
        assert!(error.code().as_str().starts_with("query_plan_expression_"));
    }
}

#[test]
fn let_assignments_are_fresh_and_references_are_already_typed() {
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::{Expr, Pattern};
    for patterns in [
        vec![
            Pattern::Let {
                binding: id(0),
                expr: Expr::Operand(QueryOperand::Binding { binding: id(1) }),
            },
            Pattern::Let {
                binding: id(1),
                expr: Expr::Operand(literal(1)),
            },
        ],
        vec![
            Pattern::Let {
                binding: id(0),
                expr: Expr::Operand(literal(1)),
            },
            Pattern::Let {
                binding: id(0),
                expr: Expr::Operand(literal(2)),
            },
        ],
        vec![
            pat(0, entity("account")),
            Pattern::Let {
                binding: id(0),
                expr: Expr::Operand(literal(1)),
            },
        ],
    ] {
        assert!(
            QueryPlanV3::new(spec(vec![Stage::Match { patterns }], vec![0]), &schema()).is_err()
        );
    }
}

#[test]
fn operations_cannot_hide_writes_or_invent_them() {
    use type_bridge_contract::query_plan::v3::{Operation, WriteReturns};
    assert_eq!(
        examples::batch()
            .with_operation(Operation::Rows, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_operation_write_mismatch"
    );
    let read = QueryPlanV3::new(spec(vec![matched()], vec![0]), &schema()).unwrap();
    assert_eq!(
        read.with_operation(
            Operation::Write {
                returns: WriteReturns::Rows
            },
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_plan_operation_write_mismatch"
    );
    assert_eq!(
        examples::batch()
            .with_operation(
                Operation::Write {
                    returns: WriteReturns::Rows
                },
                &schema()
            )
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_output_limit"
    );
}

#[test]
fn given_rows_enforce_types_presence_width_and_the_4096_ceiling() {
    use type_bridge_contract::query_plan::InputRow;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    let batch = examples::batch();
    let rows: Vec<_> = (0..4096)
        .map(|n| {
            InputRow::new(vec![
                Some(CanonicalValue::String(
                    CanonicalString::new(format!("row-{n}")).unwrap(),
                )),
                Some(CanonicalValue::Long(n)),
            ])
        })
        .collect();
    batch.with_given_rows(rows.clone(), &schema()).unwrap();
    let mut too_many = rows;
    too_many.push(too_many[0].clone());
    assert_eq!(
        batch
            .with_given_rows(too_many, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_invocation_row_limit"
    );
    for (row, code) in [
        (vec![None], "query_invocation_row_arity"),
        (
            vec![None, Some(CanonicalValue::Long(1))],
            "query_invocation_missing_value",
        ),
        (
            vec![
                Some(CanonicalValue::String(
                    CanonicalString::new("private-batch-value").unwrap(),
                )),
                Some(CanonicalValue::Boolean(true)),
            ],
            "query_invocation_value_type",
        ),
    ] {
        let error = batch
            .with_given_rows(vec![InputRow::new(row)], &schema())
            .unwrap_err();
        assert_eq!(error.code().as_str(), code);
        assert!(!format!("{error:?}").contains("private-batch-value"));
    }
}

#[test]
fn let_values_must_match_the_written_field_and_stay_bounded() {
    use type_bridge_contract::limits::QueryLimits;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::{ArithOp, Expr, Pattern};
    let expr = Expr::Arith {
        op: ArithOp::Div,
        lhs: Box::new(Expr::Operand(literal(3))),
        rhs: Box::new(Expr::Operand(literal(2))),
    };
    let plan = spec(
        vec![
            Stage::Match {
                patterns: vec![
                    pat(0, entity("account")),
                    Pattern::Let {
                        binding: id(1),
                        expr: expr.clone(),
                    },
                ],
            },
            Stage::Update {
                sets: vec![Set::Field {
                    owner: id(0),
                    field: field("score"),
                    operand: QueryOperand::Binding { binding: id(1) },
                }],
            },
        ],
        vec![0],
    );
    assert_eq!(
        QueryPlanV3::new(plan, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_write_value_type"
    );
    let mut limits = QueryLimits::CANONICAL;
    limits.structural.predicate_depth = 1;
    let error = QueryPlanV3::new_with_limits(
        spec(
            vec![Stage::Match {
                patterns: vec![Pattern::Let {
                    binding: id(0),
                    expr,
                }],
            }],
            vec![0],
        ),
        &schema(),
        limits,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_plan_expression_limit");
}

#[test]
fn let_cannot_reassign_a_branch_local_concept_binding() {
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::QueryPattern;
    use type_bridge_contract::query_plan::v3::{Expr, Pattern};
    let patterns = vec![
        pat(0, entity("account")),
        QueryPattern::Or {
            branches: vec![
                vec![isa(1, entity("account"))],
                vec![QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: literal(1),
                    right: literal(1),
                }],
            ],
        }
        .into(),
        Pattern::Let {
            binding: id(1),
            expr: Expr::Operand(literal(1)),
        },
    ];
    assert_eq!(
        QueryPlanV3::new(spec(vec![Stage::Match { patterns }], vec![1]), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_let_binding_not_fresh"
    );
}

#[test]
fn v2_inputs_lift_to_a_reusable_v3_template_then_bind_without_mutating_v2() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::v3::Operation;
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryPattern,
    };
    use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
    let old = QueryPlan::new_v2(
        vec![binding(0, "account"), binding(1, "score")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            type_bridge_contract::migration_assertion::QueryVariable::new("minimum").unwrap(),
            ValueTypeTag::Long,
            false,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                isa(0, entity("account")),
                QueryPattern::Has {
                    owner: id(0),
                    attribute: id(1),
                    attribute_id: AttributeId::new("score").unwrap(),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Greater,
                    left: QueryOperand::Binding { binding: id(1) },
                    right: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![id(0)],
        },
        semantics(),
    )
    .unwrap();
    let old_bytes = old.canonical_bytes().unwrap();
    let template = old.to_v3(&schema()).unwrap();
    assert!(template.given_rows().is_none());
    assert_eq!(
        decode_query_plan_v3(&template.canonical_bytes().unwrap(), &schema()).unwrap(),
        template
    );
    let bound = template
        .with_given_rows(
            vec![InputRow::new(vec![Some(CanonicalValue::Long(3))])],
            &schema(),
        )
        .unwrap()
        .with_operation(Operation::Count, &schema())
        .unwrap();
    assert_ne!(
        bound.fingerprint().unwrap(),
        template.fingerprint().unwrap()
    );
    assert_eq!(old.canonical_bytes().unwrap(), old_bytes);
    assert_eq!(
        decode_query_plan_v3(&bound.canonical_bytes().unwrap(), &schema()).unwrap(),
        bound
    );
}

#[test]
fn shared_plan_v3_fixtures() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/contracts/plan_v3");
    let write = std::env::var_os("TYPE_BRIDGE_UPDATE_PLAN_V3_FIXTURES").is_some();
    if write {
        std::fs::create_dir_all(&root).unwrap();
    }
    let mut files = BTreeMap::new();
    files.insert(
        "schema.json".to_owned(),
        to_canonical_json(&schema()).unwrap(),
    );
    let mut plans = stage_plans();
    plans.extend(examples::plans());
    // Hydration fetches are shared per-file fixtures only: the aggregate
    // stage and expression goldens stay byte-identical, and the capability
    // they derive is not advertised by proxies yet.
    plans.insert("fetch_entity", examples::fetch_written(false));
    plans.insert("fetch_relation", examples::fetch_written(true));
    plans.insert("attach", examples::attach());
    plans.insert("replace_players", examples::replace_players());
    let mut manifest = BTreeMap::new();
    for (name, plan) in plans {
        let bytes = plan.canonical_bytes().unwrap();
        let decoded = decode_query_plan_v3(&bytes, &schema()).unwrap();
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes);
        let fingerprint = plan
            .fingerprint()
            .unwrap()
            .as_fingerprint()
            .digest()
            .to_hex();
        assert_eq!(
            decoded
                .fingerprint()
                .unwrap()
                .as_fingerprint()
                .digest()
                .to_hex(),
            fingerprint
        );
        manifest.insert(format!("{name}.json"), fingerprint);
        files.insert(format!("{name}.json"), bytes);
    }
    files.insert(
        "manifest.json".to_owned(),
        to_canonical_json(&manifest).unwrap(),
    );
    for (name, expected) in &files {
        let path = root.join(name);
        if write {
            std::fs::write(&path, expected).unwrap();
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            *expected,
            "{}",
            path.display()
        );
    }
    let actual: std::collections::BTreeSet<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, files.keys().cloned().collect());
}

#[test]
fn delete_thing_rejects_let_and_function_scalar_bindings() {
    use type_bridge_contract::id::FunctionId;
    use type_bridge_contract::query_plan::v3::{Expr, Pattern};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, QueryPattern};
    use type_bridge_contract::value::ValueTypeTag;
    for function in [false, true] {
        let pattern = if function {
            QueryPattern::FunctionCall {
                assigned: id(1),
                function: FunctionId::new("account_count").unwrap(),
                arguments: vec![type_bridge_contract::query_plan::QueryOperand::Binding {
                    binding: id(0),
                }],
            }
            .into()
        } else {
            Pattern::Let {
                binding: id(1),
                expr: Expr::Operand(literal(1)),
            }
        };
        let mut plan = spec(
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("account")), pattern],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(1) }],
                },
            ],
            vec![],
        );
        if function {
            plan.functions = vec![LocalFunction::new(
                FunctionId::new("account_count").unwrap(),
                vec![binding(0, "account")],
                vec![type_bridge_contract::id::Label::new("account").unwrap()],
                vec![isa(0, entity("account"))],
                LocalReturn::new(Reducer::Count, id(0), ValueTypeTag::Long),
            )];
        }
        assert_eq!(
            QueryPlanV3::new(plan, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_delete_untyped_thing"
        );
    }
}

#[test]
fn iid_patterns_identify_typed_things_and_round_trip() {
    use type_bridge_contract::query_plan::v3::Pattern;
    let iid = |binding: u16, iid: &str| Pattern::Iid {
        binding: id(binding),
        iid: iid.to_owned(),
    };
    let delete = |patterns: Vec<Pattern>| {
        spec(
            vec![
                Stage::Match { patterns },
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
            ],
            vec![],
        )
    };
    let plan = QueryPlanV3::new(
        delete(vec![
            pat(0, entity("account")),
            iid(0, "0x1e00000000000000000001"),
        ]),
        &schema(),
    )
    .unwrap();
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.pattern.iid")
    );
    let bytes = plan.canonical_bytes().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains(r#"{"binding":0,"iid":"0x1e00000000000000000001","kind":"iid"}"#));
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
    for (patterns, code) in [
        (
            vec![pat(0, entity("account")), iid(0, "1e00")],
            "query_plan_invalid_iid",
        ),
        (
            vec![pat(1, entity("account")), iid(0, "0x1e00")],
            "query_plan_iid_untyped_thing",
        ),
    ] {
        let mut plan = delete(patterns);
        plan.pipeline[1] = Stage::Delete {
            items: vec![DeleteItem::Thing { binding: id(1) }],
        };
        assert_eq!(
            QueryPlanV3::new(plan, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            code
        );
    }
}

#[test]
fn iid_patterns_refuse_attribute_and_optional_or_negated_bindings() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::query_plan::QueryPattern;
    use type_bridge_contract::query_plan::v3::Pattern;
    let iid = Pattern::Iid {
        binding: id(1),
        iid: "0x1e00".to_owned(),
    };
    for typed in [
        // An attribute binding has an IID in TypeDB, but it is not a thing.
        QueryPattern::Has {
            attribute: id(1),
            attribute_id: AttributeId::new("score").unwrap(),
            owner: id(0),
        },
        QueryPattern::Try {
            patterns: vec![isa(1, entity("account"))],
        },
        QueryPattern::Not {
            patterns: vec![isa(1, entity("account"))],
        },
    ] {
        let plan = spec(
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("account")), typed.into(), iid.clone()],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
            ],
            vec![],
        );
        assert_eq!(
            QueryPlanV3::new(plan, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_iid_untyped_thing"
        );
    }
}

#[test]
fn thing_fetch_is_terminal_typed_and_lists_declared_roles_once() {
    use type_bridge_contract::query_plan::v3::{
        DocumentRole, Operation, ThingDocument, WriteReturns,
    };
    let plan = examples::fetch_written(true);
    let bytes = plan.canonical_bytes().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains(
        r#"{"kind":"fetch","roles":[{"required":true,"role":{"declaring_relation":"membership","label":"member"}}],"thing":1}"#
    ), "{text}");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.fetch.thing")
    );
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);

    let fetch =
        |thing: u16, roles: Vec<DocumentRole>| Stage::Fetch(ThingDocument::new(id(thing), roles));
    let rows = Operation::Write {
        returns: WriteReturns::Rows,
    };
    let cases: Vec<(Vec<Stage>, Operation, Vec<u16>, &str)> = vec![
        // Not last.
        (
            vec![matched(), fetch(0, vec![]), Stage::Distinct],
            Operation::Rows,
            vec![],
            "query_plan_fetch_not_last",
        ),
        // Declared output beside the documents.
        (
            vec![matched(), fetch(0, vec![])],
            Operation::Rows,
            vec![0],
            "query_plan_fetch_output",
        ),
        // Count is not a document answer.
        (
            vec![matched(), fetch(0, vec![])],
            Operation::Count,
            vec![],
            "query_plan_fetch_output",
        ),
        // Deleted thing.
        (
            vec![
                matched(),
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
                fetch(0, vec![]),
            ],
            rows,
            vec![],
            "query_plan_fetch_untyped_thing",
        ),
        // An entity does not relate the role.
        (
            vec![matched(), fetch(0, vec![DocumentRole::new(role(), true)])],
            Operation::Rows,
            vec![],
            "query_plan_wrong_role_owner",
        ),
        // Listed twice.
        (
            vec![
                Stage::Match {
                    patterns: vec![pat(1, relation())],
                },
                fetch(
                    1,
                    vec![
                        DocumentRole::new(role(), true),
                        DocumentRole::new(role(), false),
                    ],
                ),
            ],
            Operation::Rows,
            vec![],
            "query_plan_fetch_duplicate_role",
        ),
    ];
    for (pipeline, operation, output, code) in cases {
        let mut plan = spec(pipeline, output);
        plan.operation = operation;
        assert_eq!(
            QueryPlanV3::new(plan, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            code
        );
    }
}

#[test]
fn attach_adds_to_bound_things_and_refuses_keys_and_unbound_targets() {
    use type_bridge_contract::query_plan::v3::AttachItem;
    let plan = examples::attach();
    let bytes = plan.canonical_bytes().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains(r#"{"items":[{"field":{"attribute":"score","owner":{"kind":"entity","label":"account"}},"kind":"has","operand":{"kind":"literal","value":{"kind":"long","value":"3"}},"owner":0},{"kind":"links","player":0,"relation":1,"role":{"declaring_relation":"membership","label":"member"}}],"kind":"attach"}"#), "{text}");
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.write.attach")
    );
    assert!(plan.writes());
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
    let attach = |items: Vec<AttachItem>, patterns| {
        spec(
            vec![Stage::Match { patterns }, Stage::Attach { items }],
            vec![],
        )
    };
    for (plan, code) in [
        (
            attach(
                vec![AttachItem::Has {
                    owner: id(0),
                    field: field("id"),
                    operand: string("account-c"),
                }],
                vec![pat(0, entity("account"))],
            ),
            "query_plan_key_update",
        ),
        (
            attach(
                vec![AttachItem::Has {
                    owner: id(1),
                    field: field("score"),
                    operand: literal(1),
                }],
                vec![pat(0, entity("account"))],
            ),
            "query_plan_write_unbound_target",
        ),
        (
            attach(
                vec![AttachItem::Links {
                    relation: id(0),
                    role: role(),
                    player: id(0),
                }],
                vec![pat(0, entity("account"))],
            ),
            "query_plan_wrong_role_owner",
        ),
    ] {
        assert_eq!(
            QueryPlanV3::new(plan, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            code
        );
    }
}

#[test]
fn removing_every_player_of_a_role_needs_a_declared_role_of_the_relation() {
    let plan = examples::replace_players();
    let bytes = plan.canonical_bytes().unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains(
        r#"{"items":[{"kind":"players","relation":1,"role":{"declaring_relation":"membership","label":"member"}}],"kind":"delete"}"#
    ));
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
    let wrong = spec(
        vec![
            matched(),
            Stage::Delete {
                items: vec![DeleteItem::Players {
                    relation: id(0),
                    role: role(),
                }],
            },
        ],
        vec![],
    );
    assert_eq!(
        QueryPlanV3::new(wrong, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_wrong_role_owner"
    );
}
