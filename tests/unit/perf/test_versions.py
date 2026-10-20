"""The compared versions: newest patch of each line, from both registries."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness import versions

PYPI = ["1.5.11", "2.0.0", "2.0.2", "2.1.0", "2.2.5", "2.2.6", "2.3.0rc1"]
NPM = ["2.0.2", "2.1.0", "2.2.5", "2.2.6", "2.3.0rc1"]


def test_newest_patch_of_each_line_on_both_registries_then_head() -> None:
    # 2.0.0 is on PyPI only, so 2.0.2 stands for 2.0; 1.x and pre-releases drop.
    assert versions.compared(PYPI, NPM) == ["2.0.2", "2.1.0", "2.2.6", "head"]


def test_a_release_joins_the_stack_once_both_registries_serve_it() -> None:
    assert versions.compared([*PYPI, "2.3.0"], NPM)[-2] == "2.2.6"
    assert versions.compared([*PYPI, "2.3.0"], [*NPM, "2.3.0"]) == [
        "2.0.2", "2.1.0", "2.2.6", "2.3.0", "head",
    ]  # fmt: skip
    # Lines order numerically, not as text.
    assert versions.compared(["2.9.0", "2.10.1"], ["2.9.0", "2.10.1"]) == [
        "2.9.0",
        "2.10.1",
        "head",
    ]


def fetcher(pypi: list[str], npm: list[str]):
    def fetch(url: str) -> dict[str, object]:
        if url == versions.PYPI_URL:
            return {"releases": {version: [] for version in pypi}}
        assert url == versions.NPM_URL
        return {"versions": {version: {} for version in npm}}

    return fetch


def test_require_polls_until_the_release_is_on_both(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    calls: list[str] = []
    lists = iter([NPM, NPM, [*NPM, "2.3.0"]])

    def fetch(url: str) -> dict[str, object]:
        calls.append(url)
        if url == versions.PYPI_URL:
            return {"releases": {version: [] for version in [*PYPI, "2.3.0"]}}
        return {"versions": {version: {} for version in next(lists)}}

    output = tmp_path / "output"
    monkeypatch.setenv("GITHUB_OUTPUT", str(output))
    sleeps: list[float] = []
    code = versions.main(
        ["--require", "2.3.0", "--interval", "5", "--github-output"],
        fetch=fetch,
        sleep=sleeps.append,
    )
    assert code == 0
    assert sleeps == [5.0, 5.0]
    expected = ["2.0.2", "2.1.0", "2.2.6", "2.3.0", "head"]
    assert json.loads(capsys.readouterr().out) == expected
    assert output.read_text() == f"versions={json.dumps(expected)}\n"


def test_require_fails_when_the_release_never_appears(capsys: pytest.CaptureFixture[str]) -> None:
    code = versions.main(
        ["--require", "2.3.0", "--attempts", "3"], fetch=fetcher(PYPI, NPM), sleep=lambda _: None
    )
    assert code == 1
    assert "2.3.0 is not on both PyPI and npm after 3 polls" in capsys.readouterr().err


def test_require_takes_only_a_stable_version() -> None:
    with pytest.raises(SystemExit):
        versions.main(["--require", "v2.3.0"], fetch=fetcher(PYPI, NPM))
