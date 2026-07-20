//! Offline `schema generate`: every configured projection lands on disk
//! deterministically through the shipped binary.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

fn run_cli(workspace: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(workspace)
        .args(arguments)
        .output()
        .expect("the type-bridge binary runs")
}

fn assert_success(output: &std::process::Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("output directory reads") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("generated file confined to output root")
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, fs::read(&path).expect("generated file reads"));
            }
        }
    }
    files
}

#[test]
fn schema_generate_emits_all_configured_projections_deterministically() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: gen-smoke\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: gensmoke\n\
         bindings:\n  python:\n    output: generated/python\n  typescript:\n    \
         output: generated/typescript\n  rust:\n    output: generated/rust\n",
    )
    .expect("manifest writes");
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  nickname: { value: string }\n\
         entities:\n  person: { owns: [nickname] }\n",
    )
    .expect("schema writes");

    assert_success(&run_cli(root, &["schema", "generate"]), "schema generate");

    let mut snapshots = BTreeMap::new();
    for target in ["python", "typescript", "rust"] {
        let output_root = root.join("generated").join(target);
        let files = snapshot(&output_root);
        assert!(
            !files.is_empty(),
            "{target} projection produced no files under {}",
            output_root.display(),
        );
        snapshots.insert(target, files);
    }

    // A second run must succeed over the existing outputs and reproduce
    // byte-identical files: generation is deterministic and atomic
    // overwrite leaves no temporary artifacts behind.
    assert_success(
        &run_cli(root, &["schema", "generate"]),
        "schema generate rerun",
    );
    for target in ["python", "typescript", "rust"] {
        let files = snapshot(&root.join("generated").join(target));
        assert_eq!(
            &files, &snapshots[target],
            "{target} projection changed between identical runs",
        );
        assert!(
            files.keys().all(|path| !path.contains("typebridge-tmp")),
            "temporary files leaked into the {target} output",
        );
    }
}
