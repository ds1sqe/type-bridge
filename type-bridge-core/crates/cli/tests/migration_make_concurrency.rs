//! Concurrent offline authoring must serialize across independent CLI processes.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

fn write_workspace(root: &Path) {
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: race\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: race\n",
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

fn run_make(root: &Path, name: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(root)
        .args(["migration", "make", "--name", name])
        .output()
        .expect("the type-bridge binary runs")
}

fn output_context(output: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn different_names_in_independent_processes_publish_one_unambiguous_head() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(workspace.path());

    // Synchronize the process launches so both commands inspect the same empty
    // history. The OS-backed directory lock, rather than in-process state,
    // must arbitrate publication by these independent executables.
    let start = Arc::new(Barrier::new(3));
    let handles = ["first", "second"].map(|name| {
        let root = workspace.path().to_path_buf();
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            run_make(&root, name)
        })
    });
    start.wait();
    let outputs = handles.map(|handle| handle.join().expect("CLI runner does not panic"));

    let successes = outputs
        .iter()
        .filter(|output| output.status.success())
        .collect::<Vec<_>>();
    let failures = outputs
        .iter()
        .filter(|output| !output.status.success())
        .collect::<Vec<_>>();
    assert_eq!(
        successes.len(),
        1,
        "exactly one process must publish:\n{}\n{}",
        output_context(&outputs[0]),
        output_context(&outputs[1]),
    );
    assert_eq!(
        failures.len(),
        1,
        "exactly one process must fail closed:\n{}\n{}",
        output_context(&outputs[0]),
        output_context(&outputs[1]),
    );

    let failure = String::from_utf8_lossy(&failures[0].stderr);
    assert!(
        failure.contains("[migration_generation_write_conflict]")
            || failure.contains("[workspace_generated_migration_stale]"),
        "the loser must report the stable live-lock or stale-candidate diagnostic: {failure}",
    );

    let migration_directory = workspace.path().join("migrations/v2");
    let entries = fs::read_dir(&migration_directory)
        .expect("migration directory reads")
        .map(|entry| {
            entry
                .expect("migration directory entry")
                .file_name()
                .into_string()
                .expect("generated names are UTF-8")
        })
        .collect::<Vec<_>>();
    assert!(
        entries.iter().all(|name| !name.ends_with(".tmp")),
        "publication leaked a temporary file: {entries:?}",
    );
    let manifests = entries
        .iter()
        .filter(|name| name.ends_with(".tbmigration.json"))
        .collect::<Vec<_>>();
    assert_eq!(
        manifests.len(),
        1,
        "history must have one head: {entries:?}"
    );
    let previews = entries
        .iter()
        .filter(|name| name.ends_with(".typeql"))
        .collect::<Vec<_>>();
    assert_eq!(
        previews.len(),
        1,
        "history must have one preview: {entries:?}"
    );

    let migration_stem = manifests[0]
        .strip_suffix(".tbmigration.json")
        .expect("manifest suffix");
    assert_eq!(previews[0].as_str(), format!("{migration_stem}.typeql"));
    assert!(
        matches!(migration_stem, "0001_first" | "0001_second"),
        "unexpected winning migration: {migration_stem}",
    );
    let success = String::from_utf8_lossy(&successes[0].stdout);
    assert!(
        success.contains(&format!("migrations/v2/{migration_stem}.tbmigration.json"))
            && success.contains(&format!("migrations/v2/{migration_stem}.typeql")),
        "success output does not identify the committed manifest and preview: {success}",
    );
    let preview =
        fs::read_to_string(migration_directory.join(previews[0])).expect("published preview reads");
    assert!(
        preview.contains("entity person") && preview.contains("attribute name"),
        "preview does not describe the committed desired schema: {preview}",
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(workspace.path())
        .args(["migration", "plan"])
        .output()
        .expect("migration plan runs");
    assert!(plan.status.success(), "{}", output_context(&plan));
    assert_eq!(
        String::from_utf8_lossy(&plan.stdout),
        format!("race/{migration_stem}  safety=Additive  reversible=true\n"),
        "the published history must have one unambiguous replay head",
    );

    let noop = run_make(workspace.path(), "noop");
    assert!(noop.status.success(), "{}", output_context(&noop));
    assert_eq!(
        String::from_utf8_lossy(&noop.stdout),
        "history already reaches the desired schema\n",
    );
    let final_entries = fs::read_dir(migration_directory)
        .expect("migration directory rereads")
        .map(|entry| {
            entry
                .expect("migration directory entry")
                .file_name()
                .into_string()
                .expect("generated names are UTF-8")
        })
        .collect::<Vec<_>>();
    assert!(
        final_entries.iter().all(|name| !name.ends_with(".tmp")),
        "verification leaked a temporary file: {final_entries:?}",
    );
    assert_eq!(
        final_entries
            .iter()
            .filter(|name| name.ends_with(".tbmigration.json"))
            .count(),
        1,
    );
}
