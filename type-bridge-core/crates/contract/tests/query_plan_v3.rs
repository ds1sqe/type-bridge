#[path = "support/plan_v3_examples.rs"]
mod examples;

#[path = "support/plan_v3.rs"]
mod support;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use support::*;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, InsertThing, QueryPlanV3, Set, Stage, decode_query_plan_v3,
};

#[test]
fn keyed_relation_put_requires_a_player_bound_before_allocation() {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::schema::OwnsFactId;
    let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
    let seed = Stage::Put {
        include_subtypes: false,
        things: vec![InsertThing {
            binding: id(1),
            type_id: purchase.clone(),
            fields: vec![(
                OwnsFactId::new(purchase, AttributeId::new("purchase-id").unwrap()).unwrap(),
                string("key"),
            )],
            players: vec![],
        }],
    };
    for optional in [false, true] {
        let stages = vec![
            matched(),
            seed.clone(),
            Stage::Attach {
                items: vec![AttachItem::Links {
                    optional,
                    relation: id(1),
                    role: RoleId::new("purchase", "buyer").unwrap(),
                    player: id(0),
                }],
            },
        ];
        let plan = QueryPlanV3::new(spec(stages, vec![1]), &schema());
        if optional {
            plan.unwrap();
            let mut request = spec(
                vec![
                    matched(),
                    seed.clone(),
                    Stage::Attach {
                        items: vec![AttachItem::Links {
                            optional: true,
                            relation: id(1),
                            role: RoleId::new("purchase", "buyer").unwrap(),
                            player: id(0),
                        }],
                    },
                ],
                vec![1],
            );
            request.inputs = vec![InputColumn::new(
                InputColumnId::new(0),
                type_bridge_contract::migration_assertion::QueryVariable::new("seed").unwrap(),
                type_bridge_contract::value::ValueTypeTag::String,
                false,
            )];
            request.given_rows = Some(vec![InputRow::new(vec![Some(
                type_bridge_contract::value::CanonicalValue::String(
                    type_bridge_contract::value::CanonicalString::new("key").unwrap(),
                ),
            )])]);
            QueryPlanV3::new(request.clone(), &schema()).unwrap();
            request.pipeline.pop();
            assert_eq!(
                QueryPlanV3::new(request, &schema())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "query_plan_insert_relation_players"
            );
        } else {
            plan.unwrap();
        }
    }
    let bare = QueryPlanV3::new(spec(vec![matched(), seed], vec![1]), &schema()).unwrap_err();
    assert_eq!(bare.code().as_str(), "query_plan_insert_relation_players");
}

#[test]
fn optional_fetch_roles_may_belong_to_a_subset_of_candidates() {
    use type_bridge_contract::id::RoleId;
    use type_bridge_contract::query_plan::v3::{DocumentRole, ThingDocument};
    let schema = schema_with_subtype_role();
    for (role, required, accepted) in [
        (
            RoleId::new("child-membership", "extra").unwrap(),
            false,
            true,
        ),
        (
            RoleId::new("child-membership", "extra").unwrap(),
            true,
            false,
        ),
        (RoleId::new("purchase", "buyer").unwrap(), false, false),
    ] {
        let stages = vec![
            Stage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: id(0),
                        type_id: relation(),
                        include_subtypes: true,
                    }
                    .into(),
                ],
            },
            Stage::Fetch(ThingDocument::new(
                id(0),
                vec![DocumentRole::new(role, required)],
            )),
        ];
        let plan = QueryPlanV3::new(spec(stages, vec![]), &schema);
        if accepted {
            plan.unwrap();
        } else {
            assert_eq!(
                plan.unwrap_err().code().as_str(),
                "query_plan_wrong_role_owner"
            );
        }
    }
}

