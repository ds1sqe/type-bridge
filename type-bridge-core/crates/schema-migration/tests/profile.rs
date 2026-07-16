use std::collections::BTreeSet;

use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::schema_lowering::{
    SCHEMA_LOWERING_PROFILE_CANONICALIZATION, SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN,
    SchemaLoweringProfileFingerprint, SchemaLoweringProfileId,
    TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID,
};
use type_bridge_schema_migration::{
    AnnotationKind, AnnotationSubjectKind, AnnotationTransition, EvidenceFlag,
    EvidenceRequirement, FactKind, FactTransition, InterfaceDefault, InterfaceKind,
    LoweringMechanism, SafetyClass, SafetyScenario, canonical_profile_bytes,
    profile_fingerprint, typedb_3_12_1_profile,
};

fn capability(id: &str) -> CapabilityId {
    CapabilityId::new(id).expect("test capability id is valid")
}

#[test]
fn profile_identity_and_capabilities_are_closed() {
    let profile = typedb_3_12_1_profile();
    assert_eq!(profile.id.as_str(), TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID);
    assert_eq!(
        serde_json::to_value(&profile.fingerprint_domain).unwrap(),
        SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN
    );
    assert_eq!(
        serde_json::to_value(&profile.canonicalization).unwrap(),
        SCHEMA_LOWERING_PROFILE_CANONICALIZATION
    );
    assert_eq!(profile.provider, "typedb");
    assert_eq!(profile.provider_version, "3.12.1");
    assert!(profile.transactional_schema_queries);
    let expected = [
        "schema.transaction.atomic",
        "schema.transition.define",
        "schema.transition.redefine.annotation",
        "schema.transition.redefine.function",
        "schema.transition.redefine.relates.specialization",
        "schema.transition.redefine.sub",
        "schema.transition.redefine.value",
        "schema.transition.replace.sub.annotation",
        "schema.transition.undefine",
    ];
    let actual: Vec<_> = profile
        .required_capabilities
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(actual, expected);
    assert!(actual.iter().all(|id| !id.contains("struct")));
    assert!(actual.iter().all(|id| !id.contains("function.annotation")));
}

#[test]
fn fact_table_is_exhaustive_and_matches_live_evidence() {
    let profile = typedb_3_12_1_profile();
    assert_eq!(
        profile.fact_rules.len(),
        FactKind::ALL.len() * FactTransition::ALL.len()
    );
    let tuples: BTreeSet<_> = profile
        .fact_rules
        .iter()
        .map(|row| (row.fact, row.transition))
        .collect();
    assert_eq!(tuples.len(), profile.fact_rules.len());
    for fact in FactKind::ALL {
        for transition in FactTransition::ALL {
            assert!(profile.fact_rule(fact, transition).is_some());
        }
    }
    for transition in FactTransition::ALL {
        let rule = profile.fact_rule(FactKind::Struct, transition).unwrap();
        assert_eq!(rule.mechanism, LoweringMechanism::Unsupported);
        assert_eq!(rule.safety, SafetyClass::Unsupported);
        assert!(rule.required_capabilities.is_empty());
    }
    let function = profile
        .fact_rule(FactKind::Function, FactTransition::Redefine)
        .unwrap();
    assert_eq!(function.mechanism, LoweringMechanism::Redefine);
    assert_eq!(function.safety, SafetyClass::Opaque);
    assert!(function.required_capabilities.contains(&capability(
        "schema.transition.redefine.function"
    )));
    let value = profile
        .fact_rule(FactKind::Value, FactTransition::Redefine)
        .unwrap();
    assert_eq!(value.safety, SafetyClass::Destructive);
}

