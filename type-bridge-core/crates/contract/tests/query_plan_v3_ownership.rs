#[path = "support/plan_v3.rs"]
#[allow(dead_code)]
mod support;
use support::*;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::query_plan::QueryPattern;
use type_bridge_contract::query_plan::v3::{DeleteItem, QueryPlanV3, Stage, decode_query_plan_v3};

fn selective_unlink() -> DeleteItem {
    DeleteItem::HasValue {
        owner: id(0),
        field: field("score"),
        value: id(1),
    }
}
fn matched_attribute(name: &str) -> Stage {
    Stage::Match {
        patterns: vec![
            isa(0, entity("account")).into(),
            QueryPattern::Has {
                owner: id(0),
                attribute: id(1),
                attribute_id: AttributeId::new(name).unwrap(),
            }
            .into(),
        ],
    }
}
#[test]
fn selective_unlink_round_trips_and_preserves_the_bound_attribute() {
    let plan = QueryPlanV3::new(
        spec(
            vec![
                matched_attribute("score"),
                Stage::Delete {
                    items: vec![selective_unlink()],
                },
            ],
            vec![0, 1],
        ),
        &schema(),
    )
    .unwrap();
    assert_eq!(plan.statement_counts(), (0, 1));
    assert_eq!(
        plan.binding_types(id(1)).unwrap(),
        &std::collections::BTreeSet::from([TypeId::new(TypeKind::Attribute, "score").unwrap()])
    );
    let bytes = plan.canonical_bytes().unwrap();
    assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/plan_v3/delete_has_value.json");
    if std::env::var_os("TYPE_BRIDGE_WRITE_OWNERSHIP_FIXTURES").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[test]
fn selective_unlink_rejects_missing_wrong_or_twice_deleted_values() {
    for (matched, items, expected) in [
        (
            matched(),
            vec![selective_unlink()],
            "query_plan_write_unbound_target",
        ),
        (
            matched_attribute("id"),
            vec![selective_unlink()],
            "query_plan_delete_ownership_value_type",
        ),
        (
            matched_attribute("score"),
            vec![selective_unlink(), selective_unlink()],
            "query_plan_duplicate_ownership_delete",
        ),
    ] {
        let error = QueryPlanV3::new(
            spec(vec![matched, Stage::Delete { items }], vec![0]),
            &schema(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), expected);
    }
    let error = QueryPlanV3::new(
        spec(
            vec![
                matched_attribute("score"),
                Stage::Delete {
                    items: vec![selective_unlink()],
                },
                Stage::Delete {
                    items: vec![selective_unlink()],
                },
            ],
            vec![0],
        ),
        &schema(),
    )
    .unwrap_err();
    assert_eq!(
        error.code().as_str(),
        "query_plan_duplicate_ownership_delete"
    );
}

#[test]
fn selective_unlink_accepts_an_attribute_subtype_but_refuses_a_thing() {
    let schema = schema_with_attribute_subtype();
    for (value_type, expected) in [
        (
            TypeId::new(TypeKind::Attribute, "special_score").unwrap(),
            true,
        ),
        (entity("account"), false),
    ] {
        let plan = QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match {
                        patterns: vec![isa(0, entity("account")).into(), isa(1, value_type).into()],
                    },
                    Stage::Delete {
                        items: vec![selective_unlink()],
                    },
                ],
                vec![0, 1],
            ),
            &schema,
        );
        if expected {
            assert!(plan.is_ok(), "{plan:?}");
        } else {
            assert_eq!(
                plan.unwrap_err().code().as_str(),
                "query_plan_delete_ownership_value_type"
            );
        }
    }
}

fn mode_plan(
    mode: type_bridge_contract::query_plan::TypeMatchMode,
    schema: &type_bridge_contract::schema::DeclaredSchema,
) -> Result<QueryPlanV3, type_bridge_contract::diagnostic::Diagnostic> {
    QueryPlanV3::new(
        spec(
            vec![Stage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: id(0),
                        type_id: entity("account"),
                        include_subtypes: true,
                    }
                    .into(),
                    QueryPattern::HasType {
                        owner: id(0),
                        attribute: id(1),
                        attribute_id: AttributeId::new("score").unwrap(),
                        mode,
                    }
                    .into(),
                ],
            }],
            vec![0, 1],
        ),
        schema,
    )
}

#[test]
fn ownership_modes_are_explicit_and_round_trip_without_changing_exact_has() {
    use type_bridge_contract::query_plan::TypeMatchMode;
    for mode in [TypeMatchMode::Isa, TypeMatchMode::IsaExact] {
        let plan = mode_plan(mode, &schema()).unwrap();
        let bytes = plan.canonical_bytes().unwrap();
        assert_eq!(decode_query_plan_v3(&bytes, &schema()).unwrap(), plan);
        let mut wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        wire["pipeline"][0]["patterns"][1]
            .as_object_mut()
            .unwrap()
            .remove("mode");
        let missing = type_bridge_contract::codec::to_canonical_json(&wire).unwrap();
        let error = decode_query_plan_v3(&missing, &schema()).unwrap_err();
        assert_eq!(error.code().as_str(), "query_plan_type_match_mode");
        assert!(error.message().contains("isa or isa!"));
        if mode == TypeMatchMode::Isa {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/contracts/plan_v3/has_type.json");
            if std::env::var_os("TYPE_BRIDGE_WRITE_OWNERSHIP_FIXTURES").is_some() {
                std::fs::write(&path, &bytes).unwrap();
            }
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
    assert!(TypeMatchMode::from_typeql("subtypes").is_err());
}

#[test]
fn abstract_attribute_isa_matches_only_the_owning_subtype_and_isa_exact_refuses() {
    use type_bridge_contract::query_plan::TypeMatchMode;
    let schema = schema_with_ownership_modes(true);
    let plan = mode_plan(TypeMatchMode::Isa, &schema).unwrap();
    assert_eq!(
        plan.binding_types(id(0)).unwrap(),
        &std::collections::BTreeSet::from([entity("child_account")])
    );
    assert_eq!(
        plan.binding_types(id(1)).unwrap(),
        &std::collections::BTreeSet::from([
            TypeId::new(TypeKind::Attribute, "special_score").unwrap()
        ])
    );
    assert_eq!(
        mode_plan(TypeMatchMode::IsaExact, &schema)
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_abstract_exact_ownership"
    );
    let exact = mode_plan(TypeMatchMode::IsaExact, &schema_with_ownership_modes(false)).unwrap();
    assert_eq!(
        exact.binding_types(id(1)).unwrap(),
        &std::collections::BTreeSet::from([TypeId::new(TypeKind::Attribute, "score").unwrap()])
    );
}

#[test]
fn exact_abstract_mode_is_refused_even_under_negation() {
    use type_bridge_contract::query_plan::TypeMatchMode;
    let schema = schema_with_ownership_modes(true);
    let error = QueryPlanV3::new(
        spec(
            vec![Stage::Match {
                patterns: vec![
                    isa(0, entity("account")).into(),
                    QueryPattern::Not {
                        patterns: vec![QueryPattern::HasType {
                            owner: id(0),
                            attribute: id(1),
                            attribute_id: AttributeId::new("score").unwrap(),
                            mode: TypeMatchMode::IsaExact,
                        }],
                    }
                    .into(),
                ],
            }],
            vec![0],
        ),
        &schema,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_plan_abstract_exact_ownership");
}