#[test]
fn deferred_optional_seed_players_require_nonnull_rows_and_matching_existence_guards() {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::{QueryVariable, ValueComparator};
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    let column = InputColumnId::new(0);
    let operand = QueryOperand::Input { column };
    let lookup = |owner| {
        vec![
            QueryPattern::Isa {
                binding: id(owner),
                type_id: entity("account"),
                include_subtypes: false,
            },
            QueryPattern::Iid {
                binding: id(owner),
                iid: operand.clone(),
            },
        ]
    };
    let relation = TypeId::new(TypeKind::Relation, "purchase").unwrap();
    let allocated = InsertThing {
        binding: id(1),
        type_id: relation.clone(),
        fields: vec![(
            OwnsFactId::new(relation, AttributeId::new("purchase-id").unwrap()).unwrap(),
            string("seed"),
        )],
        players: vec![],
    };
    for put in [false, true] {
        let allocation = if put {
            Stage::Put {
                include_subtypes: false,
                things: vec![allocated.clone()],
            }
        } else {
            Stage::Insert {
                things: vec![allocated.clone()],
            }
        };
        let mut request = spec(
            vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Try {
                            patterns: lookup(0),
                        }
                        .into(),
                        QueryPattern::Or {
                            branches: vec![
                                vec![QueryPattern::Not {
                                    patterns: vec![QueryPattern::Value {
                                        comparator: ValueComparator::Equal,
                                        left: operand.clone(),
                                        right: operand.clone(),
                                    }],
                                }],
                                lookup(2),
                            ],
                        }
                        .into(),
                    ],
                },
                allocation,
                Stage::Attach {
                    items: vec![AttachItem::Links {
                        optional: true,
                        relation: id(1),
                        role: RoleId::new("purchase", "buyer").unwrap(),
                        player: id(0),
                    }],
                },
            ],
            vec![1],
        );
        request.inputs = vec![InputColumn::new(
            column,
            QueryVariable::new("player").unwrap(),
            ValueTypeTag::String,
            true,
        )];
        request.given_rows = Some(vec![InputRow::new(vec![Some(CanonicalValue::String(
            CanonicalString::new("0x01").unwrap(),
        ))])]);
        let valid = QueryPlanV3::new(request.clone(), &schema()).unwrap();
        let mut null = request.clone();
        null.given_rows = Some(vec![InputRow::new(vec![None])]);
        let mut unguarded = request.clone();
        let Stage::Match { patterns } = &mut unguarded.pipeline[0] else {
            unreachable!()
        };
        patterns.pop();
        let mut mismatched = request.clone();
        let Stage::Match { patterns } = &mut mismatched.pipeline[0] else {
            unreachable!()
        };
        let type_bridge_contract::query_plan::v3::Pattern::Read(QueryPattern::Or { branches }) =
            &mut patterns[1]
        else {
            unreachable!()
        };
        let QueryPattern::Iid { iid, .. } = &mut branches[1][1] else {
            unreachable!()
        };
        *iid = string("0x02");
        let mut deleted_alias = request.clone();
        deleted_alias.bindings.push(binding(3, "deleted_alias"));
        let Stage::Match { patterns } = &mut deleted_alias.pipeline[0] else {
            unreachable!()
        };
        patterns.extend(lookup(3).into_iter().map(Into::into));
        deleted_alias.pipeline.insert(
            1,
            Stage::Delete {
                items: vec![DeleteItem::Thing { binding: id(3) }],
            },
        );
        let mut no_rows = request.clone();
        no_rows.given_rows = None;
        let mut empty = request.clone();
        empty.given_rows = Some(vec![]);
        for (unsafe_request, code) in [
            (null, "query_plan_insert_relation_players"),
            (unguarded, "query_plan_insert_relation_players"),
            (mismatched, "query_plan_insert_relation_players"),
            (deleted_alias, "query_plan_insert_relation_players"),
            (no_rows, "query_plan_insert_relation_players"),
            (empty, "query_invocation_missing_inputs"),
        ] {
            assert_eq!(
                QueryPlanV3::new(unsafe_request, &schema())
                    .unwrap_err()
                    .code()
                    .as_str(),
                code
            );
        }
        assert_eq!(
            valid
                .with_given_rows(vec![InputRow::new(vec![None])], &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_insert_relation_players"
        );
        let mut wire: Value = serde_json::from_slice(&to_canonical_json(&valid).unwrap()).unwrap();
        wire["given_rows"] = json!([[null]]);
        assert_eq!(
            decode_query_plan_v3(&to_canonical_json(&wire).unwrap(), &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_insert_relation_players"
        );
    }
}

