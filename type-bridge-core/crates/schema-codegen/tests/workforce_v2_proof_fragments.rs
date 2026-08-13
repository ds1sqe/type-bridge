//! Fail-closed Rust validation for workforce-v2 deterministic proof fragments.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use type_bridge_contract::codec::to_canonical_json;

mod support;

const RUN_NONCE: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const PRODUCER_ID: &str = "rust.provider-recording";
const PRODUCER_SOURCE: &str = "tests/proofs/rust-provider.rs";
const DIRECT_TEST_ID: &str = "rust.query.cancellation";
const REMOTE_TEST_ID: &str = "rust.remote.cancellation";
const DIAGNOSTIC_TEST_ID: &str = "rust.remote.structured-diagnostic";

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "typebridge-workforce-v2-proof-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("proof test stage is created");
        Self(path)
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_contract(stage: &Stage, relative: &str, bytes: &[u8]) {
    let destination = stage.root().join(relative);
    fs::create_dir_all(destination.parent().expect("contract has a parent"))
        .expect("contract parent is created");
    fs::write(destination, bytes).expect("contract is staged");
}

fn authority() -> Stage {
    let stage = Stage::new();
    copy_contract(
        &stage,
        support::WORKFORCE_V2_PROOF_SCHEMA,
        include_bytes!(
            "../../../../tests/contracts/sdk_conformance/workforce-v2/proof-fragment-schema-v1.json"
        ),
    );
    copy_contract(
        &stage,
        support::WORKFORCE_V2_JOURNEY,
        include_bytes!("../../../../tests/contracts/sdk_conformance/workforce-v2/journey-v2.json"),
    );
    copy_contract(
        &stage,
        PRODUCER_SOURCE,
        b"// deterministic Rust proof source\n",
    );
    let allowlist = json!({
        "bindings": {
            "c": [{
                "id": PRODUCER_ID,
                "results": [
                    {"observation_ref": "cancellation_direct", "proof_kind": "direct_runtime", "test_id": DIRECT_TEST_ID},
                    {"observation_ref": "cancellation_remote", "proof_kind": "remote_runtime", "test_id": REMOTE_TEST_ID},
                    {"observation_ref": "remote_structured_diagnostic", "proof_kind": "diagnostic", "test_id": DIAGNOSTIC_TEST_ID},
                ],
                "sources": [PRODUCER_SOURCE],
            }],
            "node": [{
                "id": PRODUCER_ID,
                "results": [
                    {"observation_ref": "cancellation_direct", "proof_kind": "direct_runtime", "test_id": DIRECT_TEST_ID},
                    {"observation_ref": "cancellation_remote", "proof_kind": "remote_runtime", "test_id": REMOTE_TEST_ID},
                    {"observation_ref": "remote_structured_diagnostic", "proof_kind": "diagnostic", "test_id": DIAGNOSTIC_TEST_ID},
                ],
                "sources": [PRODUCER_SOURCE],
            }],
            "python": [{
                "id": PRODUCER_ID,
                "results": [
                    {"observation_ref": "cancellation_direct", "proof_kind": "direct_runtime", "test_id": DIRECT_TEST_ID},
                    {"observation_ref": "cancellation_remote", "proof_kind": "remote_runtime", "test_id": REMOTE_TEST_ID},
                    {"observation_ref": "remote_structured_diagnostic", "proof_kind": "diagnostic", "test_id": DIAGNOSTIC_TEST_ID},
                ],
                "sources": [PRODUCER_SOURCE],
            }],
            "rust": [{
                "id": PRODUCER_ID,
                "results": [
                    {"observation_ref": "cancellation_direct", "proof_kind": "direct_runtime", "test_id": DIRECT_TEST_ID},
                    {"observation_ref": "cancellation_remote", "proof_kind": "remote_runtime", "test_id": REMOTE_TEST_ID},
                    {"observation_ref": "remote_structured_diagnostic", "proof_kind": "diagnostic", "test_id": DIAGNOSTIC_TEST_ID},
                ],
                "sources": [PRODUCER_SOURCE],
            }],
        },
        "format": "typebridge.workforce-v2-proof-fragment-allowlist/v1",
        "semantic_profile": support::TEST_PROFILE,
    });
    let mut allowlist_bytes = to_canonical_json(&allowlist).expect("allowlist canonicalizes");
    allowlist_bytes.push(b'\n');
    copy_contract(
        &stage,
        support::WORKFORCE_V2_PROOF_ALLOWLIST,
        &allowlist_bytes,
    );
    stage
}

fn fragment(stage: &Stage) -> Value {
    json!({
        "binding": "rust",
        "contract": {
            "allowlist": support::workforce_v2_source_identity(
                stage.root(),
                support::WORKFORCE_V2_PROOF_ALLOWLIST,
            ).expect("allowlist identity"),
            "journey": support::workforce_v2_source_identity(
                stage.root(),
                support::WORKFORCE_V2_JOURNEY,
            ).expect("journey identity"),
            "proof_schema": support::workforce_v2_source_identity(
                stage.root(),
                support::WORKFORCE_V2_PROOF_SCHEMA,
            ).expect("proof schema identity"),
        },
        "format": "typebridge.workforce-v2-proof-fragment/v1",
        "producer": {
            "id": PRODUCER_ID,
            "sources": [support::workforce_v2_source_identity(stage.root(), PRODUCER_SOURCE)
                .expect("producer identity")],
        },
        "results": [
            {
                "observation": {
                    "in_flight": {"provider_await_woken": true},
                    "pre_dispatch": {"provider_calls": 0},
                },
                "observation_ref": "cancellation_direct",
                "outcome": "passed",
                "proof_kind": "direct_runtime",
                "test_id": DIRECT_TEST_ID,
            },
            {
                "observation": {
                    "before_exchange": {"exchange_count": 0},
                    "during_decode": {"exchange_count": 1},
                },
                "observation_ref": "cancellation_remote",
                "outcome": "passed",
                "proof_kind": "remote_runtime",
                "test_id": REMOTE_TEST_ID,
            },
            {
                "observation": {"claim_consumed": true, "redacted": true},
                "observation_ref": "remote_structured_diagnostic",
                "outcome": "passed",
                "proof_kind": "diagnostic",
                "test_id": DIAGNOSTIC_TEST_ID,
            },
        ],
        "run_nonce": RUN_NONCE,
        "semantic_profile": support::TEST_PROFILE,
    })
}

