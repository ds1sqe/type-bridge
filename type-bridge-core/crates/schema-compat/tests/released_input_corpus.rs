//! Adversarial released-generator-input corpus.
//!
//! Every schema here is a shape the released 1.5.x generator accepted;
//! the compatibility front-end must keep accepting each one — never
//! panicking, never inventing unsupported constructs, and recording a
//! deliberate open-world marker only for genuinely unportable syntax.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use type_bridge_contract::schema::DocumentId;
use type_bridge_core_lib::bindgen::{BindgenOptions, GeneratedPackage, TargetLanguage};
use type_bridge_schema_compat::{
    GENERATED_DECLARED_DESCRIPTOR_PATH, MAX_TYPEQL_SCHEMA_BYTES,
    empty_generated_declared_descriptors_json, generate_package_with_declared_descriptors,
    generated_declared_descriptors_json, released_typeql_to_declared_lossless_projection,
    released_typeql_to_declared_projection, typeql_to_declared,
};

fn released_schema_with_trailing_comment(total_bytes: usize) -> String {
    const PREFIX: &str = "define\nentity person;\n#";
    assert!(total_bytes >= PREFIX.len());
    let mut source = String::with_capacity(total_bytes);
    source.push_str(PREFIX);
    source.push_str(&"x".repeat(total_bytes - PREFIX.len()));
    assert_eq!(source.len(), total_bytes);
    source
}

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

fn generated_python_package(source: &str) {
    let package = generate_package_with_declared_descriptors(
        source,
        TargetLanguage::Python,
        &BindgenOptions::default(),
    )
    .unwrap_or_else(|error| panic!("released package input must adapt: {error}"));
    assert!(
        package
            .files
            .iter()
            .any(|file| file.path == GENERATED_DECLARED_DESCRIPTOR_PATH),
        "generated package carries its declared descriptor snapshot"
    );
}

fn generated_package(source: &str, target: TargetLanguage) -> Value {
    serde_json::to_value(generated_package_files(source, target))
        .expect("generated package serializes")
}

fn generated_package_files(source: &str, target: TargetLanguage) -> GeneratedPackage {
    generate_package_with_declared_descriptors(source, target, &BindgenOptions::default())
        .unwrap_or_else(|error| panic!("released package input must adapt: {error}"))
}

fn without_declared_snapshot(mut package: GeneratedPackage) -> GeneratedPackage {
    package
        .files
        .retain(|file| file.path != GENERATED_DECLARED_DESCRIPTOR_PATH);
    if let Some(registry) = package
        .files
        .iter_mut()
        .find(|file| file.path == "registry.py")
        && let Some((models, _)) = registry
            .contents
            .split_once("\nGENERATED_DECLARED_DESCRIPTORS_JSON: str = ")
    {
        registry.contents = models.to_owned();
    }
    package
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
fn released_empty_and_comment_only_inputs_emit_one_canonical_empty_snapshot() {
    let expected = empty_generated_declared_descriptors_json()
        .expect("canonical empty generated descriptor snapshot");
    let fixtures = [
        "",
        " \t\r\n",
        "# released comment only\n",
        "// released slash comment only\r\n",
        "/* released block comment only */",
        "\n# first\n// second\n/* third */\n",
        "define\n",
    ];

    for source in fixtures {
        assert_eq!(
            generated_declared_descriptors_json(source)
                .unwrap_or_else(|error| panic!("empty released input must adapt: {error}")),
            expected,
            "fixture: {source:?}"
        );
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let package = generated_package(source, target);
            assert_eq!(
                package,
                generated_package("", target),
                "empty model package is canonical for {target}: {source:?}"
            );
            let snapshot = package["files"]
                .as_array()
                .expect("generated files")
                .iter()
                .find(|file| file["path"] == GENERATED_DECLARED_DESCRIPTOR_PATH)
                .expect("declared descriptor attachment");
            assert_eq!(snapshot["contents"], expected, "fixture: {source:?}");
        }

        let document = DocumentId::new("schema/strict-v2.tql").expect("valid document");
        if !source.trim_start().starts_with("define") {
            typeql_to_declared(document, source)
                .expect_err("the strict V2 importer still requires a define query");
        }
    }
}

