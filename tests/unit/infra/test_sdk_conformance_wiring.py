"""Hosted and local fan-in for the shared generated-SDK v1 reports."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / ".github/workflows/ci.yml"
LOCAL = ROOT / "test.sh"
UPLOAD = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
DOWNLOAD = "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"
SETUP_PYTHON = "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065"
RUNNER_OWNED = {
    "TYPE_BRIDGE_SDK_QUERIES_REPORT",
    "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
    "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS",
    "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
    "TYPE_BRIDGE_SDK_QUERIES_VALIDATED_OBSERVATIONS",
    "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON",
}


@pytest.mark.parametrize("invalid", [None, "python", "node", "rust", "c", "summary", "shape"])
def test_local_sha_step_validates_every_report_and_summary(tmp_path: Path, invalid: str | None):
    source = LOCAL.read_text(encoding="utf-8")
    helpers = source[
        source.index("validate_canonical_json_files() {") : source.index(
            "# ── TypeDB container lifecycle"
        )
    ]
    paths = []
    for name in ("python", "node", "rust", "c", "summary"):
        value = {"pending_manifest_promotions": []} if name == "summary" else {"binding": name}
        if invalid == "shape" and name == "summary":
            value = {}
        path = tmp_path / f"{name}.json"
        path.write_text(json.dumps(value, separators=(",", ":"), sort_keys=True) + "\n")
        if invalid == name:
            path.write_text(path.read_text() + " ")
        paths.append(path)
    # run_step invokes helpers in a conditional, where Bash disables errexit.
    probe = helpers + '\nif log_sdk_evidence_sha256 "$@"; then exit 0; else exit 1; fi\n'
    result = subprocess.run(
        ["bash", "-c", probe, "probe", sys.executable, *map(str, paths)],
        capture_output=True,
        text=True,
        check=False,
    )
    if invalid is not None:
        assert result.returncode != 0
        assert "pending_manifest_promotions" not in result.stdout
        assert all(
            hashlib.sha256(path.read_bytes()).hexdigest() not in result.stdout for path in paths
        )
    else:
        assert result.returncode == 0, result.stderr
        assert "queries pending_manifest_promotions=[]" in result.stdout
        for path in paths:
            assert f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}" in result.stdout


def _jobs() -> dict[str, Any]:
    return yaml.safe_load(CI.read_text(encoding="utf-8"))["jobs"]


def _steps(job: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {step["name"]: step for step in job["steps"]}


def _step_index(job: dict[str, Any], name: str) -> int:
    return next(index for index, step in enumerate(job["steps"]) if step["name"] == name)


def test_v2_exact_producer_bound_fragments_precede_each_live_report() -> None:
    jobs = _jobs()
    python_remote = next(
        step
        for step in jobs["test-integration"]["steps"]
        if step["name"] == "Emit Python generated-remote queries proof fragment"
    )["run"]
    assert python_remote.index("uv pip install maturin==1.14.1") < python_remote.index(
        "uv run python type-bridge-core/crates/schema-codegen/tests/acceptance/check.py"
    )
    contracts = {
        "python": {
            "job": "test-integration",
            "condition": (
                "matrix.test-group == 'schema' && matrix.typedb-server == 'typedb/typedb:3.12.3'"
            ),
            "prepare": "Prepare Python queries evidence outputs",
            "prepare_id": "prepare-python-queries",
            "emitters": {
                "Emit Python direct-cancellation queries proof fragment": (
                    "direct_fragment",
                    "match_runtime::tests::"
                    "python_direct_cancellation_fragment_is_measured_from_owned_execution",
                ),
                "Emit Python generated-remote queries proof fragment": (
                    "remote_fragment",
                    "schema-codegen/tests/acceptance/check.py",
                ),
            },
            "producer": "Run integration tests for ${{ matrix.test-group }}",
            "upload": "Upload Python queries report",
        },
        "rust": {
            "job": "rust-integration",
            "condition": "matrix.typedb-server == 'typedb/typedb:3.12.3'",
            "prepare": "Prepare Rust queries evidence outputs",
            "prepare_id": "prepare-rust-queries",
            "emitters": {
                "Emit Rust queries deterministic proof fragment": (
                    "fragment",
                    "remote::tests::sdk_queries_rust_deterministic_proof_fragment",
                ),
            },
            "producer": "Run generated Rust projection live smoke",
            "upload": "Upload Rust queries report",
        },
        "c": {
            "job": "rust-integration",
            "condition": "matrix.typedb-server == 'typedb/typedb:3.12.3'",
            "prepare": "Prepare C queries evidence outputs",
            "prepare_id": "prepare-c-queries",
            "emitters": {
                "Emit C queries deterministic proof fragment": (
                    "fragment",
                    "query::tests::sdk_queries_c_deterministic_proof_fragment",
                ),
            },
            "producer": "Run generated C entity and relation CRUD live smoke",
            "upload": "Upload C queries report",
        },
        "node": {
            "job": "node-integration",
            "condition": "matrix.typedb-server == 'typedb/typedb:3.12.3'",
            "prepare": "Prepare Node queries evidence outputs",
            "prepare_id": "prepare-node-queries",
            "emitters": {
                "Emit Node direct-cancellation queries proof fragment": (
                    "direct_fragment",
                    "match_runtime::tests::"
                    "node_direct_cancellation_fragment_is_measured_from_owned_execution",
                ),
                "Emit Node generated-remote queries proof fragment": (
                    "remote_fragment",
                    "schema-codegen/tests/typescript_acceptance/check.mjs",
                ),
            },
            "producer": "Run generated projection live smoke",
            "upload": "Upload Node queries report",
        },
    }

    for binding, contract in contracts.items():
        job = jobs[contract["job"]]
        steps = _steps(job)
        prepare = steps[contract["prepare"]]
        prepare_id = contract["prepare_id"]
        condition = contract["condition"]
        producer = steps[contract["producer"]]
        upload = steps[contract["upload"]]

        assert prepare["id"] == prepare_id
        assert prepare["if"] == condition
        assert prepare["run"].count("secrets.token_hex(32)") == 1
        assert "^[0-9a-f]{64}$" in prepare["run"]
        assert f'"$evidence_dir/{binding}.json"' in prepare["run"]
        assert "$GITHUB_OUTPUT" in prepare["run"]
        assert "$GITHUB_ENV" not in prepare["run"]

        for emitter_name, (fragment_output, command_fragment) in contract["emitters"].items():
            emitter = steps[emitter_name]
            assert emitter["if"] == condition
            assert emitter["env"] == {
                "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT": (
                    f"${{{{ steps.{prepare_id}.outputs.{fragment_output} }}}}"
                ),
                "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE": (
                    f"${{{{ steps.{prepare_id}.outputs.run_nonce }}}}"
                ),
            }
            assert command_fragment in emitter["run"]
            assert _step_index(job, emitter_name) < _step_index(job, contract["producer"])

        if binding == "c":
            assert producer["env"]["TYPE_BRIDGE_SDK_QUERIES_REPORT"] == (
                "${{ steps.prepare-c-queries.outputs.report }}"
            )
            assert producer["env"]["TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS"] == (
                "${{ steps.prepare-c-queries.outputs.fragment }}"
            )
            assert producer["env"]["TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE"] == (
                "${{ steps.prepare-c-queries.outputs.run_nonce }}"
            )
        else:
            assert producer["env"]["SDK_QUERIES_REPORT_OUTPUT"] == (
                f"${{{{ steps.{prepare_id}.outputs.report }}}}"
            )
            assert "TYPE_BRIDGE_SDK_QUERIES_REPORT=$SDK_QUERIES_REPORT_OUTPUT" in (producer["run"])
            assert "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS=" in producer["run"]
            assert "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE=" in producer["run"]

        assert upload["uses"] == UPLOAD
        assert upload["if"] == condition
        assert upload["with"] == {
            "name": f"sdk-queries-{binding}",
            "path": f"${{{{ runner.temp }}}}/typebridge-sdk-queries/{binding}.json",
            "if-no-files-found": "error",
            "retention-days": 7,
        }

    rust_prepare = _steps(jobs["rust-integration"])["Prepare C queries evidence outputs"]["run"]
    assert "steps.prepare-rust-queries.outputs.run_nonce" in rust_prepare


def test_node_v2_validator_is_absolute_and_visible_only_to_the_live_producer() -> None:
    job = _jobs()["node-integration"]
    steps = _steps(job)
    setup = steps["Set up Python for the Node queries validator"]
    prepare = steps["Prepare Node queries evidence outputs"]
    producer = steps["Run generated projection live smoke"]

    assert setup["uses"] == SETUP_PYTHON
    assert setup["if"] == "matrix.typedb-server == 'typedb/typedb:3.12.3'"
    assert "os.path.realpath(sys.executable)" in prepare["run"]
    assert '[[ "$validator_python" == /* ]]' in prepare["run"]
    assert 'test -x "$validator_python"' in prepare["run"]
    assert producer["env"]["SDK_QUERIES_VALIDATOR_PYTHON_OUTPUT"] == (
        "${{ steps.prepare-node-queries.outputs.validator_python }}"
    )
    assert (
        "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON=$SDK_QUERIES_VALIDATOR_PYTHON_OUTPUT"
    ) in producer["run"]
    assert all(
        "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON" not in step.get("env", {})
        for step in job["steps"]
    )


def test_runner_owned_report_and_proof_inputs_never_persist_between_ci_steps() -> None:
    jobs = _jobs()
    workflow_source = CI.read_text(encoding="utf-8")
    for variable in RUNNER_OWNED:
        assert f"{variable}=%s" not in workflow_source
        assert f"{variable}=" not in "\n".join(
            line for line in workflow_source.splitlines() if "GITHUB_ENV" in line
        )

    allowed_step_envs = {
        "Emit Python direct-cancellation queries proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Emit Python generated-remote queries proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Emit Rust queries deterministic proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Emit C queries deterministic proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Run generated C entity and relation CRUD live smoke": {
            "TYPE_BRIDGE_SDK_QUERIES_REPORT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Emit Node direct-cancellation queries proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
        "Emit Node generated-remote queries proof fragment": {
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        },
    }
    observed: dict[str, set[str]] = {}
    for job in jobs.values():
        assert not (set(job.get("env", {})) & RUNNER_OWNED)
        for step in job.get("steps", []):
            runner_env = set(step.get("env", {})) & RUNNER_OWNED
            if runner_env:
                observed[step["name"]] = runner_env
    assert observed == allowed_step_envs


def test_hosted_queries_fan_in_is_provider_free_and_four_binding() -> None:
    jobs = _jobs()
    needs = ["test-integration", "rust-integration", "node-integration"]

    v2 = jobs["sdk-queries"]
    assert v2["needs"] == needs
    assert "services" not in v2
    v2_steps = _steps(v2)
    v2_downloads = [step for step in v2["steps"] if step.get("uses") == DOWNLOAD]
    assert len(v2_downloads) == 4
    for display, binding in (
        ("Python", "python"),
        ("Node", "node"),
        ("Rust", "rust"),
        ("C", "c"),
    ):
        download = v2_steps[f"Download {display} queries report"]
        assert download["with"] == {
            "name": f"sdk-queries-{binding}",
            "path": "${{ runner.temp }}/typebridge-sdk-queries",
        }
    v2_compare = v2_steps["Compare generated SDK queries reports"]["run"]
    assert v2_compare.startswith("python scripts/ci/sdk_queries_compare.py ")
    assert v2_compare.count("$RUNNER_TEMP/typebridge-sdk-queries/") == 4
    assert all(f'/{binding}.json"' in v2_compare for binding in ("python", "node", "rust", "c"))
    assert "pending_manifest_promotions" not in str(v2)
    assert "sdk_fingerprints" not in str(v2)
    assert "TYPE_BRIDGE_SDK_FINGERPRINTS_OUTPUT" not in str(v2)


def test_local_full_suite_emits_fragments_then_compares_queries() -> None:
    source = LOCAL.read_text(encoding="utf-8")

    assert "urllib.request.urlopen(" in source
    assert 'f"http://127.0.0.1:{sys.argv[1]}/v1/version"' in source
    assert "TypeDB gRPC and HTTP endpoints did not become ready in time" in source

    for variable in {
        "TYPE_BRIDGE_SDK_QUERIES_REPORT",
        "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT",
        "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS",
        "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE",
        "TYPE_BRIDGE_SDK_QUERIES_VALIDATED_OBSERVATIONS",
        "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON",
    }:
        assert variable in source
    assert "${!runner_owned_sdk_variable+x}" in source
    assert "is runner-owned; unset it before invoking test.sh" in source
    assert 'if [[ "$lane_in_sdk_matrix" == 1 && ${#pytest_args[@]} -eq 0 ]]' in source
    # Producers report under the lane's profile, never a frozen literal.
    assert source.count('"TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=$lane_profile"') == 2
    assert "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=typedb-" not in source
    assert "forwarded pytest arguments may change collection" in source
    assert 'mktemp -d "${TMPDIR:-/tmp}/typebridge-sdk.XXXXXXXXXX"' in source
    assert 'sdk_report_dir="$(cd "$sdk_report_dir" && pwd -P)"' in source
    assert 'mkdir -p "$sdk_report_dir/queries"' in source
    assert source.count("secrets.token_hex(32)") == 4
    assert "declare -A sdk_nonce_set=()" in source
    assert "SDK queries binding nonces must be distinct" in source

    for binding in ("python", "node", "rust", "c"):
        assert f"TYPE_BRIDGE_SDK_QUERIES_REPORT=$sdk_report_dir/queries/{binding}.json" in source
        assert f'"$sdk_report_dir/queries/{binding}.json"' in source

    emitter_ids = (
        "match_runtime::tests::"
        "python_direct_cancellation_fragment_is_measured_from_owned_execution",
        "schema-codegen/tests/acceptance/check.py",
        "match_runtime::tests::node_direct_cancellation_fragment_is_measured_from_owned_execution",
        "schema-codegen/tests/typescript_acceptance/check.mjs",
        "remote::tests::sdk_queries_rust_deterministic_proof_fragment",
        "query::tests::sdk_queries_c_deterministic_proof_fragment",
    )
    for emitter_id in emitter_ids:
        assert emitter_id in source
    assert source.index("emit Rust queries deterministic proof fragment") < source.index(
        'run_step "generated Rust application parity"'
    )
    assert source.index("emit C queries deterministic proof fragment") < source.index(
        'run_step "compiled generated C17 Person and Membership CRUD lifecycle"'
    )
    assert source.index("emit Python generated-remote queries proof fragment") < source.index(
        'run_step "pytest -m integration"'
    )
    assert source.index("emit Node generated-remote queries proof fragment") < source.index(
        'run_step "npm run test:projection-integration"'
    )
    assert "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON=$sdk_validator_python" in source
    assert "scripts/ci/sdk_queries_compare.py" in source
    assert 'sdk_queries_summary="$sdk_report_dir/queries-comparison.json"' in source
    assert source.count("write_sdk_summary \\") == 1
    assert 'mktemp "${summary}.tmp.XXXXXX"' in source
    assert 'ln -- "$staged" "$summary"' in source
    assert source.count('unlink -- "$staged"') == 4
    assert "validate_canonical_json_files" in source
    assert "empty JSON evidence" in source
    assert "noncanonical JSON evidence" in source
    assert "queries pending_manifest_promotions=" in source
    assert "log_sdk_evidence_sha256" in source
    assert 'sha256sum -- "$@"' in source
    hash_step = source.index('run_step "validate and log sdk report and summary SHA-256 evidence"')
    # Each lane removes or preserves its own evidence once its steps ran.
    cleanup = source.index("finish_sdk_evidence() {")
    assert hash_step < source.index("    finish_sdk_evidence\n") < cleanup
    assert "((fail == lane_fail_start))" in source[cleanup:]
    for report in (
        "queries/python.json",
        "queries/node.json",
        "queries/rust.json",
        "queries/c.json",
    ):
        assert f'"$sdk_report_dir/{report}"' in source[hash_step:cleanup]
    assert '"$sdk_queries_summary"' in source[hash_step:cleanup]
    assert "Preserved sdk reports for diagnosis" in source
    assert "Removed accepted sdk reports" in source
    assert 'preserve_sdk_evidence="${TYPE_BRIDGE_PRESERVE_SDK_EVIDENCE:-0}"' in source
    assert '[[ "$preserve_sdk_evidence" == 0 ]]' in source
    assert "Preserved accepted sdk evidence by request" in source
    for summary in ("queries-comparison.json",):
        assert f'"$sdk_report_dir/{summary}"' in source[cleanup:]
    for fragment in (
        "python-direct-proof.json",
        "python-remote-proof.json",
        "node-direct-proof.json",
        "node-remote-proof.json",
        "rust-proof.json",
        "c-proof.json",
    ):
        assert f'"$sdk_report_dir/queries/{fragment}"' in source


def test_local_3_13_6_lane_runs_the_writes_program_live_in_four_languages() -> None:
    source = LOCAL.read_text(encoding="utf-8")
    lane = source[
        source.index('if [[ "$typedb_server_version" == "3.13.6" ]]; then') : source.index(
            'run_step "cargo test -p type-bridge-orm --features integration-tests --test integration"'
        )
    ]
    step = lane[lane.index('run_step "SDK writes: one staged program in four languages') :]
    step = step[: step.index("\n    fi\n")]
    assert "scripts/ci/run_sdk_conformance.py writes" in step
    assert '--semantic-profile "$lane_profile"' in step
    assert '--address "$TYPEDB_ADDRESS" --http-port "$TYPEDB_HTTP_PORT"' in step
    assert "--offline" not in step
    assert 'mktemp -d "${TMPDIR:-/tmp}/typebridge-sdk-writes.XXXXXXXXXX"' in lane
    assert 'rm -rf -- "$writes_output"' in step


def test_hosted_3_13_6_job_runs_the_writes_program_live_in_four_languages() -> None:
    job = _jobs()["live-3-13-6"]
    steps = _steps(job)
    command = " ".join(steps["Run SDK writes in four languages"]["run"].split())
    assert command.startswith("uv run python scripts/ci/run_sdk_conformance.py writes ")
    assert "--semantic-profile typedb-3.13.6/v1" in command
    assert "--address localhost:1729 --http-port 8000" in command
    assert '--output "$RUNNER_TEMP/typebridge-sdk-writes"' in command
    assert "--offline" not in command
    # The runner builds the Node package, compiles C17 and resolves the Rust
    # program offline: each needs its toolchain or graph staged first.
    writes = _step_index(job, "Run SDK writes in four languages")
    for prerequisite in (
        "Fetch the locked Rust graph",
        "Install the source-tree Python product",
        "Install Node dependencies",
        "Wait for TypeDB 3.13.6",
    ):
        assert _step_index(job, prerequisite) < writes
    upload = steps["Upload SDK writes reports"]
    assert upload["uses"] == UPLOAD
    assert upload["with"] == {
        "name": "sdk-writes-typedb-3.13.6",
        "path": "${{ runner.temp }}/typebridge-sdk-writes",
        "if-no-files-found": "error",
        "retention-days": 7,
    }


def test_writes_suite_is_listed_and_its_producers_exist() -> None:
    readme = (ROOT / "tests/contracts/sdk_conformance/README.md").read_text(encoding="utf-8")
    assert "| `writes` |" in readme and "`sdk_writes_compare.py`" in readme
    runner = (ROOT / "scripts/ci/run_sdk_conformance.py").read_text(encoding="utf-8")
    fixture = "sdk_writes_fixture_serves_until_stopped"
    assert fixture in runner
    server = ROOT / "type-bridge-core/crates/server/tests/v3_staged_integration_tests.rs"
    assert f"async fn {fixture}()" in server.read_text(encoding="utf-8")
    for program in ("python/program.py", "ts/program.ts", "rust/main.rs", "c/program.c"):
        assert (ROOT / "examples/purchase" / program).is_file()
        assert program in runner
