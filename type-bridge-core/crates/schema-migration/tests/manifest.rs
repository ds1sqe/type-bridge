use serde::Serialize;
use serde_json::{Value, json};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::CANONICAL_CODEC_LIMITS;
use type_bridge_contract::managed_scope::{ManagedScopeId, SemanticProfileBinding};
use type_bridge_contract::migration::{
    CONDITIONAL_RESOLUTION_CAPABILITY, MigrationAppLabel, MigrationId,
    MigrationName, MigrationPlanFingerprint, MigrationStep, MigrationStepId,
    SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId,
    CanonicalValueRange, DeclaredSchema, DocumentId, SchemaAnnotationValue,
    SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId, TypeFact,
    ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile,
    derive_safety_conditions, diff_managed, inverse_delta, managed_schema_state,
    resolve,
};
use type_bridge_query::{
    MigrationAssertionValidationContext, lower_condition_to_plan,
};
use type_bridge_schema_migration::{
    SchemaMigrationDraft, build_verified_manifest, decode_verified_manifest,
    encode_verified_manifest, schema_lowering_profile_binding,
    verified_manifest_digest,
};

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, label).expect("fixture type"))
            .expect("fixture type fact"),
    )
}

fn declared(facts: Vec<SchemaFact>) -> DeclaredSchema {
    let sourced = facts
        .into_iter()
        .enumerate()
        .map(|(index, fact)| {
            let ordinal = u64::try_from(index).expect("fixture ordinal");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("manifest-fixture").expect("fixture document"),
                    ordinal,
                    ordinal + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("fixture span"),
            )
        })
        .collect::<Vec<_>>();
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("fixture schema")
}

fn context() -> ManagedDeltaContext {
    context_with_capabilities(CapabilitySet::new())
}

fn context_with_capabilities(available: CapabilitySet) -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("fixture scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile"),
        available,
    )
}

fn assertion_capabilities() -> CapabilitySet {
    [
        CONDITIONAL_RESOLUTION_CAPABILITY,
        "query.migration-assertion",
        "query.pattern.has",
        "query.pattern.isa",
        "query.pattern.isa-subtypes",
        "query.pattern.negation",
        "query.pattern.value",
    ]
    .into_iter()
    .map(|capability| {
        type_bridge_contract::capability::CapabilityId::new(capability)
            .expect("fixture capability")
    })
    .collect()
}

fn assertion_context() -> ManagedDeltaContext {
    context_with_capabilities(assertion_capabilities())
}

fn safety_derivation_profile() -> SafetyDerivationProfile {
    SafetyDerivationProfile::new(
        SemanticProfileBinding::resolve(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile"),
        )
        .expect("semantic binding"),
        schema_lowering_profile_binding().expect("lowering binding"),
    )
    .expect("safety profile")
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("example").expect("fixture app label"),
        MigrationName::new(name).expect("fixture migration name"),
    )
}

fn abstract_fact(label: &str) -> SchemaFact {
    let id = TypeId::new(TypeKind::Entity, label).expect("fixture type");
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(id),
                AnnotationKindId::Abstract,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("abstract annotation"),
    )
}

