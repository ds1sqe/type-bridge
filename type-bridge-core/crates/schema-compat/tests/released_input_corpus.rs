//! Adversarial released-generator-input corpus.
//!
//! Every schema here is a shape the released 1.5.x generator accepted;
//! the compatibility front-end must keep accepting each one — never
//! panicking, never inventing unsupported constructs, and recording a
//! deliberate open-world marker only for genuinely unportable syntax.

use serde_json::Value;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema_compat::{generated_declared_descriptors_json, typeql_to_declared};

fn descriptors(source: &str) -> Value {
    let json = generated_declared_descriptors_json(source)
        .unwrap_or_else(|error| panic!("released input must adapt: {error}"));
    serde_json::from_str(&json).expect("descriptor JSON parses")
}

fn closed_world(set: &Value) -> bool {
    set["closed_world"].as_bool().expect("closed_world is a bool")
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
fn explicit_reopened_declarations_merge() {
    let set = descriptors(
        "define\nattribute name, value string;\nentity person;\n\
         entity person, owns name;\n",
    );
    assert_eq!(section_len(&set, "entities"), 1);
    assert!(closed_world(&set));
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
    let diagnostics =
        typeql_to_declared(document, "define\nentity person;\nrelation person;\n")
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
    assert_eq!(unsupported(&set), vec!["@cascade".to_string(), "@subkey(primary)".to_string()]);
    assert_eq!(section_len(&set, "entities"), 2);
}

#[test]
fn list_markers_interleaved_with_annotations_strip_and_record() {
    let set = descriptors(
        "define\nattribute tag, value string;\n\
         entity person, owns tag[] @card(0..5) @distinct;\n",
    );
    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec!["tag[]".to_string(), "@distinct".to_string()]
    );
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
