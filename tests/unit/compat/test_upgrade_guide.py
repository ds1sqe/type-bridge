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


@pytest.mark.parametrize("published", ["Unreleased", "2026-02-30", "2026-1-02"])
def test_publication_rejects_undated_or_invalid_changelog(published: str) -> None:
    changelog = f"## [3.0.0] - {published}\n### Breaking\n{START}\n- Change\n{END}"
    with pytest.raises((AssertionError, ValueError)):
        check(changelog, INCLUDE, release=True)


def test_publication_accepts_dated_changelog() -> None:
    changelog = f"## [3.0.0] - 2026-11-16\n### Breaking\n{START}\n- Change\n{END}"
    assert check(changelog, INCLUDE, release=True) == 1
