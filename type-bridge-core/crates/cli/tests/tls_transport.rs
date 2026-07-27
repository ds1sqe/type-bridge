//! TLS manifest failures must be deterministic and precede credentials or I/O.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const USERNAME_ENV: &str = "TYPEBRIDGE_TLS_TEST_USERNAME_MUST_NOT_EXIST";
const PASSWORD_ENV: &str = "TYPEBRIDGE_TLS_TEST_PASSWORD_MUST_NOT_EXIST";

fn write_workspace(root: &Path, transport: &str) {
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::create_dir_all(root.join("certs")).expect("certificate directory");
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
    fs::write(
        root.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\n\
             schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: tls-test\n\
             compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
             migrations:\n  directory: migrations/v2\n  app-label: tlstest\n\
             environments:\n  checked:\n    database: tls_test\n    uri: never-contact.invalid:1729\n    \
             migrate: 'true'\n    credential:\n      username: env:{USERNAME_ENV}\n      password: \
             env:{PASSWORD_ENV}\n{transport}",
        ),
    )
    .expect("manifest writes");
}

fn run_verify(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(root)
        .env_remove(USERNAME_ENV)
        .env_remove(PASSWORD_ENV)
        .args(["migration", "verify", "--environment", "checked"])
        .output()
        .expect("the type-bridge binary runs")
}

fn assert_pre_provider_failure(output: &Output, expected: &str) {
    assert!(!output.status.success(), "command must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(expected), "stderr: {stderr}");
    assert!(
        !stderr.contains("credential environment variable"),
        "transport policy must fail before credential resolution; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("never-contact.invalid"),
        "transport policy must fail before provider construction; stderr: {stderr}"
    );
}

#[test]
fn custom_root_without_explicit_tls_is_rejected_before_credentials_or_provider() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(workspace.path(), "    tls-root-ca: certs/root.pem\n");
    fs::write(workspace.path().join("certs/root.pem"), b"nonempty\n").expect("root CA writes");

    assert_pre_provider_failure(
        &run_verify(workspace.path()),
        "environments.tls-root-ca requires explicit tls: true",
    );
}

#[test]
fn custom_root_with_disabled_tls_is_rejected_before_credentials_or_provider() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(
        workspace.path(),
        "    tls: 'false'\n    tls-root-ca: certs/root.pem\n",
    );
    fs::write(workspace.path().join("certs/root.pem"), b"nonempty\n").expect("root CA writes");

    assert_pre_provider_failure(
        &run_verify(workspace.path()),
        "environments.tls-root-ca contradicts tls: false",
    );
}

#[test]
fn missing_custom_root_is_rejected_before_credentials_or_provider() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(
        workspace.path(),
        "    tls: 'true'\n    tls-root-ca: certs/missing.pem\n",
    );

    assert_pre_provider_failure(
        &run_verify(workspace.path()),
        "custom root CA path cannot be canonicalized",
    );
}

#[test]
fn malformed_pem_is_rejected_before_credentials_or_provider() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(
        workspace.path(),
        "    tls: 'true'\n    tls-root-ca: certs/root.pem\n",
    );
    fs::write(
        workspace.path().join("certs/root.pem"),
        b"this is not a PEM certificate\n",
    )
    .expect("root CA writes");

    assert_pre_provider_failure(
        &run_verify(workspace.path()),
        "tls_custom_root_ca_invalid_pem",
    );
}

#[test]
fn oversized_pem_is_rejected_before_credentials_or_provider() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    write_workspace(
        workspace.path(),
        "    tls: 'true'\n    tls-root-ca: certs/root.pem\n",
    );
    let root =
        fs::File::create(workspace.path().join("certs/root.pem")).expect("root CA fixture creates");
    root.set_len(1024 * 1024 + 1)
        .expect("root CA fixture extends");
    // Close the writable fixture handle before spawning the CLI: the
    // capture opens the root CA denying concurrent writers, so a live
    // writer handle on Windows is a sharing violation, not a size failure.
    drop(root);

    assert_pre_provider_failure(
        &run_verify(workspace.path()),
        "tls_custom_root_ca_too_large",
    );
}