#[test]
fn trusted_generator_keeps_released_large_inputs_without_loosening_authority_parsers() {
    let oversized = released_schema_with_trailing_comment(MAX_TYPEQL_SCHEMA_BYTES + 1);
    let package = generated_package_files(&oversized, TargetLanguage::Python);
    let attached = package
        .file(GENERATED_DECLARED_DESCRIPTOR_PATH)
        .expect("large released input retains its descriptor attachment");
    let attached: Value =
        serde_json::from_str(&attached.contents).expect("attached descriptor JSON");
    assert_eq!(attached["entities"][0]["label"], "person");
    assert_eq!(attached["closed_world"], true);
    assert_eq!(attached["unsupported_constructs"], serde_json::json!([]));

    for diagnostics in [
        typeql_to_declared(
            DocumentId::new("schema/untrusted-strict.tql").expect("document"),
            &oversized,
        )
        .expect_err("strict authority input remains defensively bounded"),
        released_typeql_to_declared_projection(
            DocumentId::new("schema/untrusted-released.tql").expect("document"),
            &oversized,
        )
        .expect_err("released authority input remains defensively bounded"),
    ] {
        assert_eq!(
            diagnostics
                .iter()
                .next()
                .expect("size diagnostic")
                .diagnostic()
                .code()
                .as_str(),
            "typeql_schema_size_limit",
        );
    }
}

#[test]
fn released_function_and_struct_only_inputs_attach_canonical_empty_snapshots() {
    let fixtures = [
        (
            "define\nfun answer() -> integer:\n  return 1;\n",
            true,
            false,
        ),
        ("define\nstruct payload, value note string;\n", false, true),
        (
            "define\n\
             fun answer() -> integer:\n  return 1;\n\
             struct payload, value note string;\n",
            true,
            true,
        ),
    ];
    let expected = empty_generated_declared_descriptors_json()
        .expect("canonical empty generated descriptor snapshot");

    for (source, has_functions, has_structs) in fixtures {
        assert_eq!(
            generated_declared_descriptors_json(source)
                .unwrap_or_else(|error| panic!("definition-only input must adapt: {error}")),
            expected,
            "fixture: {source:?}"
        );
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let package = generated_package_files(source, target);
            assert_eq!(
                package
                    .file(GENERATED_DECLARED_DESCRIPTOR_PATH)
                    .expect("declared descriptor attachment")
                    .contents,
                expected,
                "fixture: {source:?}"
            );
            if target == TargetLanguage::Python {
                assert_eq!(package.file("functions.py").is_some(), has_functions);
                assert_eq!(package.file("structs.py").is_some(), has_structs);
            }
        }
    }
}

#[test]
fn schemas_without_unresolved_references_preserve_raw_core_model_packages() {
    let fixtures = [
        "define\n\
         fun answer() -> integer:\n  return 1;\n\
         struct payload, value note string;\n",
        "define relation relates-only, relates participant;",
        "define attribute tag, value string;\n\
         entity person, owns tag[] @distinct;",
    ];

    for source in fixtures {
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
                source,
                target,
                &BindgenOptions::default(),
            )
            .expect("raw released bindgen package renders");
            let actual = without_declared_snapshot(generated_package_files(source, target));
            assert_eq!(actual, expected, "fixture for {target}: {source:?}");
        }
    }
}

#[test]
fn unrelated_open_world_omission_preserves_relates_only_relation_projection() {
    let source = "define\n\
        entity partial, owns absent-attribute;\n\
        relation audit-event, relates subject;\n";

    for (target, relation_path) in [
        (TargetLanguage::Python, "relations.py"),
        (TargetLanguage::TypeScript, "relations.ts"),
        (TargetLanguage::Rust, "relations.rs"),
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            source,
            target,
            &BindgenOptions::default(),
        )
        .expect("raw released bindgen package renders");
        let actual = generated_package_files(source, target);
        assert_eq!(
            actual
                .file(relation_path)
                .expect("compatibility relation projection")
                .contents,
            expected
                .file(relation_path)
                .expect("raw released relation projection")
                .contents,
            "an unrelated omitted owns reference must not erase relates-only schema for {target}"
        );
    }
}

