"""Verify the intentional #174 Pyright diagnostics remain exact."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
NEGATIVE = HERE.parent / "type-check-except" / "test_typed_query.py"
CONFIG = HERE / "pyrightconfig.query-negative.json"
MARKER = "# typed-query-error:"


def main() -> None:
    pyright = sys.argv[1] if len(sys.argv) > 1 else "pyright"
    completed = subprocess.run(
        [pyright, "--outputjson", "--project", str(CONFIG)],
        cwd=HERE,
        check=False,
        capture_output=True,
        text=True,
    )
    payload: dict[str, Any] = json.loads(completed.stdout)
    diagnostics = [
        diagnostic
        for diagnostic in payload.get("generalDiagnostics", [])
        if diagnostic.get("severity") == "error"
    ]
    expected = {
        line
        for line, text in enumerate(NEGATIVE.read_text().splitlines(), start=1)
        if MARKER in text
    }
    actual = {
        diagnostic["range"]["start"]["line"] + 1
        for diagnostic in diagnostics
        if Path(diagnostic["file"]).resolve() == NEGATIVE
    }
    foreign = [
        diagnostic for diagnostic in diagnostics if Path(diagnostic["file"]).resolve() != NEGATIVE
    ]
    if completed.returncode == 0 or actual != expected or foreign:
        raise SystemExit(
            "negative typed-query fixture drifted:\n"
            f"expected={sorted(expected)} actual={sorted(actual)} "
            f"foreign={foreign}\n{json.dumps(payload, indent=2)}"
        )
    print(f"negative typed-query fixture passed: {len(diagnostics)} diagnostics")


if __name__ == "__main__":
    main()
