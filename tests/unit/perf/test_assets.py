"""The vendored chart library matches ``docs/perf/assets/CHECKSUMS``."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
ASSETS = REPO_ROOT / "docs/perf/assets"
CHECKSUMS = ASSETS / "CHECKSUMS"


def _entries() -> dict[str, str]:
    entries: dict[str, str] = {}
    for line in CHECKSUMS.read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            digest, name = line.split(maxsplit=1)
            entries[name] = digest
    return entries


def test_every_vendored_file_matches_its_checksum() -> None:
    entries = _entries()
    vendored = {path.name for path in ASSETS.iterdir() if path.name != "CHECKSUMS"}
    assert set(entries) == vendored
    for name, digest in entries.items():
        assert hashlib.sha256((ASSETS / name).read_bytes()).hexdigest() == digest, name


def test_vendored_build_is_the_recorded_version() -> None:
    match = re.search(r"^# library: uPlot (\S+)", CHECKSUMS.read_text(encoding="utf-8"), re.M)
    assert match is not None
    banner = (ASSETS / "uPlot.iife.min.js").read_text(encoding="utf-8").splitlines()[0]
    assert banner == f"/*! https://github.com/leeoniya/uPlot (v{match[1]}) */"
    assert f"uplot-{match[1]}.tgz" in CHECKSUMS.read_text(encoding="utf-8")
