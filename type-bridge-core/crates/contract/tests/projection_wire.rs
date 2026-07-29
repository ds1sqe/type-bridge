use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CompleteReadProjection, CreateProjection, DeclarationProjection, EmissionPlan,
    ModelProjection, ProjectedAnnotation, ProjectedMultiplicity, ProjectionConfig,
    ProjectionHandler, QueryTokenProjection, ReadRoleProjection, ReferenceReadProjection,
    RuntimeProjection, TargetIdentifier,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DocText, SchemaAnnotationValue,
    ValueFactId,
};
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;
use type_bridge_contract::value::Cardinality;

fn fixture() -> RuntimeProjection {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Type(person.clone()),
        AnnotationKindId::Doc,
    );
    let annotation = ProjectedAnnotation::new(
        annotation_id.clone(),
        SchemaAnnotationValue::Doc(DocText::new("A person.").unwrap()),
    )
    .unwrap();
    let model = ModelProjection::new(
        person.clone(),
        TargetIdentifier::python("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::from([(annotation_id, annotation)]),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(true, vec![], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(Some(TargetIdentifier::python("PersonRef").unwrap()), vec![])
            .unwrap(),
        QueryTokenProjection::new(person.clone(), BTreeMap::new(), BTreeMap::new()).unwrap(),
    )
    .unwrap();
    RuntimeProjection::try_new(
        BindingTarget::Python,
        ProjectionConfig::python(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"schema",
        )
        .unwrap(),
        &[ProjectionHandler::python_v1()],
        &[],
        BTreeMap::from([(person.clone(), model)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![person.clone()],
            vec![BTreeSet::from([person])],
            vec![],
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

fn rust_fixture(
    explicit_names: bool,
) -> Result<RuntimeProjection, type_bridge_contract::diagnostic::Diagnostic> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name_attr = type_bridge_contract::id::AttributeId::new("name").unwrap();
    let owns_id = type_bridge_contract::schema::OwnsFactId::new(person.clone(), name_attr).unwrap();
    let name_attr_id = TypeId::new(TypeKind::Attribute, "name").unwrap();
    let field_token = type_bridge_contract::projection::FieldTokenProjection::new(
        owns_id.clone(),
        owns_id.clone(),
        TargetIdentifier::rust("name").unwrap(),
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
        true,
        false,
        BTreeMap::new(),
    )
    .unwrap();
    let value_type = type_bridge_contract::projection::ProjectedTypeRef::Model(
        type_bridge_contract::projection::ProjectedModelUse::new(
            name_attr_id.clone(),
            type_bridge_contract::projection::ProjectedModelForm::Complete,
        ),
    );
    let create_field = type_bridge_contract::projection::CreateFieldProjection::new(
        owns_id.clone(),
        value_type.clone(),
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
    );
    let read_field = type_bridge_contract::projection::ReadFieldProjection::new(
        owns_id.clone(),
        value_type,
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
    );

    let mut create = CreateProjection::new(true, vec![create_field], BTreeMap::new()).unwrap();
    let read = CompleteReadProjection::new(vec![read_field], BTreeMap::new(), vec![]).unwrap();
    let mut query = QueryTokenProjection::new(
        person.clone(),
        BTreeMap::from([(owns_id.clone(), field_token)]),
        BTreeMap::new(),
    )
    .unwrap();
    if explicit_names {
        create = create.with_target_name(TargetIdentifier::rust("PersonCreate").unwrap());
        query = query.with_target_name(TargetIdentifier::rust("PersonType").unwrap());
    }
    let name_model = ModelProjection::new(
        name_attr_id.clone(),
        TargetIdentifier::rust("Name").unwrap(),
        DeclarationProjection::new(
            None,
            Some(type_bridge_contract::value::ValueTypeTag::String),
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(false, vec![], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(None, vec![]).unwrap(),
        QueryTokenProjection::new(name_attr_id.clone(), BTreeMap::new(), BTreeMap::new())
            .unwrap()
            .with_target_name(TargetIdentifier::rust("NameType").unwrap()),
    )
    .unwrap();

    let model = ModelProjection::new(
        person.clone(),
        TargetIdentifier::rust("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::new(),
            vec![owns_id.clone()],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        create,
        read,
        ReferenceReadProjection::new(
            Some(TargetIdentifier::rust("PersonRef").unwrap()),
            vec![owns_id.clone()],
        )
        .unwrap(),
        query,
    )
    .unwrap();
    RuntimeProjection::try_new(
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"rust-schema",
        )
        .unwrap(),
        &[ProjectionHandler::rust_v1()],
        &[],
        BTreeMap::from([(person.clone(), model), (name_attr_id.clone(), name_model)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![person.clone(), name_attr_id.clone()],
            vec![BTreeSet::from([person]), BTreeSet::from([name_attr_id])],
            vec![],
            vec![],
        )
        .unwrap(),
    )
}

fn rust_inherited_fixture() -> RuntimeProjection {
    let base = TypeId::new(TypeKind::Entity, "base_owner").unwrap();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let unrelated = TypeId::new(TypeKind::Entity, "unrelated_owner").unwrap();
    let name_attribute = type_bridge_contract::id::AttributeId::new("name").unwrap();
    let name_type = TypeId::new(TypeKind::Attribute, "name").unwrap();
    let base_owns =
        type_bridge_contract::schema::OwnsFactId::new(base.clone(), name_attribute.clone())
            .unwrap();
    let child_owns =
        type_bridge_contract::schema::OwnsFactId::new(child.clone(), name_attribute).unwrap();
    let multiplicity =
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap());
    let value_type = type_bridge_contract::projection::ProjectedTypeRef::Scalar(
        type_bridge_contract::value::ValueTypeTag::String,
    );

    let field = |effective: type_bridge_contract::schema::OwnsFactId| {
        let token = type_bridge_contract::projection::FieldTokenProjection::new(
            effective.clone(),
            base_owns.clone(),
            TargetIdentifier::rust("name").unwrap(),
            multiplicity,
            false,
            false,
            BTreeMap::new(),
        )
        .unwrap();
        (
            type_bridge_contract::projection::CreateFieldProjection::new(
                effective.clone(),
                value_type.clone(),
                multiplicity,
            ),
            type_bridge_contract::projection::ReadFieldProjection::new(
                effective.clone(),
                value_type.clone(),
                multiplicity,
            ),
            token,
        )
    };
    let (base_create_field, base_read_field, base_token) = field(base_owns.clone());
    let (child_create_field, child_read_field, child_token) = field(child_owns.clone());

    let base_model = ModelProjection::new(
        base.clone(),
        TargetIdentifier::rust("BaseOwner").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            true,
            false,
            BTreeMap::new(),
            vec![base_owns.clone()],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(false, vec![base_create_field], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![base_read_field], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::rust("BaseOwnerRef").unwrap()),
            vec![],
        )
        .unwrap(),
        QueryTokenProjection::new(
            base.clone(),
            BTreeMap::from([(base_owns.clone(), base_token)]),
            BTreeMap::new(),
        )
        .unwrap()
        .with_target_name(TargetIdentifier::rust("BaseOwnerType").unwrap()),
    )
    .unwrap();

    let sub_id = type_bridge_contract::schema::SubFactId::new(child.clone(), base.clone()).unwrap();
    let direct_sub = type_bridge_contract::projection::DirectSubProjection::new(
        sub_id.clone(),
        type_bridge_contract::schema::SchemaFactId::Sub(sub_id),
        BTreeMap::new(),
    )
    .unwrap();
    let child_model = ModelProjection::new(
        child.clone(),
        TargetIdentifier::rust("ChildOwner").unwrap(),
        DeclarationProjection::new(
            Some(base.clone()),
            None,
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap()
        .with_direct_sub(Some(direct_sub))
        .unwrap(),
        CreateProjection::new(true, vec![child_create_field], BTreeMap::new())
            .unwrap()
            .with_target_name(TargetIdentifier::rust("ChildOwnerCreate").unwrap()),
        CompleteReadProjection::new(vec![child_read_field], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::rust("ChildOwnerRef").unwrap()),
            vec![],
        )
        .unwrap(),
        QueryTokenProjection::new(
            child.clone(),
            BTreeMap::from([(child_owns.clone(), child_token)]),
            BTreeMap::new(),
        )
        .unwrap()
        .with_target_name(TargetIdentifier::rust("ChildOwnerType").unwrap()),
    )
    .unwrap();

    let unrelated_model = ModelProjection::new(
        unrelated.clone(),
        TargetIdentifier::rust("UnrelatedOwner").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(true, vec![], BTreeMap::new())
            .unwrap()
            .with_target_name(TargetIdentifier::rust("UnrelatedOwnerCreate").unwrap()),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(
            Some(TargetIdentifier::rust("UnrelatedOwnerRef").unwrap()),
            vec![],
        )
        .unwrap(),
        QueryTokenProjection::new(unrelated.clone(), BTreeMap::new(), BTreeMap::new())
            .unwrap()
            .with_target_name(TargetIdentifier::rust("UnrelatedOwnerType").unwrap()),
    )
    .unwrap();

    let name_model = ModelProjection::new(
        name_type.clone(),
        TargetIdentifier::rust("Name").unwrap(),
        DeclarationProjection::new(
            None,
            Some(type_bridge_contract::value::ValueTypeTag::String),
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(false, vec![], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(None, vec![]).unwrap(),
        QueryTokenProjection::new(name_type.clone(), BTreeMap::new(), BTreeMap::new())
            .unwrap()
            .with_target_name(TargetIdentifier::rust("NameType").unwrap()),
    )
    .unwrap();

    RuntimeProjection::try_new(
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"rust-inherited-schema",
        )
        .unwrap(),
        &[ProjectionHandler::rust_v1()],
        &[],
        BTreeMap::from([
            (base.clone(), base_model),
            (child.clone(), child_model),
            (unrelated.clone(), unrelated_model),
            (name_type.clone(), name_model),
        ]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![
                name_type.clone(),
                base.clone(),
                child.clone(),
                unrelated.clone(),
            ],
            vec![
                BTreeSet::from([name_type]),
                BTreeSet::from([base]),
                BTreeSet::from([child]),
                BTreeSet::from([unrelated]),
            ],
            vec![],
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

fn model_wire_mut<'a>(
    projection: &'a mut serde_json::Value,
    id: &TypeId,
) -> &'a mut serde_json::Value {
    let expected = serde_json::to_value(id).unwrap();
    projection["models"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|model| model["id"] == expected)
        .expect("model identity is present in canonical projection JSON")
}

fn field_wire_mut<'a>(
    projection: &'a mut serde_json::Value,
    owner: &TypeId,
    attribute: &str,
) -> &'a mut serde_json::Value {
    let model = model_wire_mut(projection, owner);
    model["query_tokens"]["fields"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|token| token["id"]["attribute"] == attribute)
        .expect("field identity is present in canonical projection JSON")
}

#[test]
fn canonical_projection_rebuilds_and_verifies_detached_fingerprints() {
    let runtime = fixture();
    let decoded = decode_runtime_projection_verified(
        &to_canonical_json(&runtime).unwrap(),
        &to_canonical_json(runtime.semantic_fingerprint()).unwrap(),
        &to_canonical_json(runtime.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, runtime);
}

#[test]
fn rust_projection_rebuilds_with_explicit_surface_names() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    assert_eq!(
        runtime.models()[&person]
            .reference_read()
            .construction_policy(),
        type_bridge_contract::projection::ReferenceConstructionPolicy::KeyFallback
    );
    let decoded = decode_runtime_projection_verified(
        &to_canonical_json(&runtime).unwrap(),
        &to_canonical_json(runtime.semantic_fingerprint()).unwrap(),
        &to_canonical_json(runtime.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, runtime);
}

#[test]
fn projection_wire_rejects_duplicate_reference_key_identities() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        let keys = model_wire_mut(value, &person)["reference_read"]["key_fields"]
            .as_array_mut()
            .unwrap();
        keys.push(keys[0].clone());
    });
    assert_eq!(error.code().as_str(), "duplicate_projected_reference_key");
}

#[test]
fn projection_wire_rejects_tampered_reference_construction_policy() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        model_wire_mut(value, &person)["reference_read"]["construction_policy"] =
            serde_json::Value::String("iid_only".to_owned());
    });
    assert_eq!(
        error.code().as_str(),
        "invalid_reference_construction_policy"
    );
}

