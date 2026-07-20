use type_bridge_contract::schema::{DocumentId, SchemaFact};
use type_bridge_schema_compat::{typeql_to_declared, typeql_to_facts};

fn document() -> DocumentId {
    DocumentId::new("schema/main.tql").expect("valid test document")
}

#[test]
fn typeql_structural_facts_share_one_assembler() {
    let source = r#"define
entity person, owns name, plays friendship:friend;
attribute name, value string;
relation friendship, relates friend;
"#;

    let declared = typeql_to_declared(document(), source).expect("TypeQL schema adapts");
    let facts = typeql_to_facts(document(), source).expect("facts adapt");

    assert_eq!(declared.facts().len(), facts.len());
    assert!(facts.iter().any(|fact| matches!(fact, SchemaFact::Owns(_))));
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, SchemaFact::Relates(_)))
    );
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, SchemaFact::Plays(_)))
    );
}

#[test]
fn typeql_reopened_type_declarations_merge_into_one_identity() {
    // Released renders split declarations of one label freely; every
    // compatible re-opening merges into the single type identity.
    let source = "define\nentity person;\nentity person;\n";
    let declared = typeql_to_declared(document(), source).expect("reopened label adapts");
    let types = declared
        .facts()
        .filter(|fact| matches!(fact, SchemaFact::Type(_)))
        .count();
    assert_eq!(types, 1);
}

#[test]
fn typeql_duplicate_fact_reports_both_source_locations() {
    // Genuine duplicates of a non-type fact still fail with both spans.
    let source = "define\nattribute name, value string;\n\
                  entity person, owns name, owns name;\n";
    let diagnostics = typeql_to_declared(document(), source).expect_err("duplicate must fail");
    let diagnostic = diagnostics.iter().next().expect("one diagnostic");

    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "duplicate_schema_fact"
    );
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn typeql_typed_annotations_are_exact_and_fail_closed() {
    let source = "define\nattribute score, value integer @values(1, 2);\n";
    let declared = typeql_to_declared(document(), source).expect("typed values adapt");
    assert!(
        declared
            .facts()
            .any(|fact| matches!(fact, SchemaFact::Annotation(_)))
    );

    let range = "define\nattribute score, value integer @range(1..10);\n";
    typeql_to_declared(document(), range).expect("typed range adapts");

    let mixed = "define\nattribute score, value integer @values(1, \"two\");\n";
    let diagnostics =
        typeql_to_declared(document(), mixed).expect_err("mixed domains must fail closed");
    let diagnostic = diagnostics.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "mixed_values_annotation_domain"
    );
    assert!(diagnostic.primary().is_some());

    let duplicate = "define\nattribute score, value integer @values(1, 1);\n";
    let diagnostics =
        typeql_to_declared(document(), duplicate).expect_err("duplicates must fail closed");
    assert_eq!(
        diagnostics
            .iter()
            .next()
            .expect("one diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "duplicate_values_annotation_value"
    );
}

#[test]
fn typeql_extended_date_domain_is_preserved() {
    let source = "define\nattribute observed, value date @values(-262143-01-01, 0000-01-01, +262142-12-31);\n";
    typeql_to_declared(document(), source).expect("extended provider dates adapt exactly");
}

#[test]
fn nonportable_v1_annotations_fail_at_the_adapter_boundary() {
    for (source, expected_code) in [
        (
            "define\nattribute name, value string;\nentity person, owns name @cascade;\n",
            "unsupported_typeql_annotation",
        ),
        (
            "define\nattribute name, value string;\nentity person, owns name[] @distinct;\n",
            "unsupported_typeql_owns",
        ),
        (
            "define\nattribute name, value string;\nentity person, owns name @subkey(primary);\n",
            "unsupported_typeql_annotation",
        ),
    ] {
        let diagnostics =
            typeql_to_declared(document(), source).expect_err("nonportable annotation must fail");
        assert_eq!(
            diagnostics
                .iter()
                .next()
                .expect("one diagnostic")
                .diagnostic()
                .code()
                .as_str(),
            expected_code,
            "fixture must parse and reject at the portability boundary: {source}"
        );
    }
}
