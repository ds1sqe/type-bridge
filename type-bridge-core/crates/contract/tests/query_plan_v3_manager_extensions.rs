#[path = "support/plan_v3.rs"]
#[allow(dead_code)]
mod support;

use support::*;
use type_bridge_contract::query_plan::QueryPattern;
use type_bridge_contract::query_plan::v3::{Pattern, QueryPlanV3, Stage, decode_query_plan_v3};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

#[test]
fn nested_iids_are_capability_fenced_and_round_trip_without_changing_root_iids() {
    let iid = "0x1e00000000000000000001";
    let root = || Pattern::Iid {
        binding: id(0),
        iid: iid.into(),
    };
    let nested = || QueryPattern::Iid {
        binding: id(0),
        iid: string(iid),
    };
    let existing = QueryPlanV3::new(
        spec(
            vec![Stage::Match {
                patterns: vec![isa(0, entity("account")).into(), root()],
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap();
    let equivalent = QueryPlanV3::new(
        spec(
            vec![Stage::Match {
                patterns: vec![isa(0, entity("account")).into(), nested().into()],
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap();
    assert_eq!(
        existing.canonical_bytes().unwrap(),
        equivalent.canonical_bytes().unwrap()
    );
    for predicate in [
        QueryPattern::Or {
            branches: vec![
                vec![nested()],
                vec![QueryPattern::Not {
                    patterns: vec![nested()],
                }],
            ],
        },
        QueryPattern::Not {
            patterns: vec![nested()],
        },
    ] {
        let plan = QueryPlanV3::new(
            spec(
                vec![Stage::Match {
                    patterns: vec![isa(0, entity("account")).into(), predicate.into()],
                }],
                vec![0],
            ),
            &schema(),
        )
        .unwrap();
        assert!(
            plan.required_capabilities()
                .iter()
                .any(|capability| capability.as_str() == "query.pattern.nested-iid")
        );
        let bytes = plan.canonical_bytes().unwrap();
        assert_eq!(
            decode_query_plan_v3(&bytes, &schema())
                .unwrap()
                .canonical_bytes()
                .unwrap(),
            bytes
        );
    }
}

#[test]
fn nested_iids_require_canonical_ids_and_positively_typed_things_in_their_scope() {
    for (binding, iid, code) in [
        (id(0), "not-an-iid", "query_plan_invalid_iid"),
        (
            id(1),
            "0x1e00000000000000000001",
            "query_plan_iid_untyped_thing",
        ),
    ] {
        let error = QueryPlanV3::new(
            spec(
                vec![Stage::Match {
                    patterns: vec![
                        isa(0, entity("account")).into(),
                        QueryPattern::Not {
                            patterns: vec![QueryPattern::Iid {
                                binding,
                                iid: string(iid),
                            }],
                        }
                        .into(),
                    ],
                }],
                vec![0],
            ),
            &schema(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), code);
    }
}

#[test]
fn subtype_put_is_opt_in_and_exact_put_bytes_omit_the_member() {
    let exact = QueryPlanV3::new(
        spec(
            vec![Stage::Put {
                things: vec![thing(0)],
                include_subtypes: false,
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap();
    let exact_json: serde_json::Value =
        serde_json::from_slice(&exact.canonical_bytes().unwrap()).unwrap();
    assert!(exact_json["pipeline"][0].get("include_subtypes").is_none());
    let inclusive = QueryPlanV3::new(
        spec(
            vec![Stage::Put {
                things: vec![thing(0)],
                include_subtypes: true,
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap();
    assert!(
        inclusive
            .required_capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.write.put-subtypes")
    );
    assert_eq!(
        inclusive.binding_types(id(0)).unwrap(),
        &std::collections::BTreeSet::from([entity("account"), entity("child_account")])
    );
    let bytes = inclusive.canonical_bytes().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["pipeline"][0]["include_subtypes"], true);
    assert_eq!(
        decode_query_plan_v3(&bytes, &schema())
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
}

#[test]
fn collected_roles_and_scalar_fields_are_default_preserving_and_fail_closed() {
    use std::collections::BTreeMap;
    use type_bridge_contract::migration_assertion::QueryVariable;
    use type_bridge_contract::query_plan::v3::{
        DocumentRole, DocumentScalar, Expr, Operation, ThingDocument,
    };
    use type_bridge_contract::query_remote::RemoteFieldValue;
    use type_bridge_contract::query_remote_v3::StagedAnswerV3;
    let mut document = ThingDocument::new(id(0), vec![DocumentRole::collected(role())]);
    document.scalars.push(DocumentScalar {
        key: QueryVariable::new("ordinal").unwrap(),
        binding: id(2),
    });
    let mut native = spec(
        vec![
            Stage::Match {
                patterns: vec![
                    isa(0, relation()).into(),
                    Pattern::Let {
                        binding: id(2),
                        expr: Expr::Operand(literal(7)),
                    },
                ],
            },
            Stage::Fetch(document),
        ],
        vec![],
    );
    native.operation = Operation::Rows;
    let plan = QueryPlanV3::new(native, &schema()).unwrap();
    let bytes = plan.canonical_bytes().unwrap();
    assert_eq!(
        decode_query_plan_v3(&bytes, &schema())
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
    for expected in ["query.fetch.role-lists", "query.fetch.scalar-fields"] {
        assert!(
            plan.required_capabilities()
                .iter()
                .any(|capability| capability.as_str() == expected)
        );
    }
    let scalar = |value| RemoteFieldValue::Scalar { value };
    let text = |value| scalar(CanonicalValue::String(CanonicalString::new(value).unwrap()));
    let player = BTreeMap::from([
        ("_iid".into(), text("0x02")),
        ("_type".into(), text("account")),
        (
            "attributes".into(),
            RemoteFieldValue::Document {
                fields: BTreeMap::from([("id".into(), text("A"))]),
            },
        ),
    ]);
    let root = BTreeMap::from([
        ("_iid".into(), text("0x01")),
        ("_type".into(), text("membership")),
        (
            "attributes".into(),
            RemoteFieldValue::Document {
                fields: BTreeMap::new(),
            },
        ),
        ("ordinal".into(), scalar(CanonicalValue::Long(7))),
        (
            "_role_0_players".into(),
            RemoteFieldValue::Documents {
                documents: vec![player.clone()],
            },
        ),
    ]);
    let answer = |document| StagedAnswerV3::Documents {
        documents: vec![document],
    };
    answer(root.clone()).validate_for(&plan).unwrap();
    assert_eq!(answer(root.clone()).item_count(), 2);
    assert_eq!(answer(root.clone()).resource_counts(), [2, 1, 2, 1]);
    let mut empty = root.clone();
    empty.insert(
        "_role_0_players".into(),
        RemoteFieldValue::Documents { documents: vec![] },
    );
    answer(empty).validate_for(&plan).unwrap();
    for key in ["ordinal", "_role_0_players"] {
        let mut bad = root.clone();
        bad.remove(key);
        assert!(answer(bad).validate_for(&plan).is_err(), "missing {key}");
    }
    let mut extra = root.clone();
    extra.insert(
        "_role_1_players".into(),
        RemoteFieldValue::Documents { documents: vec![] },
    );
    assert_eq!(
        answer(extra)
            .validate_for(&plan)
            .unwrap_err()
            .code()
            .as_str(),
        "query_remote_v3_document_key"
    );
    let mut duplicate = root.clone();
    duplicate.insert(
        "_role_0_players".into(),
        RemoteFieldValue::Documents {
            documents: vec![player.clone(), player.clone()],
        },
    );
    assert_eq!(
        answer(duplicate)
            .validate_for(&plan)
            .unwrap_err()
            .code()
            .as_str(),
        "query_remote_v3_document_role"
    );
    let mut wrong = player;
    wrong.insert("_type".into(), text("other"));
    let mut bad = root;
    bad.insert(
        "_role_0_players".into(),
        RemoteFieldValue::Documents {
            documents: vec![wrong],
        },
    );
    assert_eq!(
        answer(bad).validate_for(&plan).unwrap_err().code().as_str(),
        "query_remote_v3_document_role"
    );
    let old = ThingDocument::new(id(0), vec![DocumentRole::new(role(), false)]);
    let old = serde_json::to_value(old).unwrap();
    assert!(old.get("scalars").is_none());
    assert!(old["roles"][0].get("collect").is_none());
}

#[test]
fn frozen_plan_constructors_refuse_the_native_only_iid_node() {
    use type_bridge_contract::query_plan::{QueryPlan, ReadStage};
    let source = spec(vec![], vec![0]);
    for nested in [false, true] {
        let iid = QueryPattern::Iid {
            binding: id(0),
            iid: string("0x10"),
        };
        let pattern = if nested {
            QueryPattern::Not {
                patterns: vec![iid],
            }
        } else {
            iid
        };
        let pipeline = vec![ReadStage::Match {
            patterns: vec![isa(0, entity("account")), pattern],
        }];
        for v2 in [false, true] {
            let result = if v2 {
                QueryPlan::new_v2(
                    source.bindings.clone(),
                    source.inputs.clone(),
                    pipeline.clone(),
                    source.output.clone(),
                    source.managed_semantics.clone(),
                )
            } else {
                QueryPlan::new(
                    source.bindings.clone(),
                    source.inputs.clone(),
                    pipeline.clone(),
                    source.output.clone(),
                    source.managed_semantics.clone(),
                )
            };
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_plan_nested_iid_requires_v3"
            );
        }
    }
}

#[test]
fn document_scalars_capture_bound_thing_iids_and_optional_absence() {
    use std::collections::BTreeMap;
    use type_bridge_contract::migration_assertion::QueryVariable;
    use type_bridge_contract::query_plan::v3::{DocumentScalar, ThingDocument};
    use type_bridge_contract::query_remote::RemoteFieldValue;
    use type_bridge_contract::query_remote_v3::StagedAnswerV3;
    let text = |value: &str| RemoteFieldValue::Scalar {
        value: CanonicalValue::String(CanonicalString::new(value).unwrap()),
    };
    for optional in [false, true] {
        let mut document = ThingDocument::new(id(0), vec![]);
        document.scalars.push(DocumentScalar {
            key: QueryVariable::new("reference").unwrap(),
            binding: id(1),
        });
        let peer = isa(1, entity("account"));
        let plan = QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match {
                        patterns: vec![
                            isa(0, entity("account")).into(),
                            if optional {
                                QueryPattern::Try {
                                    patterns: vec![peer],
                                }
                                .into()
                            } else {
                                peer.into()
                            },
                        ],
                    },
                    Stage::Fetch(document.clone()),
                ],
                vec![],
            ),
            &schema(),
        )
        .unwrap();
        let bytes = plan.canonical_bytes().unwrap();
        assert_eq!(
            decode_query_plan_v3(&bytes, &schema())
                .unwrap()
                .canonical_bytes()
                .unwrap(),
            bytes
        );
        let root = BTreeMap::from([
            ("_iid".into(), text("0x01")),
            ("_type".into(), text("account")),
            (
                "attributes".into(),
                RemoteFieldValue::Document {
                    fields: BTreeMap::from([("id".into(), text("A"))]),
                },
            ),
            ("reference".into(), text("0x02")),
        ]);
        let answer = |document| StagedAnswerV3::Documents {
            documents: vec![document],
        };
        answer(root.clone()).validate_for(&plan).unwrap();
        let mut absent = root.clone();
        absent.remove("reference");
        assert_eq!(answer(absent.clone()).validate_for(&plan).is_ok(), optional);
        absent.insert("reference".into(), RemoteFieldValue::Absent);
        assert_eq!(answer(absent).validate_for(&plan).is_ok(), optional);
        let mut malformed = root.clone();
        malformed.insert("reference".into(), text("not-an-iid"));
        assert!(answer(malformed).validate_for(&plan).is_err());
        let mut bad = spec(
            vec![
                Stage::Match {
                    patterns: vec![isa(0, entity("account")).into()],
                },
                Stage::Fetch(document.clone()),
            ],
            vec![],
        );
        assert_eq!(
            QueryPlanV3::new(bad.clone(), &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_fetch_scalar_binding"
        );
        // An optional attribute is not a thing IID and remains outside the allowance.
        let Stage::Match { patterns } = &mut bad.pipeline[0] else {
            unreachable!()
        };
        patterns.push(
            QueryPattern::Try {
                patterns: vec![QueryPattern::Has {
                    attribute: id(1),
                    owner: id(0),
                    attribute_id: type_bridge_contract::id::AttributeId::new("id").unwrap(),
                }],
            }
            .into(),
        );
        assert_eq!(
            QueryPlanV3::new(bad, &schema())
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_fetch_scalar_binding"
        );
    }
}