#[test]
fn projection_wire_rejects_reference_key_without_exact_key_facet() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &person, "name")["key"] = serde_json::Value::Bool(false);
    });
    assert_eq!(error.code().as_str(), "invalid_projected_reference_key");
}

#[test]
fn projection_wire_rejects_reference_key_with_nonexact_cardinality() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        let multiplicity = &mut field_wire_mut(value, &person, "name")["multiplicity"];
        multiplicity["cardinality"]["min"] = serde_json::Value::String("0".to_owned());
        multiplicity["required"] = serde_json::Value::Bool(false);
    });
    assert_eq!(error.code().as_str(), "invalid_projected_reference_key");
}

#[test]
fn projection_wire_rejects_reference_key_with_noncomplete_value_form() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        model_wire_mut(value, &person)["complete_read"]["fields"][0]["value"]["value"]["form"] =
            serde_json::Value::String("reference".to_owned());
    });
    assert_eq!(error.code().as_str(), "invalid_projected_reference_key");
}

#[test]
fn rust_projection_rejects_missing_required_surface_names() {
    assert_eq!(
        rust_fixture(false).unwrap_err().code().as_str(),
        "missing_rust_projection_identifier",
    );
}

#[test]
fn noncanonical_and_tampered_projection_bytes_fail_closed() {
    let runtime = fixture();
    let bytes = to_canonical_json(&runtime).unwrap();
    let semantic = to_canonical_json(runtime.semantic_fingerprint()).unwrap();
    let binding = to_canonical_json(runtime.projection_fingerprint()).unwrap();

    let mut noncanonical = vec![b' '];
    noncanonical.extend_from_slice(&bytes);
    assert_eq!(
        decode_runtime_projection_verified(&noncanonical, &semantic, &binding)
            .unwrap_err()
            .code()
            .as_str(),
        "non_canonical_json",
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["models"][0]["target_name"] = serde_json::Value::String("ChangedPerson".into());
    let tampered = to_canonical_json(&value).unwrap();
    assert_eq!(
        decode_runtime_projection_verified(&tampered, &semantic, &binding)
            .unwrap_err()
            .code()
            .as_str(),
        "runtime_projection_fingerprint_mismatch",
    );
}

fn decode_mutated_projection(
    runtime: &RuntimeProjection,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> type_bridge_contract::diagnostic::Diagnostic {
    let semantic = to_canonical_json(runtime.semantic_fingerprint()).unwrap();
    let binding = to_canonical_json(runtime.projection_fingerprint()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&to_canonical_json(runtime).unwrap()).unwrap();
    mutate(&mut value);
    decode_runtime_projection_verified(&to_canonical_json(&value).unwrap(), &semantic, &binding)
        .unwrap_err()
}

#[test]
fn projection_wire_rejects_invalid_annotation_subjects_and_payloads() {
    let runtime = fixture();
    let error = decode_mutated_projection(&runtime, |value| {
        value["models"][0]["declaration"]["annotations"][0]["id"]["subject"] =
            serde_json::to_value(AnnotationSubjectId::Value(ValueFactId::new(
                type_bridge_contract::id::AttributeId::new("person").unwrap(),
            )))
            .unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_annotation_subject");

    let error = decode_mutated_projection(&runtime, |value| {
        value["models"][0]["declaration"]["annotations"][0]["value"] =
            serde_json::to_value(SchemaAnnotationValue::Presence).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_annotation_payload");
}

#[test]
fn projection_wire_rejects_duplicate_annotations_before_map_rebuild() {
    let runtime = fixture();
    let error = decode_mutated_projection(&runtime, |value| {
        let annotations = value["models"][0]["declaration"]["annotations"]
            .as_array_mut()
            .unwrap();
        annotations.push(annotations[0].clone());
    });
    assert_eq!(error.code().as_str(), "duplicate_projected_annotation");
}

#[test]
fn role_upcast_wire_carries_the_active_role_identity() {
    let child = RoleId::new("employment", "employee").unwrap();
    let parent = RoleId::new("membership", "member").unwrap();
    let role = ReadRoleProjection::new(
        child.clone(),
        BTreeSet::new(),
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
    )
    .unwrap();
    let read = CompleteReadProjection::new(vec![], BTreeMap::from([(child.clone(), role)]), vec![])
        .unwrap()
        .with_role_upcasts(BTreeMap::from([(child.clone(), vec![parent])]))
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&to_canonical_json(&read).unwrap()).unwrap();
    assert_eq!(
        value["role_upcasts"][0]["role"],
        serde_json::to_value(child).unwrap()
    );
}

#[test]
fn projection_wire_rejects_missing_mandatory_declaring_id() {
    let runtime = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &person, "name")
            .as_object_mut()
            .unwrap()
            .remove("declaring_id");
    });
    assert_eq!(error.code().as_str(), "invalid_canonical_value");
}

#[test]
fn field_token_rejects_annotation_map_key_mismatch() {
    let owner = TypeId::new(TypeKind::Entity, "person").unwrap();
    let other_owner = TypeId::new(TypeKind::Entity, "other_person").unwrap();
    let attribute = type_bridge_contract::id::AttributeId::new("name").unwrap();
    let owns = type_bridge_contract::schema::OwnsFactId::new(owner, attribute.clone()).unwrap();
    let other_owns = type_bridge_contract::schema::OwnsFactId::new(other_owner, attribute).unwrap();
    let annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Owns(owns.clone()),
        AnnotationKindId::Unique,
    );
    let annotation =
        ProjectedAnnotation::new(annotation_id, SchemaAnnotationValue::Presence).unwrap();
    let wrong_key = AnnotationFactId::new(
        AnnotationSubjectId::Owns(other_owns),
        AnnotationKindId::Unique,
    );

    let error = type_bridge_contract::projection::FieldTokenProjection::new(
        owns.clone(),
        owns,
        TargetIdentifier::rust("name").unwrap(),
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
        false,
        false,
        BTreeMap::from([(wrong_key, annotation)]),
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "invalid_projected_owns_annotation");
}

#[test]
fn projection_wire_rejects_annotation_for_another_valid_owns_edge() {
    let runtime = rust_inherited_fixture();
    let base = TypeId::new(TypeKind::Entity, "base_owner").unwrap();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let attribute = type_bridge_contract::id::AttributeId::new("name").unwrap();
    let other_valid_owns = type_bridge_contract::schema::OwnsFactId::new(base, attribute).unwrap();
    let annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Owns(other_valid_owns),
        AnnotationKindId::Unique,
    );
    let annotation =
        ProjectedAnnotation::new(annotation_id, SchemaAnnotationValue::Presence).unwrap();

    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &child, "name")["annotations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(annotation).unwrap());
    });
    assert_eq!(error.code().as_str(), "invalid_projected_owns_annotation");
}

