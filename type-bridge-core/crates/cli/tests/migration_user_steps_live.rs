//! Generated SDK callback evidence on the configured exact TypeDB profile.
//! Build the Python, Node and C cdylibs into CARGO_TARGET_DIR/debug first.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use type_bridge_typedb_runtime::{ConnectOptions, TypeDBRuntime};
mod support;

fn run(command: &mut Command) {
    let output = command.output().expect("consumer command starts");
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
fn copy_python_package(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|ext| ext == "py") {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
#[ignore = "requires exact live TypeDB, rebuilt debug cdylibs, Python dev environment and Node dependencies"]
fn four_generated_sdks_share_atomic_user_callback_transactions() {
    support::requested_administration_server_version_blocking();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .expect("reuse the coordinated CARGO_TARGET_DIR");
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path();
    let source = root.join("type-bridge-core/crates/cli/tests/migration_user_steps_live");
    let profile = support::sdk_profile_matrix::requested("administration").semantic_profile;
    fs::create_dir_all(workspace.join("schema")).unwrap();
    fs::create_dir_all(workspace.join("migrations/v2")).unwrap();
    fs::write(
        workspace.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [person.yaml]\n",
    )
    .unwrap();
    fs::copy(
        source.join("person.yaml"),
        workspace.join("schema/person.yaml"),
    )
    .unwrap();
    fs::copy(
        source.join("gender.data.yaml"),
        workspace.join("migrations/v2/gender.data.yaml"),
    )
    .unwrap();
    fs::write(workspace.join("typebridge.yaml"), format!("format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: user-steps\ncompatibility:\n  semantic-profile: {profile}\nmigrations:\n  directory: migrations/v2\n  app-label: users\nbindings:\n  python:\n    output: generated/python/callback_models\n  typescript:\n    output: generated/typescript\n  rust:\n    output: generated/rust\n  c:\n    output: generated/c\n")).unwrap();
    let cli = env!("CARGO_BIN_EXE_type-bridge");
    run(Command::new(cli)
        .current_dir(workspace)
        .args(["migration", "make", "--name", "initial"]));
    run(Command::new(cli).current_dir(workspace).args([
        "migration",
        "make",
        "--name",
        "gender",
        "--data-step",
        "gender.data.yaml",
    ]));
    run(Command::new(cli)
        .current_dir(workspace)
        .args(["schema", "generate"]));
    let prefix = env::var("TYPE_BRIDGE_CALLBACK_DATABASE_PREFIX")
        .expect("provide the allocated database prefix");
    let databases =
        ["python", "node", "rust", "c"].map(|sdk| format!("{prefix}_{sdk}_{}", std::process::id()));
    let result = std::panic::catch_unwind(|| {
        let python_root = workspace.join("generated/python");
        copy_python_package(
            &root.join("type-bridge-core/python/type_bridge_core"),
            &python_root.join("type_bridge_core"),
        );
        fs::copy(
            target.join("debug/libtype_bridge_core.so"),
            python_root.join("type_bridge_core/type_bridge_core.abi3.so"),
        )
        .unwrap();
        let python = env::var_os("TYPE_BRIDGE_CALLBACK_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(".venv/bin/python"));
        run(Command::new(python)
            .env(
                "PYTHONPATH",
                format!("{}:{}", python_root.display(), root.display()),
            )
            .env("TYPE_BRIDGE_CALLBACK_DATABASE", &databases[0])
            .arg(source.join("consumer.py")));

        let node = root.join("type-bridge-core/crates/node");
        fs::copy(
            target.join("debug/libtype_bridge_node.so"),
            node.join("type_bridge_node.linux-x64-gnu.node"),
        )
        .unwrap();
        run(Command::new("npm")
            .current_dir(&node)
            .args(["run", "build:types"]));
        let generated = workspace.join("generated/typescript");
        fs::create_dir_all(generated.join("node_modules/@type-bridge")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&node, generated.join("node_modules/@type-bridge/node"))
            .unwrap();
        run(Command::new(node.join("node_modules/.bin/tsc"))
            .arg("-p")
            .arg(generated.join("tsconfig.json")));
        fs::copy(source.join("consumer.mjs"), generated.join("consumer.mjs")).unwrap();
        run(Command::new("node")
            .env("TYPE_BRIDGE_CALLBACK_DATABASE", &databases[1])
            .arg(generated.join("consumer.mjs")));

        let generated = workspace.join("generated/rust");
        let manifest = generated.join("Cargo.toml");
        fs::write(&manifest,fs::read_to_string(&manifest).unwrap().replace("type-bridge = { version = \"=2.3.2\", default-features = false }",&format!("type-bridge = {{ path = {:?} }}\ntokio = {{ version = \"1\", features = [\"full\"] }}\nserde_json = \"1\"",root.join("type-bridge-core/crates/rust")))).unwrap();
        fs::copy(
            root.join("type-bridge-core/Cargo.lock"),
            generated.join("Cargo.lock"),
        )
        .unwrap();
        fs::create_dir_all(generated.join("examples")).unwrap();
        fs::copy(
            source.join("consumer.rs"),
            generated.join("examples/consumer.rs"),
        )
        .unwrap();
        run(Command::new("cargo")
            .env("TYPE_BRIDGE_CALLBACK_DATABASE", &databases[2])
            .args(["run", "--offline", "--manifest-path"])
            .arg(manifest)
            .args(["--example", "consumer"]));

        let generated = workspace.join("generated/c");
        let executable = workspace.join("c-consumer");
        run(Command::new("cc")
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(root.join("type-bridge-core/crates/c/include"))
            .arg("-I")
            .arg(generated.join("include"))
            .arg(source.join("consumer.c"))
            .arg(generated.join("src/models.c"))
            .arg("-L")
            .arg(target.join("debug"))
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", target.join("debug").display()))
            .arg("-o")
            .arg(&executable));
        run(Command::new(executable)
            .env("TYPE_BRIDGE_CALLBACK_DATABASE", &databases[3])
            .arg(generated.join("typebridge/migration-history.json")));
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let provider = TypeDBRuntime::connect(
            &env::var("TYPEDB_ADDRESS").unwrap(),
            &env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
            &env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
            ConnectOptions {
                http_port: env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for database in &databases {
            for name in [database.clone(), format!("{database}__tb_journal")] {
                if provider.database_exists(&name).await.unwrap() {
                    provider.delete_database(&name).await.unwrap();
                }
            }
        }
    });
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
