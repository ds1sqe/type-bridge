"""Drift gate for the checked 1-through-16 QuerySession overloads."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def test_query_overloads_match_the_generator() -> None:
    root = Path(__file__).resolve().parents[3]
    subprocess.run(
        [sys.executable, str(root / "scripts" / "generate_typed_query_overloads.py")],
        cwd=root,
        check=True,
    )