fn derived_assertion(
    id: &str,
    operation_index: usize,
    operation: &type_bridge_contract::schema::SchemaOperation,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> MigrationStep {
    let profiles = safety_derivation_profile();
    let derived = derive_safety_conditions(
        operation_index,
        operation,
        source,
        target,
        &profiles,
    )
    .expect("derived condition");
    let resolved = resolve(source, context.semantic_profile()).expect("resolved source");
    let managed = managed_schema_state(source, context).expect("managed source");
    let validation = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = lower_condition_to_plan(
        &derived.conditions()[0],
        &validation,
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("lowered assertion");
    MigrationStep::assertion(
        MigrationStepId::new(id).expect("assertion id"),
        validated.plan().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("assertion step")
}

fn conditional_steps() -> (
    DeclaredSchema,
    DeclaredSchema,
    ManagedDeltaContext,
    MigrationStep,
    SchemaDeltaStep,
) {
    let source = declared(vec![type_fact("person")]);
    let target = declared(vec![type_fact("person"), abstract_fact("person")]);
    let context = assertion_context();
    let delta = diff_managed(&source, &target, &context).expect("conditional delta");
    let assertion = derived_assertion(
        "assert-no-person",
        0,
        &delta.operations()[0],
        &source,
        &target,
        &context,
    );
    let schema = SchemaDeltaStep::new(
        MigrationStepId::new("make-person-abstract").expect("step id"),
        delta,
        None,
    )
    .expect("schema step");
    (source, target, context, assertion, schema)
}

fn additive_fixture() -> (
    DeclaredSchema,
    ManagedDeltaContext,
    type_bridge_schema_migration::VerifiedSchemaMigrationManifest,
) {
    let source = declared(vec![type_fact("person")]);
    let target = declared(vec![type_fact("person"), type_fact("company")]);
    let context = context();
    let delta = diff_managed(&source, &target, &context).expect("fixture delta");
    let reverse = inverse_delta(&delta).expect("fixture inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new("define-company").expect("fixture step id"),
        delta,
        Some(reverse),
    )
    .expect("fixture step");
    let draft = SchemaMigrationDraft::new(
        migration_id("0002_add_company"),
        Vec::new(),
        vec![step],
    )
    .expect("fixture draft");
    let verified = build_verified_manifest(draft, (&source, &context)).expect("verified fixture");
    (source, context, verified)
}

fn canonical<T: Serialize>(value: &T) -> String {
    String::from_utf8(to_canonical_json(value).expect("canonical fixture field"))
        .expect("JSON is UTF-8")
}

#[test]
fn exact_wire_golden_roundtrips_and_has_external_raw_digest() {
    let (source, context, verified) = additive_fixture();
    assert_eq!(verified.source_schema(), &source);
    let bytes = encode_verified_manifest(&verified).expect("encode verified");
    let expected = format!(
        concat!(
            "{{\"contract\":{{\"canonicalization\":\"typebridge.schema-c14n/v2\",",
            "\"codec\":\"typebridge.canonical-json/v1\",",
            "\"delta_ir\":\"typebridge.schema-delta/v1\",",
            "\"lowering_profile\":{},\"semantic_profile\":{}}},",
            "\"fingerprints\":{{\"plan\":{},",
            "\"source\":{{\"declared_identity\":{},\"resolution_identity\":{},",
            "\"semantics\":{}}},",
            "\"target\":{{\"declared_identity\":{},\"resolution_identity\":{},",
            "\"semantics\":{}}}}},",
            "\"format\":\"typebridge.migration/v1\",\"id\":{},",
            "\"managed_scope\":{},\"parents\":{},\"required_capabilities\":{},",
            "\"resources\":[],\"safety\":{{\"classification\":\"additive\",",
            "\"reversible\":true}},\"steps\":{}}}"
        ),
        canonical(verified.lowering_profile()),
        canonical(verified.semantic_profile()),
        canonical(verified.plan_fingerprint()),
        canonical(verified.source_state().managed_declared_identity()),
        canonical(verified.source_state().declared_identity()),
        canonical(verified.source_state().managed_semantic_schema()),
        canonical(verified.target_state().managed_declared_identity()),
        canonical(verified.target_state().declared_identity()),
        canonical(verified.target_state().managed_semantic_schema()),
        canonical(verified.id()),
        canonical(verified.managed_scope()),
        canonical(&verified.parents()),
        canonical(verified.required_capabilities()),
        canonical(&verified.steps()),
    );
    assert_eq!(bytes, expected.as_bytes());

    let decoded = decode_verified_manifest(&bytes, (&source, &context)).expect("decode verified");
    assert_eq!(decoded, verified);
    assert_eq!(decoded.source_schema(), &source);
    let digest = verified_manifest_digest(&verified).expect("external digest");
    assert_eq!(digest.to_hex().len(), 64);
    assert_eq!(digest, type_bridge_contract::migration::MigrationManifestDigest::compute(&bytes));
}

#[test]
fn tamper_unknown_noncanonical_and_resource_inputs_fail_closed() {
    let (source, context, verified) = additive_fixture();
    let bytes = encode_verified_manifest(&verified).expect("encode verified");
    let mut value: Value = serde_json::from_slice(&bytes).expect("fixture JSON");

    value["safety"]["classification"] = json!("formal_only");
    let tampered = serde_json::to_vec(&value).expect("canonical tamper");
    assert_eq!(
        decode_verified_manifest(&tampered, (&source, &context))
            .expect_err("tampered safety")
            .code()
            .as_str(),
        "migration_manifest_verification_mismatch"
    );

    let mut unknown: Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    unknown["unknown"] = json!(true);
    assert!(decode_verified_manifest(
        &serde_json::to_vec(&unknown).expect("canonical unknown"),
        (&source, &context)
    )
    .is_err());

    let mut spaced = bytes.clone();
    spaced.insert(0, b' ');
    assert_eq!(
        decode_verified_manifest(&spaced, (&source, &context))
            .expect_err("noncanonical input")
            .code()
            .as_str(),
        "non_canonical_json"
    );

    let mut resources: Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    resources["resources"] = json!([{"kind": "hook"}]);
    assert_eq!(
        decode_verified_manifest(
            &serde_json::to_vec(&resources).expect("canonical resources"),
            (&source, &context)
        )
        .expect_err("nonempty resources")
        .code()
        .as_str(),
        "migration_manifest_resources_not_empty"
    );
}

#[test]
fn limits_profiles_and_capability_claims_fail_closed() {
    let (source, context, verified) = additive_fixture();
    let bytes = encode_verified_manifest(&verified).expect("encode verified");
    let oversized = vec![b' '; CANONICAL_CODEC_LIMITS.max_bytes + 1];
    assert_eq!(
        decode_verified_manifest(&oversized, (&source, &context))
            .expect_err("oversize input")
            .code()
            .as_str(),
        "canonical_json_too_large"
    );

    let mut profile: Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    profile["contract"]["semantic_profile"]["id"] = json!("typedb-9.9.9/v1");
    assert!(decode_verified_manifest(
        &serde_json::to_vec(&profile).expect("canonical profile tamper"),
        (&source, &context)
    )
    .is_err());

    let mut capability: Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    capability["required_capabilities"] = json!(["schema.future"]);
    assert!(decode_verified_manifest(
        &serde_json::to_vec(&capability).expect("canonical capability tamper"),
        (&source, &context)
    )
    .is_err());
}

#[test]
fn unresolved_safety_and_broken_step_chains_are_rejected_but_destructive_is_verified() {
    let source = declared(vec![type_fact("person")]);
    let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
    let employee = TypeId::new(TypeKind::Entity, "employee").expect("employee type");
    let target = declared(vec![
        type_fact("person"),
        type_fact("employee"),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person).expect("fixture sub id"),
        )),
    ]);
    let context = context();
    let conditional = diff_managed(&source, &target, &context).expect("conditional delta");
    let conditional_step = SchemaDeltaStep::new(
        MigrationStepId::new("conditional-sub").expect("step id"),
        conditional,
        None,
    )
    .expect("conditional step");
    let conditional_draft = SchemaMigrationDraft::new(
        migration_id("0003_conditional"),
        Vec::new(),
        vec![conditional_step],
    )
    .expect("conditional draft");
    assert_eq!(
        build_verified_manifest(conditional_draft, (&source, &context))
            .expect_err("conditional safety")
            .code()
            .as_str(),
        "migration_manifest_unresolvable_conditional_assertion"
    );

    let additive_target = declared(vec![type_fact("person"), type_fact("company")]);
    let additive = diff_managed(&source, &additive_target, &context).expect("additive delta");
    let first = SchemaDeltaStep::new(
        MigrationStepId::new("first").expect("step id"),
        additive.clone(),
        None,
    )
    .expect("first step");
    let second = SchemaDeltaStep::new(
        MigrationStepId::new("second").expect("step id"),
        additive.clone(),
        None,
    )
    .expect("second step");
    let broken = SchemaMigrationDraft::new(
        migration_id("0004_broken"),
        Vec::new(),
        vec![first, second],
    )
    .expect("broken draft shape");
    assert_eq!(
        build_verified_manifest(broken, (&source, &context))
            .expect_err("broken chain")
            .code()
            .as_str(),
        "migration_manifest_step_chain_mismatch"
    );

    let destructive = inverse_delta(&additive).expect("destructive inverse");
    let reverse = SchemaDeltaStep::new(
        MigrationStepId::new("remove-company").expect("step id"),
        destructive,
        Some(additive),
    )
    .expect("destructive step");
    let destructive_draft = SchemaMigrationDraft::new(
        migration_id("0005_remove_company"),
        Vec::new(),
        vec![reverse],
    )
    .expect("destructive draft");
    let destructive_verified =
        build_verified_manifest(destructive_draft, (&additive_target, &context))
            .expect("verified destructive schema-only claim");
    assert_eq!(destructive_verified.safety(), SafetyClass::Destructive);

    let destructive_bytes =
        encode_verified_manifest(&destructive_verified).expect("destructive encoding");
    let mut inverse_tamper: Value =
        serde_json::from_slice(&destructive_bytes).expect("destructive JSON");
    inverse_tamper["steps"][0]["contract"]["reverse"] = Value::Null;
    assert!(decode_verified_manifest(
        &serde_json::to_vec(&inverse_tamper).expect("canonical inverse tamper"),
        (&additive_target, &context)
    )
    .is_err());
}

