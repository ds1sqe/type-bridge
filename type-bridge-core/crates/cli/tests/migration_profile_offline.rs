//! Offline migration authoring binds the workspace profile's lowering registry.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn write_workspace(root: &Path, semantic_profile: &str) {
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::write(
        root.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\n\
             schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: app\n\
             compatibility:\n  semantic-profile: {semantic_profile}\n\
             migrations:\n  directory: migrations/v2\n  app-label: app\n"
        ),
    )
    .expect("manifest writes");
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("schema writes");
}

fn run(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(root)
        .args(args)
        .output()
        .expect("the type-bridge binary runs");
    assert!(
        output.status.success(),
        "{args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn assert_make_and_plan_bind(semantic_profile: &str, lowering: &str, other_lowering: &str) {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(workspace.path(), semantic_profile);

    run(workspace.path(), &["migration", "make", "--name", "init"]);
    let manifests = fs::read_dir(workspace.path().join("migrations/v2"))
        .expect("migration directory reads")
        .map(|entry| entry.expect("migration directory entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".tbmigration.json"))
        .collect::<Vec<_>>();
    assert_eq!(manifests.len(), 1, "one manifest: {manifests:?}");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifests[0]).expect("manifest reads"))
            .expect("manifest is JSON");
    let text = manifest.to_string();
    assert!(
        text.contains(&format!("\"{lowering}\"")),
        "manifest binds {lowering}: {text}"
    );
    assert!(
        !text.contains(other_lowering),
        "manifest must not bind {other_lowering}: {text}"
    );

    // `plan` re-verifies every committed manifest under the workspace profile.
    let plan = run(workspace.path(), &["migration", "plan"]);
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("app/"),
        "plan lists the migration: {stdout}"
    );
}

#[test]
fn typedb_3_12_1_workspace_binds_its_lowering_profile() {
    assert_make_and_plan_bind(
        "typedb-3.12.1/v1",
        "typedb-3.12.1-schema-lowering/v1",
        "typedb-3.13.6-schema-lowering/v1",
    );
}

#[test]
fn typedb_3_13_6_workspace_binds_its_lowering_profile() {
    assert_make_and_plan_bind(
        "typedb-3.13.6/v1",
        "typedb-3.13.6-schema-lowering/v1",
        "typedb-3.12.1-schema-lowering/v1",
    );
}
