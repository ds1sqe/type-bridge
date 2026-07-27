use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SchemaFactId, SourceSpan,
    SourcedSchemaFact, TypeFact,
};
use type_bridge_contract::semantic_profile::SemanticProfile;
use type_bridge_contract::value::Cardinality;
use type_bridge_schema::{
    OBSERVED_SCHEMA_CANONICALIZATION_VERSION, ObservedFactProvenance, ObservedFactScope,
    ObservedSchema, ObservedSchemaFact, canonicalize_observed_schema,
};

fn type_fact(kind: TypeKind, label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(kind, label).expect("type identifier is valid"))
            .expect("type fact is valid"),
    )
}

fn observed(fact: SchemaFact, provenance: ObservedFactProvenance) -> ObservedSchemaFact {
    ObservedSchemaFact::new(fact, provenance, ObservedFactScope::Managed)
}

fn profile() -> SemanticProfile {
    SemanticProfile::resolve(
        &SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile identifier is valid"),
    )
    .expect("semantic profile is supported")
}

fn source() -> SourceSpan {
    SourceSpan::new(
        DocumentId::new("expected.schema").expect("document identifier is valid"),
        0,
        1,
        1,
        1,
        1,
        2,
    )
    .expect("source span is valid")
}

#[test]
fn capture_order_does_not_change_direct_identity_or_managed_order() {
    let person = type_fact(TypeKind::Entity, "person");
    let company = type_fact(TypeKind::Entity, "company");
    let first = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [
            observed(person.clone(), ObservedFactProvenance::Direct),
            observed(company.clone(), ObservedFactProvenance::Direct),
        ],
    );
    let reversed = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [
            observed(company.clone(), ObservedFactProvenance::Direct),
            observed(person.clone(), ObservedFactProvenance::Direct),
        ],
    );

    let first = canonicalize_observed_schema(&first, &profile()).expect("capture canonicalizes");
    let reversed =
        canonicalize_observed_schema(&reversed, &profile()).expect("capture canonicalizes");
    let authored = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        [person, company]
            .into_iter()
            .map(|fact| SourcedSchemaFact::new(fact, source())),
    )
    .expect("authored schema validates");

    assert_eq!(
        first.canonicalization_version(),
        OBSERVED_SCHEMA_CANONICALIZATION_VERSION
    );
    assert_eq!(
        first.canonical_identity_bytes().unwrap(),
        reversed.canonical_identity_bytes().unwrap()
    );
    assert_eq!(
        first.declared_identity_fingerprint(),
        authored.declared_identity_fingerprint()
    );
    assert_eq!(first.managed_scope(), reversed.managed_scope());
    assert!(
        first
            .managed_scope()
            .iter()
            .collect::<Vec<_>>()
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    );
}

#[test]
fn inherited_interfaces_and_proven_server_defaults_are_not_direct_facts() {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let employee = TypeId::new(TypeKind::Entity, "employee").unwrap();
    let name = AttributeId::new("name").unwrap();
    let direct_owns = OwnsFactId::new(person.clone(), name.clone()).unwrap();
    let inherited_owns = OwnsFactId::new(employee.clone(), name).unwrap();
    let default_card = AnnotationFact::new(
        AnnotationFactId::new(
            AnnotationSubjectId::Owns(direct_owns.clone()),
            AnnotationKindId::Card,
        ),
        SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(1)).unwrap()),
    )
    .unwrap();

    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [
            observed(
                SchemaFact::Type(TypeFact::new(person).unwrap()),
                ObservedFactProvenance::Direct,
            ),
            observed(
                SchemaFact::Type(TypeFact::new(employee).unwrap()),
                ObservedFactProvenance::Direct,
            ),
            observed(
                SchemaFact::Type(
                    TypeFact::new(TypeId::new(TypeKind::Attribute, "name").unwrap()).unwrap(),
                ),
                ObservedFactProvenance::Direct,
            ),
            observed(
                SchemaFact::Owns(OwnsFact::new(direct_owns.clone())),
                ObservedFactProvenance::Direct,
            ),
            observed(
                SchemaFact::Owns(OwnsFact::new(inherited_owns.clone())),
                ObservedFactProvenance::Inherited {
                    declared_fact: SchemaFactId::Owns(direct_owns.clone()),
                },
            ),
            observed(
                SchemaFact::Annotation(default_card),
                ObservedFactProvenance::ServerDefault,
            ),
        ],
    );

    let canonical =
        canonicalize_observed_schema(&capture, &profile()).expect("capture canonicalizes");
    assert!(
        canonical
            .direct_schema()
            .fact(&SchemaFactId::Owns(direct_owns))
            .is_some()
    );
    assert!(
        canonical
            .direct_schema()
            .fact(&SchemaFactId::Owns(inherited_owns))
            .is_none()
    );
    assert_eq!(canonical.direct_schema().facts().len(), 4);
}

