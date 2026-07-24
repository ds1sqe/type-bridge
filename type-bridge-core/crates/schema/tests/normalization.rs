use std::collections::BTreeSet;

use type_bridge_contract::id::{FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::schema::DocumentId;
use type_bridge_contract::schema::{AnnotationSubjectId, RelatesFactId, SchemaFact, SchemaFactId};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents};

fn documents(source: &str) -> SchemaDocumentSet {
    SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("fixture document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses")
}

#[test]
fn normalizes_types_interfaces_annotations_and_relation_players() {
    let source = r#"format: typebridge.schema/v2
attributes:
  name:
    value:
      type: string
      regex: "^.+$"
entities:
  person:
    abstract: true
    owns:
      - name:
          card: { min: 1, max: 1 }
          doc: "Primary name"
relations:
  friendship:
    relates:
      friend:
        card: 2
plays:
  person:
    friendship:
      - friend:
          card: { min: 0, max: 1 }
"#;

    let declared = normalize_documents(&documents(source)).expect("schema normalizes");
    let person = TypeId::new(TypeKind::Entity, "person").expect("type identifier is valid");
    assert!(declared.fact(&SchemaFactId::Type(person)).is_some());
    assert_eq!(declared.facts().len(), 13);
}

#[test]
fn removing_independent_annotations_preserves_every_structural_subject() {
    let annotated = r#"format: typebridge.schema/v2
attributes:
  name:
    value:
      type: string
      regex: "^.+$"
entities:
  person:
    abstract: true
    owns:
      name:
        doc: "Primary name"
relations:
  friendship:
    relates:
      friend:
        card: { min: 0, max: 1 }
plays:
  person:
    friendship:
      friend:
        meta: { edge: "person-friend" }
"#;
    let plain = r#"format: typebridge.schema/v2
attributes:
  name:
    value: string
entities:
  person:
    owns: [name]
relations:
  friendship:
    relates: [friend]
plays:
  person:
    friendship: [friend]
"#;

    let annotated =
        normalize_documents(&documents(annotated)).expect("annotated schema normalizes");
    let plain = normalize_documents(&documents(plain)).expect("plain schema normalizes");
    let annotated_structural = annotated
        .facts()
        .filter(|fact| !matches!(fact, SchemaFact::Annotation(_)))
        .cloned()
        .collect::<Vec<_>>();
    let plain_structural = plain
        .facts()
        .filter(|fact| !matches!(fact, SchemaFact::Annotation(_)))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(annotated_structural, plain_structural);

    let subjects = annotated
        .facts()
        .filter_map(|fact| {
            let SchemaFact::Annotation(annotation) = fact else {
                return None;
            };
            Some(match annotation.id().subject() {
                AnnotationSubjectId::Type(_) => "type",
                AnnotationSubjectId::Value(_) => "value",
                AnnotationSubjectId::Owns(_) => "owns",
                AnnotationSubjectId::Relates(_) => "relates",
                AnnotationSubjectId::Plays(_) => "plays",
                AnnotationSubjectId::Sub(_) => "sub",
                AnnotationSubjectId::Function(_) => "function",
            })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        subjects,
        BTreeSet::from(["type", "value", "owns", "relates", "plays"])
    );
    assert_ne!(
        annotated.declared_identity_fingerprint(),
        plain.declared_identity_fingerprint()
    );
}

#[test]
fn compact_and_expanded_facts_have_equal_declared_identity() {
    let compact = r#"format: typebridge.schema/v2
attributes:
  name:
    value: string
entities:
  person:
    owns: [name]
"#;
    let expanded = r#"format: typebridge.schema/v2
attributes:
  name:
    value: { type: string }
entities:
  person:
    owns:
      name: {}
"#;

    let compact = normalize_documents(&documents(compact)).expect("compact schema normalizes");
    let expanded = normalize_documents(&documents(expanded)).expect("expanded schema normalizes");
    assert_eq!(
        compact.declared_identity_fingerprint(),
        expanded.declared_identity_fingerprint()
    );
}

#[test]
fn required_capability_order_is_non_semantic() {
    let first = r#"format: typebridge.schema/v2
capabilities:
  required: [schema.roles, schema.annotations]
"#;
    let second = r#"format: typebridge.schema/v2
capabilities:
  required: [schema.annotations, schema.roles]
"#;

    let first = normalize_documents(&documents(first)).expect("first schema normalizes");
    let second = normalize_documents(&documents(second)).expect("second schema normalizes");
    assert_eq!(
        first.declared_identity_fingerprint(),
        second.declared_identity_fingerprint()
    );
}

#[test]
fn unknown_core_keys_fail_closed() {
    let source = r#"format: typebridge.schema/v2
entities:
  person:
    typo: true
"#;
    assert!(normalize_documents(&documents(source)).is_err());
}

#[test]
fn fragment_level_sources_are_rejected() {
    let source = r#"format: typebridge.schema/v2
sources: [fragments/*.yaml]
"#;
    let error = normalize_documents(&documents(source))
        .expect_err("source discovery belongs to the workspace manifest");
    let diagnostic = error.iter().next().unwrap();
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "unknown_schema_document_key",
    );
    let span = diagnostic
        .primary()
        .expect("unknown key retains its source span");
    let start = source.find("sources").unwrap();
    assert_eq!(span.document().as_str(), "schema.yaml");
    assert_eq!(
        (span.byte_start(), span.byte_end()),
        (start as u64, (start + 7) as u64)
    );
    assert_eq!((span.line(), span.column()), (2, 1));
    assert_eq!((span.end_line(), span.end_column()), (2, 8));
}

#[test]
fn scalar_and_expanded_sub_facts_have_equal_declared_identity() {
    let scalar = r#"format: typebridge.schema/v2
attributes:
  base-name: { value: string }
  display-name: { sub: base-name }
entities:
  actor: {}
  person: { sub: actor }
relations:
  association: {}
  friendship: { sub: association }
"#;
    let expanded = r#"format: typebridge.schema/v2
attributes:
  base-name: { value: string }
  display-name: { sub: { type: base-name } }
entities:
  actor: {}
  person: { sub: { type: actor } }
relations:
  association: {}
  friendship: { sub: { type: association } }
"#;

    let scalar = normalize_documents(&documents(scalar)).expect("scalar sub facts normalize");
    let expanded = normalize_documents(&documents(expanded)).expect("expanded sub facts normalize");
    assert_eq!(
        scalar.declared_identity_fingerprint(),
        expanded.declared_identity_fingerprint(),
    );
}

#[test]
fn expanded_sub_facts_attach_doc_and_meta_to_the_edge() {
    let source = r#"format: typebridge.schema/v2
attributes:
  base-name: { value: string }
  display-name:
    sub: { type: base-name, doc: "attribute edge", meta: { owner: "schema" } }
entities:
  actor: {}
  person:
    sub: { type: actor, doc: "entity edge", meta: { owner: "schema" } }
relations:
  association: {}
  friendship:
    sub: { type: association, doc: "relation edge", meta: { owner: "schema" } }
"#;
    let declared = normalize_documents(&documents(source)).expect("expanded sub facts normalize");
    let edge_annotations = declared
        .facts()
        .filter(|fact| {
            matches!(
                fact,
                SchemaFact::Annotation(annotation)
                    if matches!(annotation.id().subject(), AnnotationSubjectId::Sub(_))
            )
        })
        .count();
    assert_eq!(edge_annotations, 6);
}

#[test]
fn expanded_sub_shape_is_closed() {
    for (sub, code) in [
        ("[base]", "invalid_schema_sub_shape"),
        (
            "{ type: base, future: true }",
            "unknown_schema_document_key",
        ),
    ] {
        let source = format!(
            "format: typebridge.schema/v2\nentities:\n  base: {{}}\n  child:\n    sub: {sub}\n"
        );
        let error = normalize_documents(&documents(&source)).expect_err("shape must fail closed");
        assert_eq!(
            error.iter().next().unwrap().diagnostic().code().as_str(),
            code
        );
    }
}

#[test]
fn unknown_expanded_sub_key_reports_the_exact_key_span() {
    let source = r#"format: typebridge.schema/v2
entities:
  base: {}
  child:
    sub:
      type: base
      future: true
"#;
    let error = normalize_documents(&documents(source)).expect_err("unknown key must fail closed");
    let diagnostic = error.iter().next().unwrap();
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "unknown_schema_document_key",
    );
    let span = diagnostic
        .primary()
        .expect("unknown key retains its source span");
    let start = source.find("future").unwrap();
    assert_eq!(span.document().as_str(), "schema.yaml");
    assert_eq!(
        (span.byte_start(), span.byte_end()),
        (start as u64, (start + 6) as u64)
    );
    assert_eq!((span.line(), span.column()), (7, 7));
    assert_eq!((span.end_line(), span.end_column()), (7, 13));
}

#[test]
fn bare_null_named_fact_bodies_are_rejected() {
    let source = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name:
"#;
    let error = normalize_documents(&documents(source)).expect_err("null is not an empty body");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "invalid_named_schema_fact_body",
    );
}

#[test]
fn v1_extensions_are_requirement_only_and_never_ignore_payloads() {
    let supported = r#"format: typebridge.schema/v2
extensions:
  acme.schema.audit:
    required: true
"#;
    normalize_documents(&documents(supported)).expect("dotted requirement ID is supported");

    for key in ["payload", "data"] {
        let source = format!(
            "format: typebridge.schema/v2\nextensions:\n  acme.schema.audit:\n    required: true\n    {key}: {{ enabled: true }}\n"
        );
        let error = normalize_documents(&documents(&source))
            .expect_err("unimplemented extension bodies must fail closed");
        assert_eq!(
            error.iter().next().unwrap().diagnostic().code().as_str(),
            "unknown_schema_document_key",
        );
    }
}

#[test]
fn normalizes_ordered_builtin_struct_fields() {
    let source = r#"format: typebridge.schema/v2
structs:
  player-stats:
    fields:
      - name: wins
        type: integer
      - name: losses
        type: integer
      - name: nickname
        type: string
        optional: true
"#;

    let declared = normalize_documents(&documents(source)).expect("schema normalizes");
    let id =
        SchemaFactId::Struct(StructId::new("player-stats").expect("struct identifier is valid"));
    let Some(SchemaFact::Struct(fact)) = declared.fact(&id) else {
        panic!("struct fact exists");
    };

    assert_eq!(fact.fields().len(), 3);
    assert_eq!(fact.fields()[0].name().as_str(), "wins");
    assert_eq!(fact.fields()[1].name().as_str(), "losses");
    assert_eq!(fact.fields()[2].name().as_str(), "nickname");
    assert_eq!(fact.fields()[0].value_type(), ValueTypeTag::Long);
    assert!(!fact.fields()[0].optional());
    assert!(fact.fields()[2].optional());
}

#[test]
fn rejects_non_builtin_and_nested_struct_field_types() {
    let unknown = r#"format: typebridge.schema/v2
structs:
  invalid:
    fields:
      - name: value
        type: custom-value
"#;
    let error = normalize_documents(&documents(unknown)).unwrap_err();
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "unknown_schema_value_type",
    );

    let nested = r#"format: typebridge.schema/v2
structs:
  invalid:
    fields:
      - name: value
        type: { struct: another }
"#;
    let error = normalize_documents(&documents(nested)).unwrap_err();
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "schema_scalar_required",
    );
}

#[test]
fn duplicate_struct_fields_report_both_spans() {
    let source = r#"format: typebridge.schema/v2
structs:
  invalid:
    fields:
      - name: value
        type: string
      - name: value
        type: integer
"#;
    let error = normalize_documents(&documents(source)).unwrap_err();
    let diagnostic = error.iter().next().unwrap();
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "duplicate_struct_field",
    );
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn omitted_and_explicit_false_struct_optionality_normalize_identically() {
    let omitted = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: value
        type: string
"#;
    let explicit = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: value
        type: string
        optional: false
"#;

    let omitted = normalize_documents(&documents(omitted)).unwrap();
    let explicit = normalize_documents(&documents(explicit)).unwrap();
    assert_eq!(
        omitted.declared_identity_fingerprint(),
        explicit.declared_identity_fingerprint(),
    );
}

#[test]
fn root_plays_can_reference_types_in_a_later_fragment() {
    let set = SchemaDocumentSet::parse([
        (
            DocumentId::new("a-plays.yaml").expect("document identifier is valid"),
            "format: typebridge.schema/v2\nplays:\n  person:\n    friendship: [friend]\n",
        ),
        (
            DocumentId::new("z-types.yaml").expect("document identifier is valid"),
            "format: typebridge.schema/v2\nentities:\n  person: {}\nrelations:\n  friendship:\n    relates: [friend]\n",
        ),
    ])
    .expect("fragment set parses");
    assert!(normalize_documents(&set).is_ok());
}

#[test]
fn role_specialization_resolves_the_declaring_ancestor() {
    let source = r#"format: typebridge.schema/v2
relations:
  root:
    relates: [member]
  middle:
    sub: root
  leaf:
    sub: middle
    relates:
      participant: { as: member }
"#;
    let declared = normalize_documents(&documents(source)).expect("schema normalizes");
    let relation = TypeId::new(TypeKind::Relation, "leaf").expect("type identifier is valid");
    let role = RoleId::new("leaf", "participant").expect("role identifier is valid");
    let id = RelatesFactId::new(relation, role).expect("relates identifier is valid");
    let Some(SchemaFact::Relates(relates)) = declared.fact(&SchemaFactId::Relates(id)) else {
        panic!("specialized role fact exists");
    };
    assert_eq!(
        relates
            .specializes()
            .expect("role specializes an ancestor")
            .declaring_relation()
            .as_str(),
        "root"
    );
}

#[test]
fn function_structural_source_excludes_doc_and_meta() {
    let first = r#"format: typebridge.schema/v2
functions:
  answer:
    returns: { stream: [integer] }
    body: { typeql: "match let $x = 42; return first $x;" }
    doc: "first"
"#;
    let second = r#"format: typebridge.schema/v2
functions:
  answer:
    returns: { stream: [integer] }
    body: { typeql: "match let $x = 42; return first $x;" }
    doc: "second"
"#;
    let first = normalize_documents(&documents(first)).expect("first schema normalizes");
    let second = normalize_documents(&documents(second)).expect("second schema normalizes");
    let id =
        SchemaFactId::Function(FunctionId::new("answer").expect("function identifier is valid"));
    let Some(SchemaFact::Function(first_function)) = first.fact(&id) else {
        panic!("first function exists");
    };
    let Some(SchemaFact::Function(second_function)) = second.fact(&id) else {
        panic!("second function exists");
    };
    assert_eq!(first_function.signature(), second_function.signature());
    assert_eq!(first_function.body(), second_function.body());
    assert_eq!(
        first_function.body().text(),
        "match let $x = 42; return first $x;"
    );
    assert_ne!(
        first.declared_identity_fingerprint(),
        second.declared_identity_fingerprint()
    );
}

#[test]
fn canonical_string_limits_apply_to_yaml_values_and_metadata() {
    fn value_source(payload: &str) -> String {
        format!(
            "format: typebridge.schema/v2\nattributes:\n  note:\n    value:\n      type: string\n      values:\n        - \"{payload}\"\n"
        )
    }

    fn meta_source(payload: &str) -> String {
        format!(
            "format: typebridge.schema/v2\nentities:\n  person:\n    meta:\n      note: \"{payload}\"\n"
        )
    }

    let escaped_boundary = "\\u0078".repeat(MAX_CANONICAL_STRING_BYTES);
    assert!(escaped_boundary.len() > MAX_CANONICAL_STRING_BYTES);
    normalize_documents(&documents(&value_source(&escaped_boundary)))
        .expect("an escaped boundary-sized string value normalizes");

    let boundary = "x".repeat(MAX_CANONICAL_STRING_BYTES);
    normalize_documents(&documents(&meta_source(&boundary)))
        .expect("a boundary-sized metadata value normalizes");

    let oversized = "x".repeat(MAX_CANONICAL_STRING_BYTES + 1);
    for source in [value_source(&oversized), meta_source(&oversized)] {
        let diagnostics =
            normalize_documents(&documents(&source)).expect_err("oversized text must fail");
        let diagnostic = diagnostics
            .iter()
            .next()
            .expect("one diagnostic is emitted");
        assert_eq!(
            diagnostic.diagnostic().code().as_str(),
            "canonical_string_limit_exceeded"
        );
        assert_eq!(
            diagnostic
                .primary()
                .expect("the offending YAML scalar has provenance")
                .document()
                .as_str(),
            "schema.yaml"
        );
    }
}

#[test]
fn named_timezone_values_use_the_pinned_provider_policy() {
    let valid = r#"format: typebridge.schema/v2
attributes:
  observed:
    value:
      type: datetime-tz
      values: ["2024-07-01T12:00:00[europe/paris]"]
"#;
    normalize_documents(&documents(valid)).expect("ordinary named local datetime resolves");

    for (value, code) in [
        (
            "2024-03-31T01:30:00[Europe/London]",
            "nonexistent_named_timezone_local_datetime",
        ),
        (
            "2024-10-27T01:30:00[Europe/London]",
            "ambiguous_named_timezone_local_datetime",
        ),
    ] {
        let source = format!(
            "format: typebridge.schema/v2\nattributes:\n  observed:\n    value:\n      type: datetime-tz\n      values: [\"{value}\"]\n"
        );
        let diagnostics = normalize_documents(&documents(&source))
            .expect_err("provider-ambiguous local datetime must fail closed");
        assert_eq!(
            diagnostics
                .iter()
                .next()
                .unwrap()
                .diagnostic()
                .code()
                .as_str(),
            code,
        );
    }
}
