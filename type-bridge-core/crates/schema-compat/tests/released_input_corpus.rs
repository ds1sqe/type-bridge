//! Adversarial input coverage for the retained source-projection adapters.

#![allow(deprecated)] // Frozen schema readers are removed with their remaining callers.
use serde_json::Value;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema_compat::{
    generated_declared_descriptors_json, released_typeql_to_declared_lossless_projection,
    released_typeql_to_declared_projection, typeql_to_declared,
};

fn descriptors(source: &str) -> Value {
    let json = generated_declared_descriptors_json(source)
        .unwrap_or_else(|error| panic!("released input must adapt: {error}"));
    serde_json::from_str(&json).expect("descriptor JSON parses")
}

fn closed_world(set: &Value) -> bool {
    set["closed_world"]
        .as_bool()
        .expect("closed_world is a bool")
}

fn unsupported(set: &Value) -> Vec<String> {
    set["unsupported_constructs"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().expect("construct is a string").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn section_len(set: &Value, section: &str) -> usize {
    set[section].as_array().map_or(0, Vec::len)
}

/// Drop `source` provenance objects so orderings can be compared on
/// declared identity alone: which declaration owns a span legitimately
/// depends on declaration order.
fn without_provenance(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "source")
                .map(|(key, item)| (key.clone(), without_provenance(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_provenance).collect()),
        other => other.clone(),
    }
}

#[test]
fn unresolved_reference_projection_is_one_indexed_pass() {
    const REFERENCE_COUNT: usize = 512;
    let mut source = String::from("define\n");
    for index in 0..REFERENCE_COUNT {
        source.push_str(&format!(
            "entity partial-{index}, owns missing-attribute-{index};\n"
        ));
    }

    let set = descriptors(&source);
    assert!(!closed_world(&set));
    assert_eq!(unsupported(&set).len(), REFERENCE_COUNT);
    assert_eq!(section_len(&set, "entities"), REFERENCE_COUNT);
    assert!(
        set["entities"]
            .as_array()
            .expect("entity descriptors")
            .iter()
            .all(|entity| entity["owns"].as_array().is_some_and(Vec::is_empty))
    );
}

#[test]
fn unresolved_reference_omission_is_generator_only() {
    let fixtures = [
        "define entity child, sub missing-parent;",
        "define entity child, owns missing-attribute;",
        "define entity child, plays missing-relation:member;",
        "define relation base, relates existing; entity child, plays base:missing-role;",
        "define relation base, relates existing; \
         relation child, sub base, relates replacement as absent;",
    ];

    for source in fixtures {
        generated_declared_descriptors_json(source)
            .unwrap_or_else(|error| panic!("released generator remains operational: {error}"));
        let document = DocumentId::new("schema/strict-v2.tql").expect("valid document");
        released_typeql_to_declared_projection(document.clone(), source)
            .expect_err("portable compatibility projection remains closed over references");
        released_typeql_to_declared_lossless_projection(document, source)
            .expect_err("lossless journal projection remains closed over references");
    }
}

#[test]
fn all_open_world_evidence_is_ordered_by_original_source_offset() {
    let set = descriptors(
        "define\n\
         attribute tag, value string;\n\
         entity child, sub missing-parent, owns tag[] @distinct,\
           plays missing-relation:member;\n",
    );
    assert_eq!(
        unsupported(&set),
        vec![
            "sub missing-parent".to_owned(),
            "plays missing-relation:member".to_owned(),
        ]
    );
    assert_eq!(set["format"], "typebridge.generated-descriptors/v2");
    assert_eq!(
        set["entities"][0]["owns"][0]["collection_mode"],
        "ordered_list"
    );
    assert_eq!(set["entities"][0]["owns"][0]["distinct"], true);
}

#[test]
fn unicode_comment_generates_without_panic() {
    let set = descriptors("define\n# café — résumé ✓\nentity person;\n");
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    assert_eq!(section_len(&set, "entities"), 1);
}

#[test]
fn unicode_in_string_literals_generates_without_panic() {
    let set = descriptors(
        "define\nattribute name, value string @regex(\"café .* ✓\");\n\
         entity person, owns name;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
}

#[test]
fn released_separators_remain_optional_across_comments_and_crlf() {
    let no_commas = descriptors(
        "define\r\nattribute name value string @regex(\"x\");\r\n\
         entity person @abstract owns name;\r\n",
    );
    assert_eq!(no_commas["attributes"][0]["regex"], "x");
    assert_eq!(no_commas["entities"][0]["is_abstract"], true);
    assert_eq!(no_commas["entities"][0]["owns"][0]["attribute"], "name");

    let bare_lf =
        descriptors("define\nattribute name value string;\nentity person @abstract\nowns name;\n");
    assert_eq!(bare_lf["entities"][0]["is_abstract"], true);
    assert_eq!(bare_lf["entities"][0]["owns"][0]["attribute"], "name");

    let comments = descriptors(
        "define\r\nattribute email, value string, # comma ; @abstract in comment\r\n\
         @regex(\"literal,;@abstract\");\r\n",
    );
    assert_eq!(comments["attributes"][0]["regex"], "literal,;@abstract");
}

#[test]
fn attribute_annotations_survive_earlier_value_clauses() {
    let set = descriptors(
        "define\n\
         attribute score, value string @regex(\"x\") @values(\"a\", \"b\"), value string;\n\
         attribute bounded, value integer @range(1..), value integer @range(..5);\n",
    );
    let score = &set["attributes"][1];
    assert_eq!(score["label"], "score");
    assert_eq!(score["value_type"], "string");
    assert_eq!(score["regex"], "x");
    assert_eq!(score["values"][0]["value"], "a");
    assert_eq!(score["values"][1]["value"], "b");

    let bounded = &set["attributes"][0];
    assert_eq!(bounded["label"], "bounded");
    assert_eq!(bounded["range"]["min"]["value"], "1");
    assert_eq!(bounded["range"]["max"]["value"], "5");
}

#[test]
fn annotation_projection_recovery_is_monotonic_and_ignores_markers_in_trivia() {
    let source = "define\n\
        attribute note, value string @doc(\"literal @key @card(0)\");\n\
        attribute score, value integer @range(5..2);\n\
        relation event, relates participant @card(0);\n\
        entity sample, owns note @key /* @unique in comment */ @card(1..1);\n";

    let set = descriptors(source);
    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec![
            "@range(5..2)".to_owned(),
            "@card(0)".to_owned(),
            "@key".to_owned(),
        ]
    );
    assert_eq!(set["attributes"][0]["doc"], "literal @key @card(0)");

    // This compatibility recovery is not part of either strict projection.
    let document = DocumentId::new("schema/strict-v2.tql").expect("valid document");
    released_typeql_to_declared_lossless_projection(document.clone(), source)
        .expect_err("lossless projection retains canonical annotation validation");
    typeql_to_declared(document, source)
        .expect_err("strict V2 import retains canonical annotation validation");
}

#[test]
fn comma_terminates_object_capability_annotation_ownership() {
    let set = descriptors(
        "define\n\
         attribute name, value string;\n\
         entity person, owns name @doc(\"own\"), @doc(\"entity\");\n\
         relation interaction, relates member @abstract, @abstract;\n",
    );
    assert_eq!(set["entities"][0]["doc"], "entity");
    assert_eq!(set["entities"][0]["owns"][0]["doc"], "own");
    assert_eq!(set["relations"][0]["is_abstract"], true);
    assert_eq!(set["relations"][0]["relates"][0]["is_abstract"], true);
}

#[test]
fn standalone_plays_prefers_an_explicit_relation_and_rejects_attribute_conflicts() {
    let relation = descriptors(
        "define\nfriendship plays friendship:friend;\n\
         relation friendship, relates friend;\n",
    );
    assert_eq!(section_len(&relation, "entities"), 0);
    assert_eq!(relation["relations"][0]["label"], "friendship");
    assert_eq!(relation["plays"][0]["player"], "friendship");

    let document = DocumentId::new("schema/main.tql").expect("valid document");
    let source = "define\nattribute ghost, value string;\n\
                  relation friendship, relates friend;\n\
                  ghost plays friendship:friend;\n";
    let diagnostics =
        type_bridge_schema_compat::released_typeql_to_declared_projection(document, source)
            .expect_err("an attribute cannot also be the released plays-only shell entity");
    assert_eq!(
        diagnostics
            .iter()
            .next()
            .expect("diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "conflicting_typeql_kind"
    );
}

#[test]
fn standalone_plays_annotations_remain_capability_annotations() {
    let set = descriptors(
        "define\n\
         relation friendship, relates friend;\n\
         person plays friendship:friend @card(0..3) @doc(\"edge\") @meta(\"source\", \"shell\");\n\
         relation club, relates member;\n\
         club plays club:member @card(1..2) @doc(\"relation edge\") @meta(\"source\", \"relation\");\n",
    );

    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    let plays = set["plays"].as_array().expect("plays descriptors");
    let shell = plays
        .iter()
        .find(|plays| plays["player"] == "person")
        .expect("kindless shell plays descriptor");
    assert_eq!(shell["card"]["min"], "0");
    assert_eq!(shell["card"]["max"], "3");
    assert_eq!(shell["doc"], "edge");
    assert_eq!(shell["meta"]["source"], "shell");

    let relation = plays
        .iter()
        .find(|plays| plays["player"] == "club")
        .expect("relation reopening plays descriptor");
    assert_eq!(relation["card"]["min"], "1");
    assert_eq!(relation["card"]["max"], "2");
    assert_eq!(relation["doc"], "relation edge");
    assert_eq!(relation["meta"]["source"], "relation");
}

#[test]
fn explicit_reopened_declarations_merge() {
    let set = descriptors(
        "define\nattribute name, value string;\nentity person;\n\
         entity person, owns name;\n",
    );
    assert_eq!(section_len(&set, "entities"), 1);
    assert!(closed_world(&set));
}

#[test]
fn reopened_facts_and_annotations_follow_the_frozen_merge_algebra() {
    let set = descriptors(
        "define\n\
         attribute name, value string;\n\
         entity base; entity alternate;\n\
         relation interaction, relates participant @card(0..2) @doc(\"first role\");\n\
         entity person @doc(\"first type\") @meta(\"source\", \"first\"), sub base,\n\
           owns name @card(0..1) @doc(\"first own\"),\n\
           plays interaction:participant @card(0..3) @doc(\"first play\");\n\
         define\n\
         relation interaction, relates participant @card(1..1) @doc(\"ignored role\");\n\
         entity person @doc(\"last type\") @meta(\"source\", \"last\"), sub alternate,\n\
           owns name @key @doc(\"ignored own\"),\n\
           plays interaction:participant @card(1..1) @doc(\"ignored play\");\n",
    );

    let person = set["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|entity| entity["label"] == "person")
        .expect("person descriptor");
    assert_eq!(person["parent"], "alternate", "the final sub wins");
    assert_eq!(person["doc"], "last type", "the final type doc wins");
    assert_eq!(person["meta"]["source"], "last");
    assert_eq!(person["owns"].as_array().expect("owns").len(), 1);
    assert_eq!(person["owns"][0]["doc"], "first own");
    assert_eq!(person["owns"][0]["key"], false);
    assert_eq!(person["owns"][0]["card"]["min"], "0");

    let plays = set["plays"]
        .as_array()
        .expect("plays")
        .iter()
        .find(|plays| plays["player"] == "person")
        .expect("person play");
    assert_eq!(plays["doc"], "first play");
    assert_eq!(plays["card"]["max"], "3");

    let interaction = set["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .find(|relation| relation["label"] == "interaction")
        .expect("interaction descriptor");
    assert_eq!(interaction["relates"].as_array().expect("relates").len(), 1);
    assert_eq!(interaction["relates"][0]["doc"], "first role");
    assert_eq!(interaction["relates"][0]["card"]["max"], "2");
}

#[test]
fn repeated_facts_inside_one_declaration_follow_released_precedence() {
    let set = descriptors(
        "define\n\
         attribute name, value string;\n\
         attribute score, value string, value integer @range(1..) @range(..5);\n\
         relation interaction,\n\
           relates participant @card(0..2) @doc(\"first role\"),\n\
           relates participant @card(1..1) @doc(\"ignored role\");\n\
         entity person,\n\
           owns name @card(0..1) @doc(\"first own\"),\n\
           owns name @key @doc(\"ignored own\"),\n\
           plays interaction:participant @card(0..3) @doc(\"first play\"),\n\
           plays interaction:participant @card(1..1) @doc(\"ignored play\");\n",
    );

    let person = set["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|entity| entity["label"] == "person")
        .expect("person descriptor");
    assert_eq!(person["owns"].as_array().expect("owns").len(), 1);
    assert_eq!(person["owns"][0]["doc"], "first own");
    assert_eq!(person["owns"][0]["key"], false);
    assert_eq!(person["owns"][0]["card"]["min"], "0");

    let plays = set["plays"]
        .as_array()
        .expect("plays")
        .iter()
        .find(|plays| plays["player"] == "person")
        .expect("person play");
    assert_eq!(plays["doc"], "first play");
    assert_eq!(plays["card"]["max"], "3");

    let interaction = set["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .find(|relation| relation["label"] == "interaction")
        .expect("interaction descriptor");
    assert_eq!(interaction["relates"].as_array().expect("relates").len(), 1);
    assert_eq!(interaction["relates"][0]["doc"], "first role");
    assert_eq!(interaction["relates"][0]["card"]["max"], "2");

    let score = set["attributes"]
        .as_array()
        .expect("attributes")
        .iter()
        .find(|attribute| attribute["label"] == "score")
        .expect("score descriptor");
    assert_eq!(score["value_type"], "long", "the final value clause wins");
    assert_eq!(score["range"]["min"]["value"], "1");
    assert_eq!(score["range"]["max"]["value"], "5");
}

#[test]
fn kindless_reopening_is_order_independent() {
    let after = "define\nrelation friendship, relates friend;\n\
                 entity person;\nperson plays friendship:friend;\n";
    let before = "define\nrelation friendship, relates friend;\n\
                  person plays friendship:friend;\nentity person;\n";
    let after = descriptors(after);
    let before = descriptors(before);
    assert_eq!(section_len(&after, "plays"), 1);
    assert_eq!(section_len(&before, "plays"), 1);
    assert_eq!(
        without_provenance(&after["entities"]),
        without_provenance(&before["entities"])
    );
    assert_eq!(
        without_provenance(&after["plays"]),
        without_provenance(&before["plays"])
    );
}

#[test]
fn conflicting_kinds_still_fail_closed() {
    let document = DocumentId::new("schema/main.tql").expect("valid test document");
    let diagnostics = typeql_to_declared(document, "define\nentity person;\nrelation person;\n")
        .expect_err("kind conflict must fail");
    let diagnostic = diagnostics.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "conflicting_typeql_kind"
    );
}

#[test]
fn cascade_and_subkey_are_recorded_not_fatal() {
    let set = descriptors(
        "define\nattribute name, value string;\n\
         entity person, owns name @cascade;\n\
         entity company, owns name @subkey(primary);\n",
    );
    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec!["@cascade".to_string(), "@subkey(primary)".to_string()]
    );
    assert_eq!(section_len(&set, "entities"), 2);
}

#[test]
fn subkey_redaction_consumes_released_trivia_and_comment_grammar() {
    for annotation in [
        "@subkey (primary)",
        "@subkey # comment contains )\r\n ( primary )",
        "@subkey // comment contains ( )\r\n(primary)",
        "@subkey /* comment contains ) */ (primary /* and ( */ )",
    ] {
        let source = format!(
            "define\nattribute name, value string;\nentity person, owns name {annotation} @key;\n"
        );
        let set = descriptors(&source);
        assert!(!closed_world(&set), "fixture: {annotation:?}");
        assert_eq!(
            unsupported(&set),
            vec![annotation.to_owned()],
            "fixture: {annotation:?}"
        );
        assert_eq!(set["entities"][0]["owns"][0]["key"], true);
    }
}

#[test]
fn list_markers_and_distinct_select_the_closed_v2_descriptor() {
    let set = descriptors(
        "define\nattribute tag, value string;\n\
         entity person, owns tag[] @card(0..5) @distinct;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    assert_eq!(set["format"], "typebridge.generated-descriptors/v2");
    assert_eq!(
        set["entities"][0]["owns"][0]["collection_mode"],
        "ordered_list"
    );
    assert_eq!(set["entities"][0]["owns"][0]["distinct"], true);
}

#[test]
fn comments_mentioning_stripped_syntax_stay_closed_world() {
    let set = descriptors(
        "define\n# mention thing[] and @distinct and @cascade and fun f()\n\
         entity person;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
}

#[test]
fn slash_and_block_comments_mentioning_stripped_syntax_stay_closed_world() {
    let set = descriptors(
        "define\n// mention @subkey (fake) and @cascade\n\
         /* mention thing[] @distinct @subkey(other) */\nentity person;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
}

#[test]
fn string_literals_mentioning_stripped_syntax_stay_intact() {
    let set = descriptors(
        "define\nattribute note, value string @regex(\"thing[] @distinct @cascade\");\n\
         entity person, owns note;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
}

#[test]
fn function_text_inside_string_is_not_stripped() {
    let set = descriptors(
        "define\nattribute note, value string \
         @regex(\"prefix fun f() -> integer: return 1; suffix\");\n\
         entity person, owns note;\n",
    );
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    assert_eq!(section_len(&set, "attributes"), 1);
}

#[test]
fn released_function_definitions_still_strip() {
    let set = descriptors(
        "define\nentity person;\n\
         fun answer() -> integer:\n  match $p isa person;\n  return count($p);\n",
    );
    assert!(closed_world(&set));
    assert_eq!(section_len(&set, "entities"), 1);
}

#[test]
fn provenance_offsets_index_the_original_document() {
    // Stripping is length-preserving, so a declaration that follows a
    // stripped function still reports its span in the caller's source.
    let source = "define\nfun answer() -> integer:\n  match $p isa person;\n  \
                  return count($p);\nentity person;\n";
    let set = descriptors(source);
    let span = &set["entities"][0]["source"];
    let start = span["byte_start"].as_u64().expect("byte_start") as usize;
    let end = span["byte_end"].as_u64().expect("byte_end") as usize;
    assert_eq!(&source[start..end], "entity person");
}

#[test]
fn repeated_define_normalization_keeps_global_original_offsets() {
    let source = "define\nentity alpha;\ndefine\nentity omega, @abstract;\n";
    let set = descriptors(source);
    let omega = set["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|entity| entity["label"] == "omega")
        .expect("omega entity");
    let span = &omega["source"];
    let start = span["byte_start"].as_u64().expect("byte_start") as usize;
    let end = span["byte_end"].as_u64().expect("byte_end") as usize;
    assert_eq!(&source[start..end], "entity omega, @abstract");
    assert_eq!(span["line"], 4);
}