#[test]
fn absent_user_types_in_released_functions_use_the_historical_scalar_fallback() {
    let source = "define\n\
        fun inspect($input: missing-input) -> missing-output:\n\
          return $input;\n";

    let parsed = type_bridge_core_lib::schema::TypeSchema::from_typeql(source)
        .expect("the released parser accepts identifier types in function signatures");
    assert_eq!(
        parsed.functions["inspect"].parameters[0].type_,
        "missing-input"
    );
    assert_eq!(
        parsed.functions["inspect"].return_type.types[0].name,
        "missing-output"
    );

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::BindgenPlan::from_schema(&parsed)
            .render(target, &BindgenOptions::default());
        let actual = without_declared_snapshot(generated_package_files(source, target));
        assert_eq!(
            actual, expected,
            "function-only compatibility generation remains the raw released projection for {target}"
        );
    }

    let python = generated_package_files(source, TargetLanguage::Python);
    let functions = &python
        .file("functions.py")
        .expect("released Python function wrappers")
        .contents;
    assert!(functions.contains("def inspect(input: str | str) -> FunctionQuery[str]:"));
    assert!(!functions.contains("MissingInput"));
    assert!(!functions.contains("MissingOutput"));
}

#[test]
fn absent_user_type_in_struct_field_remains_a_released_parse_error() {
    let source = "define struct payload, value item missing-type;";
    type_bridge_core_lib::schema::TypeSchema::from_typeql(source)
        .expect_err("the released struct grammar accepts only its fixed value-type vocabulary");
    generate_package_with_declared_descriptors(
        source,
        TargetLanguage::Python,
        &BindgenOptions::default(),
    )
    .expect_err("the compatibility generator must not broaden the released struct grammar");
}

