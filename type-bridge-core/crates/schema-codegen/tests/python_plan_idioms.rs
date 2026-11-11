//! Generated Python authoring, declaration and core-builder equality acceptance.
use std::fs;
use std::process::Command;

use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_schema::project;
use type_bridge_schema_codegen::PythonEmitter;

mod support;

#[test]
#[ignore = "requires the installed Python facade and Pyright"]
fn generated_plan_idioms_are_typed_and_equal_the_core_builder() {
    let source = include_str!("acceptance/schema-plan-idioms.yaml");
    let authority = support::authority(source);
    let schema = authority.resolved_schema();
    let emitter = PythonEmitter::new();
    let projection = project(
        schema,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &emitter.generator_handlers_for(schema),
        &emitter.code_resources_for(schema).unwrap(),
    )
    .unwrap();
    let stage = support::Stage::new();
    let root = stage.path();
    let generated = root.join("generated_idioms");
    support::write_package(&emitter.emit(&projection, &authority).unwrap(), &generated);
    let repository = support::repository_root();
    let python = repository.join(".venv/bin/python");
    let pyright = repository.join(".venv/bin/pyright");
    for (name, source) in [
        (
            "positive.py",
            include_str!("acceptance/plan_idioms_positive.py"),
        ),
        (
            "negative.py",
            include_str!("acceptance/plan_idioms_negative.py"),
        ),
        ("check.py", include_str!("acceptance/plan_idioms_check.py")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    for name in ["positive.py", "negative.py"] {
        let config = root.join("pyrightconfig.json");
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "include": [name, "generated_idioms"],
                "extraPaths": [repository, root],
                "pythonVersion": "3.12", "typeCheckingMode": "strict",
                "reportMissingModuleSource": "none"
            }))
            .unwrap(),
        )
        .unwrap();
        let output = Command::new(&pyright)
            .arg("--outputjson")
            .arg("--pythonpath")
            .arg(&python)
            .arg("--project")
            .arg(&config)
            .output()
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
                panic!(
                    "Pyright did not emit JSON: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        let diagnostics = report["generalDiagnostics"].as_array().unwrap();
        if name == "positive.py" {
            assert!(output.status.success(), "{report:#}");
        } else {
            assert!(
                !output.status.success(),
                "negative calls unexpectedly type-checked"
            );
            let expected = include_str!("acceptance/plan_idioms_negative.py")
                .lines()
                .enumerate()
                .filter_map(|(line, text)| {
                    text.contains("# error")
                        .then_some(u64::try_from(line).unwrap())
                })
                .collect::<std::collections::BTreeSet<_>>();
            let actual = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic["severity"] == "error")
                .map(|diagnostic| {
                    assert!(
                        diagnostic["file"]
                            .as_str()
                            .unwrap()
                            .ends_with("negative.py"),
                        "unrelated diagnostic: {diagnostic}"
                    );
                    diagnostic["range"]["start"]["line"].as_u64().unwrap()
                })
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual, expected, "{report:#}");
        }
    }
    let output = Command::new(&python)
        .arg(root.join("check.py"))
        .env("PYTHONPATH", root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated runtime failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
