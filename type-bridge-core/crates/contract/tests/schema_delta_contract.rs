use serde_json::Value;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::limits::CANONICAL_CODEC_LIMITS;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId,
    DeclaredIdentityFingerprint, DeclaredSchema, DocText, DocumentId, FunctionBody, FunctionFact,
    FunctionReturnElement, FunctionReturnMode, FunctionSignature,
    ManagedDeclaredIdentityFingerprint, ManagedFactSelection, ManagedSchemaState,
    ManagedScopeBinding, ManagedScopeId, ManagedSemanticSchemaFingerprint, OwnsFact, OwnsFactId,
    PatchFormatVersion, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId, SchemaAnnotationValue,
    SchemaDelta, SchemaFact, SchemaOperation, SourceSpan, SourcedSchemaFact, StructFact,
    StructField, SubFact, SubFactId, TypeFact, TypeReference, ValueFact, ValueFactId,
    decode_schema_delta, encode_schema_delta,
};
use type_bridge_contract::value::ValueTypeTag;

fn capability(value: &str) -> CapabilityId {
    CapabilityId::new(value).unwrap()
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn declared_identity(marker: &str) -> DeclaredIdentityFingerprint {
    let label = format!("state-{marker}");
    let fact = SchemaFact::Type(TypeFact::new(type_id(TypeKind::Entity, &label)).unwrap());
    DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        [SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("schema-delta-state").unwrap(),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .unwrap(),
        )],
    )
    .unwrap()
    .declared_identity_fingerprint()
    .clone()
}

fn all_fact_variants() -> Vec<SchemaFact> {
    let person = type_id(TypeKind::Entity, "person");
    let employee = type_id(TypeKind::Entity, "employee");
    let name = AttributeId::new("name").unwrap();
    let friendship = type_id(TypeKind::Relation, "friendship");
    let friend = RoleId::new("friendship", "friend").unwrap();

    let annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Type(person.clone()),
        AnnotationKindId::Doc,
    );
    let function = FunctionFact::new(
        FunctionId::new("identity").unwrap(),
        FunctionSignature::new(
            Vec::new(),
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::String),
                false,
            )),
        )
        .unwrap(),
        FunctionBody::new("return \"ok\";").unwrap(),
    );
    let structure = StructFact::new(
        StructId::new("record").unwrap(),
        vec![StructField::new(
            Label::new("value").unwrap(),
            ValueTypeTag::String,
            false,
        )],
    )
    .unwrap();

    vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person.clone()).unwrap(),
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name).unwrap(),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(friendship, friend.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(PlaysFactId::new(person, friend).unwrap())),
        SchemaFact::Annotation(
            AnnotationFact::new(
                annotation_id,
                SchemaAnnotationValue::Doc(DocText::new("Person documentation").unwrap()),
            )
            .unwrap(),
        ),
        SchemaFact::Function(function),
        SchemaFact::Struct(structure),
    ]
}

fn state(
    scope: ManagedScopeBinding,
    selection: ManagedFactSelection,
    marker: &str,
    semantic_profile: &str,
    required_capabilities: CapabilitySet,
) -> ManagedSchemaState {
    let profile = SemanticProfileId::new(semantic_profile).unwrap();
    let declared_marker = if selection.iter().next().is_none() {
        "empty"
    } else {
        marker
    };
    ManagedSchemaState::new(
        FormatVersion::V1,
        required_capabilities,
        scope,
        selection,
        declared_identity(declared_marker),
        ManagedDeclaredIdentityFingerprint::compute(format!("declared-{marker}").as_bytes())
            .unwrap(),
        ManagedSemanticSchemaFingerprint::compute(profile, format!("semantic-{marker}").as_bytes())
            .unwrap(),
    )
    .unwrap()
}

fn defined_delta() -> SchemaDelta {
    let facts = all_fact_variants();
    let scope = ManagedScopeBinding::exclusive(ManagedScopeId::new("test-scope").unwrap()).unwrap();
    let source = state(
        scope.clone(),
        ManagedFactSelection::empty(),
        "source",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.source")]),
    );
    let target = state(
        scope,
        ManagedFactSelection::new(facts.iter().map(SchemaFact::id)).unwrap(),
        "target",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.target")]),
    );
    SchemaDelta::new(
        PatchFormatVersion::V1,
        source,
        target,
        vec![SchemaOperation::define(facts).unwrap()],
    )
    .unwrap()
}

fn doc_fact(text: &str) -> SchemaFact {
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(type_id(TypeKind::Entity, "person")),
                AnnotationKindId::Doc,
            ),
            SchemaAnnotationValue::Doc(DocText::new(text).unwrap()),
        )
        .unwrap(),
    )
}