fn write_fragment(path: &Path, value: &Value) {
    let mut bytes = to_canonical_json(value).expect("proof fragment canonicalizes");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("proof fragment writes");
}

#[test]
fn shared_validator_accepts_only_the_exact_source_bound_three_lane_fragment() {
    let stage = authority();
    let path = stage.root().join("fragment.json");
    write_fragment(&path, &fragment(&stage));

    let observations = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect("exact fragment validates");

    assert_eq!(observations.len(), 3);
    assert_eq!(
        observations
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        support::workforce_v2_proof_lanes(stage.root(), "rust").expect("committed lanes validate")
    );
}

#[test]
fn shared_validator_rejects_wrong_run_source_drift_and_lane_coverage() {
    let stage = authority();
    let path = stage.root().join("fragment.json");
    write_fragment(&path, &fragment(&stage));

    let wrong_run = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        &"8".repeat(64),
        std::slice::from_ref(&path),
    )
    .expect_err("wrong-run fragment rejects");
    assert!(wrong_run.contains("another run"));

    fs::write(stage.root().join(PRODUCER_SOURCE), "// changed\n").expect("producer source drifts");
    let drift = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("source-drifted fragment rejects");
    assert!(drift.contains("digest does not match"));

    fs::write(
        stage.root().join(PRODUCER_SOURCE),
        "// deterministic Rust proof source\n",
    )
    .expect("producer source restores");

    let mut wrong_producer = fragment(&stage);
    wrong_producer["producer"]["id"] = json!("rust.uncommitted");
    write_fragment(&path, &wrong_producer);
    let wrong_producer = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("uncommitted producer rejects");
    assert!(wrong_producer.contains("is not committed"));

    let wrong_source_relative = "tests/proofs/rust-other-provider.rs";
    copy_contract(&stage, wrong_source_relative, b"// other source\n");
    let mut wrong_source = fragment(&stage);
    wrong_source["producer"]["sources"] =
        json!([
            support::workforce_v2_source_identity(stage.root(), wrong_source_relative)
                .expect("other producer identity")
        ]);
    write_fragment(&path, &wrong_source);
    let wrong_source = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("uncommitted producer source rejects");
    assert!(wrong_source.contains("source paths differ"));

    let mut wrong_test = fragment(&stage);
    wrong_test["results"][0]["test_id"] = json!("rust.query.wrong-test");
    write_fragment(&path, &wrong_test);
    let wrong_test = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("uncommitted test ID rejects");
    assert!(wrong_test.contains("wrong test ID"));

    let mut missing = fragment(&stage);
    missing["results"]
        .as_array_mut()
        .expect("results are an array")
        .pop();
    write_fragment(&path, &missing);
    let missing = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("missing lane rejects");
    assert!(missing.contains("did not emit its exact committed lanes"));
}

#[test]
fn shared_validator_rejects_noncanonical_and_duplicate_paths() {
    let stage = authority();
    let path = stage.root().join("fragment.json");
    let value = fragment(&stage);
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&value).expect("pretty JSON")
        ),
    )
    .expect("noncanonical fragment writes");
    let noncanonical = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&path),
    )
    .expect_err("noncanonical fragment rejects");
    assert!(noncanonical.contains("not compact canonical JSON"));

    write_fragment(&path, &value);
    let duplicate = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        &[path.clone(), path],
    )
    .expect_err("duplicate path rejects");
    assert!(duplicate.contains("supplied twice"));
}

#[cfg(unix)]
#[test]
fn shared_validator_rejects_fragment_symlinks_and_symlinked_parents() {
    use std::os::unix::fs::symlink;

    let stage = authority();
    let real_directory = stage.root().join("real-fragments");
    fs::create_dir(&real_directory).expect("real fragment directory is created");
    let real_fragment = real_directory.join("fragment.json");
    write_fragment(&real_fragment, &fragment(&stage));

    let linked_fragment = stage.root().join("fragment-link.json");
    symlink(&real_fragment, &linked_fragment).expect("fragment symlink is created");
    let rejected = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        std::slice::from_ref(&linked_fragment),
    )
    .expect_err("fragment symlink rejects");
    assert!(rejected.contains("traverses a symlink"));

    let linked_directory = stage.root().join("linked-fragments");
    symlink(&real_directory, &linked_directory).expect("fragment directory symlink is created");
    let rejected = support::validate_workforce_v2_proof_fragments(
        stage.root(),
        "rust",
        RUN_NONCE,
        &[linked_directory.join("fragment.json")],
    )
    .expect_err("symlinked fragment parent rejects");
    assert!(rejected.contains("traverses a symlink"));
}