#[test]
fn relation_insert_seeds_require_a_later_player_attachment() {
    let seed = Stage::Insert {
        things: vec![InsertThing {
            binding: id(1),
            type_id: relation(),
            fields: vec![],
            players: vec![],
        }],
    };
    let attach = Stage::Attach {
        items: vec![AttachItem::Links {
            optional: true,
            relation: id(1),
            role: role(),
            player: id(0),
        }],
    };
    QueryPlanV3::new(
        spec(vec![matched(), seed.clone(), attach], vec![1]),
        &schema(),
    )
    .unwrap();
    assert_eq!(
        QueryPlanV3::new(spec(vec![matched(), seed], vec![1]), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_insert_relation_players"
    );
}
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm, QueryOutput, QueryPattern,
    QueryPlan, ReadStage, ReduceAssignment, Reducer, decode_query_plan,
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
                include_subtypes: false,
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
fn native_plan_refuses_removed_compatibility_evidence() {
    let plan = QueryPlanV3::new(spec(vec![matched()], vec![0]), &schema()).unwrap();
    let bytes = plan.canonical_bytes().unwrap();
    let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire.get("compatibility").is_none());
    for compatibility in [Value::Null, json!({})] {
        wire["compatibility"] = compatibility;
        assert_eq!(
            decode_query_plan_v3(&to_canonical_json(&wire).unwrap(), &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "invalid_canonical_value"
        );
    }
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
fn native_input_template_binds_without_mutating_the_template() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::v3::{Operation, PlanSpec};
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryPattern,
    };
    use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
    let template = QueryPlanV3::new(
        PlanSpec {
            bindings: vec![binding(0, "account"), binding(1, "score")],
            inputs: vec![InputColumn::new(
                InputColumnId::new(0),
                type_bridge_contract::migration_assertion::QueryVariable::new("minimum").unwrap(),
                ValueTypeTag::Long,
                false,
            )],
            given_rows: None,
            operation: Operation::Rows,
            pipeline: vec![Stage::Match {
                patterns: vec![
                    isa(0, entity("account")).into(),
                    QueryPattern::Has {
                        owner: id(0),
                        attribute: id(1),
                        attribute_id: AttributeId::new("score").unwrap(),
                    }
                    .into(),
                    QueryPattern::Value {
                        comparator: ValueComparator::Greater,
                        left: QueryOperand::Binding { binding: id(1) },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
                    }
                    .into(),
                ],
            }],
            output: QueryOutput::Rows {
                columns: vec![id(0)],
            },
            managed_semantics: semantics(),
            functions: vec![],
        },
        &schema(),
    )
    .unwrap();
    let template_bytes = template.canonical_bytes().unwrap();
    assert!(template.given_rows().is_none());
    assert_eq!(
        decode_query_plan_v3(&template_bytes, &schema()).unwrap(),
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
    assert_eq!(template.canonical_bytes().unwrap(), template_bytes);
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
    plans.insert("purchase-read-back", examples::purchase_read_back());
    plans.insert("fetch_entity", examples::fetch_written(false));
    plans.insert("fetch_relation", examples::fetch_written(true));
    plans.insert("delete_returning_entity", examples::delete_returning(false));
    plans.insert(
        "delete_returning_relation",
        examples::delete_returning(true),
    );
    plans.insert("attach", examples::attach());
    plans.insert("replace_players", examples::replace_players());
    plans.insert("rematch_window", examples::rematch_window());
    plans.insert("iid_expression", examples::iid_expression());
    use type_bridge_contract::migration_assertion::ValueComparator;
    for (name, comparator) in [
        ("compare_contains", ValueComparator::Contains),
        ("compare_starts_with", ValueComparator::StartsWith),
        ("compare_ends_with", ValueComparator::EndsWith),
        ("compare_like", ValueComparator::Like),
    ] {
        plans.insert(name, examples::compare_string(comparator));
    }

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
    // The ORM owns explanation fixtures because it owns provider lowering.
    let mut expected: std::collections::BTreeSet<_> = files.keys().cloned().collect();
    expected.insert("explain".to_owned());
    assert_eq!(actual, expected);
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
fn attach_adds_to_bound_things_and_refuses_unbound_targets() {
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
    let key = QueryPlanV3::new(
        attach(
            vec![AttachItem::Has {
                optional: false,
                owner: id(0),
                field: field("id"),
                operand: string("account-c"),
            }],
            vec![pat(0, entity("account"))],
        ),
        &schema(),
    )
    .unwrap();
    assert!(
        key.required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.write.attach-key")
    );
    for (plan, code) in [
        (
            attach(
                vec![AttachItem::Has {
                    optional: false,
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
                    optional: false,
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

#[test]
fn typedb_3_14_query_rules_are_refused_at_build_on_every_profile() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
    let shift = || {
        vec![
            isa(0, entity("account")).into(),
            QueryPattern::Has {
                owner: id(0),
                attribute: id(1),
                attribute_id: AttributeId::new("shift").unwrap(),
            }
            .into(),
        ]
    };
    // Sort on a duration binding: unordered value type.
    assert_eq!(
        QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match { patterns: shift() },
                    Stage::Sort {
                        terms: vec![OrderTerm::new(id(1), OrderDirection::Ascending)],
                    },
                ],
                vec![0],
            ),
            &schema_with_duration(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_plan_sort_unordered_value"
    );
    // Range comparison with a duration operand, also inside a branch.
    let range = |comparator| {
        let mut patterns = shift();
        patterns.push(
            QueryPattern::Or {
                branches: vec![
                    vec![QueryPattern::Value {
                        comparator,
                        left: QueryOperand::Binding { binding: id(1) },
                        right: QueryOperand::Binding { binding: id(1) },
                    }],
                    vec![isa(0, entity("account"))],
                ],
            }
            .into(),
        );
        QueryPlanV3::new(
            spec(vec![Stage::Match { patterns }], vec![0]),
            &schema_with_duration(),
        )
    };
    assert_eq!(
        range(ValueComparator::GreaterOrEqual)
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_duration_range"
    );
    assert!(
        range(ValueComparator::Equal).is_ok(),
        "duration equality stays admitted"
    );
}

#[test]
fn deletion_snapshot_keeps_pre_delete_authority_and_requires_write_rows() {
    use type_bridge_contract::query_plan::v3::{Operation, ThingDocument, WriteReturns};
    let pipeline = vec![
        matched(),
        Stage::Fetch(ThingDocument::new(id(0), vec![])),
        Stage::Delete {
            items: vec![DeleteItem::Thing { binding: id(0) }],
        },
    ];
    let mut candidate = spec(pipeline.clone(), vec![]);
    candidate.operation = Operation::Write {
        returns: WriteReturns::Rows,
    };
    let plan = QueryPlanV3::new(candidate, &schema()).unwrap();
    assert!(plan.fetch_document_shape().is_some());
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|c| c.as_str() == "query.write.delete-snapshot")
    );
    assert_eq!(
        decode_query_plan_v3(&plan.canonical_bytes().unwrap(), &schema()).unwrap(),
        plan
    );
    for operation in [
        Operation::Rows,
        Operation::Count,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
    ] {
        let mut candidate = spec(pipeline.clone(), vec![]);
        candidate.operation = operation;
        assert_eq!(
            QueryPlanV3::new(candidate, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_fetch_delete_operation"
        );
    }
    let mut wrong_target = pipeline;
    wrong_target[2] = Stage::Delete {
        items: vec![DeleteItem::Has {
            owner: id(0),
            field: field("score"),
        }],
    };
    let mut candidate = spec(wrong_target, vec![]);
    candidate.operation = Operation::Write {
        returns: WriteReturns::Rows,
    };
    assert_eq!(
        QueryPlanV3::new(candidate, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_fetch_delete_target"
    );
}

#[test]
fn deletion_snapshot_refuses_replayed_mutations_and_nonterminal_fetches() {
    use type_bridge_contract::query_plan::v3::{Operation, ThingDocument, WriteReturns};
    let fetch = Stage::Fetch(ThingDocument::new(id(0), vec![]));
    let delete = Stage::Delete {
        items: vec![DeleteItem::Thing { binding: id(0) }],
    };
    for (pipeline, code) in [
        (
            vec![
                Stage::Insert {
                    things: vec![thing(0)],
                },
                fetch.clone(),
                delete.clone(),
            ],
            "query_plan_fetch_delete_prefix",
        ),
        (
            vec![matched(), fetch.clone(), delete.clone(), Stage::Distinct],
            "query_plan_fetch_not_last",
        ),
        (
            vec![matched(), fetch, Stage::Distinct, delete],
            "query_plan_fetch_not_last",
        ),
    ] {
        let mut candidate = spec(pipeline, vec![]);
        candidate.operation = Operation::Write {
            returns: WriteReturns::Rows,
        };
        assert_eq!(
            QueryPlanV3::new(candidate, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            code
        );
    }
    let ordinary = QueryPlanV3::new(
        spec(
            vec![
                matched(),
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
            ],
            vec![],
        ),
        &schema(),
    )
    .unwrap();
    assert!(
        !ordinary
            .required_capabilities()
            .iter()
            .any(|cap| cap.as_str() == "query.write.delete-snapshot")
    );
}

#[test]
fn diagnostic_shape_fingerprint_omits_given_rows_and_uses_its_own_domain() {
    let plan = examples::purchase();
    let original_bytes = plan.canonical_bytes().unwrap();
    let original_fingerprint = plan.fingerprint().unwrap();
    let mut changed: Value = serde_json::from_slice(&original_bytes).unwrap();
    changed["given_rows"][0][0]["value"] = json!("other-purchase");
    let other = decode_query_plan_v3(&to_canonical_json(&changed).unwrap(), &schema()).unwrap();
    let shape = plan.shape_fingerprint().unwrap();
    assert_eq!(shape, other.shape_fingerprint().unwrap());
    assert_eq!(shape.domain().as_str(), "typebridge.query.plan-shape");
    assert_ne!(
        shape.digest(),
        original_fingerprint.as_fingerprint().digest()
    );
    assert_ne!(
        shape.digest(),
        other.fingerprint().unwrap().as_fingerprint().digest()
    );
    assert_ne!(original_fingerprint, other.fingerprint().unwrap());
    assert_eq!(original_bytes, plan.canonical_bytes().unwrap());
    assert_eq!(original_fingerprint, plan.fingerprint().unwrap());
}

#[test]
fn string_comparators_require_string_domains_and_bind_input_values() {
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::{InputRow, QueryPattern};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    for comparator in [
        ValueComparator::Contains,
        ValueComparator::StartsWith,
        ValueComparator::EndsWith,
        ValueComparator::Like,
    ] {
        let plan = examples::compare_string(comparator);
        assert!(
            plan.required_capabilities()
                .iter()
                .any(|c| c.as_str() == "query.pattern.string-operators")
        );
        let different = plan
            .with_given_rows(
                vec![InputRow::new(vec![Some(CanonicalValue::String(
                    CanonicalString::new("secret").unwrap(),
                ))])],
                &schema(),
            )
            .unwrap();
        assert_ne!(
            plan.fingerprint().unwrap(),
            different.fingerprint().unwrap()
        );
        let mut bad = serde_json::to_value(&plan).unwrap();
        // Both operands are present and typed, but integer comparisons are invalid.
        let value = &mut bad["pipeline"][0]["patterns"][2];
        *value = serde_json::to_value(QueryPattern::Value {
            comparator,
            left: literal(1),
            right: literal(2),
        })
        .unwrap();
        let error = decode_query_plan_v3(&to_canonical_json(&bad).unwrap(), &schema()).unwrap_err();
        assert_eq!(error.code().as_str(), "query_plan_compare_string_type");
    }
}

#[test]
fn string_comparisons_validate_branch_local_attribute_domains() {
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, QueryOperand, QueryPattern,
    };
    use type_bridge_contract::value::ValueTypeTag;
    let inner = vec![
        QueryPattern::Has {
            owner: id(0),
            attribute: id(1),
            attribute_id: AttributeId::new("id").unwrap(),
        },
        QueryPattern::Value {
            comparator: ValueComparator::Contains,
            left: QueryOperand::Binding { binding: id(1) },
            right: QueryOperand::Input {
                column: InputColumnId::new(0),
            },
        },
    ];
    for nested in [
        QueryPattern::Not {
            patterns: inner.clone(),
        },
        QueryPattern::Try {
            patterns: inner.clone(),
        },
        QueryPattern::Or {
            branches: vec![inner.clone(), inner],
        },
    ] {
        let mut spec = spec(
            vec![Stage::Match {
                patterns: vec![isa(0, entity("account")).into(), nested.into()],
            }],
            vec![0],
        );
        spec.inputs = vec![InputColumn::new(
            InputColumnId::new(0),
            type_bridge_contract::migration_assertion::QueryVariable::new("needle").unwrap(),
            ValueTypeTag::String,
            false,
        )];
        QueryPlanV3::new(spec, &schema()).unwrap();
    }
}

#[test]
fn rematch_retains_only_the_selected_scope_and_preserves_other_order_refusals() {
    let plan = examples::rematch_window();
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|cap| cap.as_str() == "query.pipeline.rematch")
    );
    assert_eq!(plan.statement_counts(), (1, 0));
    let bytes = plan.canonical_bytes().unwrap();
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    let mut hidden = original.clone();
    hidden["pipeline"][6]["patterns"] = json!([{
        "kind": "value", "comparator": "equal",
        "left": {"kind": "binding", "binding": 2},
        "right": {"kind": "literal", "value": {"kind": "string", "value": "A"}}
    }]);
    assert_eq!(
        decode_query_plan_v3(&to_canonical_json(&hidden).unwrap(), &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_fetch_untyped_thing"
    );
    for case in 0..4 {
        let mut invalid = original.clone();
        let stages = invalid["pipeline"].as_array_mut().unwrap();
        match case {
            0 => stages.insert(7, stages[6].clone()),
            1 => {
                stages.drain(1..6);
            }
            2 => {
                stages.swap(2, 3);
            }
            _ => {
                stages.insert(
                    1,
                    json!({"kind":"update", "sets":[{
                        "kind":"field", "owner":0,
                        "field":{"owner":{"kind":"entity","label":"account"}, "attribute":"score"},
                        "operand":{"kind":"literal","value":{"kind":"long","value":"1"}}
                    }]}),
                );
                invalid["operation"] = json!({"kind":"write", "returns":"rows"});
            }
        }
        assert_eq!(
            decode_query_plan_v3(&to_canonical_json(&invalid).unwrap(), &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_stage_order"
        );
    }
}

#[test]
fn iid_expressions_require_present_things_and_infer_string_inside_calls() {
    use type_bridge_contract::query_plan::v3::{Builtin, Expr, Pattern};
    let plan = examples::iid_expression();
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|cap| cap.as_str() == "query.expr.iid")
    );
    assert_eq!(
        decode_query_plan_v3(&plan.canonical_bytes().unwrap(), &schema()).unwrap(),
        plan
    );
    for typed in [entity("account"), relation()] {
        QueryPlanV3::new(
            spec(
                vec![Stage::Match {
                    patterns: vec![
                        isa(0, typed).into(),
                        Pattern::Let {
                            binding: id(2),
                            expr: Expr::Call {
                                func: Builtin::Length,
                                args: vec![Expr::Iid { binding: id(0) }],
                            },
                        },
                    ],
                }],
                vec![2],
            ),
            &schema(),
        )
        .unwrap();
    }
    for patterns in [
        vec![isa(0, entity("account")).into()],
        vec![
            isa(0, entity("account")).into(),
            type_bridge_contract::query_plan::QueryPattern::Has {
                owner: id(0),
                attribute: id(1),
                attribute_id: type_bridge_contract::id::AttributeId::new("id").unwrap(),
            }
            .into(),
        ],
        vec![
            isa(0, entity("account")).into(),
            type_bridge_contract::query_plan::QueryPattern::Try {
                patterns: vec![isa(1, entity("account"))],
            }
            .into(),
        ],
    ] {
        let mut patterns = patterns;
        patterns.push(Pattern::Let {
            binding: id(2),
            expr: Expr::Iid { binding: id(1) },
        });
        assert_eq!(
            QueryPlanV3::new(spec(vec![Stage::Match { patterns }], vec![2]), &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_iid_untyped_thing"
        );
    }
}

#[test]
fn schema_derived_zero_hop_branches_share_the_predicate_node_budget() {
    use type_bridge_contract::limits::QueryLimits;
    let spec = spec(
        vec![Stage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: id(0),
                    type_id: entity("account"),
                    include_subtypes: true,
                }
                .into(),
                QueryPattern::Isa {
                    binding: id(1),
                    type_id: entity("account"),
                    include_subtypes: true,
                }
                .into(),
                QueryPattern::Reachable {
                    min_depth: 0,
                    max_depth: 0,
                    relation: relation(),
                    role_from: role(),
                    role_to: role(),
                    source: id(0),
                    target: id(1),
                }
                .into(),
            ],
        }],
        vec![0],
    );
    QueryPlanV3::new(spec.clone(), &schema()).unwrap();
    let mut limits = QueryLimits::CANONICAL;
    limits.structural.predicate_nodes = 4;
    let error = QueryPlanV3::new_with_limits(spec, &schema(), limits).unwrap_err();
    assert_eq!(
        error.code().as_str(),
        "query_plan_reachable_expansion_limit"
    );
}

#[test]
fn native_output_columns_use_the_binding_budget_while_v1_remains_frozen() {
    use type_bridge_contract::limits::{MAX_BINDINGS, QueryLimits};
    let columns = (0..17).collect::<Vec<_>>();
    let patterns = columns
        .iter()
        .map(|column| isa(*column, entity("account")))
        .collect::<Vec<_>>();
    let mut wide = spec(
        vec![Stage::Match {
            patterns: patterns.iter().cloned().map(Into::into).collect(),
        }],
        columns.clone(),
    );
    wide.bindings = columns
        .iter()
        .map(|column| binding(*column, &format!("b{column}")))
        .collect();
    let plan = QueryPlanV3::new(wide.clone(), &schema()).unwrap();
    assert_eq!(plan.limits().structural.selected_slots, MAX_BINDINGS);
    assert!(
        QueryPlan::new(
            wide.bindings.clone(),
            vec![],
            vec![ReadStage::Match { patterns }],
            wide.output.clone(),
            wide.managed_semantics.clone()
        )
        .is_err()
    );
    let lower =
        QueryPlanV3::new_with_limits(wide.clone(), &schema(), QueryLimits::CANONICAL).unwrap_err();
    assert_eq!(lower.code().as_str(), "query_plan_output_limit");
    let mut invalid = QueryLimits::V3;
    invalid.structural.bindings = 16;
    invalid.structural.selected_slots = 17;
    assert_eq!(invalid.validate_v3().unwrap_err().limit(), "selected_slots");
    let invalid = QueryPlanV3::new_with_limits(wide, &schema(), invalid).unwrap_err();
    assert_eq!(
        invalid.details().get("limit"),
        Some(
            &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text("selected_slots".into())
        )
    );
    assert_eq!(
        invalid.details().get("maximum"),
        Some(&type_bridge_contract::diagnostic::DiagnosticDetailValue::Long(16))
    );
}

#[test]
fn native_write_stages_refuse_ordered_collections_without_a_scalar_escape() {
    use type_bridge_contract::diagnostic::DiagnosticCategory;
    use type_bridge_contract::query_plan::v3::AttachItem;
    let schema = schema_with_ordered_collections();
    let mut stages = stage_plans()
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                *name,
                "insert" | "put" | "update" | "delete_has" | "link" | "delete_links"
            )
        })
        .map(|(name, plan)| (name, plan.pipeline().to_vec()))
        .collect::<Vec<_>>();
    for (name, stage) in [
        (
            "attach_field",
            Stage::Attach {
                items: vec![AttachItem::Has {
                    owner: id(0),
                    field: field("score"),
                    operand: literal(2),
                    optional: false,
                }],
            },
        ),
        (
            "attach_role",
            Stage::Attach {
                items: vec![AttachItem::Links {
                    relation: id(1),
                    role: role(),
                    player: id(0),
                    optional: false,
                }],
            },
        ),
        (
            "clear_role",
            Stage::Delete {
                items: vec![DeleteItem::Players {
                    relation: id(1),
                    role: role(),
                }],
            },
        ),
    ] {
        stages.push((
            name,
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("account")), pat(1, relation())],
                },
                stage,
            ],
        ));
    }
    for (name, pipeline) in stages {
        let error = QueryPlanV3::new(spec(pipeline, vec![]), &schema).unwrap_err();
        assert_eq!(
            error.category(),
            DiagnosticCategory::UnsupportedCapability,
            "{name}"
        );
        assert_eq!(
            error.code().as_str(),
            "query_plan_ordered_collection_unsupported",
            "{name}"
        );
    }
}

