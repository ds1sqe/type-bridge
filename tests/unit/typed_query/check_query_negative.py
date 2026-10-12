"""Verify the intentional query Pyright diagnostics remain exact."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
NEGATIVES = {
    HERE.parent / "type-check-except" / "test_typed_query.py": "# typed-query-error:",
    HERE.parent / "type-check-except" / "test_query_v2_authoring_negative.py": "# query-v2-error:",
}
CONFIG = HERE / "pyrightconfig.query-negative.json"


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
        (path, line)
        for path, marker in NEGATIVES.items()
        for line, text in enumerate(path.read_text().splitlines(), start=1)
        if marker in text
    }
    actual = {
        (Path(diagnostic["file"]).resolve(), diagnostic["range"]["start"]["line"] + 1)
        for diagnostic in diagnostics
        if Path(diagnostic["file"]).resolve() in NEGATIVES
    }
    foreign = [
        diagnostic
        for diagnostic in diagnostics
        if Path(diagnostic["file"]).resolve() not in NEGATIVES
    ]
    if completed.returncode == 0 or actual != expected or foreign:
        raise SystemExit(
            "negative query fixture drifted:\n"
            f"expected={sorted(map(str, expected))} actual={sorted(map(str, actual))} "
            f"foreign={foreign}\n{json.dumps(payload, indent=2)}"
        )
    print(f"negative query fixture passed: {len(diagnostics)} diagnostics")


if __name__ == "__main__":
    main()
