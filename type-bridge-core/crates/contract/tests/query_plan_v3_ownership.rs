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
