"""Publisher compatibility must be checked before any registry is mutated."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
GATE = ROOT / "scripts/ci/check_pypi_publisher_metadata.sh"
PUBLISHER = "dc37677b2e1c63e2034f94d8a5b11f265b73ba33"
DIGEST = "sha256:a68d05519f6d7e47372aeaddab80b851b69afa89be179ec41775c72c4e3ab2d5"
IMAGE = f"ghcr.io/pypa/gh-action-pypi-publish@{DIGEST}"


def test_release_requires_compatible_publisher_and_metadata_preflight() -> None:
    source = (ROOT / ".github/workflows/release.yml").read_text()
    jobs = yaml.load(source, Loader=yaml.BaseLoader)["jobs"]
    for job in ("publish-core-pypi", "publish-python-pypi"):
        steps = jobs[job]["steps"]
        publisher = next(
            step for step in steps if "pypa/gh-action-pypi-publish@" in step.get("uses", "")
        )
        assert publisher["uses"] == f"pypa/gh-action-pypi-publish@{PUBLISHER}"
        assert publisher.get("with", {}).get("attestations", "true") == "true"
        assert publisher.get("with", {}).get("verify-metadata", "true") == "true"
        assert "continue-on-error" not in publisher
    build = jobs["build-python"]
    assert "continue-on-error" not in build
    gate = next(
        step for step in build["steps"] if step.get("run") == f"bash {GATE.relative_to(ROOT)} dist"
    )
    assert "if" not in gate and "continue-on-error" not in gate
    assert source.index("run: uv build") < source.index(gate["run"])
    assert PUBLISHER in GATE.read_text() and DIGEST in GATE.read_text()


@pytest.mark.parametrize("failure", ["", "pull", "inspect", "metadata"])
def test_metadata_gate_pins_image_and_propagates_failures(tmp_path: Path, failure: str) -> None:
    binary = tmp_path / "bin"
    binary.mkdir()
    docker = binary / "docker"
    docker.write_text(
        """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$TEST_DOCKER_LOG"
case "$1" in
  pull) [[ "$TEST_FAILURE" != pull ]] || exit 17 ;;
  image)
    if [[ "$TEST_FAILURE" == inspect ]]; then
      printf '["ghcr.io/pypa/gh-action-pypi-publish@sha256:wrong"]\\n'
    else
      printf '["%s"]\\n' "$TEST_IMAGE"
    fi
    ;;
  run) [[ "$TEST_FAILURE" != metadata ]] || exit 23 ;;
  *) exit 29 ;;
esac
"""
    )
    docker.chmod(0o755)
    distribution = tmp_path / "dist"
    distribution.mkdir()
    log = tmp_path / "docker.log"
    result = subprocess.run(
        ["bash", str(GATE), str(distribution)],
        capture_output=True,
        text=True,
        check=False,
        env={
            **os.environ,
            "PATH": f"{binary}{os.pathsep}{os.environ['PATH']}",
            "TEST_FAILURE": failure,
            "TEST_IMAGE": IMAGE,
            "TEST_DOCKER_LOG": str(log),
        },
    )
    commands = log.read_text().splitlines()
    assert (result.returncode == 0) is (failure == "")
    if failure in ("pull", "inspect"):
        assert not any(command.startswith("run ") for command in commands)
    else:
        run = commands[-1]
        assert "--network none --read-only" in run
        assert f"source={distribution},target=/dist,readonly" in run
        assert IMAGE in run and "strict=True" in run
        assert "--env" not in run


def test_patched_python_matrix_does_not_enable_future_interpreters() -> None:
    for path in (".github/workflows/ci.yml", ".github/workflows/release.yml", "scripts/check.sh"):
        assert "PYO3_USE_ABI3_FORWARD_COMPATIBILITY" not in (ROOT / path).read_text()
