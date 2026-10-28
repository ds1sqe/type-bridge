"""Source acceptance uses the exact validated Maturin backend through uv."""

import os
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RESOLVER = ROOT / "scripts/ci/run_maturin.sh"


def test_maturin_resolver_uses_backend_pin_and_preserves_arguments(tmp_path: Path) -> None:
    requirement = tomllib.loads((ROOT / "type-bridge-core/pyproject.toml").read_text())[
        "build-system"
    ]["requires"][0]
    for name in ("uv", "uvx"):
        executable = tmp_path / name
        executable.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        executable.chmod(0o755)
    result = subprocess.run(
        ["/bin/bash", str(RESOLVER), "develop", "a path with spaces"],
        env={**os.environ, "PATH": str(tmp_path)},
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.splitlines() == [
        "--from",
        requirement,
        "maturin",
        "develop",
        "a path with spaces",
    ]


def test_maturin_resolver_reports_missing_uv_before_build(tmp_path: Path) -> None:
    result = subprocess.run(
        ["/bin/bash", str(RESOLVER), "develop"],
        env={**os.environ, "PATH": str(tmp_path)},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1
    assert "required tool is unavailable: uv" in result.stderr
