"""The checkout guide includes the tested shop programs.

The four `examples/shop` programs are the `writes` conformance suite's `shop`
programs, which the conformance runner builds and runs live.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
GUIDE = ROOT / "docs/guide/checkout.md"
INCLUDE = re.compile(r'^ *--8<-- "([^":]+):([a-z][-_0-9a-z]*)"$', re.MULTILINE)


def test_every_guide_include_names_a_section_of_an_existing_file() -> None:
    # pymdownx.snippets skips a missing file or section silently.
    includes = INCLUDE.findall(GUIDE.read_text(encoding="utf-8"))
    assert {path for path, _ in includes} == {
        "examples/shop/python/program.py",
        "examples/shop/ts/program.ts",
        "examples/shop/rust/main.rs",
        "examples/shop/c/program.c",
    }
    for path, section in includes:
        text = (ROOT / path).read_text(encoding="utf-8")
        for marker in ("start", "end"):
            assert text.count(f"--8<-- [{marker}:{section}]") == 1, (path, section, marker)
