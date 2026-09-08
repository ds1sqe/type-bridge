"""Fail-closed tests for Sdk V5 measured-evidence composition."""

from __future__ import annotations

import base64
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
    "sdk_v5_evidence_composer", CI / "compose_sdk_v5_evidence.py"
)
assert SPEC is not None and SPEC.loader is not None
composer = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = composer
SPEC.loader.exec_module(composer)


def write(path: Path, value: dict[str, Any]) -> Path:
    path.write_bytes(composer.conformance.canonical_json_bytes(value))
    return path


def inputs(tmp_path: Path, binding: str = "rust") -> dict[str, Path]:
    corpus = {
        "archive_b64": base64.b64encode(b"archive").decode(),
        "binding": binding,
        "format": composer.corpora.FORMAT,
        "record_b64": [base64.b64encode(f"record-{index}".encode()).decode() for index in range(9)],
    }
    controls = {
        "binding": binding,
        **deepcopy(composer.operational.EXPECTED),
        "format": composer.operational.FORMAT,
        "test_id": composer.operational.TEST_IDS[binding],
    }
    live = {
        "binding": binding,
        "detached_mutation_code": "projected_snapshot_detached",
        "direct_remote_equal": True,
        "entity_snapshot_b64": base64.b64encode(b"entity").decode(),
        "format": composer.LIVE_FORMAT,
        "relation_snapshot_b64": base64.b64encode(b"relation").decode(),
        "remote_exchange_count": 1,
        "rebound_mutation": True,
    }
    cleanup = {
        "binding": binding,
        "format": composer.CLEANUP_FORMAT,
        "managed_database_absent": True,
        "partial_output_absent": True,
        "temporary_evidence_absent": True,
    }
    return {
        "corpus_path": write(tmp_path / "corpus.json", corpus),
        "operational_path": write(tmp_path / "operational.json", controls),
        "live_path": write(tmp_path / "live.json", live),
        "cleanup_path": write(tmp_path / "cleanup.json", cleanup),
    }


def test_composes_exact_assembler_input_from_measured_artifacts(tmp_path: Path) -> None:
    value = composer.compose(binding="rust", run_nonce="a" * 64, root=ROOT, **inputs(tmp_path))
    report = __import__("assemble_sdk_conformance_v5").assemble_report(
        value, binding="rust", run_nonce="a" * 64, root=ROOT
    )
    assert value["record_b64"][0] == base64.b64encode(b"record-0").decode()
    assert report["results"][1]["observation"] == composer.operational.EXPECTED["cancellation"]


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
def test_rejects_unmeasured_or_failed_inputs(
    tmp_path: Path,
    filename: str,
    mutation: Callable[[dict[str, Any]], None],
    code: str,
) -> None:
    arguments = inputs(tmp_path)
    path = tmp_path / filename
    value = json.loads(path.read_bytes())
    mutation(value)
    write(path, value)
    with pytest.raises(composer.CompositionError) as rejected:
        composer.compose(binding="rust", run_nonce="a" * 64, root=ROOT, **arguments)
    assert rejected.value.code == code


def test_create_new_publication_rejects_replacement(tmp_path: Path) -> None:
    value = composer.compose(binding="rust", run_nonce="a" * 64, root=ROOT, **inputs(tmp_path))
    output = tmp_path / "producer.json"
    composer.publish(output, value)
    assert output.read_bytes() == composer.conformance.canonical_json_bytes(value)
    with pytest.raises(composer.CompositionError) as rejected:
        composer.publish(output, value)
    assert rejected.value.code == "evidence_publication_failed"
