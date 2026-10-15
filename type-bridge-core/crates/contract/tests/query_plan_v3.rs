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
                            patterns: vec![isa(0, entity("account")), isa(1, relation())],
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
                patterns: vec![isa(0, entity("other")), isa(1, relation())],
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
        patterns: vec![isa(0, entity("child_account")), isa(1, relation())],
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