#[test]
fn ownership_delete_accepts_only_bound_schema_typed_optional_owners() {
    use type_bridge_contract::query_plan::v3::{Expr, Pattern};
    for (patterns, owner, expected) in [
        (
            vec![
                isa(0, entity("account")).into(),
                QueryPattern::Try {
                    patterns: vec![isa(1, entity("account"))],
                }
                .into(),
            ],
            id(1),
            None,
        ),
        (
            vec![
                isa(0, entity("account")).into(),
                Pattern::Let {
                    binding: id(1),
                    expr: Expr::Operand(literal(1)),
                },
            ],
            id(1),
            Some("query_plan_write_untyped_target"),
        ),
        (
            vec![isa(0, entity("account")).into()],
            id(9),
            Some("query_plan_write_unbound_target"),
        ),
        (
            vec![isa(0, entity("account")).into()],
            id(1),
            Some("query_plan_write_unbound_target"),
        ),
    ] {
        let plan = QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match { patterns },
                    Stage::Delete {
                        items: vec![DeleteItem::Has {
                            owner,
                            field: field("score"),
                        }],
                    },
                ],
                vec![],
            ),
            &schema(),
        );
        match expected {
            None => {
                plan.unwrap();
            }
            Some(code) => assert_eq!(plan.unwrap_err().code().as_str(), code),
        }
    }
}

