#!/usr/bin/env python3
"""Keep the upstream TypeQL AST behind first-party private implementation fields."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def violations(source: str) -> list[int]:
    """Return public declarations that expose upstream paths or imported types."""
    # Preserve line numbers while excluding comments and quoted diagnostic text.
    source = re.sub(
        r'//[^\n]*|/\*.*?\*/|r(#+)?".*?"\1|"(?:\\.|[^"\\])*"',
        lambda match: "\n" * match[0].count("\n") + " ",
        source,
        flags=re.S,
    )
    imports = set()
    for match in re.finditer(r"\buse\s+typeql::([^;]+);", source):
        for item in re.finditer(r"\b([A-Z]\w*)(?:\s+as\s+(\w+))?", match[1]):
            imports.add(item[2] or item[1])
    upstream = r"\btypeql\s*::"
    if imports:
        upstream += "|\\b(?:" + "|".join(sorted(imports)) + ")\\b"
    candidates = [
        (match.start(), match[0])
        for match in re.finditer(
            r"\bpub\s+(?:(?:async|unsafe|const)\s+)*(?:fn\s+\w+[^{};]*|"
            r"(?:type|use)\s+[^;]*|[A-Za-z_]\w*\s*:\s*[^,}]+)",
            source,
        )
    ]
    # Enum variants and tuple-struct fields can expose types without `pub`.
    for declaration in re.finditer(r"\bpub\s+(?:enum|struct)\s+\w+[^;{]*(?:\{|\()", source):
        opening = declaration[0][-1]
        if opening == "(" or declaration[0].startswith("pub enum"):
            closing = ")" if opening == "(" else "}"
            depth = 1
            end = declaration.end()
            while end < len(source) and depth:
                depth += (source[end] == opening) - (source[end] == closing)
                end += 1
            body = source[declaration.end() : end - 1]
            if opening == "(" and not re.search(r"\bpub\s+(?!\()", body):
                continue
            candidates.append((declaration.start(), body))
    return [
        source.count("\n", 0, start) + 1
        for start, snippet in candidates
        if re.search(upstream, snippet)
    ]


def main() -> int:
    files = subprocess.check_output(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "type-bridge-core/crates/**/*.rs",
        ],
        cwd=ROOT,
        text=True,
    ).splitlines()
    found = []
    for relative in files:
        if "/src/" not in relative:
            continue
        path = ROOT / relative
        found.extend(f"{relative}:{line}" for line in violations(path.read_text()))
    if found:
        print("Upstream TypeQL types exposed by public declarations:\n" + "\n".join(found))
        return 1
    print("TypeQL public API boundary: passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
