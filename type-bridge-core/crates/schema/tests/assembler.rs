use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    DocumentId, PlaysFactId, RelatesFactId, SchemaFact, SchemaFactId, SourceSpan, SubFact,
    SubFactId, TypeFact,
};
use type_bridge_schema::{FactAssembler, SchemaDocumentSet, normalize_documents};

fn span(document: &str, start: u64) -> SourceSpan {
    SourceSpan::new(
        DocumentId::new(document).expect("document ID is valid"),
        start,
        start + 1,
        1,
        u32::try_from(start + 1).expect("small test column"),
        1,
        u32::try_from(start + 2).expect("small test column"),
    )
    .expect("source span is valid")
}

fn type_fact(kind: TypeKind, label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(kind, label).expect("type ID is valid"))
            .expect("type fact is valid"),
    )
}

#[test]
fn duplicate_fact_reports_both_sources() {
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    assembler
        .insert_fact(type_fact(TypeKind::Entity, "person"), span("first", 0))
        .expect("first fact is accepted");
    let error = assembler
        .insert_fact(type_fact(TypeKind::Entity, "person"), span("second", 1))
        .expect_err("duplicate fact is rejected");
    let diagnostic = error.iter().next().expect("one diagnostic exists");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "duplicate_schema_fact"
    );
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn type_and_struct_labels_share_one_global_namespace() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
entities:
  shared-label: {}
structs:
  shared-label:
    fields:
      - { name: count, type: integer }
"#,
    )])
    .expect("fixture YAML parses");
    let error = normalize_documents(&documents)
        .expect_err("a type and struct cannot share one schema label");
    let diagnostic = error.iter().next().expect("one diagnostic exists");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "duplicate_schema_label"
    );
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn forward_plays_resolves_after_type_and_role_declarations() {
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    assembler.insert_plays(
        Label::new("person").unwrap(),
        Label::new("membership").unwrap(),
        Label::new("member").unwrap(),
        span("plays", 0),
    );
    assembler
        .insert_fact(type_fact(TypeKind::Entity, "person"), span("types", 1))
        .unwrap();
    assembler
        .insert_fact(
            type_fact(TypeKind::Relation, "membership"),
            span("types", 2),
        )
        .unwrap();

    let relation = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let role = RoleId::new("membership", "member").unwrap();
    assembler
        .insert_relates(
            RelatesFactId::new(relation, role.clone()).unwrap(),
            None,
            span("relates", 3),
        )
        .unwrap();

    let schema = assembler.finish().expect("forward references resolve");
    let player = TypeId::new(TypeKind::Entity, "person").unwrap();
    let id = SchemaFactId::Plays(PlaysFactId::new(player, role).unwrap());
    assert!(matches!(schema.fact(&id), Some(SchemaFact::Plays(_))));
}

#[test]
fn role_specialization_resolves_the_declaring_ancestor() {
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    let parent = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let child = TypeId::new(TypeKind::Relation, "employment").unwrap();
    assembler
        .insert_fact(
            type_fact(TypeKind::Relation, "membership"),
            span("types", 0),
        )
        .unwrap();
    assembler
        .insert_fact(
            type_fact(TypeKind::Relation, "employment"),
            span("types", 1),
        )
        .unwrap();
    assembler
        .insert_fact(
            SchemaFact::Sub(SubFact::new(
                SubFactId::new(child.clone(), parent.clone()).unwrap(),
            )),
            span("sub", 2),
        )
        .unwrap();

    let parent_role = RoleId::new("membership", "member").unwrap();
    assembler
        .insert_relates(
            RelatesFactId::new(parent, parent_role.clone()).unwrap(),
            None,
            span("parent-role", 3),
        )
        .unwrap();
    let child_role = RoleId::new("employment", "employee").unwrap();
    let child_id = RelatesFactId::new(child, child_role).unwrap();
    assembler
        .insert_relates(
            child_id.clone(),
            Some((Label::new("member").unwrap(), span("as", 4))),
            span("child-role", 5),
        )
        .unwrap();

    let schema = assembler.finish().expect("specialization resolves");
    let Some(SchemaFact::Relates(fact)) = schema.fact(&SchemaFactId::Relates(child_id)) else {
        panic!("child relates fact exists");
    };
    assert_eq!(fact.specializes(), Some(&parent_role));
}

#[test]
fn required_capabilities_are_retained_and_duplicates_report_sources() {
    let capability = CapabilityId::new("schema.future-feature").unwrap();
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    assembler
        .require_capability(capability.clone(), span("first", 0))
        .unwrap();
    let error = assembler
        .require_capability(capability.clone(), span("second", 1))
        .expect_err("duplicate capability is rejected");
    assert_eq!(error.iter().next().unwrap().related().len(), 1);

    let mut assembler = FactAssembler::new(FormatVersion::V1);
    assembler
        .require_capability(capability.clone(), span("only", 0))
        .unwrap();
    let schema = assembler.finish().expect("capability-only schema is valid");
    assert!(schema.required_capabilities().contains(&capability));
}