#[test]
fn ambiguous_provenance_fails_closed() {
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [observed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::Ambiguous,
        )],
    );

    let error =
        canonicalize_observed_schema(&capture, &profile()).expect_err("ambiguity is rejected");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "ambiguous_observed_provenance"
    );
}

#[test]
fn only_cardinality_annotations_may_be_server_defaults() {
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [observed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::ServerDefault,
        )],
    );

    let error = canonicalize_observed_schema(&capture, &profile())
        .expect_err("invalid default is rejected");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "invalid_observed_server_default"
    );
}

#[test]
fn server_default_cardinality_must_match_selected_profile() {
    let owns = OwnsFactId::new(
        TypeId::new(TypeKind::Entity, "person").unwrap(),
        AttributeId::new("name").unwrap(),
    )
    .unwrap();
    let wrong_default = AnnotationFact::new(
        AnnotationFactId::new(AnnotationSubjectId::Owns(owns), AnnotationKindId::Card),
        SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(2)).unwrap()),
    )
    .unwrap();
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [observed(
            SchemaFact::Annotation(wrong_default),
            ObservedFactProvenance::ServerDefault,
        )],
    );

    let error = canonicalize_observed_schema(&capture, &profile())
        .expect_err("a mismatched server default is rejected");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "invalid_observed_server_default"
    );
}

#[test]
fn inherited_origin_must_be_an_existing_direct_fact() {
    let declared_owns = OwnsFactId::new(
        TypeId::new(TypeKind::Entity, "person").unwrap(),
        AttributeId::new("name").unwrap(),
    )
    .unwrap();
    let inherited_owns = OwnsFactId::new(
        TypeId::new(TypeKind::Entity, "employee").unwrap(),
        AttributeId::new("name").unwrap(),
    )
    .unwrap();
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [observed(
            SchemaFact::Owns(OwnsFact::new(inherited_owns)),
            ObservedFactProvenance::Inherited {
                declared_fact: SchemaFactId::Owns(declared_owns),
            },
        )],
    );

    let error = canonicalize_observed_schema(&capture, &profile())
        .expect_err("a missing direct inheritance origin is rejected");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "invalid_observed_inheritance_origin"
    );
}

#[test]
fn type_bridge_internal_facts_are_excluded_from_managed_scope() {
    let managed = type_fact(TypeKind::Entity, "person");
    let internal = type_fact(TypeKind::Entity, "typebridge-migration-state");
    let internal_id = internal.id();
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [
            observed(managed, ObservedFactProvenance::Direct),
            ObservedSchemaFact::new(
                internal,
                ObservedFactProvenance::Direct,
                ObservedFactScope::TypeBridgeInternal,
            ),
        ],
    );

    let canonical =
        canonicalize_observed_schema(&capture, &profile()).expect("capture canonicalizes");
    assert_eq!(canonical.direct_schema().facts().len(), 2);
    assert_eq!(canonical.managed_scope().len(), 1);
    assert!(!canonical.managed_scope().contains(&internal_id));
}

#[test]
fn duplicate_observed_fact_id_is_rejected() {
    let person = type_fact(TypeKind::Entity, "person");
    let capture = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [
            observed(person.clone(), ObservedFactProvenance::Direct),
            observed(person, ObservedFactProvenance::Direct),
        ],
    );

    let error =
        canonicalize_observed_schema(&capture, &profile()).expect_err("duplicates are rejected");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "duplicate_observed_fact"
    );
}
