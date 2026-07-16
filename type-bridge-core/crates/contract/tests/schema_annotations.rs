use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocText, DocumentId, OwnsFact, OwnsFactId, RegexPattern, RelatesFactId,
    SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId,
    TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type identifier is valid")
}

fn attribute_id(label: &str) -> AttributeId {
    AttributeId::new(label).expect("fixture attribute identifier is valid")
}

fn sourced(fact: SchemaFact, index: u32) -> SourcedSchemaFact {
    let start = u64::from(index) * 10;
    SourcedSchemaFact::new(
        fact,
        SourceSpan::new(
            DocumentId::new("annotations.yaml").expect("fixture document identifier is valid"),
            start,
            start + 1,
            index + 1,
            1,
            index + 1,
            2,
        )
        .expect("fixture source span is valid"),
    )
}

fn annotation(
    subject: AnnotationSubjectId,
    kind: AnnotationKindId,
    value: SchemaAnnotationValue,
) -> Result<AnnotationFact, type_bridge_contract::diagnostic::Diagnostic> {
    AnnotationFact::new(AnnotationFactId::new(subject, kind), value)
}

#[test]
fn annotation_subject_registry_matches_typedb_3121() {
    let entity = type_id(TypeKind::Entity, "person");
    assert!(
        annotation(
            AnnotationSubjectId::Type(entity),
            AnnotationKindId::Independent,
            SchemaAnnotationValue::Presence,
        )
        .is_err()
    );

    let relation = type_id(TypeKind::Relation, "membership");
    let role = RoleId::new("membership", "member").expect("fixture role identifier is valid");
    let relates = RelatesFactId::new(relation, role).expect("fixture relates identifier is valid");
    assert!(
        annotation(
            AnnotationSubjectId::Relates(relates),
            AnnotationKindId::Abstract,
            SchemaAnnotationValue::Presence,
        )
        .is_ok()
    );

    let value = ValueFactId::new(attribute_id("name"));
    assert!(
        annotation(
            AnnotationSubjectId::Value(value),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("not a concept").expect("doc text is valid")),
        )
        .is_err()
    );
}

#[test]
fn metadata_payload_is_exactly_one_string_value() {
    let subject = AnnotationSubjectId::Type(type_id(TypeKind::Entity, "person"));
    let kind = AnnotationKindId::meta("source").expect("metadata key is valid");

    assert!(
        annotation(
            subject.clone(),
            kind.clone(),
            SchemaAnnotationValue::Meta(CanonicalValue::Long(1)),
        )
        .is_err()
    );
    assert!(
        annotation(
            subject,
            kind,
            SchemaAnnotationValue::Meta(CanonicalValue::String(
                type_bridge_contract::value::CanonicalString::new("probe").unwrap(),
            )),
        )
        .is_ok()
    );
}

#[test]
fn declared_schema_rejects_incompatible_value_constraints() {
    let attribute_type = type_id(TypeKind::Attribute, "age");
    let value_id = ValueFactId::new(attribute_id("age"));
    let regex = annotation(
        AnnotationSubjectId::Value(value_id.clone()),
        AnnotationKindId::Regex,
        SchemaAnnotationValue::Regex(RegexPattern::new(".+").expect("regex is valid")),
    )
    .expect("regex annotation is structurally valid");

    let result = DeclaredSchema::from_facts(
        FormatVersion::V1,
        Default::default(),
        [
            sourced(
                SchemaFact::Type(TypeFact::new(attribute_type).expect("type fact is valid")),
                0,
            ),
            sourced(
                SchemaFact::Value(ValueFact::new(value_id, ValueTypeTag::Long)),
                1,
            ),
            sourced(SchemaFact::Annotation(regex), 2),
        ],
    );

    assert!(result.is_err());
}

#[test]
fn declared_schema_resolves_inherited_value_domain_for_owns_constraints() {
    let parent = type_id(TypeKind::Attribute, "text");
    let child = type_id(TypeKind::Attribute, "name");
    let owner = type_id(TypeKind::Entity, "person");
    let parent_value = ValueFactId::new(attribute_id("text"));
    let sub = SubFactId::new(child.clone(), parent.clone()).expect("sub fact identifier is valid");
    let owns = OwnsFactId::new(owner.clone(), attribute_id("name"))
        .expect("owns fact identifier is valid");
    let regex = annotation(
        AnnotationSubjectId::Owns(owns.clone()),
        AnnotationKindId::Regex,
        SchemaAnnotationValue::Regex(RegexPattern::new(".+").expect("regex is valid")),
    )
    .expect("regex annotation is structurally valid");

    let result = DeclaredSchema::from_facts(
        FormatVersion::V1,
        Default::default(),
        [
            sourced(
                SchemaFact::Type(TypeFact::new(parent).expect("parent type fact is valid")),
                0,
            ),
            sourced(
                SchemaFact::Type(TypeFact::new(child).expect("child type fact is valid")),
                1,
            ),
            sourced(
                SchemaFact::Type(TypeFact::new(owner).expect("owner type fact is valid")),
                2,
            ),
            sourced(
                SchemaFact::Value(ValueFact::new(parent_value, ValueTypeTag::String)),
                3,
            ),
            sourced(SchemaFact::Sub(SubFact::new(sub)), 4),
            sourced(SchemaFact::Owns(OwnsFact::new(owns)), 5),
            sourced(SchemaFact::Annotation(regex), 6),
        ],
    );

    assert!(result.is_ok());
}

#[test]
fn declared_schema_rejects_double_keys() {
    let owner = type_id(TypeKind::Entity, "sample");
    let attribute = type_id(TypeKind::Attribute, "measurement");
    let value = ValueFactId::new(attribute_id("measurement"));
    let owns = OwnsFactId::new(owner.clone(), attribute_id("measurement"))
        .expect("owns fact identifier is valid");
    let key = annotation(
        AnnotationSubjectId::Owns(owns.clone()),
        AnnotationKindId::Key,
        SchemaAnnotationValue::Presence,
    )
    .expect("key annotation is structurally valid");

    let result = DeclaredSchema::from_facts(
        FormatVersion::V1,
        Default::default(),
        [
            sourced(
                SchemaFact::Type(TypeFact::new(owner).expect("owner type fact is valid")),
                0,
            ),
            sourced(
                SchemaFact::Type(TypeFact::new(attribute).expect("attribute type fact is valid")),
                1,
            ),
            sourced(
                SchemaFact::Value(ValueFact::new(value, ValueTypeTag::Double)),
                2,
            ),
            sourced(SchemaFact::Owns(OwnsFact::new(owns)), 3),
            sourced(SchemaFact::Annotation(key), 4),
        ],
    );

    assert!(result.is_err());
}
