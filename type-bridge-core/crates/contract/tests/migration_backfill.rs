use serde_json::json;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_backfill::{
    AttributeBackfillPlan, BackfillConflictPolicy, BackfillPartition, BackfillPostcondition,
    BackfillReverseProgram, BackfillValueTransform, COPY_ATTRIBUTE_BACKFILL_CAPABILITY,
    MAX_BACKFILL_BATCH_ROWS, decode_attribute_backfill_plan,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;

fn semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"migration-backfill-intermediate-schema",
    )
    .expect("managed semantic fingerprint")
}

fn plan() -> AttributeBackfillPlan {
    AttributeBackfillPlan::new(
        TypeId::new(TypeKind::Entity, "person").expect("owner"),
        AttributeId::new("legacy-name").expect("source"),
        AttributeId::new("display-name").expect("destination"),
        BackfillPartition::new(
            512,
            AttributeId::new("person-id").expect("stable partition attribute"),
        )
        .expect("partition"),
        semantics(),
        Some(BackfillReverseProgram::RemoveEqualCopiedDestination),
    )
    .expect("backfill plan")
}

#[test]
fn copy_attribute_plan_freezes_closed_semantics_and_canonical_wire() {
    let plan = plan();
    assert_eq!(
        plan.conflict(),
        BackfillConflictPolicy::SkipEqualRejectDifferent
    );
    assert_eq!(plan.transform(), BackfillValueTransform::Identity);
    assert_eq!(
        plan.postcondition(),
        BackfillPostcondition::SourceValueCopiedExactly
    );
    assert_eq!(
        plan.reverse(),
        Some(BackfillReverseProgram::RemoveEqualCopiedDestination)
    );
    assert_eq!(
        plan.required_capabilities()
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>(),
        [COPY_ATTRIBUTE_BACKFILL_CAPABILITY]
    );
    assert_eq!(plan.partition().batch_rows(), 512);

    let value: serde_json::Value =
        serde_json::from_slice(&plan.canonical_bytes().expect("canonical bytes"))
            .expect("canonical JSON");
    assert_eq!(
        value,
        json!({
            "conflict": "skip_equal_reject_different",
            "destination": "display-name",
            "format": 1,
            "managed_semantics": plan.managed_semantics(),
            "owner": {"kind": "entity", "label": "person"},
            "partition": {"batch_rows": 512, "stable_attribute": "person-id"},
            "postcondition": "source_value_copied_exactly",
            "required_capabilities": ["migration.backfill.copy-attribute"],
            "reverse": "remove_equal_copied_destination",
            "source": "legacy-name",
            "transform": "identity"
        })
    );
    assert_eq!(
        plan.canonical_bytes().unwrap(),
        to_canonical_json(&value).unwrap()
    );
    assert_eq!(plan.fingerprint().unwrap(), plan.fingerprint().unwrap());
    assert_eq!(
        decode_attribute_backfill_plan(&plan.canonical_bytes().unwrap()).unwrap(),
        plan
    );

    let mut tampered = value;
    tampered["conflict"] = json!("overwrite");
    assert_eq!(
        decode_attribute_backfill_plan(&to_canonical_json(&tampered).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "migration_backfill_contract_mismatch"
    );
}

#[test]
fn copy_attribute_plan_rejects_ambiguous_or_unbounded_work() {
    assert_eq!(
        BackfillPartition::new(
            0,
            AttributeId::new("person-id").expect("stable partition attribute")
        )
        .expect_err("zero batch rejects")
        .code()
        .as_str(),
        "migration_backfill_batch_rows_out_of_range"
    );
    assert!(
        BackfillPartition::new(
            MAX_BACKFILL_BATCH_ROWS + 1,
            AttributeId::new("person-id").expect("stable partition attribute")
        )
        .is_err()
    );
    let same = AttributeId::new("name").expect("attribute");
    assert_eq!(
        AttributeBackfillPlan::new(
            TypeId::new(TypeKind::Entity, "person").expect("owner"),
            same.clone(),
            same,
            BackfillPartition::new(
                1,
                AttributeId::new("person-id").expect("stable partition attribute")
            )
            .unwrap(),
            semantics(),
            None,
        )
        .expect_err("same source and destination reject")
        .code()
        .as_str(),
        "migration_backfill_same_attribute"
    );
}