#[test]
fn all_fact_variants_round_trip_with_canonical_bytes_and_operation_accessors() {
    let delta = defined_delta();
    let bytes = encode_schema_delta(&delta).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["source"]["declared_identity"],
        serde_json::to_value(delta.source().declared_identity()).unwrap(),
    );
    assert_eq!(delta.canonical_bytes().unwrap(), bytes);
    assert_eq!(decode_schema_delta(&bytes).unwrap(), delta);

    let operation = &delta.operations()[0];
    assert_eq!(operation.defined_facts().unwrap().len(), 9);
    assert_eq!(operation.affected_ids().len(), 9);
    assert_eq!(operation.inverse().len(), 9);
    assert!(operation.expected_fact().is_none());
    assert!(operation.replacement_fact().is_none());
    assert!(operation.undefined_fact().is_none());
}

#[test]
fn constructors_reject_malformed_operations_and_state_transitions() {
    assert_eq!(
        SchemaOperation::define(Vec::new())
            .unwrap_err()
            .code()
            .as_str(),
        "empty_schema_define",
    );
    let type_fact = SchemaFact::Type(TypeFact::new(type_id(TypeKind::Entity, "person")).unwrap());
    assert_eq!(
        SchemaOperation::define(vec![type_fact.clone(), type_fact.clone()])
            .unwrap_err()
            .code()
            .as_str(),
        "duplicate_schema_operation_fact_id",
    );
    let other = SchemaFact::Type(TypeFact::new(type_id(TypeKind::Entity, "company")).unwrap());
    assert_eq!(
        SchemaOperation::redefine(type_fact.clone(), other)
            .unwrap_err()
            .code()
            .as_str(),
        "schema_redefine_identity_mismatch",
    );
    assert_eq!(
        SchemaOperation::redefine(type_fact.clone(), type_fact.clone())
            .unwrap_err()
            .code()
            .as_str(),
        "schema_redefine_noop",
    );

    let scope =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("negative-scope").unwrap()).unwrap();
    let source = state(
        scope.clone(),
        ManagedFactSelection::empty(),
        "negative-source",
        "typedb-3.12.1/v1",
        CapabilitySet::new(),
    );
    let target = state(
        scope,
        ManagedFactSelection::empty(),
        "negative-target",
        "typedb-3.12.1/v1",
        CapabilitySet::new(),
    );
    assert_eq!(
        SchemaDelta::new(
            PatchFormatVersion::V1,
            source,
            target,
            vec![SchemaOperation::define(vec![type_fact]).unwrap()],
        )
        .unwrap_err()
        .code()
        .as_str(),
        "schema_delta_selection_mismatch",
    );
}

#[test]
fn capabilities_are_derived_from_both_states_and_the_transition_table() {
    let old = doc_fact("old");
    let new = doc_fact("new");
    let selection = ManagedFactSelection::new([old.id()]).unwrap();
    let scope =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("redefine-scope").unwrap()).unwrap();
    let source = state(
        scope.clone(),
        selection.clone(),
        "old",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.source")]),
    );
    let target = state(
        scope,
        selection,
        "new",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.target")]),
    );
    let operation = SchemaOperation::redefine(old.clone(), new.clone()).unwrap();
    assert_eq!(operation.expected_fact(), Some(&old));
    assert_eq!(operation.replacement_fact(), Some(&new));
    let inverse = operation.inverse();
    assert_eq!(inverse[0].expected_fact(), Some(&new));
    assert_eq!(inverse[0].replacement_fact(), Some(&old));

    let delta = SchemaDelta::new(PatchFormatVersion::V1, source, target, vec![operation]).unwrap();
    let bytes = encode_schema_delta(&delta).unwrap();
    assert_eq!(decode_schema_delta(&bytes).unwrap(), delta);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        to_canonical_json(&value["operations"][0]).unwrap(),
        br#"{"expected":{"kind":"annotation","value":{"id":{"kind":{"kind":"doc"},"subject":{"kind":"type","value":{"kind":"entity","label":"person"}}},"value":{"kind":"doc","value":"old"}}},"kind":"redefine","replacement":{"kind":"annotation","value":{"id":{"kind":{"kind":"doc"},"subject":{"kind":"type","value":{"kind":"entity","label":"person"}}},"value":{"kind":"doc","value":"new"}}}}"#,
    );
    assert_eq!(
        delta
            .required_capabilities()
            .iter()
            .map(CapabilityId::as_str)
            .collect::<Vec<_>>(),
        vec!["schema.redefine", "schema.source", "schema.target"],
    );
}

#[test]
fn capability_only_transition_round_trips_with_exact_derived_union() {
    let scope =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("capability-only-scope").unwrap())
            .unwrap();
    let source = state(
        scope.clone(),
        ManagedFactSelection::empty(),
        "capability-only-source",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.source")]),
    );
    let target = state(
        scope,
        ManagedFactSelection::empty(),
        "capability-only-target",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.target")]),
    );

    let delta = SchemaDelta::new(PatchFormatVersion::V1, source, target, Vec::new()).unwrap();
    assert!(delta.operations().is_empty());
    assert_eq!(
        delta
            .required_capabilities()
            .iter()
            .map(CapabilityId::as_str)
            .collect::<Vec<_>>(),
        vec!["schema.source", "schema.target"],
    );
    let bytes = delta.canonical_bytes().unwrap();
    assert_eq!(decode_schema_delta(&bytes).unwrap(), delta);
}

