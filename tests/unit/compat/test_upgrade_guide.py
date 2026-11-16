"""The shared snippet must cover the whole breaking-change inventory."""

import subprocess
import sys

import pytest

from scripts.ci.check_upgrade_guide import END, INCLUDE, ROOT, START, check, check_release_date


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
        check(changelog, INCLUDE, release=True, version="3.0.0")


def test_publication_accepts_dated_changelog() -> None:
    changelog = f"## [3.0.0] - 2026-11-16\n### Breaking\n{START}\n- Change\n{END}"
    assert check(changelog, INCLUDE, release=True, version="3.0.0") == 1


def test_release_date_checks_selected_version_instead_of_previous_release() -> None:
    changelog = "## [3.1.0] - Unreleased\n\n## [3.0.0] - 2026-11-16\n"
    with pytest.raises(ValueError, match="release 3.1.0"):
        check_release_date(changelog, "3.1.0")
    check_release_date(changelog.replace("[3.1.0] - Unreleased", "[3.1.0] - 2026-12-01"), "3.1.0")


def test_release_date_is_enforced_with_python_optimization() -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-O",
            "-c",
            "from scripts.ci.check_upgrade_guide import check_release_date; "
            "check_release_date('## [3.1.0] - Unreleased\\n', '3.1.0')",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "must have a publication date" in result.stderr
