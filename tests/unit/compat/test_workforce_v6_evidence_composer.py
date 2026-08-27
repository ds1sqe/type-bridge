"""Fail-closed tests for Workforce V6 evidence composition."""

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
    "workforce_v6_evidence_composer", CI / "compose_workforce_v6_evidence.py"
)
assert SPEC is not None and SPEC.loader is not None
COMPOSER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMPOSER
SPEC.loader.exec_module(COMPOSER)


def _phase4() -> dict[str, Any]:
    return {
        "format": "typebridge.c-artifact-phase4-report/v1",
        "publication-disposition": "candidate-only-unpublished-unsupported",
        "artifacts": {
            "cli": {"candidate-id": f"sha256:{'1' * 64}", "sha256": "2" * 64},
            "generated-package": {
                "candidate-id": f"sha256:{'3' * 64}",
                "sha256": "4" * 64,
            },
            "runtime": {"candidate-id": f"sha256:{'5' * 64}", "sha256": "6" * 64},
        },
        "steps": [{"id": f"{index:02d}-step", "status": "passed"} for index in range(1, 15)],
    }


def _surface(binding: str = "rust") -> dict[str, Any]:
    return {
        "format": COMPOSER.SURFACE_CONSUMER_FORMAT,
        "binding": binding,
        "source-commit": "a" * 40,
        "surface-sha256": "7" * 64,
        "cli-candidate-id": f"sha256:{'1' * 64}",
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
        values.append((report, COMPOSER.conformance.canonical_json_bytes(report)))
    return values


def _compose(**overrides: Any) -> dict[str, Any]:
    phase4 = overrides.pop("phase4", _phase4())
    surface = overrides.pop("surface_consumer", _surface())
    return COMPOSER.compose(
        binding="rust",
        run_nonce="b" * 64,
        source_commit="a" * 40,
        surface_consumer=surface,
        surface_consumer_bytes=COMPOSER.conformance.canonical_json_bytes(surface),
        phase4=phase4,
        phase4_bytes=COMPOSER.conformance.canonical_json_bytes(phase4),
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


def test_rejects_incomplete_phase4(tmp_path: Path) -> None:
    phase4 = _phase4()
    phase4["steps"].pop()
    with pytest.raises(COMPOSER.CompositionError) as raised:
        _compose(phase4=phase4)
    assert raised.value.code == "invalid_phase4_report"


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
    phase4 = _phase4()
    surface = {
        "format": COMPOSER.SURFACE_CONSUMER_FORMAT,
        "binding": "c",
        "source-commit": "a" * 40,
        "surface-sha256": phase4["artifacts"]["generated-package"]["sha256"],
        "cli-candidate-id": phase4["artifacts"]["cli"]["candidate-id"],
        "runtime-provenance": "candidate-c-runtime",
        "checks": ["phase4-c17-cpp17", "sanitizers", "loader-unload"],
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }
    value = COMPOSER.compose(
        binding="c",
        run_nonce="b" * 64,
        source_commit="a" * 40,
        surface_consumer=surface,
        surface_consumer_bytes=COMPOSER.conformance.canonical_json_bytes(surface),
        phase4=phase4,
        phase4_bytes=COMPOSER.conformance.canonical_json_bytes(phase4),
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