#[test]
fn identical_operation_free_state_is_rejected_as_a_noop() {
    let scope =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("identical-noop-scope").unwrap())
            .unwrap();
    let source = state(
        scope,
        ManagedFactSelection::empty(),
        "identical-noop",
        "typedb-3.12.1/v1",
        CapabilitySet::from_iter([capability("schema.same")]),
    );
    assert_eq!(
        SchemaDelta::new(PatchFormatVersion::V1, source.clone(), source, Vec::new(),)
            .unwrap_err()
            .code()
            .as_str(),
        "schema_delta_noop",
    );
}

#[test]
fn canonical_decoder_rejects_unknown_forged_oversize_depth_and_noncanonical_inputs() {
    let delta = defined_delta();
    let bytes = delta.canonical_bytes().unwrap();

    let mut unknown: Value = serde_json::from_slice(&bytes).unwrap();
    let facts = unknown["operations"][0]["facts"].as_array_mut().unwrap();
    let type_fact = facts
        .iter_mut()
        .find(|fact| fact["kind"] == "type")
        .unwrap();
    type_fact["value"]["id"]
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), Value::Bool(true));
    let unknown_bytes = to_canonical_json(&unknown).unwrap();
    assert_eq!(
        decode_schema_delta(&unknown_bytes)
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_canonical_value",
    );

    let mut forged: Value = serde_json::from_slice(&bytes).unwrap();
    forged["source"]["managed_declared_identity"]["domain"] =
        Value::String("forged.schema-domain".to_owned());
    let forged_bytes = to_canonical_json(&forged).unwrap();
    assert_eq!(
        decode_schema_delta(&forged_bytes)
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_managed_declared_identity_fingerprint",
    );

    let mut forged_full: Value = serde_json::from_slice(&bytes).unwrap();
    forged_full["source"]["declared_identity"]["domain"] =
        Value::String("forged.schema-domain".to_owned());
    let forged_full_bytes = to_canonical_json(&forged_full).unwrap();
    assert_eq!(
        decode_schema_delta(&forged_full_bytes)
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_declared_identity_fingerprint",
    );

    let mut unsorted: Value = serde_json::from_slice(&bytes).unwrap();
    unsorted["operations"][0]["facts"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_eq!(
        decode_schema_delta(&to_canonical_json(&unsorted).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "non_canonical_schema_delta",
    );

    let mut unsupported: Value = serde_json::from_slice(&bytes).unwrap();
    unsupported["format"] = Value::from(2);
    assert_eq!(
        decode_schema_delta(&to_canonical_json(&unsupported).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_patch_format_version",
    );

    let mut spaced = Vec::with_capacity(bytes.len() + 1);
    spaced.push(b' ');
    spaced.extend_from_slice(&bytes);
    assert_eq!(
        decode_schema_delta(&spaced).unwrap_err().code().as_str(),
        "non_canonical_json",
    );

    let oversized = vec![b' '; CANONICAL_CODEC_LIMITS.max_bytes + 1];
    assert_eq!(
        decode_schema_delta(&oversized).unwrap_err().code().as_str(),
        "canonical_json_too_large",
    );

    let depth = CANONICAL_CODEC_LIMITS.max_depth + 1;
    let mut too_deep = "[".repeat(depth);
    too_deep.push('0');
    too_deep.push_str(&"]".repeat(depth));
    assert_eq!(
        decode_schema_delta(too_deep.as_bytes())
            .unwrap_err()
            .code()
            .as_str(),
        "canonical_json_too_deep",
    );
}

#[test]
fn decoder_rejects_forged_capabilities_and_duplicate_affected_ids() {
    let delta = defined_delta();
    let mut forged: Value = serde_json::from_slice(&delta.canonical_bytes().unwrap()).unwrap();
    forged["required_capabilities"] = Value::Array(vec![Value::String("schema.forged".to_owned())]);
    assert_eq!(
        decode_schema_delta(&to_canonical_json(&forged).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "schema_delta_capability_mismatch",
    );

    let fact = doc_fact("remove");
    let scope =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("duplicate-scope").unwrap()).unwrap();
    let source = state(
        scope.clone(),
        ManagedFactSelection::new([fact.id()]).unwrap(),
        "duplicate-source",
        "typedb-3.12.1/v1",
        CapabilitySet::new(),
    );
    let target = state(
        scope,
        ManagedFactSelection::new([fact.id()]).unwrap(),
        "duplicate-target",
        "typedb-3.12.1/v1",
        CapabilitySet::new(),
    );
    assert_eq!(
        SchemaDelta::new(
            PatchFormatVersion::V1,
            source,
            target,
            vec![
                SchemaOperation::undefine(fact.clone()),
                SchemaOperation::define(vec![fact]).unwrap(),
            ],
        )
        .unwrap_err()
        .code()
        .as_str(),
        "duplicate_schema_delta_fact_id",
    );
}