#[test]
fn unresolved_released_references_are_exact_open_world_evidence() {
    let source = "define\n\
        entity child, sub missing-parent, owns missing-attribute @card(0..1),\
          plays missing-relation:member;\n\
        relation base, relates existing;\n\
        relation specialized, sub base, relates replacement as absent;\n\
        entity player, plays base:missing-role;\n\
        ghost plays missing-relation:missing-role;\n";
    let set = descriptors(source);

    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec![
            "sub missing-parent".to_owned(),
            "owns missing-attribute @card(0..1)".to_owned(),
            "plays missing-relation:member".to_owned(),
            "relates replacement as absent".to_owned(),
            "plays base:missing-role".to_owned(),
            "plays missing-relation:missing-role".to_owned(),
        ]
    );

    let child = set["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|entity| entity["label"] == "child")
        .expect("child descriptor");
    assert!(child["parent"].is_null());
    assert_eq!(child["owns"].as_array().expect("owns").len(), 0);
    assert_eq!(section_len(&set, "plays"), 0);

    let ghost = set["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|entity| entity["label"] == "ghost")
        .expect("standalone plays retains its released shell entity");
    assert_eq!(ghost["label"], "ghost");

    let specialized = set["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .find(|relation| relation["label"] == "specialized")
        .expect("specialized relation descriptor");
    assert_eq!(specialized["parent"], "base");
    assert_eq!(specialized["relates"].as_array().expect("relates").len(), 0);

    let canonical = generated_declared_descriptors_json(source).expect("descriptor snapshot");
    assert_eq!(
        generated_declared_descriptors_json(source).expect("repeat descriptor snapshot"),
        canonical,
        "open-world descriptor output is deterministic"
    );
    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let package = generated_package(source, target);
        let snapshot = package["files"]
            .as_array()
            .expect("generated files")
            .iter()
            .find(|file| file["path"] == GENERATED_DECLARED_DESCRIPTOR_PATH)
            .expect("declared descriptor attachment");
        assert_eq!(snapshot["contents"], canonical);
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
fn unresolved_references_do_not_escape_into_generated_model_sources() {
    let source = "define\n\
        entity child, sub missing-parent, owns missing-attribute @card(0..1),\
          plays missing-relation:member;\n\
        relation base, relates existing;\n\
        relation specialized, sub base, relates replacement as absent;\n\
        entity player, plays base:missing-role;\n\
        ghost plays missing-relation:missing-role;\n";

    let python = generated_package_files(source, TargetLanguage::Python);
    let entities = &python
        .file("entities.py")
        .expect("Python entities")
        .contents;
    let relations = &python
        .file("relations.py")
        .expect("Python relations")
        .contents;
    assert!(entities.contains("class Child(Entity):"));
    assert!(!entities.contains("missing-"));
    assert!(!entities.contains("plays: ClassVar"));
    assert!(!relations.contains("absent"));
    assert!(!relations.contains("existing: Role"));

    let typescript = generated_package_files(source, TargetLanguage::TypeScript);
    let entities = &typescript
        .file("entities.ts")
        .expect("TypeScript entities")
        .contents;
    let relations = &typescript
        .file("relations.ts")
        .expect("TypeScript relations")
        .contents;
    assert!(entities.contains("export class Child extends Entity(\"child\", {}) {}"));
    assert!(!entities.contains("Missing"));
    assert!(!relations.contains("Missing"));
    assert!(!relations.contains("Absent"));
    assert!(!relations.contains("existing: role"));

    let rust = generated_package_files(source, TargetLanguage::Rust);
    let entities = &rust.file("entities.rs").expect("Rust entities").contents;
    let relations = &rust.file("relations.rs").expect("Rust relations").contents;
    assert!(entities.contains("#[entity(name = \"child\")]"));
    assert!(!entities.contains("missing-"));
    assert!(!entities.contains("Missing"));
    assert!(!relations.contains("missing-"));
    assert!(!relations.contains("absent"));
    assert!(!relations.contains("replacement"));
    assert!(
        relations.contains("#[role(name = \"existing\", player_type = \"unknown\")]"),
        "the valid released relates-only base role is not an unresolved reference"
    );
}

struct TestStage(PathBuf);

impl TestStage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "type-bridge-schema-compat-rust-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("Rust compile stage is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sanitized_open_world_rust_package_compiles() {
    let package = generated_package_files(
        "define\n\
         entity member, plays membership:member;\n\
         relation membership, relates member;\n\
         entity child, sub missing-parent, owns missing-attribute,\
           plays missing-relation:member;\n\
         relation base, relates existing;\n\
         relation specialized, sub base, relates replacement as absent;\n\
         relation audit-event, relates subject;\n",
        TargetLanguage::Rust,
    );
    assert!(
        package
            .file("relations.rs")
            .expect("Rust relations")
            .contents
            .contains("player_type = \"member\""),
        "a fully resolved relation survives unrelated open-world omissions"
    );
    assert!(
        package
            .file("relations.rs")
            .expect("Rust relations")
            .contents
            .contains("#[role(name = \"subject\", player_type = \"unknown\")]"),
        "a relates-only relation survives and remains compilable beside an unrelated omission"
    );
    let stage = TestStage::new();
    let generated = stage.path().join("src/generated");
    fs::create_dir_all(&generated).expect("generated module directory is created");
    for file in package
        .files
        .iter()
        .filter(|file| file.path.ends_with(".rs"))
    {
        fs::write(generated.join(&file.path), &file.contents)
            .expect("generated Rust file is written");
    }
    fs::write(stage.path().join("src/lib.rs"), "pub mod generated;\n")
        .expect("generated consumer root is written");

    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("schema-compat lives below crates")
        .to_path_buf();
    fs::write(
        stage.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"schema-compat-generated-check\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nserde_json = \"1\"\ntype-bridge-orm = {{ path = {:?}, default-features = false, features = [\"derive\"] }}\n\n[workspace]\n",
            crates.join("orm")
        ),
    )
    .expect("generated consumer manifest is written");

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["check", "--quiet", "--offline", "--manifest-path"])
        .arg(stage.path().join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/schema-compat-generated-check"),
        )
        .output()
        .expect("generated Rust package cargo check starts");
    assert!(
        output.status.success(),
        "sanitized generated Rust package failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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
            "tag[]".to_owned(),
            "@distinct".to_owned(),
            "plays missing-relation:member".to_owned(),
        ]
    );
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
fn released_optional_comma_and_annotation_order_fixtures_generate() {
    let fixtures = [
        "define attribute email, value string, @regex(\"^[a-z]+$\");",
        "define attribute score @regex(\"x\"), value string;",
        "define attribute score, value string @abstract;",
        "define entity person, @abstract;",
        "define relation friendship, relates friend; ghost plays friendship:friend;",
    ];
    for source in fixtures {
        generated_python_package(source);
    }

    let email = descriptors(fixtures[0]);
    assert_eq!(email["attributes"][0]["regex"], "^[a-z]+$");

    let score = descriptors(fixtures[1]);
    assert_eq!(score["attributes"][0]["regex"], "x");
    assert_eq!(score["attributes"][0]["value_type"], "string");

    let abstract_score = descriptors(fixtures[2]);
    assert_eq!(abstract_score["attributes"][0]["is_abstract"], true);
    assert_eq!(abstract_score["attributes"][0]["value_type"], "string");

    let person = descriptors(fixtures[3]);
    assert_eq!(person["entities"][0]["is_abstract"], true);

    let shell = descriptors(fixtures[4]);
    assert_eq!(shell["entities"][0]["label"], "ghost");
    assert_eq!(shell["plays"][0]["player"], "ghost");
    assert_eq!(shell["plays"][0]["relation"], "friendship");
    assert_eq!(shell["plays"][0]["role"], "friend");
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
fn domain_incoherent_released_value_annotations_remain_generatable_and_open_world() {
    let fixtures = [
        (
            "define attribute score, value string @regex(\"x\"), value integer;",
            "@regex(\"x\")",
        ),
        (
            "define attribute status, value string @values(\"a\", \"b\"), value integer;",
            "@values(\"a\", \"b\")",
        ),
        (
            "define attribute score, value integer @range(1..5), value double;",
            "@range(1..5)",
        ),
        (
            "define attribute score, value string, @regex(\"x\"), value integer;",
            "@regex(\"x\")",
        ),
        (
            "define attribute status, value string, /* released trivia */ @values(\"a\", \"b\"), value integer;",
            "@values(\"a\", \"b\")",
        ),
        (
            "define attribute score, value integer,\n# released trivia\n@range(1..5), value double;",
            "@range(1..5)",
        ),
    ];
    for (source, annotation) in fixtures {
        let document = DocumentId::new("schema/main.tql").expect("valid document");
        let diagnostics =
            type_bridge_schema_compat::released_typeql_to_declared_projection(document, source)
                .expect_err("domain-incoherent annotations stay invalid declared facts");
        assert_eq!(
            diagnostics
                .iter()
                .next()
                .expect("diagnostic")
                .diagnostic()
                .code()
                .as_str(),
            "invalid_annotation_value_domain",
            "fixture: {source}"
        );
        generated_python_package(source);
        let set = descriptors(source);
        assert!(!closed_world(&set), "fixture: {source}");
        assert_eq!(unsupported(&set), vec![annotation.to_owned()]);
    }
}

#[test]
fn released_annotation_contract_mismatches_remain_generatable_and_open_world() {
    let fixtures: &[(&str, &[&str])] = &[
        (
            "define attribute name, value string; entity person, owns name @key @card(1..1);",
            &["@key"],
        ),
        (
            "define attribute name, value string; entity person, owns name @key @unique;",
            &["@key"],
        ),
        (
            "define relation event, relates participant @card(0);",
            &["@card(0)"],
        ),
        (
            "define relation event, relates participant @card(0..0);",
            &["@card(0..0)"],
        ),
        (
            "define attribute score, value integer @range(5..5);",
            &["@range(5..5)"],
        ),
        (
            "define attribute score, value integer @range(5..2);",
            &["@range(5..2)"],
        ),
        (
            "define attribute score, value integer @range(..);",
            &["@range(..)"],
        ),
        (
            "define attribute elapsed, value duration @range(P1D..P2D);",
            &["@range(P1D..P2D)"],
        ),
        (
            "define attribute score, value double; entity sample, owns score @key;",
            &["@key"],
        ),
        (
            "define attribute score, value double; entity sample, owns score @unique;",
            &["@unique"],
        ),
        ("define attribute tag @regex(\"x\");", &["@regex(\"x\")"]),
        ("define attribute tag @values(\"x\");", &["@values(\"x\")"]),
        (
            "define attribute tag, value string @regex(\"\");",
            &["@regex(\"\")"],
        ),
        ("define entity sample @doc(\"\");", &["@doc(\"\")"]),
        (
            "define entity sample @meta(\"match\", \"legacy\");",
            &["@meta(\"match\", \"legacy\")"],
        ),
        (
            "define attribute score, value integer @range(1..2.0);",
            &["@range(1..2.0)"],
        ),
    ];

    for (source, evidence) in fixtures {
        // The frozen renderer is the compatibility oracle: attaching a
        // descriptor must never turn one of its successes into an error or
        // alter any pre-existing generated file.
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
                source,
                target,
                &BindgenOptions::default(),
            )
            .unwrap_or_else(|error| panic!("frozen renderer accepts fixture: {error}"));
            let actual = without_declared_snapshot(generated_package_files(source, target));
            assert_eq!(actual, expected, "released output changed for {source}");
        }

        let set = descriptors(source);
        assert!(!closed_world(&set), "fixture: {source}");
        assert_eq!(
            unsupported(&set),
            evidence
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>(),
            "fixture: {source}"
        );
    }
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
fn nonportable_released_identifiers_omit_their_reference_closure() {
    let long_attribute = "a".repeat(256);
    let long_entity = "e".repeat(256);
    let long_relation = "r".repeat(256);
    let long_role = "p".repeat(256);
    let source = format!(
        "define\n\
         attribute {long_attribute}, value string;\n\
         entity {long_entity};\n\
         relation {long_relation}, relates visible;\n\
         entity portable-child, sub {long_entity};\n\
         entity portable-owner, owns {long_attribute};\n\
         relation portable-relation, relates {long_role};\n\
         entity portable-player, plays {long_relation}:visible, \
             plays portable-relation:{long_role};\n\
         relation specialized, sub portable-relation, \
             relates replacement as {long_role};\n\
         entity specialized-player, plays specialized:replacement;\n\
         relation recursive-child, sub {long_relation};\n\
         relation recursive-leaf, sub recursive-child;\n\
         entity recursive-player, plays recursive-leaf:visible;\n"
    );

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            &source,
            target,
            &BindgenOptions::default(),
        )
        .expect("frozen renderer accepts long identifiers");
        let actual = without_declared_snapshot(generated_package_files(&source, target));
        assert_eq!(actual, expected, "released output changed for {target}");
    }

    let set = descriptors(&source);
    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec![
            format!("attribute {long_attribute}, value string"),
            format!("entity {long_entity}"),
            format!("relation {long_relation}, relates visible"),
            format!("sub {long_entity}"),
            format!("owns {long_attribute}"),
            format!("relates {long_role}"),
            format!("plays {long_relation}:visible"),
            format!("plays portable-relation:{long_role}"),
            format!("relates replacement as {long_role}"),
            "plays specialized:replacement".to_owned(),
            format!("sub {long_relation}"),
            "plays recursive-leaf:visible".to_owned(),
        ]
    );
    assert_eq!(section_len(&set, "attributes"), 0);
    assert_eq!(section_len(&set, "relations"), 4);
    assert_eq!(section_len(&set, "plays"), 0);
    let recursive_leaf = set["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .find(|relation| relation["label"] == "recursive-leaf")
        .expect("portable recursive leaf remains");
    assert_eq!(recursive_leaf["parent"], "recursive-child");
}

