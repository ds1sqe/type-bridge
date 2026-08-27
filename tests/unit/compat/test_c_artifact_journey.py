"""Fail-closed tests for the Plan 08 artifact-only C journey."""

from __future__ import annotations

import copy
import importlib.util
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT_DIRECTORY = ROOT / "scripts/ci"
sys.path.insert(0, str(SCRIPT_DIRECTORY))
SPEC = importlib.util.spec_from_file_location(
    "c_artifact_journey", SCRIPT_DIRECTORY / "c_artifact_journey.py"
)
assert SPEC is not None and SPEC.loader is not None
JOURNEY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(JOURNEY)


def _report() -> dict[str, Any]:
    return {
        "artifacts": {
            "cli": {"candidate-id": "cli-id", "sha256": "cli-sha"},
            "generated-package": {"candidate-id": "generated-id", "sha256": "generated-sha"},
            "runtime": {"candidate-id": "runtime-id", "sha256": "runtime-sha"},
        },
        "connected": None,
        "format": JOURNEY.FORMAT,
        "platform": JOURNEY.packages.TARGET,
        "provider-free": {
            "archive-package-smoke": {},
            "canonical-live-sources-compile": {"c17": True, "cpp17": True},
            "compile-negative": [
                "gcc-c17",
                "clang-c17",
                "gcc-cpp17",
                "clang-cpp17",
            ],
            "loader-unload": "unmapped-after-dlclose",
            "sanitizers": ["address", "leak", "undefined"],
        },
        "publication-disposition": JOURNEY.DISPOSITION,
    }


def _stub_artifacts(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> tuple[Path, Path, Path]:
    archives = []
    for name, body in (("cli", b"cli"), ("runtime", b"runtime"), ("generated", b"generated")):
        path = tmp_path / name
        path.write_bytes(body)
        archives.append(path)
    monkeypatch.setattr(JOURNEY.cli, "validate", lambda _path: {"candidate-id": "cli-id"})
    monkeypatch.setattr(JOURNEY.cli, "sha256", lambda _body: "cli-sha")
    monkeypatch.setattr(
        JOURNEY.packages,
        "validate_runtime",
        lambda _path: {"candidate-id": "runtime-id"},
    )
    monkeypatch.setattr(
        JOURNEY.packages,
        "validate_generated",
        lambda _path: {"candidate-id": "generated-id"},
    )
    monkeypatch.setattr(
        JOURNEY.packages,
        "sha256",
        lambda body: {b"runtime": "runtime-sha", b"generated": "generated-sha"}[body],
    )
    return archives[0], archives[1], archives[2]


def test_canonical_consumers_adapt_only_package_identity() -> None:
    for fixture in (JOURNEY.FULL_CONSUMER, JOURNEY.PHASE4_CONSUMER):
        source = JOURNEY.adapted_fixture(fixture).decode()
        assert "fixture_" not in source
        assert "FIXTURE_" not in source
        assert "#include <tb_workforcev3/tb_workforcev3.h>" in source
        assert "tb_workforcev3_schema_package_open" in source
    flat = JOURNEY.adapted_flat_package().decode()
    assert '#include "src/tb_workforcev3.c"' in flat
    assert "tb_workforcev3_schema_package_chunks_v1" in flat


def test_report_revalidates_exact_archive_bindings(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    archives = _stub_artifacts(monkeypatch, tmp_path)
    JOURNEY.validate_report(_report(), *archives)


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("publication-disposition", "supported", "publication authority"),
        ("platform", "aarch64-unknown-linux-gnu", "identity drifted"),
    ],
)
def test_report_rejects_scope_widening(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    field: str,
    value: object,
    message: str,
) -> None:
    archives = _stub_artifacts(monkeypatch, tmp_path)
    report = copy.deepcopy(_report())
    report[field] = value
    with pytest.raises(JOURNEY.JourneyError, match=message):
        JOURNEY.validate_report(report, *archives)


@pytest.mark.parametrize(
    ("path", "value", "message"),
    [
        (("artifacts", "runtime", "sha256"), "tampered", "artifact binding"),
        (("provider-free", "loader-unload"), "not-checked", "loader-unload"),
        (("provider-free", "sanitizers"), ["address"], "sanitizer"),
        (("provider-free", "compile-negative"), ["gcc-c17"], "compile-negative"),
    ],
)
def test_report_rejects_missing_or_tampered_evidence(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    path: tuple[str, ...],
    value: object,
    message: str,
) -> None:
    archives = _stub_artifacts(monkeypatch, tmp_path)
    report = copy.deepcopy(_report())
    target: dict[str, Any] = report
    for member in path[:-1]:
        target = target[member]
    target[path[-1]] = value
    with pytest.raises(JOURNEY.JourneyError, match=message):
        JOURNEY.validate_report(report, *archives)
