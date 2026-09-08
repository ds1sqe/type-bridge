"""Fail-closed tests for Sdk V6 evidence composition."""

from __future__ import annotations

import base64
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_v6_evidence_composer", CI / "compose_sdk_v6_evidence.py"
)
assert SPEC is not None and SPEC.loader is not None
COMPOSER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMPOSER
SPEC.loader.exec_module(COMPOSER)


def _acceptance() -> dict[str, Any]:
    return {
        "format": "typebridge.c-artifact-acceptance-report/v1",
        "publication-disposition": "artifact-only-unpublished-unsupported",
        "artifacts": {
            "cli": {"artifact-id": f"sha256:{'1' * 64}", "sha256": "2" * 64},
            "generated-package": {
                "artifact-id": f"sha256:{'3' * 64}",
                "sha256": "4" * 64,
            },
            "runtime": {"artifact-id": f"sha256:{'5' * 64}", "sha256": "6" * 64},
        },
        "steps": [{"id": f"{index:02d}-step", "status": "passed"} for index in range(1, 15)],
    }


def _surface(binding: str = "rust") -> dict[str, Any]:
    return {
        "format": COMPOSER.SURFACE_CONSUMER_FORMAT,
        "binding": binding,
        "source-commit": "a" * 40,
        "surface-sha256": "7" * 64,
        "cli-artifact-id": f"sha256:{'1' * 64}",
        "runtime-provenance": "current-sdk-regression",
        "checks": ["offline-cargo-check", "all-targets"],
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }


def _predecessors(binding: str = "rust") -> list[tuple[dict[str, Any], bytes]]:
    values = []
    for version in COMPOSER.conformance.predecessor_versions(binding):
        report = {
            "format": f"typebridge.sdk-conformance-report/v{version}",
            "binding": binding,
        }
        body = COMPOSER.conformance.canonical_json_bytes(report)
        if version in (1, 2, 3):
            body += b"\n"
        elif version == 4:
            body = json.dumps(report, indent=2).encode() + b"\n"
        values.append((report, body))
    return values


def _compose(**overrides: Any) -> dict[str, Any]:
    acceptance = overrides.pop("acceptance", _acceptance())
    surface = overrides.pop("surface_consumer", _surface())
    return COMPOSER.compose(
        binding="rust",
        run_nonce="b" * 64,
        source_commit="a" * 40,
        surface_consumer=surface,
        surface_consumer_bytes=COMPOSER.conformance.canonical_json_bytes(surface),
        acceptance=acceptance,
        acceptance_bytes=COMPOSER.conformance.canonical_json_bytes(acceptance),
        predecessors=overrides.pop("predecessors", _predecessors()),
        root=ROOT,
        **overrides,
    )


def test_composes_exact_25_fresh_digest_bound_fragments() -> None:
    value = _compose()
    assert value["format"] == COMPOSER.assembler.FORMAT
    assert len(value["non_selected_proofs"]) == 25
    fragment = json.loads(base64.b64decode(value["non_selected_proofs"][0]["evidence_b64"]))
    assert fragment["source_commit"] == "a" * 40
    assert fragment["predecessor_reports"] == [
        {"version": version, "sha256": COMPOSER.hashlib.sha256(body).hexdigest()}
        for version, (_report, body) in enumerate(_predecessors(), 1)
    ]
    assert fragment["publication_authority"] is False


def test_rejects_incomplete_acceptance(tmp_path: Path) -> None:
    acceptance = _acceptance()
    acceptance["steps"].pop()
    with pytest.raises(COMPOSER.CompositionError) as raised:
        _compose(acceptance=acceptance)
    assert raised.value.code == "invalid_acceptance_report"


def test_rejects_stale_surface_source() -> None:
    surface = _surface()
    surface["source-commit"] = "c" * 40
    with pytest.raises(COMPOSER.CompositionError) as raised:
        _compose(surface_consumer=surface)
    assert raised.value.code == "surface_consumer_identity_drift"


def test_rejects_wrong_predecessor_binding() -> None:
    predecessors = _predecessors()
    predecessors[1][0]["binding"] = "node"
    with pytest.raises(COMPOSER.CompositionError) as raised:
        _compose(predecessors=predecessors)
    assert raised.value.code == "predecessor_report_identity_drift"


def test_c_predecessor_history_starts_at_v2() -> None:
    acceptance = _acceptance()
    surface = {
        "format": COMPOSER.SURFACE_CONSUMER_FORMAT,
        "binding": "c",
        "source-commit": "a" * 40,
        "surface-sha256": acceptance["artifacts"]["generated-package"]["sha256"],
        "cli-artifact-id": acceptance["artifacts"]["cli"]["artifact-id"],
        "runtime-provenance": "artifact-c-runtime",
        "checks": ["query-c17-cpp17", "sanitizers", "loader-unload"],
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }
    value = COMPOSER.compose(
        binding="c",
        run_nonce="b" * 64,
        source_commit="a" * 40,
        surface_consumer=surface,
        surface_consumer_bytes=COMPOSER.conformance.canonical_json_bytes(surface),
        acceptance=acceptance,
        acceptance_bytes=COMPOSER.conformance.canonical_json_bytes(acceptance),
        predecessors=_predecessors("c"),
        root=ROOT,
    )
    fragment = json.loads(base64.b64decode(value["non_selected_proofs"][0]["evidence_b64"]))
    assert [item["version"] for item in fragment["predecessor_reports"]] == [2, 3, 4, 5]


def test_publication_is_create_new(tmp_path: Path) -> None:
    path = tmp_path / "evidence.json"
    COMPOSER.publish(path, _compose())
    with pytest.raises(COMPOSER.CompositionError) as raised:
        COMPOSER.publish(path, _compose())
    assert raised.value.code == "invalid_output_path"


def test_loader_preserves_acceptance_and_historical_canonical_spelling(tmp_path: Path) -> None:
    acceptance = _acceptance()
    acceptance_path = tmp_path / "acceptance.json"
    acceptance_path.write_bytes(COMPOSER.conformance.canonical_json_bytes(acceptance) + b"\n")
    assert (
        COMPOSER.load_canonical(acceptance_path, "Artifact acceptance", trailing_newline=True)[0]
        == acceptance
    )

    v3 = _predecessors()[2][0]
    v3_path = tmp_path / "v3.json"
    v3_path.write_bytes(COMPOSER.conformance.canonical_json_bytes(v3) + b"\n")
    assert COMPOSER.load_canonical(v3_path, "V3", trailing_newline=True)[0] == v3
    with pytest.raises(COMPOSER.CompositionError) as raised:
        COMPOSER.load_canonical(v3_path, "V3")
    assert raised.value.code == "noncanonical_composition_json"

    v4 = _predecessors()[3]
    v4_path = tmp_path / "v4.json"
    v4_path.write_bytes(v4[1])
    assert COMPOSER.load_canonical(v4_path, "V4", require_canonical=False)[0] == v4[0]
