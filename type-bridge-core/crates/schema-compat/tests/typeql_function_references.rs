use type_bridge_contract::{
    id::{FunctionId, Label},
    schema::DocumentId,
};
use type_bridge_schema_compat::{
    SchemaReference, typeql_to_declared, typeql_to_declared_with_references,
};

fn document() -> DocumentId {
    DocumentId::new("function-references.tql").expect("valid fixture document id")
}

#[test]
fn collects_static_references_without_changing_declared_schema() {
    let source = include_str!("fixtures/cross_form/schema.tql");
    let enriched = typeql_to_declared_with_references(document(), source).expect("valid TypeQL");
    let legacy = typeql_to_declared(document(), source).expect("valid TypeQL");

    assert_eq!(enriched.declared(), &legacy);

    let function = FunctionId::new("people").expect("valid function id");
    let body = enriched
        .function_body_references()
        .get(&function)
        .expect("people function reference index");
    assert!(body.references().contains(&SchemaReference::Label(
        Label::new("person").expect("valid label")
    )));
    assert!(!body.has_dynamic_type_reference());
}

#[test]
fn marks_variable_type_references_as_dynamic() {
    let source = "define\nentity person;\nfun people($candidate: person) -> { person }: match $candidate isa $kind; return { $candidate };\n";
    let enriched = typeql_to_declared_with_references(document(), source).expect("valid TypeQL");
    let function = FunctionId::new("people").expect("valid function id");

    assert!(enriched.function_body_references()[&function].has_dynamic_type_reference());
}
