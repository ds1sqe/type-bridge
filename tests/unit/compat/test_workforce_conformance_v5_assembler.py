"""Fail-closed tests for Workforce V5 producer-evidence assembly."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "workforce_v5_assembler", CI / "assemble_workforce_conformance_v5.py"
)
assert SPEC is not None and SPEC.loader is not None
assembler = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = assembler
SPEC.loader.exec_module(assembler)


def _evidence(binding: str = "rust") -> dict[str, Any]:
    producer = assembler.conformance.load_contracts(ROOT).catalog["report_producers"][binding]
    return {
        "format": assembler.FORMAT,
        "binding": binding,
        "producer": producer["id"],
        "test_id": producer["test_id"],
        "run_nonce": "a" * 64,
        "record_b64": [base64.b64encode(f"record-{index}".encode()).decode() for index in range(7)],
        "archive_b64": base64.b64encode(b"archive").decode(),
        "round_trip": True,
        "foreign_schema_code": "projected_record_schema_mismatch",
        "direct_remote_equal": True,
        "detached_mutation_code": "projected_snapshot_detached",
        "serialization_cancellation": {
            "code": "projected_codec_cancelled",
            "partial_output": False,
        },
        "serialization_resource_limits": {
            "input_code": "projected_codec_input_limit",
            "output_code": "projected_codec_output_limit",
            "member_code": "projected_codec_member_limit",
            "depth_code": "projected_codec_depth_limit",
            "partial_output": False,
        },
        "serialization_structured_diagnostic": {
            "code": "projected_record_schema_mismatch",
            "category": "invalid_input",
            "path": ["declared_schema_identity"],
            "payload_absent": True,
        },
        "serialization_resource_lifecycle": {
            "bytes_closed": True,
            "builder_closed": True,
            "archive_closed": True,
            "decoded_closed": True,
            "repeat_close": True,
            "sibling_usable": True,
        },
        "cleanup": {
            "managed_database_absent": True,
            "temporary_evidence_absent": True,
            "partial_output_absent": True,
        },
    }


def test_assembler_computes_digests_and_binds_exact_report() -> None:
    evidence = _evidence()

    report = assembler.assemble_report(evidence, binding="rust", run_nonce="a" * 64, root=ROOT)

    observation = report["results"][0]["observation"]
    assert observation["record_sha256"] == [
        hashlib.sha256(f"record-{index}".encode()).hexdigest() for index in range(7)
    ]
    assert observation["archive_sha256"] == hashlib.sha256(b"archive").hexdigest()
    assembler.conformance.validate_report(report, assembler.conformance.load_contracts(ROOT), ROOT)


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda value: value.__setitem__("run_nonce", "b" * 64), "run_nonce_mismatch"),
        (lambda value: value.__setitem__("producer", "foreign"), "producer_identity_mismatch"),
        (lambda value: value["record_b64"].__setitem__(0, "not base64"), "invalid_evidence_bytes"),
    ],
)
def test_assembler_rejects_unbound_evidence(
    mutation: Callable[[dict[str, Any]], None], code: str
) -> None:
    evidence = _evidence()
    mutation(evidence)

    with pytest.raises(assembler.AssemblyError) as raised:
        assembler.assemble_report(evidence, binding="rust", run_nonce="a" * 64, root=ROOT)
    assert raised.value.code == code


def test_publication_is_canonical_and_create_new(tmp_path: Path) -> None:
    report = assembler.assemble_report(_evidence(), binding="rust", run_nonce="a" * 64, root=ROOT)
    output = tmp_path / "rust.json"

    assembler.publish(output, report)

    assert output.read_bytes() == assembler.conformance.canonical_json_bytes(report)
    with pytest.raises(assembler.AssemblyError) as raised:
        assembler.publish(output, report)
    assert raised.value.code == "report_publication_failed"