#[test]
fn conditional_assertions_are_exact_ordered_and_canonical() {
    let (source, target, context, assertion, schema) = conditional_steps();
    let draft = SchemaMigrationDraft::new(
        migration_id("0006_make_abstract"),
        Vec::new(),
        vec![assertion, MigrationStep::from(schema)],
    )
    .expect("conditional draft");
    let verified = build_verified_manifest(draft, (&source, &context))
        .expect("conditional manifest");
    assert_eq!(verified.safety(), SafetyClass::Conditional);
    assert_eq!(
        verified.target_schema().declared_identity_fingerprint(),
        target.declared_identity_fingerprint(),
    );
    assert_eq!(
        verified
            .target_schema()
            .canonical_identity_bytes()
            .expect("replayed identity bytes"),
        target
            .canonical_identity_bytes()
            .expect("authored identity bytes"),
    );
    assert!(verified.required_capabilities().contains(
        &type_bridge_contract::capability::CapabilityId::new(
            CONDITIONAL_RESOLUTION_CAPABILITY,
        )
        .expect("capability")
    ));
    let bytes = encode_verified_manifest(&verified).expect("canonical manifest");
    assert_eq!(
        decode_verified_manifest(&bytes, (&source, &context)).expect("decode"),
        verified
    );
}

#[test]
fn missing_extra_reordered_and_tampered_assertions_fail_closed() {
    let (source, _, context, assertion, schema) = conditional_steps();
    let missing = SchemaMigrationDraft::new(
        migration_id("0007_missing"),
        Vec::new(),
        vec![schema.clone()],
    )
    .expect("missing draft");
    assert_eq!(
        build_verified_manifest(missing, (&source, &context))
            .expect_err("missing assertion")
            .code()
            .as_str(),
        "migration_manifest_missing_assertion"
    );

    let reordered = SchemaMigrationDraft::new(
        migration_id("0008_reordered"),
        Vec::new(),
        vec![MigrationStep::from(schema.clone()), assertion.clone()],
    )
    .expect("reordered draft");
    assert!(build_verified_manifest(reordered, (&source, &context)).is_err());

    let additive_target = declared(vec![type_fact("person"), type_fact("company")]);
    let additive = diff_managed(&source, &additive_target, &context).expect("additive");
    let additive_step = SchemaDeltaStep::new(
        MigrationStepId::new("add-company").expect("step id"),
        additive,
        None,
    )
    .expect("additive step");
    let extra = SchemaMigrationDraft::new(
        migration_id("0009_extra"),
        Vec::new(),
        vec![assertion, MigrationStep::from(additive_step)],
    )
    .expect("extra draft");
    assert_eq!(
        build_verified_manifest(extra, (&source, &context))
            .expect_err("extra assertion")
            .code()
            .as_str(),
        "migration_manifest_extra_assertion"
    );

    let (_, _, _, assertion, schema) = conditional_steps();
    let valid = SchemaMigrationDraft::new(
        migration_id("0010_tamper"),
        Vec::new(),
        vec![assertion, MigrationStep::from(schema)],
    )
    .expect("valid draft");
    let verified = build_verified_manifest(valid, (&source, &context)).expect("verified");
    let mut value: Value = serde_json::from_slice(
        &encode_verified_manifest(&verified).expect("manifest bytes"),
    )
    .expect("manifest JSON");
    value["steps"][0]["kind"] = json!("future");
    assert_eq!(
        decode_verified_manifest(
            &serde_json::to_vec(&value).expect("tampered JSON"),
            (&source, &context),
        )
        .expect_err("unknown kind")
        .code()
        .as_str(),
        "migration_manifest_unknown_step_kind"
    );
}

