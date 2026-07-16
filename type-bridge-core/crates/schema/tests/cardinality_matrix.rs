use std::collections::BTreeSet;

use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationKindId, AnnotationSubjectId, DeclaredSchema, DocumentId, SchemaAnnotationValue,
    SchemaDiagnostics, SchemaFact,
};
use type_bridge_contract::value::Cardinality;
use type_bridge_schema::{
    SchemaDocumentSet, normalize_documents, resolve, semantic_schema_fingerprint,
};

fn normalize(source: &str) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("fixture document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses");
    normalize_documents(&documents)
}

fn declared(source: &str) -> DeclaredSchema {
    normalize(source).expect("fixture normalizes")
}

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid")
}

fn cardinalities(schema: &DeclaredSchema) -> Vec<Cardinality> {
    schema
        .facts()
        .filter_map(|fact| match fact {
            SchemaFact::Annotation(annotation)
                if annotation.id().kind() == &AnnotationKindId::Card =>
            {
                let SchemaAnnotationValue::Cardinality(cardinality) = annotation.value() else {
                    panic!("card annotation has a cardinality payload")
                };
                Some(*cardinality)
            }
            _ => None,
        })
        .collect()
}

fn ownership_annotation_kinds(schema: &DeclaredSchema) -> BTreeSet<AnnotationKindId> {
    schema
        .facts()
        .filter_map(|fact| match fact {
            SchemaFact::Annotation(annotation)
                if matches!(annotation.id().subject(), AnnotationSubjectId::Owns(_)) =>
            {
                Some(annotation.id().kind().clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn scalar_exact_and_expanded_exact_cardinalities_normalize_identically() {
    let scalar = declared(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name: { card: 2 }
"#,
    );
    let expanded = declared(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name: { card: { min: 2, max: 2 } }
"#,
    );

    assert_eq!(
        scalar.declared_identity_fingerprint(),
        expanded.declared_identity_fingerprint(),
    );
    assert_eq!(
        cardinalities(&scalar),
        [Cardinality::new(2, Some(2)).unwrap()],
    );
}

#[test]
fn range_bounds_preserve_u64_maximum_and_omitted_maximum() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  finite: { value: string }
entities:
  player:
    owns:
      finite:
        card: { min: 0, max: 18446744073709551615 }
relations:
  link:
    relates:
      endpoint:
        card: { min: 1 }
plays:
  player:
    link:
      endpoint:
        card: { min: 2, max: 4 }
"#,
    );
    let cards = cardinalities(&schema);

    assert!(cards.contains(&Cardinality::new(0, Some(u64::MAX)).unwrap()));
    assert!(cards.contains(&Cardinality::new(1, None).unwrap()));
    assert!(cards.contains(&Cardinality::new(2, Some(4)).unwrap()));
}

#[test]
fn omitted_interface_cards_resolve_to_all_three_provider_defaults() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns: [name]
relations:
  link:
    relates: [endpoint]
plays:
  person:
    link: [endpoint]
"#,
    );
    assert!(cardinalities(&schema).is_empty());

    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let person = TypeId::new(TypeKind::Entity, "person").expect("type identifier is valid");
    let link = TypeId::new(TypeKind::Relation, "link").expect("type identifier is valid");
    let person = &resolved.types()[&person];
    let link = &resolved.types()[&link];

    assert_eq!(
        person.owns().values().next().unwrap().cardinality(),
        Cardinality::new(0, Some(1)).unwrap(),
    );
    assert_eq!(
        link.relates().values().next().unwrap().cardinality(),
        Cardinality::new(0, Some(1)).unwrap(),
    );
    assert_eq!(
        person.plays().values().next().unwrap().cardinality(),
        Cardinality::new(0, None).unwrap(),
    );
}

#[test]
fn negative_inverted_and_exact_zero_cardinalities_fail_closed() {
    let negative = normalize(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name: { card: -1 }
"#,
    )
    .expect_err("negative cardinality must fail");
    let negative = negative.iter().next().expect("one diagnostic is emitted");
    assert_eq!(
        negative.diagnostic().category(),
        DiagnosticCategory::InvalidContract,
    );
    assert!(negative.primary().is_some());

    for card in ["{ min: 2, max: 1 }", "{ min: 0, max: 0 }", "0"] {
        let source = format!(
            "format: typebridge.schema/v2\nattributes:\n  name: {{ value: string }}\nentities:\n  person:\n    owns:\n      name: {{ card: {card} }}\n"
        );
        let diagnostics = normalize(&source).expect_err("invalid cardinality must fail");
        let diagnostic = diagnostics.iter().next().expect("one diagnostic is emitted");
        assert_eq!(diagnostic.diagnostic().code().as_str(), "invalid_cardinality");
        assert!(diagnostic.primary().is_some());
    }
}

#[test]
fn key_remains_distinct_from_coexisting_unique_and_card() {
    let key = declared(
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
"#,
    );
    let unique_card = declared(
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { unique: true, card: 1 }
"#,
    );

    assert_eq!(
        ownership_annotation_kinds(&key),
        BTreeSet::from([AnnotationKindId::Key]),
    );
    assert_eq!(
        ownership_annotation_kinds(&unique_card),
        BTreeSet::from([AnnotationKindId::Unique, AnnotationKindId::Card]),
    );
    assert_ne!(
        key.declared_identity_fingerprint(),
        unique_card.declared_identity_fingerprint(),
    );
    assert_ne!(
        semantic_schema_fingerprint(&key, &profile()).unwrap(),
        semantic_schema_fingerprint(&unique_card, &profile()).unwrap(),
    );

    let key = resolve(&key, &profile()).expect("key schema resolves");
    let unique_card = resolve(&unique_card, &profile()).expect("unique/card schema resolves");
    let person = TypeId::new(TypeKind::Entity, "person").expect("type identifier is valid");
    let key = key.types()[&person].owns().values().next().unwrap();
    let unique_card = unique_card.types()[&person].owns().values().next().unwrap();
    assert!(key.is_key());
    assert!(!unique_card.is_key());
    assert!(unique_card.is_unique());
    assert_eq!(
        unique_card.cardinality(),
        Cardinality::new(1, Some(1)).unwrap(),
    );
}
