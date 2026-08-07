//! Offline `schema generate`: every configured projection lands on disk
//! deterministically through the shipped binary.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::schema::{decode_declared_schema, encode_declared_schema};
use type_bridge_schema::{
    decode_schema_authority, encode_schema_authority, schema_authority_capability_vocabulary,
};

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
fn published_split_yaml_v1_fixture_passes_offline_schema_check() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/fixtures/split-yaml-v1");
    let output = run_cli(&fixture, &["schema", "check"]);
    assert_success(&output, "published split-YAML V1 fixture");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("schema sources are valid"),
        "schema check omitted its success contract: {}",
        String::from_utf8_lossy(&output.stdout),
    );
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
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n  require: [schema.transition.define]\n\
         migrations:\n  directory: migrations/v2\n  app-label: gensmoke\n\
         bindings:\n  python:\n    output: generated/python\n  typescript:\n    \
         output: generated/typescript\n  rust:\n    output: generated/rust\n\
         artifacts:\n  schema-authority:\n    output: generated/schema-authority.json\n",
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
         entities:\n  person: { owns: [nickname] }\n  employee: { sub: { type: person } }\n",
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
        assert!(
            files
                .values()
                .any(|contents| String::from_utf8_lossy(contents).contains("Employee")),
            "{target} projection omitted the expanded-sub employee type",
        );
        snapshots.insert(target, files);
    }
    let authority_path = root.join("generated/schema-authority.json");
    let authority_bytes = fs::read(&authority_path).expect("generated authority artifact reads");
    let available = schema_authority_capability_vocabulary();
    let authority = decode_schema_authority(&authority_bytes, &available)
        .expect("generated authority artifact reconstructs without schema sources");
    assert!(
        authority.required_capabilities().contains(
            &CapabilityId::new("schema.transition.define")
                .expect("additive execution capability is canonical")
        ),
        "generated authority omitted the additive workspace requirement",
    );
    assert_eq!(
        encode_schema_authority(&authority),
        authority_bytes,
        "generated authority artifact must already use canonical bytes",
    );
    let authority_value: serde_json::Value =
        serde_json::from_slice(&authority_bytes).expect("authority is canonical JSON");
    let authority_digest = authority_value["authority_fingerprint"]["digest"]
        .as_str()
        .expect("authority fingerprint digest is a string");
    for target in ["python", "typescript", "rust"] {
        assert!(
            snapshots[target]
                .values()
                .any(|contents| { String::from_utf8_lossy(contents).contains(authority_digest) }),
            "{target} projection did not embed the generated server authority identity",
        );
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
    assert_eq!(
        fs::read(&authority_path).expect("generated authority artifact rereads"),
        authority_bytes,
        "authority artifact changed between identical generation runs",
    );

    let declared_path = root.join("generated/authority/declared-schema.json");
    assert_success(
        &run_cli(
            root,
            &[
                "schema",
                "export-declared",
                "--output",
                "generated/authority/declared-schema.json",
            ],
        ),
        "schema export-declared",
    );
    let first = fs::read(&declared_path).expect("declared artifact reads");
    let decoded = decode_declared_schema(&first).expect("declared artifact decodes");
    assert_eq!(
        encode_declared_schema(&decoded).expect("declared artifact re-encodes"),
        first,
        "CLI output must already be canonical low-level bytes",
    );
    assert_success(
        &run_cli(
            root,
            &[
                "schema",
                "export-declared",
                "--output",
                "generated/authority/declared-schema.json",
            ],
        ),
        "schema export-declared rerun",
    );
    assert_eq!(
        fs::read(&declared_path).expect("declared artifact rereads"),
        first,
        "declared artifact changed between identical runs",
    );

    let escaped = run_cli(
        root,
        &["schema", "export-declared", "--output", "../escaped.json"],
    );
    assert!(
        !escaped.status.success(),
        "escaping low-level output unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&escaped.stderr).contains("confined portable workspace path"),
        "escaping output did not return the stable confinement diagnostic: {}",
        String::from_utf8_lossy(&escaped.stderr),
    );
    assert!(!root.parent().unwrap().join("escaped.json").exists());
}