#[test]
fn reverse_conditional_evidence_is_not_implied_by_forward_reversibility() {
    let plain = declared(vec![type_fact("person")]);
    let abstracted = declared(vec![type_fact("person"), abstract_fact("person")]);
    let context = context();
    let forward = diff_managed(&abstracted, &plain, &context).expect("remove abstract");
    let reverse = inverse_delta(&forward).expect("restore abstract");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new("remove-abstract").expect("step id"),
        forward,
        Some(reverse),
    )
    .expect("reversible step");
    let draft = SchemaMigrationDraft::new(
        migration_id("0011_reverse_conditional"),
        Vec::new(),
        vec![step],
    )
    .expect("draft");
    assert_eq!(
        build_verified_manifest(draft, (&abstracted, &context))
            .expect_err("reverse assertion absent")
            .code()
            .as_str(),
        "migration_manifest_reverse_requires_assertions"
    );
}

#[test]
fn assertion_capabilities_are_gated_before_coverage_trust() {
    let (source, _, _, assertion, schema) = conditional_steps();
    let draft = |name: &str| {
        SchemaMigrationDraft::new(
            migration_id(name),
            Vec::new(),
            vec![assertion.clone(), MigrationStep::from(schema.clone())],
        )
        .expect("conditional draft")
    };
    assert!(build_verified_manifest(
        draft("0012_missing_all_assertion_caps"),
        (&source, &context()),
    )
    .is_err());

    let without_isa = [
        CONDITIONAL_RESOLUTION_CAPABILITY,
        "query.migration-assertion",
    ]
    .into_iter()
    .map(|capability| {
        type_bridge_contract::capability::CapabilityId::new(capability)
            .expect("fixture capability")
    })
    .collect();
    assert!(build_verified_manifest(
        draft("0013_missing_pattern_cap"),
        (&source, &context_with_capabilities(without_isa)),
    )
    .is_err());

    let without_resolution = ["query.migration-assertion", "query.pattern.isa"]
        .into_iter()
        .map(|capability| {
            type_bridge_contract::capability::CapabilityId::new(capability)
                .expect("fixture capability")
        })
        .collect();
    assert!(build_verified_manifest(
        draft("0014_missing_conditional_resolution"),
        (&source, &context_with_capabilities(without_resolution)),
    )
    .is_err());
}