#[test]
fn write_rematches_keep_selected_bindings_and_admit_fresh_bindings() {
    use type_bridge_contract::query_plan::v3::Operation;
    let put = |target, key| Stage::Put {
        include_subtypes: false,
        things: vec![InsertThing {
            binding: id(target),
            type_id: entity("account"),
            fields: vec![(field("id"), string(key))],
            players: vec![],
        }],
    };
    let pipeline = vec![
        put(0, "a"),
        Stage::Select {
            bindings: vec![id(0)],
        },
        Stage::Distinct,
        Stage::Match {
            patterns: vec![isa(1, entity("account")).into()],
        },
        put(2, "b"),
        Stage::Select {
            bindings: vec![id(0), id(1), id(2)],
        },
        Stage::Distinct,
        Stage::Match {
            patterns: vec![isa(3, entity("account")).into()],
        },
        Stage::Attach {
            items: vec![AttachItem::Has {
                optional: false,
                owner: id(2),
                field: field("score"),
                operand: literal(7),
            }],
        },
    ];
    let mut request = spec(pipeline, vec![0, 1, 2, 3]);
    request.bindings.push(binding(3, "again"));
    QueryPlanV3::new(request.clone(), &schema()).unwrap();
    let mut absent = request.clone();
    absent.pipeline[5] = Stage::Select {
        bindings: vec![id(0), id(1)],
    };
    absent.output = type_bridge_contract::query_plan::QueryOutput::Rows {
        columns: vec![id(0)],
    };
    assert_eq!(
        QueryPlanV3::new(absent, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_write_unbound_target"
    );
    let mut reused = request.clone();
    reused.pipeline[4] = put(0, "duplicate");
    assert_eq!(
        QueryPlanV3::new(reused, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_write_binding_not_fresh"
    );
    let mut unselected = request;
    unselected.pipeline.drain(1..3);
    assert_eq!(
        QueryPlanV3::new(unselected, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_stage_order"
    );
    let mut read = spec(
        vec![
            matched(),
            Stage::Select {
                bindings: vec![id(0)],
            },
            Stage::Distinct,
            Stage::Match {
                patterns: vec![isa(1, entity("account")).into()],
            },
            Stage::Select {
                bindings: vec![id(0), id(1)],
            },
            Stage::Distinct,
            Stage::Match {
                patterns: vec![isa(2, entity("account")).into()],
            },
        ],
        vec![0],
    );
    read.operation = Operation::Rows;
    assert_eq!(
        QueryPlanV3::new(read, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_stage_order"
    );
}

#[test]
fn optional_has_attachment_requires_a_bound_typed_owner_and_optional_flag() {
    use type_bridge_contract::query_plan::v3::{Expr, Pattern};
    for (patterns, owner, optional, expected) in [
        (
            vec![
                isa(0, entity("account")).into(),
                QueryPattern::Try {
                    patterns: vec![isa(1, entity("account"))],
                }
                .into(),
            ],
            id(1),
            true,
            None,
        ),
        (
            vec![
                isa(0, entity("account")).into(),
                QueryPattern::Try {
                    patterns: vec![isa(1, entity("account"))],
                }
                .into(),
            ],
            id(1),
            false,
            Some("query_plan_write_unbound_target"),
        ),
        (
            vec![
                isa(0, entity("account")).into(),
                Pattern::Let {
                    binding: id(1),
                    expr: Expr::Operand(literal(1)),
                },
            ],
            id(1),
            true,
            Some("query_plan_write_untyped_target"),
        ),
        (
            vec![isa(0, entity("account")).into()],
            id(9),
            true,
            Some("query_plan_write_unbound_target"),
        ),
        (
            vec![isa(0, entity("account")).into()],
            id(1),
            true,
            Some("query_plan_write_unbound_target"),
        ),
    ] {
        let plan = QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match { patterns },
                    Stage::Attach {
                        items: vec![AttachItem::Has {
                            optional,
                            owner,
                            field: field("score"),
                            operand: literal(1),
                        }],
                    },
                ],
                vec![],
            ),
            &schema(),
        );
        match expected {
            None => {
                plan.unwrap();
            }
            Some(code) => assert_eq!(plan.unwrap_err().code().as_str(), code),
        }
    }
}

#[test]
fn native_optional_unions_keep_branch_visibility_and_legacy_fences() {
    let optional = QueryPattern::Try {
        patterns: vec![QueryPattern::Or {
            branches: vec![
                vec![isa(0, entity("account"))],
                vec![isa(0, entity("account"))],
            ],
        }],
    };
    let request = spec(
        vec![Stage::Match {
            patterns: vec![optional.clone().into()],
        }],
        vec![0],
    );
    let plan = QueryPlanV3::new(request.clone(), &schema()).unwrap();
    let bytes = plan.canonical_bytes().unwrap();
    assert_eq!(
        decode_query_plan_v3(&bytes, &schema())
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
    assert_eq!(
        QueryPlan::new(
            request.bindings,
            vec![],
            vec![ReadStage::Match {
                patterns: vec![optional]
            }],
            request.output,
            request.managed_semantics
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_plan_v1_disjunction_unsupported"
    );
    let not_in_every_branch = QueryPattern::Try {
        patterns: vec![QueryPattern::Or {
            branches: vec![
                vec![isa(0, entity("account"))],
                vec![isa(1, entity("account"))],
            ],
        }],
    };
    assert_eq!(
        QueryPlanV3::new(
            spec(
                vec![Stage::Match {
                    patterns: vec![not_in_every_branch.into()]
                }],
                vec![0]
            ),
            &schema()
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_plan_output_not_visible"
    );
    for forbidden in [
        QueryPattern::Not {
            patterns: vec![isa(0, entity("account"))],
        },
        QueryPattern::Try {
            patterns: vec![isa(0, entity("account"))],
        },
    ] {
        for child in [
            forbidden.clone(),
            QueryPattern::Or {
                branches: vec![vec![forbidden]],
            },
        ] {
            assert_eq!(
                QueryPlanV3::new(
                    spec(
                        vec![Stage::Match {
                            patterns: vec![
                                QueryPattern::Try {
                                    patterns: vec![child]
                                }
                                .into()
                            ]
                        }],
                        vec![0]
                    ),
                    &schema()
                )
                .unwrap_err()
                .code()
                .as_str(),
                "query_plan_try_body_unsupported"
            );
        }
    }
}
