"""Source-boundary probes run in CI's rust-check job as scripts/check.sh runs them."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
COMMANDS = (
    "python3 scripts/ci/check_internal_eager.py",
    "python scripts/ci/check_typeql_text.py",
    "python scripts/ci/check_typeql_public_api.py",
)


def rust_check_steps() -> list[dict]:
    source = (ROOT / ".github/workflows/ci.yml").read_text()
    jobs = yaml.load(source, Loader=yaml.BaseLoader)["jobs"]
    assert "continue-on-error" not in jobs["rust-check"]
    return jobs["rust-check"]["steps"]


@pytest.mark.parametrize("command", COMMANDS)
def test_boundary_probe_is_required_in_rust_check(command: str) -> None:
    assert command in (ROOT / "scripts/check.sh").read_text()
    step = next(step for step in rust_check_steps() if step.get("run") == command)
    assert step["if"] == "runner.os == 'Linux'"
    assert "continue-on-error" not in step


def test_internal_eager_probe_has_ripgrep_and_cargo() -> None:
    steps = rust_check_steps()
    names = [step.get("name") for step in steps]
    eager = next(i for i, step in enumerate(steps) if step.get("run") == COMMANDS[0])
    ripgrep = names.index("Install ripgrep")
    assert ripgrep < eager
    assert "apt-get install --no-install-recommends -y ripgrep" in steps[ripgrep]["run"]
    assert steps[ripgrep]["if"] == "runner.os == 'Linux'"
    assert names.index("Install Rust toolchain") < eager