#[test]
fn released_identifier_boundary_and_marker_controls_are_exact() {
    let boundary_type = "t".repeat(255);
    let boundary_attribute = "a".repeat(255);
    let boundary_role = "r".repeat(255);
    let marker = "z".repeat(256);
    let source = format!(
        "define\n\
         # entity {marker}; relates {marker};\n\
         attribute {boundary_attribute}, value string @doc(\"{marker}\");\n\
         relation {boundary_type}, relates {boundary_role};\n\
         entity holder, owns {boundary_attribute}, \
             plays {boundary_type}:{boundary_role};\n"
    );
    let set = descriptors(&source);
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());

    let reserved = "define entity match; attribute string, value string; \
        relation role-host, relates match; entity role-player, plays role-host:match;";
    let reserved_set = descriptors(reserved);
    assert!(!closed_world(&reserved_set));
    assert_eq!(
        unsupported(&reserved_set),
        vec![
            "entity match".to_owned(),
            "attribute string, value string".to_owned(),
            "relates match".to_owned(),
            "plays role-host:match".to_owned(),
        ]
    );
    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            reserved,
            target,
            &BindgenOptions::default(),
        )
        .expect("frozen renderer accepts its historical identifier vocabulary");
        let actual = without_declared_snapshot(generated_package_files(reserved, target));
        assert_eq!(actual, expected, "released output changed for {target}");
    }
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
fn keyword_shaped_standalone_plays_players_keep_released_output() {
    let source = "define\n\
        relation friendship, relates friend;\n\
        /* `entity plays` inside comments is inert. */\n\
        entity /* player trivia */ plays friendship:friend \
            @card(0..2) @doc(\"entity edge\");\n\
        relation /* player trivia */ plays friendship:friend \
            @card(1..3) @doc(\"relation edge\");\n\
        attribute /* player trivia */ plays friendship:friend \
            @doc(\"attribute edge\") @meta(\"source\", \"compat\");\n\
        define /* this is the player label, not a block marker */ \
            plays friendship:friend @card(1..1) @doc(\"define edge\");\n";

    let set = descriptors(source);
    let serialized = serde_json::to_string(&set).expect("descriptor set serializes");
    assert!(
        !serialized.contains("x_____"),
        "strict-parser placeholder labels must never escape the compatibility boundary"
    );

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            source,
            target,
            &BindgenOptions::default(),
        )
        .expect("frozen renderer accepts keyword-shaped standalone players");
        let actual = without_declared_snapshot(generated_package_files(source, target));
        assert_eq!(actual, expected, "released output changed for {target}");
    }
}

