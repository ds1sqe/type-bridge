"""The ordinary Python integration tree must be represented in live CI."""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_live_ci_matrix_covers_every_ordinary_integration_group() -> None:
    workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    match = re.search(r"^\s*test-group:\s*\n\s*\[([^]]+)]", workflow, re.MULTILINE)
    assert match is not None, "test-integration matrix group list is missing"
    configured = {entry.strip() for entry in match.group(1).split(",")}

    integration_root = REPO_ROOT / "tests/integration"
    dedicated = {"parity", "proxy"}
    ordinary = {
        directory.name
        for directory in integration_root.iterdir()
        if directory.is_dir()
        and not directory.name.startswith("__")
        and directory.name not in dedicated
        and any(directory.glob("test_*.py"))
    }

    assert configured == ordinary
    assert {"queries", "schema", "expressions", "session"} == configured
