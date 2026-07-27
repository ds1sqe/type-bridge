use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    QueryOperand, QueryOutput, QueryPattern, QueryPlan, ReadStage, decode_query_plan,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::temporal::CanonicalDuration;
use type_bridge_contract::value::CanonicalValue;

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn managed_semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"duration-query-plan-wire-fixture",
    )
    .expect("managed semantic fingerprint")
}

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
    let entity = BindingId::new(0).expect("binding id");
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
        vec![binding(0, "person")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: entity,
                    include_subtypes: true,
                    type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
                },
                comparison(above_u32),
                comparison(maximum),
            ],
        }],
        QueryOutput::Rows {
            columns: vec![entity],
        },
        managed_semantics(),
    )
    .expect("query plan");

    let bytes = plan.canonical_bytes().expect("canonical query-plan bytes");
    assert_eq!(decode_query_plan(&bytes).expect("decode query plan"), plan);
}