#[test]
fn declarations_named_plays_are_not_standalone_capabilities() {
    let fixtures = [
        "define\nentity plays;\n",
        "define\nrelation plays, relates friend;\n",
        "define\nattribute plays, value string;\n",
        "define\n\
            entity anchor;\n\
            define /* real repeated marker */\n\
            entity /* declaration label trivia */ plays \
                @doc(\"entity declaration named plays\");\n",
        "define\n\
            entity anchor;\n\
            define /* real repeated marker */\n\
            relation /* declaration label trivia */ plays, \
                relates friend @doc(\"role on relation named plays\");\n",
        "define\n\
            entity anchor;\n\
            define /* real repeated marker */\n\
            attribute /* declaration label trivia */ plays, value string \
                @doc(\"attribute declaration named plays\");\n",
    ];

    for source in fixtures {
        descriptors(source);
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
                source,
                target,
                &BindgenOptions::default(),
            )
            .expect("frozen renderer accepts a declaration named plays");
            let actual = without_declared_snapshot(generated_package_files(source, target));
            assert_eq!(
                actual, expected,
                "a declaration named plays changed for {target}: {source}"
            );
        }
    }
}

#[test]
fn keyword_markers_in_real_declarations_comments_and_strings_are_not_players() {
    let source = "define\n\
        attribute note, value string \
            @regex(\"define plays entity plays relation plays attribute plays\");\n\
        relation friendship, relates friend;\n\
        define\n\
        # define plays friendship:friend;\n\
        entity person @doc(\"define plays is literal text\"), \
            owns note, plays friendship:friend @doc(\"real edge\");\n";

    let set = descriptors(source);
    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    assert_eq!(section_len(&set, "entities"), 1);
    assert_eq!(section_len(&set, "relations"), 1);
    assert_eq!(section_len(&set, "attributes"), 1);
    assert_eq!(section_len(&set, "plays"), 1);

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            source,
            target,
            &BindgenOptions::default(),
        )
        .expect("frozen renderer accepts the false-positive control");
        let actual = without_declared_snapshot(generated_package_files(source, target));
        assert_eq!(
            actual, expected,
            "false-positive output changed for {target}"
        );
    }
}

