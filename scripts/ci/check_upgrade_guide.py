#!/usr/bin/env python3
"""Require the complete 3.0 Breaking section in the upgrade guide."""

import argparse
import re
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
START = "<!-- --8<-- [start:breaking-3-0] -->"
END = "<!-- --8<-- [end:breaking-3-0] -->"
INCLUDE = '--8<-- "CHANGELOG.md:breaking-3-0"'


def check(changelog: str, guide: str, *, release: bool = False) -> int:
    """Check snippet boundaries so every Breaking entry appears in both pages."""
    section = changelog.split("## [3.0.0] - ", 1)[1].split("\n## [", 1)[0]
    if release:
        published = section.splitlines()[0]
        assert re.fullmatch(r"\d{4}-\d{2}-\d{2}", published), (
            "release changelog must have a publication date"
        )
        date.fromisoformat(published)
    assert section.count("### Breaking\n") == 1, "expected one Breaking section"
    breaking = section.split("### Breaking\n", 1)[1].split("\n### ", 1)[0]
    assert breaking.count(START) == breaking.count(END) == 1, "missing snippet boundary"
    before, included = breaking.split(START)
    included, after = included.split(END)
    assert not before.strip() and not after.strip(), "Breaking entry outside shared snippet"
    assert guide.count(INCLUDE) == 1, "upgrade guide must include the complete shared snippet"
    assert "draft" not in guide.lower(), "upgrade guide still marked draft"
    count = sum(line.startswith("- ") for line in included.splitlines())
    assert count, "empty Breaking section"
    return count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true", help="require a dated release changelog")
    args = parser.parse_args()
    count = check(
        (ROOT / "CHANGELOG.md").read_text(),
        (ROOT / "docs/guide/upgrade-3.0.md").read_text(),
        release=args.release,
    )
    print(f"Upgrade guide includes all {count} Breaking entries.")
