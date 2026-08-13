use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CompleteReadProjection, CreateFieldProjection, CreateProjection,
    DeclarationProjection, EmissionPlan, FieldTokenProjection, ModelProjection, PlayingProjection,
    ProjectedAnnotation, ProjectedContainer, ProjectedMultiplicity, ProjectedTypeRef,
    ProjectionConfig, ProjectionHandler, QueryTokenProjection, ReadFieldProjection,
    ReferenceReadProjection, RoleTokenProjection, RuntimeProjection, TargetIdentifier,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CollectionMode, OwnsFactId,
    PlaysFactId, RelatesFactId, SchemaAnnotationValue,
};
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;
use type_bridge_contract::value::{Cardinality, ValueTypeTag};

fn multiplicity(mode: CollectionMode) -> ProjectedMultiplicity {
    ProjectedMultiplicity::new(Cardinality::new(0, Some(1)).unwrap(), mode)
}

fn field_model(
    token_mode: CollectionMode,
    create_mode: CollectionMode,
    read_mode: CollectionMode,
) -> Result<ModelProjection, type_bridge_contract::diagnostic::Diagnostic> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let owns = OwnsFactId::new(person.clone(), AttributeId::new("name").unwrap()).unwrap();
    let token = FieldTokenProjection::new(
        owns.clone(),
        owns.clone(),
        TargetIdentifier::python("name").unwrap(),
        multiplicity(token_mode),
        false,
        false,
        BTreeMap::new(),
    )?;
    ModelProjection::new(
        person.clone(),
        TargetIdentifier::python("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::new(),
            vec![owns.clone()],
            BTreeMap::new(),
            BTreeSet::new(),
        )?,
        CreateProjection::new(
            true,
            vec![CreateFieldProjection::new(
                owns.clone(),
                ProjectedTypeRef::Scalar(ValueTypeTag::String),
                multiplicity(create_mode),
            )],
            BTreeMap::new(),
        )?,
        CompleteReadProjection::new(
            vec![ReadFieldProjection::new(
                owns.clone(),
                ProjectedTypeRef::Scalar(ValueTypeTag::String),
                multiplicity(read_mode),
            )],
            BTreeMap::new(),
            vec![],
        )?,
        ReferenceReadProjection::new(None, vec![])?,
        QueryTokenProjection::new(person, BTreeMap::from([(owns, token)]), BTreeMap::new())?,
    )
}