#[test]
fn invalid_direct_specialization_cannot_borrow_a_coincident_parent_role_name() {
    let source = "define\n\
        relation parent, relates author;\n\
        relation child, sub parent, /* provenance boundary */\n\
          relates author as contributor;\n\
        entity person, plays child:author;\n";
    let set = descriptors(source);

    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec!["relates author as contributor".to_owned()]
    );
    assert_eq!(section_len(&set, "plays"), 1);
    assert_eq!(
        set["plays"][0]["relation"], "parent",
        "portable plays facts identify the direct ancestor role declaration"
    );
    assert_eq!(set["plays"][0]["role"], "author");
    let child = set["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .find(|relation| relation["label"] == "child")
        .expect("child relation descriptor");
    assert!(
        child["relates"]
            .as_array()
            .expect("direct relates")
            .is_empty()
    );

    for (target, relation_path) in [
        (TargetLanguage::Python, "relations.py"),
        (TargetLanguage::TypeScript, "relations.ts"),
        (TargetLanguage::Rust, "relations.rs"),
    ] {
        let package = generated_package_files(source, target);
        let relations = &package
            .file(relation_path)
            .expect("generated relations source")
            .contents;
        assert!(
            relations.to_ascii_lowercase().contains("parent")
                && relations.to_ascii_lowercase().contains("child"),
            "the inherited relation hierarchy survives for {target}"
        );
        assert!(
            !relations.to_ascii_lowercase().contains("contributor"),
            "the invalid direct specialization cannot escape for {target}"
        );
    }
}

