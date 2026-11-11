#[allow(dead_code)]
#[path = "support/plan.rs"]
mod support;
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::query_plan::{QueryPlan, Stage, decode_query_plan};
use type_bridge_contract::temporal::CanonicalDuration;
use type_bridge_contract::value::CanonicalValue;

#[test]
fn duration_u64_components_round_trip_through_serde() {
    let values = [
        CanonicalDuration::new(false, u64::from(u32::MAX) + 1, 0, 0, 0).expect("months above u32"),
        CanonicalDuration::new(false, 0, u64::MAX, 0, 0).expect("maximum days"),
        CanonicalDuration::new(false, 0, 0, u64::MAX, 999_999_999).expect("maximum seconds"),
    ];

    for value in values {
        let bytes = serde_json::to_vec(&value).expect("serialize duration");
        assert_eq!(
            serde_json::from_slice::<CanonicalDuration>(&bytes).expect("deserialize duration"),
            value,
        );
    }
}

#[test]
fn u64_duration_boundaries_round_trip_through_query_plan_wire() {
    let above_u32 = CanonicalDuration::new(false, u64::from(u32::MAX) + 1, 0, 0, 0)
        .expect("duration above u32");
    let maximum = CanonicalDuration::new(false, u64::MAX, u64::MAX, u64::MAX, 999_999_999)
        .expect("maximum duration components");
    let comparison = |duration| QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Literal {
            value: CanonicalValue::Duration(duration),
        },
        right: QueryOperand::Literal {
            value: CanonicalValue::Duration(duration),
        },
    };
    let plan = QueryPlan::new(
        support::spec(
            vec![Stage::Match {
                patterns: vec![
                    support::isa(0, support::entity("account")).into(),
                    comparison(above_u32).into(),
                    comparison(maximum).into(),
                ],
            }],
            vec![0],
        ),
        &support::schema(),
    )
    .expect("query plan");

    let bytes = plan.canonical_bytes().expect("canonical query-plan bytes");
    assert_eq!(
        decode_query_plan(&bytes, &support::schema()).expect("decode query plan"),
        plan
    );
}
