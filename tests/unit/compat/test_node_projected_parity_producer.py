"""Fail-closed tests for Node's provider-free Projected report producer."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PRODUCER_PATH = (
    ROOT
    / "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance"
    / "projected_parity_check.mjs"
)
ACCEPTANCE_PATH = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs"
)


def _node(source: str, *arguments: str, expected: int = 0) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        [
            "node",
            "--input-type=module",
            "--eval",
            source,
            PRODUCER_PATH.as_uri(),
            *arguments,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == expected, (
        f"Node exited {completed.returncode}, expected {expected}\n"
        f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
    )
    return completed


def test_publisher_is_canonical_create_new_and_bounded(tmp_path: Path) -> None:
    output = tmp_path / "node.json"
    publish = """
const producer = await import(process.argv[1]);
producer.publishReport(process.argv[2], {
  binding: "node",
  observed: { value: 38 },
});
"""
    _node(publish, str(output))
    assert output.read_bytes() == b'{"binding":"node","observed":{"value":38}}\n'

    reject_existing = """
const producer = await import(process.argv[1]);
try {
  producer.publishReport(process.argv[2], {});
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(reject_existing, str(output), expected=23)
    assert completed.stdout == "output_exists"

    oversized = """
const producer = await import(process.argv[1]);
try {
  producer.publishReport(process.argv[2], {
    value: "x".repeat(producer.MAX_REPORT_BYTES),
  });
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(oversized, str(tmp_path / "oversized.json"), expected=23)
    assert completed.stdout == "report_size_limit"


def test_paths_and_authority_files_reject_symlinks(tmp_path: Path) -> None:
    relative = """
const producer = await import(process.argv[1]);
try {
  producer.outputPath({ [producer.OUTPUT_ENV]: "relative.json" });
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(relative, expected=23)
    assert completed.stdout == "invalid_output_path"

    real_parent = tmp_path / "real"
    real_parent.mkdir()
    linked_parent = tmp_path / "linked"
    linked_parent.symlink_to(real_parent, target_is_directory=True)
    reject_parent = """
const producer = await import(process.argv[1]);
try {
  producer.publishReport(process.argv[2], {});
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(reject_parent, str(linked_parent / "report.json"), expected=23)
    assert completed.stdout == "invalid_output_parent"

    authority = tmp_path / "authority.json"
    authority.write_text("{}")
    linked_authority = tmp_path / "linked-authority.json"
    linked_authority.symlink_to(authority)
    reject_authority = """
const producer = await import(process.argv[1]);
try {
  producer.sourceIdentity(process.argv[2], "linked-authority.json", "linked authority");
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(reject_authority, str(tmp_path), expected=23)
    assert completed.stdout == "invalid_authority"


def test_acceptance_runs_real_producer_then_committed_comparator() -> None:
    acceptance = ACCEPTANCE_PATH.read_text()
    producer = PRODUCER_PATH.read_text()

    producer_call = 'resolve(STAGE, "projected_parity_check.mjs")'
    comparator_call = 'resolve(ROOT, "scripts/ci/compare_projected_parity.py")'
    assert acceptance.index(producer_call) < acceptance.index(comparator_call)
    assert "module._load_report" in acceptance
    assert "expected_report" not in producer
    assert "compare_projected_parity" not in producer
    assert "journey-v3.json" in producer
    assert "RemoteQuerySession" in producer
    assert "remoteSignedReply" in producer
    assert ".sign(null, digest, SIGNING_PRIVATE_KEY)" in producer
    assert "generated_projected_foreign" in producer
    assert "batch" not in producer.split("token_package_fencing:", maxsplit=1)[1]
    assert "filter" not in producer.split("token_package_fencing:", maxsplit=1)[1]


def test_producer_does_not_require_generated_packages_for_publication_helpers() -> None:
    environment = os.environ.copy()
    environment.pop("TYPE_BRIDGE_PROJECTED_PARITY_REPORT", None)
    source = """
const producer = await import(process.argv[1]);
process.stdout.write(`${producer.REPORT_FORMAT}\n${producer.SEMANTIC_PROFILE}`);
"""
    completed = subprocess.run(
        [
            "node",
            "--input-type=module",
            "--eval",
            source,
            PRODUCER_PATH.as_uri(),
        ],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout == ("typebridge.projected-parity-report/v1\ntypedb-3.12.1/v1")
