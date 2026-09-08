"""Patched dependency floors and fail-closed security gate contracts."""

from __future__ import annotations

import os
import subprocess
import tomllib
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
GATE = ROOT / "scripts/ci/check_dependency_security.sh"
LOCKFILES = (
    "type-bridge-core/Cargo.lock",
    "type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.lock",
)


@pytest.mark.parametrize(
    ("name", "series", "minimum"),
    [
        ("pyo3", "0.", (0, 29, 2)),
        ("pythonize", "0.", (0, 29, 0)),
        ("crossbeam-epoch", "0.", (0, 9, 20)),
        ("h2", "0.", (0, 4, 16)),
        ("rustls-webpki", "0.", (0, 103, 13)),
        ("anyhow", "1.", (1, 0, 103)),
        ("rand", "0.8.", (0, 8, 6)),
        ("chacha20", "0.10.", (0, 10, 2)),
    ],
)
def test_remediated_dependency_floors(
    name: str, series: str, minimum: tuple[int, int, int]
) -> None:
    matched = []
    for lockfile in LOCKFILES:
        lock = tomllib.loads((ROOT / lockfile).read_text())
        for package in lock["package"]:
            if package["name"] == name and package["version"].startswith(series):
                version = tuple(int(part) for part in package["version"].split("."))
                assert version >= minimum, (lockfile, name, version)
                matched.append(version)
    assert matched, f"Update this contract if {name} is removed from the lockfiles"


def test_python_conversion_features_share_the_patched_binding_and_abi() -> None:
    core = ROOT / "type-bridge-core/crates"
    python = tomllib.loads((core / "python/Cargo.toml").read_text())["dependencies"]
    optional = tomllib.loads((core / "core/Cargo.toml").read_text())["dependencies"]
    assert python["pyo3"]["version"] == optional["pyo3"]["version"] == "0.29.2"
    assert python["pythonize"] == optional["pythonize"]["version"] == "0.29"
    assert python["pyo3"]["features"] == ["abi3-py312"]
    # PyO3 0.28+ defaults to free-threaded; this migration must not opt us in.
    assert "#[pymodule(gil_used = true)]" in (core / "python/src/lib.rs").read_text()


def run_gate(
    tmp_path: Path, *, version: str = "0.22.2", fail_at: str = "", failure: int = 1
) -> tuple[subprocess.CompletedProcess[str], list[str]]:
    cargo = tmp_path / "cargo"
    cargo.write_text(
        """#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == 'audit --version' ]]; then
    printf 'cargo-audit-audit %s\\n' "$TEST_AUDIT_VERSION"
    exit 0
fi
printf '%s\\n' "$*" >> "$TEST_AUDIT_LOG"
if [[ "$3" == "$TEST_FAIL_AT" ]]; then
    exit "$TEST_AUDIT_FAILURE"
fi
printf 'informational maintenance warning remains visible\\n'
"""
    )
    cargo.chmod(0o755)
    log = tmp_path / "audit.log"
    result = subprocess.run(
        ["bash", str(GATE)],
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}",
            "TEST_AUDIT_VERSION": version,
            "TEST_AUDIT_LOG": str(log),
            "TEST_FAIL_AT": fail_at,
            "TEST_AUDIT_FAILURE": str(failure),
        },
        capture_output=True,
        text=True,
        check=False,
    )
    return result, log.read_text().splitlines() if log.exists() else []


def test_gate_audits_every_lockfile_without_filters_or_stale_database(tmp_path: Path) -> None:
    result, commands = run_gate(tmp_path)
    assert result.returncode == 0, result.stderr
    assert commands == [
        f"audit --file {lockfile} --deny unsound --deny yanked" for lockfile in LOCKFILES
    ]
    assert result.stdout.count("informational maintenance warning remains visible") == 2


@pytest.mark.parametrize("lockfile", LOCKFILES)
@pytest.mark.parametrize("failure", [1, 2, 101])
def test_gate_propagates_findings_and_audit_errors(
    tmp_path: Path, lockfile: str, failure: int
) -> None:
    result, commands = run_gate(tmp_path, fail_at=lockfile, failure=failure)
    assert result.returncode == failure
    assert len(commands) == LOCKFILES.index(lockfile) + 1


def test_gate_rejects_unpinned_auditor(tmp_path: Path) -> None:
    result, commands = run_gate(tmp_path, version="0.21.0")
    assert result.returncode != 0
    assert "Expected cargo-audit 0.22.2" in result.stderr
    assert not commands


@pytest.mark.parametrize(("workflow", "job"), [("ci", "rust-check"), ("release", "test")])
def test_security_gate_is_required_in_ci_and_before_release(workflow: str, job: str) -> None:
    source = (ROOT / f".github/workflows/{workflow}.yml").read_text()
    jobs = yaml.load(source, Loader=yaml.BaseLoader)["jobs"]
    assert "continue-on-error" not in jobs[job]
    steps = jobs[job]["steps"]
    audit = next(step for step in steps if step.get("run") == f"bash {GATE.relative_to(ROOT)}")
    assert "continue-on-error" not in audit
    assert any(step.get("with", {}).get("tool") == "cargo-audit@0.22.2" for step in steps)
    assert "bash scripts/ci/check_dependency_security.sh" in (ROOT / "scripts/check.sh").read_text()