fn field_runtime(mode: CollectionMode) -> RuntimeProjection {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name = TypeId::new(TypeKind::Attribute, "name").unwrap();
    let person_model = field_model(mode, mode, mode).unwrap();
    let name_model = ModelProjection::new(
        name.clone(),
        TargetIdentifier::python("Name").unwrap(),
        DeclarationProjection::new(
            None,
            Some(ValueTypeTag::String),
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
        QueryTokenProjection::new(name.clone(), BTreeMap::new(), BTreeMap::new()).unwrap(),
    )
    .unwrap();
    let handlers = match mode {
        CollectionMode::Unordered => vec![ProjectionHandler::python_v1()],
        CollectionMode::OrderedList => vec![ProjectionHandler::python_v2()],
    };
    RuntimeProjection::try_new(
        BindingTarget::Python,
        ProjectionConfig::python(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"ordered-collection-schema",
        )
        .unwrap(),
        &handlers,
        &[],
        BTreeMap::from([(person.clone(), person_model), (name.clone(), name_model)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![name.clone(), person.clone()],
            vec![BTreeSet::from([name]), BTreeSet::from([person])],
            vec![],
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn ordered_multiplicity_is_always_a_sequence_and_unordered_wire_stays_legacy() {
    let cardinality = Cardinality::new(0, Some(1)).unwrap();
    let legacy = ProjectedMultiplicity::from_cardinality(cardinality);
    let ordered = ProjectedMultiplicity::new(cardinality, CollectionMode::OrderedList);

    assert_eq!(legacy.collection_mode(), CollectionMode::Unordered);
    assert_eq!(legacy.container(), ProjectedContainer::Scalar);
    assert_eq!(ordered.collection_mode(), CollectionMode::OrderedList);
    assert_eq!(ordered.container(), ProjectedContainer::Sequence);

    let legacy_json: Value = serde_json::to_value(legacy).unwrap();
    let ordered_json: Value = serde_json::to_value(ordered).unwrap();
    assert!(legacy_json.get("collection_mode").is_none());
    assert_eq!(ordered_json["collection_mode"], "ordered_list");
}

#[test]
fn ordered_collection_handlers_use_the_frozen_successor_versions() {
    for (handler, id, version) in [
        (
            ProjectionHandler::python_v2(),
            "typebridge.generator.python",
            2,
        ),
        (
            ProjectionHandler::typescript_v2(),
            "typebridge.generator.typescript",
            2,
        ),
        (ProjectionHandler::rust_v2(), "typebridge.generator.rust", 2),
        (ProjectionHandler::c_v3(), "typebridge.generator.c", 3),
    ] {
        assert_eq!(handler.id().as_str(), id);
        assert_eq!(handler.version().get(), version);
    }
}

#[test]
fn ordered_projection_round_trips_and_explicit_unordered_wire_is_noncanonical() {
    let ordered = field_runtime(CollectionMode::OrderedList);
    let ordered_bytes = to_canonical_json(&ordered).unwrap();
    let decoded = decode_runtime_projection_verified(
        &ordered_bytes,
        &to_canonical_json(ordered.semantic_fingerprint()).unwrap(),
        &to_canonical_json(ordered.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, ordered);
    assert!(
        ordered_bytes
            .windows(b"\"collection_mode\":\"ordered_list\"".len())
            .any(|window| window == b"\"collection_mode\":\"ordered_list\"")
    );

    let unordered = field_runtime(CollectionMode::Unordered);
    assert_ne!(
        ordered.projection_fingerprint(),
        unordered.projection_fingerprint()
    );
    let mut value: Value = serde_json::from_slice(&to_canonical_json(&unordered).unwrap()).unwrap();
    let person = value["models"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|model| model["id"]["label"] == "person")
        .unwrap();
    person["query_tokens"]["fields"][0]["multiplicity"]["collection_mode"] =
        Value::String("unordered".to_owned());
    person["create"]["fields"][0]["multiplicity"]["collection_mode"] =
        Value::String("unordered".to_owned());
    person["complete_read"]["fields"][0]["multiplicity"]["collection_mode"] =
        Value::String("unordered".to_owned());
    assert_eq!(
        decode_runtime_projection_verified(
            &to_canonical_json(&value).unwrap(),
            &to_canonical_json(unordered.semantic_fingerprint()).unwrap(),
            &to_canonical_json(unordered.projection_fingerprint()).unwrap(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "non_canonical_runtime_projection",
    );
}

#[test]
fn distinct_projected_tokens_require_ordered_collection_mode() {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let owns = OwnsFactId::new(person, AttributeId::new("name").unwrap()).unwrap();
    let owns_annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Owns(owns.clone()),
        AnnotationKindId::Distinct,
    );
    let owns_annotation =
        ProjectedAnnotation::new(owns_annotation_id.clone(), SchemaAnnotationValue::Presence)
            .unwrap();
    let owns_annotations = BTreeMap::from([(owns_annotation_id, owns_annotation)]);

    assert_eq!(
        FieldTokenProjection::new(
            owns.clone(),
            owns.clone(),
            TargetIdentifier::python("name").unwrap(),
            multiplicity(CollectionMode::Unordered),
            false,
            false,
            owns_annotations.clone(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "distinct_requires_ordered_collection",
    );
    assert!(
        FieldTokenProjection::new(
            owns.clone(),
            owns,
            TargetIdentifier::python("name").unwrap(),
            multiplicity(CollectionMode::OrderedList),
            false,
            false,
            owns_annotations,
        )
        .is_ok()
    );

    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    let relates = RelatesFactId::new(membership.clone(), member.clone()).unwrap();
    let role_annotation_id = AnnotationFactId::new(
        AnnotationSubjectId::Relates(relates),
        AnnotationKindId::Distinct,
    );
    let role_annotation =
        ProjectedAnnotation::new(role_annotation_id.clone(), SchemaAnnotationValue::Presence)
            .unwrap();
    let role_annotations = BTreeMap::from([(role_annotation_id, role_annotation)]);
    assert_eq!(
        RoleTokenProjection::new(
            membership.clone(),
            member.clone(),
            TargetIdentifier::python("member").unwrap(),
            BTreeSet::new(),
            None,
            multiplicity(CollectionMode::Unordered),
            false,
            role_annotations.clone(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "distinct_requires_ordered_collection",
    );
    assert!(
        RoleTokenProjection::new(
            membership,
            member,
            TargetIdentifier::python("member").unwrap(),
            BTreeSet::new(),
            None,
            multiplicity(CollectionMode::OrderedList),
            false,
            role_annotations,
        )
        .is_ok()
    );
}

#[test]
fn model_facets_require_exact_collection_multiplicity_equality() {
    assert!(
        field_model(
            CollectionMode::OrderedList,
            CollectionMode::OrderedList,
            CollectionMode::OrderedList,
        )
        .is_ok()
    );
    for result in [
        field_model(
            CollectionMode::OrderedList,
            CollectionMode::Unordered,
            CollectionMode::OrderedList,
        ),
        field_model(
            CollectionMode::OrderedList,
            CollectionMode::OrderedList,
            CollectionMode::Unordered,
        ),
    ] {
        assert_eq!(
            result.unwrap_err().code().as_str(),
            "projected_multiplicity_mismatch",
        );
    }
}

#[test]
fn playing_projection_rejects_ordered_multiplicity() {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    let plays = PlaysFactId::new(person, member.clone()).unwrap();
    assert_eq!(
        PlayingProjection::new(
            plays,
            member,
            multiplicity(CollectionMode::OrderedList),
            BTreeMap::new(),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "ordered_playing_projection",
    );
}