#[test]
fn assertions_may_be_adjacent_to_their_chunk_between_state_changes() {
    let source = declared(vec![type_fact("person")]);
    let intermediate = declared(vec![type_fact("person"), type_fact("company")]);
    let target = declared(vec![
        type_fact("person"),
        type_fact("company"),
        abstract_fact("person"),
    ]);
    let context = assertion_context();
    let first_delta =
        diff_managed(&source, &intermediate, &context).expect("first delta");
    let second_delta =
        diff_managed(&intermediate, &target, &context).expect("second delta");
    let assertion = derived_assertion(
        "assert-between-chunks",
        0,
        &second_delta.operations()[0],
        &intermediate,
        &target,
        &context,
    );
    let first = SchemaDeltaStep::new(
        MigrationStepId::new("add-company-first").expect("step id"),
        first_delta,
        None,
    )
    .expect("first step");
    let second = SchemaDeltaStep::new(
        MigrationStepId::new("abstract-person-second").expect("step id"),
        second_delta,
        None,
    )
    .expect("second step");
    let draft = SchemaMigrationDraft::new(
        migration_id("0015_multi_chunk"),
        Vec::new(),
        vec![
            MigrationStep::from(first),
            assertion,
            MigrationStep::from(second),
        ],
    )
    .expect("multi-chunk draft");
    let verified = build_verified_manifest(draft, (&source, &context))
        .expect("multi-chunk verification");
    assert_eq!(
        verified.target_schema().declared_identity_fingerprint(),
        target.declared_identity_fingerprint(),
    );
}