#[test]
fn schema_generate_supports_a_root_level_schema_set_and_json_authority() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema.yaml\n  ownership: exclusive\n  managed-scope: root-smoke\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: rootsmoke\n\
         bindings:\n  python:\n    output: generated/python\n\
         artifacts:\n  schema-authority:\n    output: generated/schema-authority.json\n",
    )
    .expect("workspace manifest writes");
    fs::write(
        root.join("schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [model.yaml]\n",
    )
    .expect("root schema set writes");
    fs::write(
        root.join("model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("root schema source writes");

    assert_success(
        &run_cli(root, &["schema", "generate"]),
        "root-level schema generate",
    );
    let authority = root.join("generated/schema-authority.json");
    assert!(authority.is_file(), "schema authority was not emitted");
    assert!(root.join("generated/python/_authority.py").is_file());
}

#[cfg(unix)]
#[test]
fn schema_generate_rejects_symlinked_output_directory() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().expect("workspace directory");
    let outside = tempfile::tempdir().expect("outside directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::create_dir(root.join("generated")).expect("generated parent");
    symlink(outside.path(), root.join("generated/python")).expect("output symlink");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: gen-link\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: genlink\n\
         bindings:\n  python:\n    output: generated/python\n",
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

    let output = run_cli(root, &["schema", "generate"]);
    assert!(
        !output.status.success(),
        "symlinked output unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("real directory, not a link"),
        "unexpected diagnostic: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        fs::read_dir(outside.path()).expect("outside reads").count(),
        0,
        "generation escaped into the symlink target",
    );

    let declared = run_cli(
        root,
        &[
            "schema",
            "export-declared",
            "--output",
            "generated/python/declared-schema.json",
        ],
    );
    assert!(
        !declared.status.success(),
        "declared-schema export through a symlink unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&declared.stderr).contains("real directory, not a link"),
        "unexpected declared-schema diagnostic: {}",
        String::from_utf8_lossy(&declared.stderr),
    );
    assert_eq!(
        fs::read_dir(outside.path()).expect("outside reads").count(),
        0,
        "declared-schema export escaped into the symlink target",
    );

    fs::remove_file(root.join("generated/python")).expect("remove directory symlink");
    fs::create_dir(root.join("generated/python")).expect("real output directory");
    let outside_file = outside.path().join("victim.py");
    fs::write(&outside_file, b"untouched").expect("outside file writes");
    symlink(&outside_file, root.join("generated/python/_models.py")).expect("final output symlink");

    let output = run_cli(root, &["schema", "generate"]);
    assert!(
        !output.status.success(),
        "symlinked final output unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("regular file"),
        "unexpected final-output diagnostic: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        fs::read(&outside_file).expect("outside file reads"),
        b"untouched",
        "generation followed the final output symlink",
    );
}

#[test]
fn schema_generate_rejects_hostile_later_target_before_replacing_earlier_binding() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: gen-batch\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: genbatch\n\
         bindings:\n  python:\n    output: generated/python\n",
    )
    .expect("initial manifest writes");
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
    .expect("initial schema writes");
    assert_success(
        &run_cli(root, &["schema", "generate"]),
        "initial Python generation",
    );
    let accepted_python = snapshot(&root.join("generated/python"));

    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  nickname: { value: string }\n\
         entities:\n  person: { owns: [nickname] }\n  company: { owns: [nickname] }\n",
    )
    .expect("changed schema writes");
    fs::write(
        root.join("typebridge.yaml"),
        "format: typebridge.workspace/v1\n\
         schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: gen-batch\n\
         compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
         migrations:\n  directory: migrations/v2\n  app-label: genbatch\n\
         bindings:\n  python:\n    output: generated/python\n  typescript:\n    output: generated/typescript\n\
         artifacts:\n  schema-authority:\n    output: generated/schema-authority.json\n",
    )
    .expect("expanded manifest writes");
    fs::create_dir_all(root.join("generated/typescript/package.json"))
        .expect("hostile later final directory creates");

    let output = run_cli(root, &["schema", "generate"]);
    assert!(
        !output.status.success(),
        "generation with a hostile later target unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("regular file, not a link or special entry"),
        "unexpected hostile-target diagnostic: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        snapshot(&root.join("generated/python")),
        accepted_python,
        "the earlier accepted Python generation changed before the later target rejected",
    );
    assert!(
        snapshot(&root.join("generated")).keys().all(|path| {
            !path.contains("typebridge-tmp")
                && !path.contains("typebridge-backup")
                && !path.contains("typebridge-rollback")
        }),
        "generation leaked a temporary or backup after prevalidation rejection",
    );
    assert!(
        !root.join("generated/schema-authority.json").exists(),
        "schema authority was published despite an earlier batch rejection",
    );
}