#[test]
fn inherited_and_multilevel_specialized_roles_remain_playable() {
    let source = "define\n\
        relation root-relation, relates root-role;\n\
        relation middle-relation, sub root-relation, relates middle-role as root-role;\n\
        relation leaf-relation, sub middle-relation, relates leaf-role as root-role;\n\
        entity player, plays leaf-relation:middle-role, plays leaf-relation:leaf-role;\n";
    let document = DocumentId::new("schema/multilevel.tql").expect("valid document");
    released_typeql_to_declared_projection(document, source)
        .expect("released projection resolves inherited role identities");
    let set = descriptors(source);

    assert!(closed_world(&set));
    assert!(unsupported(&set).is_empty());
    let plays = set["plays"].as_array().expect("plays descriptors");
    assert_eq!(plays.len(), 2);
    assert!(plays.iter().any(|plays| plays["role"] == "middle-role"));
    assert!(plays.iter().any(|plays| plays["role"] == "leaf-role"));

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        let expected = type_bridge_core_lib::bindgen::generate_from_typeql(
            source,
            target,
            &BindgenOptions::default(),
        )
        .expect("raw released multilevel specialization renders");
        let actual = without_declared_snapshot(generated_package_files(source, target));
        assert_eq!(
            actual, expected,
            "multilevel specialization parity for {target}"
        );
    }
}

#[test]
fn inherited_plays_aliases_collapse_to_one_portable_role_identity() {
    let source = "define\n\
        relation parent, relates author;\n\
        relation child, sub parent;\n\
        entity person, plays parent:author @doc(\"direct scope\"),\n\
          plays child:author @doc(\"inherited alias\");\n";
    let set = descriptors(source);

    assert!(!closed_world(&set));
    assert_eq!(
        unsupported(&set),
        vec!["plays child:author @doc(\"inherited alias\")".to_owned()]
    );
    assert_eq!(section_len(&set, "plays"), 1);
    assert_eq!(set["plays"][0]["relation"], "parent");
    assert_eq!(set["plays"][0]["role"], "author");
    assert_eq!(set["plays"][0]["doc"], "direct scope");

    for target in [
        TargetLanguage::Python,
        TargetLanguage::TypeScript,
        TargetLanguage::Rust,
    ] {
        generated_package_files(source, target);
    }
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