#[test]
fn canonical_plan_tamper_with_recomputed_outer_claims_is_rejected() {
    let (source, _, context, assertion, schema) = conditional_steps();
    let valid = SchemaMigrationDraft::new(
        migration_id("0016_direct_plan_tamper"),
        Vec::new(),
        vec![assertion.clone(), MigrationStep::from(schema.clone())],
    )
    .expect("valid draft");
    let verified = build_verified_manifest(valid, (&source, &context)).expect("verified");
    let mut manifest: Value = serde_json::from_slice(
        &encode_verified_manifest(&verified).expect("manifest bytes"),
    )
    .expect("manifest JSON");
    let mut plan = manifest["steps"][0]["plan"].clone();
    plan["bindings"][0]["variable"] = json!("tampered_instance");
    let tampered_plan =
        type_bridge_contract::migration_assertion::decode_migration_assertion_plan(
            &to_canonical_json(&plan).expect("tampered canonical plan"),
        )
        .expect("tampered plan remains a valid contract");
    let tampered_assertion = MigrationStep::assertion(
        assertion.id().clone(),
        tampered_plan,
        AssertionExpectation::NoRows,
    )
    .expect("recomputed assertion contract");
    let steps = vec![tampered_assertion.clone(), MigrationStep::from(schema)];
    let outer_plan = MigrationPlanFingerprint::compute(&steps)
        .expect("recomputed heterogeneous plan fingerprint");
    manifest["steps"][0] = serde_json::from_slice(
        &tampered_assertion
            .canonical_bytes()
            .expect("tampered assertion bytes"),
    )
    .expect("tampered assertion JSON");
    manifest["fingerprints"]["plan"] = serde_json::from_slice(
        &to_canonical_json(&outer_plan).expect("outer plan bytes"),
    )
    .expect("outer plan JSON");
    assert_eq!(
        decode_verified_manifest(
            &serde_json::to_vec(&manifest).expect("tampered manifest bytes"),
            (&source, &context),
        )
        .expect_err("direct plan tamper")
        .code()
        .as_str(),
        "migration_manifest_assertion_plan_mismatch"
    );
}

#[test]
fn reordered_assertions_have_a_stable_plan_mismatch_diagnostic() {
    let age = AttributeId::new("age").expect("attribute");
    let subject = AnnotationSubjectId::Value(ValueFactId::new(age.clone()));
    let base = vec![
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, "age").expect("attribute type"),
            )
            .expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age),
            ValueTypeTag::Long,
        )),
    ];
    let range = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(subject, AnnotationKindId::Range),
            SchemaAnnotationValue::Range(
                CanonicalValueRange::new(
                    Some(CanonicalValue::Long(1)),
                    Some(CanonicalValue::Long(9)),
                )
                .expect("range"),
            ),
        )
        .expect("range annotation"),
    );
    let source = declared(base.clone());
    let target = declared(base.into_iter().chain([range]).collect());
    let context = assertion_context();
    let delta = diff_managed(&source, &target, &context).expect("range delta");
    let profiles = safety_derivation_profile();
    let derived = derive_safety_conditions(
        0,
        &delta.operations()[0],
        &source,
        &target,
        &profiles,
    )
    .expect("range conditions");
    assert_eq!(derived.conditions().len(), 2);
    let resolved = resolve(&source, context.semantic_profile()).expect("resolved source");
    let managed = managed_schema_state(&source, &context).expect("managed source");
    let validation = MigrationAssertionValidationContext::new(&resolved, &managed);
    let assertions = derived
        .conditions()
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            let validated = lower_condition_to_plan(
                condition,
                &validation,
                type_bridge_contract::limits::StructuralLimits::CANONICAL,
            )
            .expect("lowered range condition");
            MigrationStep::assertion(
                MigrationStepId::new(format!("range-assertion-{index}"))
                    .expect("assertion id"),
                validated.plan().clone(),
                AssertionExpectation::NoRows,
            )
            .expect("assertion step")
        })
        .collect::<Vec<_>>();
    let schema = SchemaDeltaStep::new(
        MigrationStepId::new("add-range").expect("step id"),
        delta,
        None,
    )
    .expect("schema step");
    let draft = SchemaMigrationDraft::new(
        migration_id("0017_reordered_assertions"),
        Vec::new(),
        vec![
            assertions[1].clone(),
            assertions[0].clone(),
            MigrationStep::from(schema),
        ],
    )
    .expect("reordered draft");
    assert_eq!(
        build_verified_manifest(draft, (&source, &context))
            .expect_err("reordered assertions")
            .code()
            .as_str(),
        "migration_manifest_assertion_plan_mismatch"
    );
}