#[test]
fn annotation_table_is_exhaustive_and_fail_closed() {
    let profile = typedb_3_12_1_profile();
    let expected_len = AnnotationSubjectKind::ALL.len()
        * AnnotationKind::ALL.len()
        * AnnotationTransition::ALL.len();
    assert_eq!(profile.annotation_rules.len(), expected_len);
    let tuples: BTreeSet<_> = profile
        .annotation_rules
        .iter()
        .map(|row| (row.subject, row.annotation, row.transition))
        .collect();
    assert_eq!(tuples.len(), expected_len);
    for subject in AnnotationSubjectKind::ALL {
        for annotation in AnnotationKind::ALL {
            for transition in AnnotationTransition::ALL {
                assert!(profile
                    .annotation_rule(subject, annotation, transition)
                    .is_some());
            }
        }
    }
    let sub_meta = profile
        .annotation_rule(
            AnnotationSubjectKind::Sub,
            AnnotationKind::Meta,
            AnnotationTransition::Change,
        )
        .unwrap();
    assert_eq!(sub_meta.mechanism, LoweringMechanism::AtomicUndefineDefine);
    assert_eq!(sub_meta.safety, SafetyClass::SchemaMetadata);
    assert!(sub_meta.keyed_meta);
    for id in [
        "schema.transaction.atomic",
        "schema.transition.define",
        "schema.transition.undefine",
        "schema.transition.replace.sub.annotation",
    ] {
        assert!(sub_meta.required_capabilities.contains(&capability(id)));
    }
    let independent_remove = profile
        .annotation_rule(
            AnnotationSubjectKind::Type,
            AnnotationKind::Independent,
            AnnotationTransition::Remove,
        )
        .unwrap();
    assert_eq!(independent_remove.mechanism, LoweringMechanism::Undefine);
    assert_eq!(independent_remove.safety, SafetyClass::Destructive);
    for transition in AnnotationTransition::ALL {
        let persistent_function_meta = profile
            .annotation_rule(
                AnnotationSubjectKind::Function,
                AnnotationKind::Meta,
                transition,
            )
            .unwrap();
        assert_eq!(
            persistent_function_meta.mechanism,
            LoweringMechanism::Unsupported
        );
        assert!(persistent_function_meta.required_capabilities.is_empty());
        assert!(persistent_function_meta.keyed_meta);
    }
}

#[test]
fn defaults_safety_and_evidence_are_frozen() {
    let profile = typedb_3_12_1_profile();
    assert_eq!(
        profile.interface_defaults,
        vec![
            InterfaceDefault {
                interface: InterfaceKind::Owns,
                min: 0,
                max: Some(1),
            },
            InterfaceDefault {
                interface: InterfaceKind::Relates,
                min: 0,
                max: Some(1),
            },
            InterfaceDefault {
                interface: InterfaceKind::Plays,
                min: 0,
                max: None,
            },
        ]
    );
    assert_eq!(profile.safety_rules.len(), SafetyScenario::ALL.len());
    for scenario in SafetyScenario::ALL {
        assert!(profile.safety_rule(scenario).is_some());
    }
    let card_removal = profile
        .safety_rule(SafetyScenario::RemoveCardinalityToNarrowerDefault)
        .unwrap();
    assert_eq!(card_removal.safety, SafetyClass::BackfillRequired);
    assert_eq!(card_removal.evidence, EvidenceRequirement::Backfill);
    let independent = profile
        .safety_rule(SafetyScenario::RemoveIndependent)
        .unwrap();
    assert_eq!(independent.safety, SafetyClass::Destructive);
    assert_eq!(independent.evidence, EvidenceRequirement::OperatorApproval);
    assert_eq!(profile.evidence, EvidenceFlag::ALL);
    assert!(profile
        .evidence
        .contains(&EvidenceFlag::StructTransitionsUnsupported));
    assert!(profile
        .evidence
        .contains(&EvidenceFlag::FunctionRedefineLeavesStoredMetadataStale));
    assert!(profile
        .evidence
        .contains(&EvidenceFlag::IndependentRemovalDeletesOwnerlessAttributes));
}

#[test]
fn canonical_profile_and_fingerprint_match_goldens() {
    assert_eq!(
        canonical_profile_bytes().as_slice(),
        include_bytes!("fixtures/typedb-3.12.1-schema-lowering-v1.json")
    );
    assert_eq!(
        serde_json::to_vec(&profile_fingerprint()).unwrap().as_slice(),
        include_bytes!("fixtures/typedb-3.12.1-schema-lowering-v1.fingerprint.json")
    );
}

#[test]
fn contract_profile_identity_and_fingerprint_decode_fail_closed() {
    assert!(SchemaLoweringProfileId::new(TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID).is_ok());
    assert!(SchemaLoweringProfileId::new("typedb-3.12.0-schema-lowering/v1").is_err());
    let bytes = include_bytes!("fixtures/typedb-3.12.1-schema-lowering-v1.fingerprint.json");
    let decoded: SchemaLoweringProfileFingerprint = serde_json::from_slice(bytes).unwrap();
    assert_eq!(decoded, profile_fingerprint());
    let mut wrong_domain: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    wrong_domain["domain"] = serde_json::Value::String("typebridge.schema.other".into());
    assert!(serde_json::from_value::<SchemaLoweringProfileFingerprint>(wrong_domain).is_err());
}
