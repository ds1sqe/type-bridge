"""Fail-closed tests for Sdk V5 producer-evidence assembly."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import sys
from collections.abc import Callable
from copy import deepcopy
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_v5_assembler", CI / "assemble_sdk_conformance_v5.py"
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
        "record_b64": [base64.b64encode(f"record-{index}".encode()).decode() for index in range(9)],
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
        hashlib.sha256(f"record-{index}".encode()).hexdigest() for index in range(9)
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


def composition_write(path: Path, value: dict[str, Any]) -> Path:
    path.write_bytes(assembler.conformance.canonical_json_bytes(value))
    return path


def composition_inputs(tmp_path: Path, binding: str = "rust") -> dict[str, Path]:
    corpus = {
        "archive_b64": base64.b64encode(b"archive").decode(),
        "binding": binding,
        "format": assembler.corpora.FORMAT,
        "record_b64": [base64.b64encode(f"record-{index}".encode()).decode() for index in range(9)],
    }
    controls = {
        "binding": binding,
        **deepcopy(assembler.operational.EXPECTED),
        "format": assembler.operational.FORMAT,
        "test_id": assembler.operational.TEST_IDS[binding],
    }
    live = {
        "binding": binding,
        "detached_mutation_code": "projected_snapshot_detached",
        "direct_remote_equal": True,
        "entity_snapshot_b64": base64.b64encode(b"entity").decode(),
        "format": assembler.LIVE_FORMAT,
        "relation_snapshot_b64": base64.b64encode(b"relation").decode(),
        "remote_exchange_count": 1,
        "rebound_mutation": True,
    }
    cleanup = {
        "binding": binding,
        "format": assembler.CLEANUP_FORMAT,
        "managed_database_absent": True,
        "partial_output_absent": True,
        "temporary_evidence_absent": True,
    }
    return {
        "corpus_path": composition_write(tmp_path / "corpus.json", corpus),
        "operational_path": composition_write(tmp_path / "operational.json", controls),
        "live_path": composition_write(tmp_path / "live.json", live),
        "cleanup_path": composition_write(tmp_path / "cleanup.json", cleanup),
    }


def test_composition_composes_exact_assembler_input_from_measured_artifacts(tmp_path: Path) -> None:
    value = assembler.compose(
        binding="rust", run_nonce="a" * 64, root=ROOT, **composition_inputs(tmp_path)
    )
    report = assembler.assemble_report(value, binding="rust", run_nonce="a" * 64, root=ROOT)
    assert value["record_b64"][0] == base64.b64encode(b"record-0").decode()
    assert report["results"][1]["observation"] == assembler.operational.EXPECTED["cancellation"]


@pytest.mark.parametrize(
    ("filename", "mutation", "code"),
    [
        (
            "live.json",
            lambda value: value.update({"remote_exchange_count": 2}),
            "live_parity_failure",
        ),
        (
            "cleanup.json",
            lambda value: value.update({"managed_database_absent": False}),
            "cleanup_invariant_failed",
        ),
        (
            "operational.json",
            lambda value: value["diagnostic"].update({"payload_absent": False}),
            "operational_observation_drift",
        ),
    ],
)
def test_composition_rejects_unmeasured_or_failed_inputs(
    tmp_path: Path, filename: str, mutation: Callable[[dict[str, Any]], None], code: str
) -> None:
    arguments = composition_inputs(tmp_path)
    path = tmp_path / filename
    value = json.loads(path.read_bytes())
    mutation(value)
    composition_write(path, value)
    with pytest.raises(assembler.CompositionError) as rejected:
        assembler.compose(binding="rust", run_nonce="a" * 64, root=ROOT, **arguments)
    assert rejected.value.code == code


@pytest.mark.parametrize("binding", ["python", "node", "rust", "c"])
def test_combined_cli_preserves_report_bytes(tmp_path: Path, binding: str) -> None:
    import subprocess

    paths = composition_inputs(tmp_path, binding)
    output = tmp_path / "report.json"
    expected = assembler.assemble_report(
        assembler.compose(binding=binding, run_nonce="a" * 64, **paths),
        binding=binding,
        run_nonce="a" * 64,
    )
    command = [
        sys.executable,
        str(CI / "assemble_sdk_conformance_v5.py"),
        "--binding",
        binding,
        "--run-nonce",
        "a" * 64,
        "--output",
        str(output),
    ]
    for option, key in (
        ("corpus", "corpus_path"),
        ("live-evidence", "live_path"),
        ("operational-evidence", "operational_path"),
        ("cleanup-evidence", "cleanup_path"),
    ):
        command.extend([f"--{option}", str(paths[key])])
    subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True)
    assert output.read_bytes() == assembler.conformance.canonical_json_bytes(expected)
    output.unlink()
    live = json.loads(paths["live_path"].read_bytes())
    live["remote_exchange_count"] = 2
    composition_write(paths["live_path"], live)
    rejected = subprocess.run(command, cwd=ROOT, check=False, capture_output=True, text=True)
    assert rejected.returncode == 1 and "live_parity_failure" in rejected.stderr
    assert not output.exists()


def test_combined_composition_retains_evidence_size_limit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import argparse

    paths = composition_inputs(tmp_path)
    args = argparse.Namespace(
        binding="rust",
        run_nonce="a" * 64,
        corpus=paths["corpus_path"],
        live_evidence=paths["live_path"],
        operational_evidence=paths["operational_path"],
        cleanup_evidence=paths["cleanup_path"],
    )
    monkeypatch.setattr(assembler, "MAX_EVIDENCE_BYTES", 1)
    with pytest.raises(assembler.AssemblyError) as rejected:
        assembler.compose_inputs(args)
    assert rejected.value.code == "evidence_size_limit"
