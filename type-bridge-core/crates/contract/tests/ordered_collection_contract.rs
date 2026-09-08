use serde_json::Value;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CollectionMode,
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFactId, RelatesFact, RelatesFactId,
    SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
    decode_declared_schema, encode_declared_schema,
};

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type identity is valid")
}

fn sourced(fact: SchemaFact, index: u32) -> SourcedSchemaFact {
    SourcedSchemaFact::new(
        fact,
        SourceSpan::new(
            DocumentId::new("ordered-collections.yaml").unwrap(),
            u64::from(index),
            u64::from(index + 1),
            index + 1,
            1,
            index + 1,
            2,
        )
        .unwrap(),
    )
}

fn collection_schema(mode: CollectionMode, distinct: bool) -> Result<DeclaredSchema, String> {
    let person = type_id(TypeKind::Entity, "person");
    let name_type = type_id(TypeKind::Attribute, "name");
    let name = AttributeId::new("name").unwrap();
    let membership = type_id(TypeKind::Relation, "membership");
    let member = RoleId::new("membership", "member").unwrap();
    let owns = OwnsFactId::new(person.clone(), name).unwrap();
    let relates = RelatesFactId::new(membership.clone(), member).unwrap();

    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person).unwrap()),
        SchemaFact::Type(TypeFact::new(name_type).unwrap()),
        SchemaFact::Type(TypeFact::new(membership).unwrap()),
        SchemaFact::Owns(OwnsFact::new_with_collection_mode(owns.clone(), mode)),
        SchemaFact::Relates(
            RelatesFact::new_with_collection_mode(relates.clone(), None, mode).unwrap(),
        ),
    ];
    if distinct {
        for subject in [
            AnnotationSubjectId::Owns(owns),
            AnnotationSubjectId::Relates(relates),
        ] {
            facts.push(SchemaFact::Annotation(
                AnnotationFact::new(
                    AnnotationFactId::new(subject, AnnotationKindId::Distinct),
                    SchemaAnnotationValue::Presence,
                )
                .unwrap(),
            ));
        }
    }

    DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts
            .into_iter()
            .enumerate()
            .map(|(index, fact)| sourced(fact, u32::try_from(index).unwrap())),
    )
    .map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.diagnostic().code().as_str())
            .collect::<Vec<_>>()
            .join(",")
    })
}

#[test]
fn collection_mode_is_content_not_fact_identity_and_unordered_stays_omitted() {
    let person = type_id(TypeKind::Entity, "person");
    let owns_id = OwnsFactId::new(person, AttributeId::new("name").unwrap()).unwrap();
    let unordered = OwnsFact::new(owns_id.clone());
    let ordered = OwnsFact::new_with_collection_mode(owns_id, CollectionMode::OrderedList);

    assert_eq!(unordered.collection_mode(), CollectionMode::Unordered);
    assert_eq!(ordered.collection_mode(), CollectionMode::OrderedList);
    assert_eq!(
        SchemaFact::Owns(unordered.clone()).id(),
        SchemaFact::Owns(ordered.clone()).id(),
    );

    let unordered_json: Value = serde_json::to_value(unordered).unwrap();
    let ordered_json: Value = serde_json::to_value(ordered).unwrap();
    assert!(unordered_json.get("collection_mode").is_none());
    assert_eq!(ordered_json["collection_mode"], "ordered_list");
}

#[test]
fn legacy_unordered_declared_schema_bytes_and_fingerprint_remain_exact() {
    let legacy = include_bytes!("../../../../tests/fixtures/query-v2-model-remote-declared.json")
        .strip_suffix(b"\n")
        .unwrap();
    let decoded = decode_declared_schema(legacy).unwrap();

    assert_eq!(encode_declared_schema(&decoded).unwrap(), legacy);
    assert_eq!(
        decoded
            .declared_identity_fingerprint()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "b3b1026c5bcb153c951cc5787bef5899a24ecac21f428e708ac8507c36ae346f",
    );
    assert!(decoded.facts().all(|fact| match fact {
        SchemaFact::Owns(fact) => fact.collection_mode().is_unordered(),
        SchemaFact::Relates(fact) => fact.collection_mode().is_unordered(),
        _ => true,
    }));
}

#[test]
fn explicit_unordered_declared_wire_is_rejected_as_noncanonical() {
    let legacy = include_bytes!("../../../../tests/fixtures/query-v2-model-remote-declared.json")
        .strip_suffix(b"\n")
        .unwrap();
    let mut value: Value = serde_json::from_slice(legacy).unwrap();
    let owns = value["facts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|fact| fact["kind"] == "owns")
        .unwrap();
    owns["value"]["collection_mode"] = Value::String("unordered".to_owned());

    assert_eq!(
        decode_declared_schema(&to_canonical_json(&value).unwrap())
            .unwrap_err()
            .code()
            .as_str(),
        "non_canonical_declared_schema",
    );
}

#[test]
fn distinct_is_presence_only_and_requires_ordered_owns_or_relates() {
    assert!(collection_schema(CollectionMode::OrderedList, true).is_ok());
    assert_eq!(
        collection_schema(CollectionMode::Unordered, true).unwrap_err(),
        "distinct_requires_ordered_collection,distinct_requires_ordered_collection",
    );

    let person = type_id(TypeKind::Entity, "person");
    let role = RoleId::new("membership", "member").unwrap();
    let plays = PlaysFactId::new(person.clone(), role).unwrap();
    assert_eq!(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Plays(plays),
                AnnotationKindId::Distinct,
            ),
            SchemaAnnotationValue::Presence,
        )
        .unwrap_err()
        .code()
        .as_str(),
        "invalid_annotation_subject",
    );
    assert_eq!(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(person),
                AnnotationKindId::Distinct,
            ),
            SchemaAnnotationValue::Presence,
        )
        .unwrap_err()
        .code()
        .as_str(),
        "invalid_annotation_subject",
    );
}