#[test]
fn projection_accepts_direct_self_and_true_inherited_declaring_owns_ids() {
    let direct = rust_fixture(true).unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name = type_bridge_contract::id::AttributeId::new("name").unwrap();
    let direct_owns =
        type_bridge_contract::schema::OwnsFactId::new(person.clone(), name.clone()).unwrap();
    let direct_token = direct.models()[&person].query_tokens().fields()[&direct_owns].clone();
    assert_eq!(direct_token.id(), &direct_owns);
    assert_eq!(direct_token.declaring_id(), &direct_owns);

    let inherited = rust_inherited_fixture();
    let base = TypeId::new(TypeKind::Entity, "base_owner").unwrap();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let effective =
        type_bridge_contract::schema::OwnsFactId::new(child.clone(), name.clone()).unwrap();
    let declaring = type_bridge_contract::schema::OwnsFactId::new(base, name).unwrap();
    let inherited_token = &inherited.models()[&child].query_tokens().fields()[&effective];
    assert_eq!(inherited_token.id(), &effective);
    assert_eq!(inherited_token.declaring_id(), &declaring);

    let decoded = decode_runtime_projection_verified(
        &to_canonical_json(&inherited).unwrap(),
        &to_canonical_json(inherited.semantic_fingerprint()).unwrap(),
        &to_canonical_json(inherited.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    let decoded_token = &decoded.models()[&child].query_tokens().fields()[&effective];
    assert_eq!(decoded_token.id(), &effective);
    assert_eq!(decoded_token.declaring_id(), &declaring);
}

#[test]
fn projection_wire_rejects_different_attribute_declaring_id() {
    let runtime = rust_inherited_fixture();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &child, "name")["declaring_id"]["attribute"] =
            serde_json::Value::String("different_attribute".to_owned());
    });
    assert_eq!(error.code().as_str(), "invalid_projection_reference");
}

