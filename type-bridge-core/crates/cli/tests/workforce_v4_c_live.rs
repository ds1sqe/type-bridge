//! Exact-live internal C ABI 1.5 Workforce V4 report evidence.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use type_bridge_typedb_runtime::{ConnectOptions, QueryResult, TxType, TypeDBRuntime};

const CONSUMER: &str = include_str!("workforce_v4_c_live/consumer.c");

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/workforce-v4/workspace")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination creates");
    for entry in fs::read_dir(source).expect("source reads") {
        let entry = entry.expect("source entry reads");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("source type reads").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("source file copies");
        }
    }
}

fn source_identity(root: &Path, relative: &str) -> Value {
    let bytes = fs::read(root.join(relative)).expect("authority source reads");
    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
}

fn result(case: &str, capability: &str, proof: &str, observation: Value) -> Value {
    json!({"case_id": case, "capability_id": capability, "proof_kind": proof, "outcome": "passed", "observation": observation})
}

fn run(command: &mut Command, description: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{description} starts: {error}"));
    assert!(
        output.status.success(),
        "{description} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

async fn fixture_runtime() -> TypeDBRuntime {
    TypeDBRuntime::connect(
        &std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".to_owned())
                .parse()
                .expect("HTTP port is valid"),
            ..ConnectOptions::default()
        },
    )
    .await
    .expect("fixture runtime connects")
}

async fn fixture_write(runtime: &TypeDBRuntime, database: &str, query: &str) {
    let mut transaction = runtime
        .open_transaction(database, TxType::Write)
        .await
        .unwrap();
    transaction.query(query).await.unwrap();
    transaction.commit().await.unwrap();
}

async fn fixture_count(runtime: &TypeDBRuntime, database: &str, query: &str) -> usize {
    let mut transaction = runtime
        .open_transaction(database, TxType::Read)
        .await
        .unwrap();
    let count = match transaction.query(query).await.unwrap() {
        QueryResult::Documents(values) | QueryResult::Rows(values) => values.len(),
        QueryResult::Ok => 0,
    };
    transaction.close().await.unwrap();
    count
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires an isolated exact TypeDB 3.12.3 server and a C17 compiler"]
async fn generated_c_emits_complete_v4_report_on_3_12_3() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    run(
        Command::new(env!("CARGO_BIN_EXE_type-bridge"))
            .current_dir(temporary.path())
            .args(["schema", "generate"]),
        "schema generation",
    );
    let root = repository_root();
    let core = root.join("type-bridge-core");
    run(
        Command::new("cargo").current_dir(&core).args([
            "build",
            "-p",
            "type-bridge-c",
            "--features",
            "abi-1-5",
        ]),
        "ABI 1.5 shared library build",
    );
    let generated = temporary.path().join("generated/c");
    let executable = temporary.path().join("workforce-v4-c-consumer");
    let consumer = temporary.path().join("consumer.c");
    fs::write(&consumer, CONSUMER).unwrap();
    run(
        Command::new("cc")
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(generated.join("include"))
            .arg("-I")
            .arg(core.join("crates/c/include"))
            .arg(&consumer)
            .arg(generated.join("src/models.c"))
            .arg("-L")
            .arg(core.join("target/debug"))
            .arg("-ltype_bridge_c")
            .arg(format!(
                "-Wl,-rpath,{}",
                core.join("target/debug").display()
            ))
            .arg("-o")
            .arg(&executable),
        "strict C17 consumer compile",
    );

    let database = format!("workforce_v4_c_{}", std::process::id());
    let mut child = Command::new(&executable)
        .current_dir(&generated)
        .env("TYPE_BRIDGE_WORKFORCE_V4_DATABASE", &database)
        .envs(std::env::vars().filter(|(name, _)| name.starts_with("TYPEDB_")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("C producer starts");
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let fixture = fixture_runtime().await;
    let mut observation = None;
    loop {
        let mut line = String::new();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        match line.trim() {
            "TYPE_BRIDGE_C_FIXTURE seed" => {
                fixture_write(&fixture, &database, "insert\n  $first isa person, has person-id \"p1\", has legacy-name \"Ada\";\n  $second isa person, has person-id \"p2\", has legacy-name \"Bob\", has display-name \"conflict\";").await;
                writeln!(input, "0").unwrap();
                input.flush().unwrap();
            }
            "TYPE_BRIDGE_C_FIXTURE repair" => {
                let count = fixture_count(
                    &fixture,
                    &database,
                    "match $person isa person, has display-name $name; fetch { \"name\": $name };",
                )
                .await;
                fixture_write(&fixture, &database, "match $person isa person, has person-id \"p2\", has display-name $name; delete has $name of $person;").await;
                writeln!(input, "{count}").unwrap();
                input.flush().unwrap();
            }
            "TYPE_BRIDGE_C_FIXTURE equal" => {
                let count = fixture_count(&fixture, &database, "match $person isa person, has legacy-name $source, has display-name $destination; $source == $destination; fetch { \"name\": $destination };").await;
                writeln!(input, "{count}").unwrap();
                input.flush().unwrap();
            }
            "TYPE_BRIDGE_C_FIXTURE remaining" => {
                let count = fixture_count(
                    &fixture,
                    &database,
                    "match $person isa person, has display-name $name; fetch { \"name\": $name };",
                )
                .await;
                writeln!(input, "{count}").unwrap();
                input.flush().unwrap();
            }
            value if value.starts_with('{') => {
                observation =
                    Some(serde_json::from_str::<Value>(value).expect("C producer emits JSON"));
            }
            value => panic!("unexpected C producer output: {value}"),
        }
    }
    let status = child.wait().unwrap();
    let stderr = child
        .stderr
        .take()
        .map(|mut value| {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut value, &mut bytes).unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        })
        .unwrap_or_default();
    assert!(status.success(), "C producer failed: {stderr}");
    let observation = observation.expect("C observation exists");
    let report = json!({
        "format": "typebridge.sdk-conformance-report/v4", "binding": "c",
        "manifest": source_identity(&root, "tests/contracts/sdk_conformance/manifest-v1.json"),
        "catalog": source_identity(&root, "tests/contracts/sdk_conformance/workforce-v4/catalog-v4.json"),
        "journey": source_identity(&root, "tests/contracts/sdk_conformance/workforce-v4/journey-v4.json"),
        "server_version": "3.12.3",
        "results": [
            result("workforce.runtime.database-administration", "runtime.database-administration", "direct_runtime", observation["administration"].clone()),
            result("workforce.migration.rollback", "migration.reverse-cli", "direct_runtime", observation["rollback"].clone()),
            result("workforce.migration.backfill", "migration.binding-neutral-backfill", "direct_runtime", observation["backfill"].clone()),
            result("workforce.migration.runtime-facade", "migration.sdk-runtime-facade", "direct_runtime", observation["runtime_facade"].clone()),
            result("workforce.runtime.cancellation", "runtime.cancellation", "direct_runtime", observation["cancellation"].clone()),
            result("workforce.runtime.timeout-resource-limits", "runtime.timeout-and-resource-limits", "direct_runtime", observation["resource_limits"].clone()),
            result("workforce.diagnostic.all-workflows", "diagnostic.all-workflows-structured", "diagnostic", observation["diagnostic"].clone()),
            result("workforce.runtime.explicit-close", "runtime.explicit-close", "lifecycle", observation["lifecycle"].clone()),
        ], "cleanup": observation["cleanup"].clone(),
    });
    let report_path = temporary.path().join("c-workforce-v4-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    run(
        Command::new("python3").args(["-c", "import importlib.util,json,pathlib,sys; root=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('v4',root/'scripts/ci/compare_workforce_conformance_v4.py'); mod=importlib.util.module_from_spec(spec); sys.modules[spec.name]=mod; spec.loader.exec_module(mod); report=json.loads(pathlib.Path(sys.argv[2]).read_text()); contracts=mod.load_contracts(root); mod._validate_source_identity(report['manifest'],mod.MANIFEST_RELATIVE,root,'manifest'); mod._validate_source_identity(report['catalog'],mod.CATALOG_RELATIVE,root,'catalog'); mod._validate_source_identity(report['journey'],mod.JOURNEY_RELATIVE,root,'journey'); mod._validate_report(report,contracts.observation_refs)", root.to_str().unwrap(), report_path.to_str().unwrap()]),
        "V4 comparator",
    );
    assert_eq!(report["results"][2]["observation"]["forward_changed"], 2);
    assert_eq!(
        report["results"][3]["observation"]["catalog_fingerprint"],
        "b59eb4988620a941a7531432eb622d04fc0aafe0238dabf047056138c78ea99c"
    );
    if let Some(directory) = std::env::var_os("TYPE_BRIDGE_WORKFORCE_V4_REPORT_DIR") {
        fs::copy(
            &report_path,
            PathBuf::from(directory).join("c-workforce-v4-report.json"),
        )
        .expect("validated C report publishes to the fan-in directory");
    }
}
