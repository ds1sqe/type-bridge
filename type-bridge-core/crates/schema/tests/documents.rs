use type_bridge_contract::id::FunctionId;
use type_bridge_contract::schema::{DocumentId, SchemaFact, SchemaFactId};
use type_bridge_schema::{
    SchemaDocument, SchemaDocumentSet, YamlNode, YamlScalarStyle, normalize_documents,
};

fn document_id(value: &str) -> DocumentId {
    DocumentId::new(value).expect("fixture document identifier is valid")
}

#[test]
fn retains_source_comments_and_block_scalar_style() {
    let source = "# schema heading\nformat: typebridge.schema/v2\nmessage: |\n  first line\n  second line\n";
    let document = SchemaDocument::parse(document_id("lossless.yaml"), source)
        .expect("lossless fixture parses");

    assert_eq!(document.source(), source);
    assert_eq!(document.comments().len(), 1);
    assert_eq!(document.comments()[0].text(), " schema heading");

    let message = document
        .root()
        .entries()
        .iter()
        .find(|entry| entry.key().value() == "message")
        .expect("message entry exists");
    let YamlNode::Scalar(message) = message.value() else {
        panic!("message is a scalar");
    };
    assert_eq!(message.style(), YamlScalarStyle::Literal);
    assert_eq!(message.value(), "first line\nsecond line\n");
}

#[test]
fn rejects_closed_grammar_violations() {
    let invalid = [
        ("duplicate.yaml", "format: one\nformat: two\n"),
        ("anchor.yaml", "format: &format typebridge.schema/v2\n"),
        ("alias.yaml", "format: *format\n"),
        ("tag.yaml", "format: !schema typebridge.schema/v2\n"),
        ("merge.yaml", "format: typebridge.schema/v2\ntarget:\n  <<:\n    doc: base\n"),
        ("directive.yaml", "%YAML 1.2\n---\nformat: typebridge.schema/v2\n"),
        ("documents.yaml", "format: one\n---\nformat: two\n"),
        ("key.yaml", "? [format]\n: typebridge.schema/v2\n"),
        ("yes.yaml", "format: yes\n"),
        ("on.yaml", "format: ON\n"),
        ("date.yaml", "format: 2026-07-15\n"),
        ("timestamp.yaml", "format: 2026-07-15T12:00:00Z\n"),
    ];

    for (name, source) in invalid {
        assert!(
            SchemaDocument::parse(document_id(name), source).is_err(),
            "{name} must be rejected"
        );
    }
}

#[test]
fn document_set_iteration_is_identifier_ordered() {
    let documents = SchemaDocumentSet::parse([
        (document_id("z.yaml"), "format: z"),
        (document_id("a.yaml"), "format: a"),
    ])
    .expect("document set parses");

    let ids = documents
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["a.yaml", "z.yaml"]);
}

#[test]
fn retained_source_is_the_byte_exact_function_reemission_witness() {
    let source = r#"# exact presentation survives
format: typebridge.schema/v2
functions:
  answer:
    returns: { stream: [integer] }
    body:
      typeql: |
        match let $answer = 42;
        # function-body comment
        return first $answer;
"#;
    let id = document_id("function.yaml");
    let documents = SchemaDocumentSet::parse([(id.clone(), source)])
        .expect("function document parses losslessly");
    assert_eq!(
        documents.get(&id).expect("document exists").source().as_bytes(),
        source.as_bytes()
    );

    let declared = normalize_documents(&documents).expect("function schema normalizes");
    let function_id = FunctionId::new("answer").expect("function identifier is valid");
    let Some(SchemaFact::Function(function)) =
        declared.fact(&SchemaFactId::Function(function_id))
    else {
        panic!("function fact exists");
    };
    assert_eq!(
        function.body().text(),
        "match let $answer = 42;\n# function-body comment\nreturn first $answer;\n"
    );
}
