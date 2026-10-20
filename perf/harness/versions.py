"""The released versions the comparison runs: the newest patch of each line.

A line is one ``major.minor`` from 2.0 on. A version is compared once both
registries have it, ``type-bridge`` on PyPI and ``@type-bridge/node`` on
npm, so a release joins the stack on its own after it is published; no list
is edited. Pre-releases are never compared. ``head`` (the source tree) is
always last.

``python -m perf.harness.versions`` prints the JSON list. ``--require X.Y.Z``
fails unless that version is in it, polling the registries while a just
published release propagates; ``--github-output`` also writes ``versions``
for a workflow matrix.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.request
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path

HEAD = "head"
FIRST_LINE = (2, 0)
PYPI_URL = "https://pypi.org/pypi/type-bridge/json"
NPM_URL = "https://registry.npmjs.org/@type-bridge%2Fnode"
STABLE = re.compile(r"(\d+)\.(\d+)\.(\d+)")

Version = tuple[int, int, int]


def parse(version: str) -> Version | None:
    """``(2, 2, 6)`` for ``"2.2.6"``; ``None`` for a pre-release or other text."""
    match = STABLE.fullmatch(version)
    return (int(match[1]), int(match[2]), int(match[3])) if match else None


def line(version: str) -> str:
    """The ``major.minor`` line of a stable version: ``"2.2"`` for ``"2.2.6"``."""
    parsed = parse(version)
    if parsed is None:
        raise ValueError(f"not a stable version: {version!r}")
    return f"{parsed[0]}.{parsed[1]}"


def compared(pypi: Iterable[str], npm: Iterable[str]) -> list[str]:
    """The newest patch of each line from 2.0 that both registries publish."""
    shared = {parsed for v in set(pypi) & set(npm) if (parsed := parse(v)) is not None}
    newest: dict[tuple[int, int], Version] = {}
    for version in shared:
        key = version[:2]
        if key >= FIRST_LINE and version > newest.get(key, (0, 0, -1)):
            newest[key] = version
    return [".".join(map(str, newest[key])) for key in sorted(newest)] + [HEAD]


def _fetch_json(url: str) -> dict[str, object]:
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def registry_versions(fetch: Callable[[str], dict[str, object]] = _fetch_json) -> list[str]:
    """``compared`` over the live PyPI and npm version lists."""
    pypi = fetch(PYPI_URL)["releases"]
    npm = fetch(NPM_URL)["versions"]
    assert isinstance(pypi, dict) and isinstance(npm, dict)
    return compared(pypi, npm)


def main(
    argv: Sequence[str] | None = None,
    *,
    fetch: Callable[[str], dict[str, object]] = _fetch_json,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    parser = argparse.ArgumentParser(description="Print the released versions the comparison runs.")
    parser.add_argument("--require", help="stable version that must be compared")
    parser.add_argument("--attempts", type=int, default=20, help="registry polls for --require")
    parser.add_argument("--interval", type=float, default=60.0, help="seconds between polls")
    parser.add_argument("--github-output", action="store_true", help="also write $GITHUB_OUTPUT")
    arguments = parser.parse_args(argv)
    if arguments.require is not None and parse(arguments.require) is None:
        parser.error(f"--require takes a stable version, got {arguments.require!r}")

    versions = registry_versions(fetch)
    attempt = 1
    while arguments.require is not None and arguments.require not in versions:
        if attempt >= arguments.attempts:
            print(
                f"{arguments.require} is not on both PyPI and npm after {attempt} polls; "
                f"compared: {', '.join(versions)}",
                file=sys.stderr,
            )
            return 1
        sleep(arguments.interval)
        versions = registry_versions(fetch)
        attempt += 1

    text = json.dumps(versions)
    print(text)
    if arguments.github_output:
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
            output.write(f"versions={text}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
