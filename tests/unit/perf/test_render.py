"""The MkDocs hook that publishes the perf index beside ``/perf/``."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness import render, store
from perf.harness.render import (
    DATA_URI,
    LATEST_URI,
    RenderError,
    data_dir_from_env,
    empty_index,
    site_files,
)

FIXTURES = Path(__file__).parent / "fixtures"
DATA = FIXTURES / "data"


def test_empty_index_matches_the_store() -> None:
    assert render.INDEX_FORMAT == store.INDEX_FORMAT
    assert empty_index() == store.build_index(FIXTURES / "absent")


@pytest.mark.parametrize("value", [None, ""], ids=["unset", "blank"])
def test_no_data_dir_publishes_an_empty_index(value: str | None) -> None:
    env = {} if value is None else {render.ENV: value}
    data_dir = data_dir_from_env(env)

    files = site_files(data_dir)

    assert data_dir is None
    assert list(files) == [DATA_URI]
    assert json.loads(files[DATA_URI]) == empty_index()


def test_empty_data_dir_publishes_an_empty_index(tmp_path: Path) -> None:
    files = site_files(tmp_path)

    assert list(files) == [DATA_URI]
    assert json.loads(files[DATA_URI])["runs"] == 0


def test_missing_data_dir_fails_the_build(tmp_path: Path) -> None:
    with pytest.raises(RenderError, match="not a directory"):
        site_files(tmp_path / "absent")


def test_fixture_data_dir_publishes_index_and_latest_run() -> None:
    files = site_files(data_dir_from_env({render.ENV: str(DATA)}))

    data = json.loads(files[DATA_URI])
    index = json.loads((DATA / "index.json").read_text(encoding="utf-8"))
    assert data == index | {"latest_url": "latest.json"}
    assert "micro.core.compilation.insert_entity" in data["metrics"]
    assert files[LATEST_URI] == (DATA / index["latest"]).read_text(encoding="utf-8")


def test_index_of_another_format_is_rejected(tmp_path: Path) -> None:
    (tmp_path / "index.json").write_text('{"format": "perf-index/v0"}', encoding="utf-8")

    with pytest.raises(RenderError, match="perf-index/v1"):
        site_files(tmp_path)


def test_latest_outside_runs_is_rejected(tmp_path: Path) -> None:
    (tmp_path / "secret.json").write_text("{}", encoding="utf-8")
    index = empty_index() | {"runs": 1, "latest": "secret.json"}
    (tmp_path / "index.json").write_text(json.dumps(index), encoding="utf-8")

    with pytest.raises(RenderError, match="not a stored run"):
        site_files(tmp_path)


def test_mkdocs_loads_the_hook_by_path(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """MkDocs imports the hook file with only its own directory on sys.path."""
    pytest.importorskip("mkdocs")
    # The docs extra is optional; CI type-checks tests without it.
    from mkdocs.config import load_config  # pyright: ignore[reportMissingImports]
    from mkdocs.structure.files import Files  # pyright: ignore[reportMissingImports]

    (tmp_path / "docs").mkdir()
    (tmp_path / "mkdocs.yml").write_text(
        f"site_name: test\nhooks:\n  - {Path(render.__file__).as_posix()}\n", encoding="utf-8"
    )
    config = load_config(str(tmp_path / "mkdocs.yml"))
    monkeypatch.setenv(render.ENV, str(DATA))

    files = config.plugins.on_files(Files([]), config=config)

    assert {file.src_uri for file in files} == {DATA_URI, LATEST_URI}
    data = files.get_file_from_path(DATA_URI)
    assert data is not None
    assert json.loads(data.content_string)["runs"] == 3
