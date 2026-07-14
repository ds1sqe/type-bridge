"""Run and validate the intentional negative Pyright contract fixture."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
NEGATIVE = HERE / "negative.py"
CONFIG = HERE / "pyrightconfig.negative.json"
MARKER = "# contract-error:"


def _expected_diagnostics() -> dict[int, str]:
    return {
        line_number: (
            "reportAttributeAccessIssue"
            if line.split(MARKER, 1)[1].strip() == "like-absent"
            else "reportCallIssue"
        )
        for line_number, line in enumerate(
            NEGATIVE.read_text(encoding="utf-8").splitlines(), start=1
        )
        if MARKER in line
    }


def _pyright(explicit: str | None) -> str:
    candidate = explicit or os.environ.get("PYRIGHT_BIN") or shutil.which("pyright")
    if candidate is None:
        raise SystemExit("pyright not found; pass --pyright PATH or set PYRIGHT_BIN")
    return str(Path(candidate).expanduser().absolute())


def _error_diagnostics(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        diagnostic
        for diagnostic in payload.get("generalDiagnostics", [])
        if diagnostic.get("severity") == "error"
    ]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pyright", help="Path to the Pyright executable")
    args = parser.parse_args()

    completed = subprocess.run(
        [_pyright(args.pyright), "--outputjson", "--project", str(CONFIG)],
        cwd=HERE,
        check=False,
        capture_output=True,
        text=True,
    )
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(
            f"pyright did not return JSON (exit {completed.returncode}):\n"
            f"{completed.stdout}\n{completed.stderr}"
        ) from error

    errors = _error_diagnostics(payload)
    foreign = [
        diagnostic for diagnostic in errors if Path(diagnostic["file"]).resolve() != NEGATIVE
    ]
    actual_lines = {
        diagnostic["range"]["start"]["line"] + 1
        for diagnostic in errors
        if Path(diagnostic["file"]).resolve() == NEGATIVE
    }
    expected_diagnostics = _expected_diagnostics()
    expected_lines = set(expected_diagnostics)
    unexpected_rules = [
        diagnostic
        for diagnostic in errors
        if Path(diagnostic["file"]).resolve() == NEGATIVE
        and diagnostic.get("rule")
        != expected_diagnostics.get(diagnostic["range"]["start"]["line"] + 1)
    ]

    if completed.returncode == 0 or foreign or actual_lines != expected_lines or unexpected_rules:
        raise SystemExit(
            "negative Pyright contract drifted:\n"
            f"expected lines: {sorted(expected_lines)}\n"
            f"actual lines: {sorted(actual_lines)}\n"
            f"unexpected rules: {unexpected_rules}\n"
            f"foreign diagnostics: {foreign}\n"
            f"payload: {json.dumps(payload, indent=2)}"
        )

    print(f"negative typed-query contract passed: {len(errors)} expected diagnostic(s)")


if __name__ == "__main__":
    main()
