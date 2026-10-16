//! The Rust guide's plan-builder example compiles against a generated
//! crate and builds the same plan bytes as the hand-built purchase.

use std::fs;
use std::path::{Path, PathBuf};

use type_bridge_schema_codegen::RustEmitter;

mod support;
use support::{Stage, cargo_with_env, repository_root, write_package};

#[path = "../../rust/tests/support/purchase_plan.rs"]
mod purchase_plan;

fn package() -> type_bridge_schema_codegen::GeneratedPackage {
    let authority = support::authority_with(
        purchase_plan::SOURCE,
        purchase_plan::SCOPE,
        purchase_plan::PROFILE,
    );
    RustEmitter::new()
        .emit(
            &support::rust_projection_on(
                purchase_plan::SOURCE,
                "purchase.yaml",
                purchase_plan::PROFILE,
            ),
            &authority,
        )
        .expect("purchase package emits")
}

fn guide_example() -> String {
    let guide =
        fs::read_to_string(repository_root().join("docs/guide/rust.md")).expect("Rust guide reads");
    let begin = guide
        .find("<!-- rust-plan-example:begin -->")
        .expect("guide example begins");
    let end = guide
        .find("<!-- rust-plan-example:end -->")
        .expect("guide example ends");
    let block = &guide[begin..end];
    let code = block
        .split_once("```rust\n")
        .and_then(|(_, rest)| rest.rsplit_once("```"))
        .map(|(code, _)| code)
        .expect("guide example is one rust block");
    code.to_owned()
}

fn write_consumer(root: &Path, source: &str) {
    let rust = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust = rust.to_string_lossy().replace('\\', "\\\\");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"rust-plan-guide\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nmodels = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\ntype-bridge = {{ path = \"{rust}\" }}\n\n[patch.crates-io]\ntype-bridge = {{ path = \"{rust}\" }}\n\n[workspace]\n"
        ),
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), source).unwrap();
}

/// The guide example compiles against the generated crate, and its plan
/// has the hand-built purchase's canonical bytes.
#[test]
fn guide_plan_example_compiles_and_matches_the_hand_built_plan() {
    let stage = Stage::new();
    write_package(&package(), &stage.path().join("generated"));
    let consumer = stage.path().join("consumer");
    write_consumer(
        &consumer,
        &format!(
            "#![allow(dead_code)]\n{}\nfn main() {{\n    let plan = purchase(\"O1\", \"I1\", \"A1\").expect(\"plan builds\");\n    let bytes = plan.canonical_bytes().expect(\"canonical bytes\");\n    print!(\"{{}}\", bytes.iter().map(|b| format!(\"{{b:02x}}\")).collect::<String>());\n}}\n",
            guide_example()
        ),
    );
    let manifest = consumer.join("Cargo.toml");
    let output = cargo_with_env(
        &[
            "run",
            "--quiet",
            "--manifest-path",
            manifest.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        output.status.success(),
        "guide consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let authority = support::authority_with(
        purchase_plan::SOURCE,
        purchase_plan::SCOPE,
        purchase_plan::PROFILE,
    );
    let expected = purchase_plan::hand_built_purchase(
        authority.declared_schema(),
        authority.managed_state().managed_semantic_schema().clone(),
        "O1",
        "I1",
        "A1",
    )
    .canonical_bytes()
    .unwrap();
    let expected: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

/// The `writes` conformance suite runs the hand-built purchase's schema, and
/// its expected purchase bytes are the hand-built purchase under the suite's
/// managed scope, so all four SDK programs are held to the golden plan.
#[test]
fn writes_suite_purchase_bytes_are_the_hand_built_purchase() {
    let suite = repository_root().join("tests/contracts/sdk_conformance/writes/purchase");
    let source = fs::read_to_string(suite.join("schema/fragments/purchase.yaml"))
        .expect("suite schema reads");
    assert_eq!(source, purchase_plan::SOURCE);
    let workspace = fs::read_to_string(suite.join("typebridge.yaml")).expect("workspace reads");
    let scope = workspace
        .lines()
        .find_map(|line| line.trim().strip_prefix("managed-scope: "))
        .expect("workspace names its managed scope");
    assert!(workspace.contains(&format!("semantic-profile: {}", purchase_plan::PROFILE)));
    let expected: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(suite.join("expected.json")).expect("expected results read"),
    )
    .expect("expected results parse");
    let authority = support::authority_with(&source, scope, purchase_plan::PROFILE);
    let bytes = purchase_plan::hand_built_purchase(
        authority.declared_schema(),
        authority.managed_state().managed_semantic_schema().clone(),
        "O1",
        "I1",
        "A1",
    )
    .canonical_bytes()
    .unwrap();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(expected["plans"]["purchase"], hex);
}