#[test]
fn projection_wire_rejects_unrelated_declaring_owner() {
    let runtime = rust_inherited_fixture();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let unrelated = TypeId::new(TypeKind::Entity, "unrelated_owner").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &child, "name")["declaring_id"]["owner"] =
            serde_json::to_value(unrelated).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_projection_reference");
}

#[test]
fn projection_wire_rejects_descendant_declaring_owner() {
    let runtime = rust_inherited_fixture();
    let base = TypeId::new(TypeKind::Entity, "base_owner").unwrap();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &base, "name")["declaring_id"]["owner"] =
            serde_json::to_value(child).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_projection_reference");
}

#[test]
fn projection_wire_rejects_missing_declaring_owner() {
    let runtime = rust_inherited_fixture();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let missing = TypeId::new(TypeKind::Entity, "missing_owner").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &child, "name")["declaring_id"]["owner"] =
            serde_json::to_value(missing).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_projection_reference");
}

#[test]
fn projection_wire_rejects_wrong_kind_declaring_owner() {
    let runtime = rust_inherited_fixture();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let wrong_kind = TypeId::new(TypeKind::Attribute, "name").unwrap();
    let error = decode_mutated_projection(&runtime, |value| {
        field_wire_mut(value, &child, "name")["declaring_id"]["owner"] =
            serde_json::to_value(wrong_kind).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_owns_owner");
}

#[test]
fn projection_wire_parent_cycle_terminates_before_declaring_owner_is_reached() {
    let runtime = rust_inherited_fixture();
    let base = TypeId::new(TypeKind::Entity, "base_owner").unwrap();
    let child = TypeId::new(TypeKind::Entity, "child_owner").unwrap();
    let unrelated = TypeId::new(TypeKind::Entity, "unrelated_owner").unwrap();
    let reverse_sub_id =
        type_bridge_contract::schema::SubFactId::new(base.clone(), child.clone()).unwrap();
    let reverse_sub = type_bridge_contract::projection::DirectSubProjection::new(
        reverse_sub_id.clone(),
        type_bridge_contract::schema::SchemaFactId::Sub(reverse_sub_id),
        BTreeMap::new(),
    )
    .unwrap();

    let error = decode_mutated_projection(&runtime, |value| {
        let base_wire = model_wire_mut(value, &base);
        base_wire["declaration"]["parent"] = serde_json::to_value(child.clone()).unwrap();
        base_wire["declaration"]["direct_sub"] = serde_json::to_value(reverse_sub).unwrap();
        field_wire_mut(value, &child, "name")["declaring_id"]["owner"] =
            serde_json::to_value(unrelated).unwrap();
    });
    assert_eq!(error.code().as_str(), "invalid_projection_reference");
}
