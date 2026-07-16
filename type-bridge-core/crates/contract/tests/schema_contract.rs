use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::id::{AttributeId, Label, StructId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
    SourcedSchemaFact, StructFact, StructField, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::ValueTypeTag;

fn span(line: u32) -> SourceSpan {
    SourceSpan::new(
        DocumentId::new("schema/types.yaml").unwrap(),
        u64::from(line - 1),
        u64::from(line),
        line,
        1,
        line,
        2,
    )
    .unwrap()
}

fn overlap_facts() -> Vec<SourcedSchemaFact> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let age_type = TypeId::new(TypeKind::Attribute, "age").unwrap();
    let age = AttributeId::new("age").unwrap();
    vec![
        SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
            span(1),
        ),
        SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(age_type).unwrap()),
            span(2),
        ),
        SourcedSchemaFact::new(
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(age.clone()),
                ValueTypeTag::Long,
            )),
            span(3),
        ),
        SourcedSchemaFact::new(
            SchemaFact::Owns(OwnsFact::new(OwnsFactId::new(person, age).unwrap())),
            span(4),
        ),
    ]
}

#[test]
fn declared_identity_is_order_and_source_invariant() {
    let first = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        overlap_facts(),
    )
    .unwrap();
    let mut reordered = overlap_facts();
    reordered.reverse();
    let second = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        reordered,
    )
    .unwrap();
    assert_eq!(
        first.declared_identity_fingerprint(),
        second.declared_identity_fingerprint(),
    );
    assert_eq!(
        first.canonical_identity_bytes().unwrap(),
        second.canonical_identity_bytes().unwrap(),
    );
}

#[test]
fn duplicate_facts_report_both_sources() {
    let mut facts = overlap_facts();
    let duplicate = facts[0].fact().clone();
    facts.push(SourcedSchemaFact::new(duplicate, span(9)));
    let diagnostics = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts,
    )
    .unwrap_err();
    let diagnostic = diagnostics.iter().next().unwrap();
    assert_eq!(diagnostic.diagnostic().code().as_str(), "duplicate_schema_fact");
    assert_eq!(diagnostic.primary().unwrap().line(), 9);
    assert_eq!(diagnostic.related()[0].span().line(), 1);
}

#[test]
fn dangling_references_fail_before_provider_io() {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let missing = AttributeId::new("missing").unwrap();
    let facts = vec![
        SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
            span(1),
        ),
        SourcedSchemaFact::new(
            SchemaFact::Owns(OwnsFact::new(OwnsFactId::new(person, missing).unwrap())),
            span(2),
        ),
    ];
    let diagnostics = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts,
    )
    .unwrap_err();
    assert_eq!(
        diagnostics.iter().next().unwrap().diagnostic().code().as_str(),
        "unknown_schema_fact_reference",
    );
}

#[test]
fn struct_fact_preserves_order_and_canonical_bytes() {
    let fact = StructFact::new(
        StructId::new("player-stats").unwrap(),
        vec![
            StructField::new(
                Label::new("wins").unwrap(),
                ValueTypeTag::Long,
                false,
            ),
            StructField::new(
                Label::new("nickname").unwrap(),
                ValueTypeTag::String,
                true,
            ),
        ],
    )
    .unwrap();

    assert_eq!(fact.fields()[0].name().as_str(), "wins");
    assert_eq!(fact.fields()[1].name().as_str(), "nickname");
    assert_eq!(
        String::from_utf8(to_canonical_json(&fact).unwrap()).unwrap(),
        r#"{"fields":[{"name":"wins","optional":false,"value_type":"long"},{"name":"nickname","optional":true,"value_type":"string"}],"id":"player-stats"}"#,
    );
}

#[test]
fn struct_fact_rejects_empty_and_duplicate_fields() {
    let empty =
        StructFact::new(StructId::new("empty").unwrap(), Vec::new()).unwrap_err();
    assert_eq!(empty.code().as_str(), "empty_struct_fields");

    let field = StructField::new(
        Label::new("wins").unwrap(),
        ValueTypeTag::Long,
        false,
    );
    let duplicate = StructFact::new(
        StructId::new("duplicate").unwrap(),
        vec![field.clone(), field],
    )
    .unwrap_err();
    assert_eq!(duplicate.code().as_str(), "duplicate_struct_field");
}
