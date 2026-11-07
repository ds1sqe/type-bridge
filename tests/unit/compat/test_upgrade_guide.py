"""The shared snippet must cover the whole breaking-change inventory."""

import pytest

from scripts.ci.check_upgrade_guide import END, INCLUDE, ROOT, START, check


def test_repository_upgrade_guide_covers_every_breaking_entry() -> None:
    assert (
        check(
            (ROOT / "CHANGELOG.md").read_text(),
            (ROOT / "docs/guide/upgrade-3.0.md").read_text(),
        )
        > 0
    )


@pytest.mark.parametrize("extra", ["- Unshared change\n", "\n- Unshared change"])
def test_entries_outside_shared_snippet_are_rejected(extra: str) -> None:
    snippet = f"{START}\n- Shared change\n{END}"
    breaking = extra + snippet if extra.startswith("-") else snippet + extra
    with pytest.raises(AssertionError, match="outside shared snippet"):
        check(f"## [3.0.0] - Unreleased\n### Breaking\n{breaking}", INCLUDE)


def test_missing_guide_include_is_rejected() -> None:
    with pytest.raises(AssertionError, match="complete shared snippet"):
        check(f"## [3.0.0] - Unreleased\n### Breaking\n{START}\n- Change\n{END}", "")
