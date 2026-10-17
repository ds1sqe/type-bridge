"""MkDocs hook: publish the perf history beside the ``/perf/`` page.

``mkdocs.yml`` lists this file under ``hooks``. When the environment variable
``PERF_DATA_DIR`` names a checkout of the ``perf-data`` branch, the build
gains:

- ``perf/data.json``: that branch's ``index.json`` (see ``perf.harness.store``)
  plus ``latest_url`` when there is a newest run;
- ``perf/latest.json``: the newest run's raw result.

Without ``PERF_DATA_DIR``, or with a directory that has no ``index.json``,
``perf/data.json`` is an empty index and the page shows "no data yet". A
``PERF_DATA_DIR`` that is not a directory fails the build.

MkDocs loads a hook by file path with only this directory on ``sys.path``,
so the module depends on the standard library alone and imports MkDocs only
inside the event handler.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from mkdocs.config.defaults import MkDocsConfig
    from mkdocs.structure.files import Files

ENV = "PERF_DATA_DIR"
# Must equal ``perf.harness.store.INDEX_FORMAT``; a unit test keeps them in step.
INDEX_FORMAT = "perf-index/v1"
DATA_URI = "perf/data.json"
LATEST_URI = "perf/latest.json"


class RenderError(ValueError):
    """The data directory holds an index the page cannot show."""


def empty_index() -> dict[str, Any]:
    return {"format": INDEX_FORMAT, "runs": 0, "versions": [], "latest": None, "metrics": {}}


def site_files(data_dir: Path | None) -> dict[str, str]:
    """Generated site files, by docs-relative path, for the data in ``data_dir``."""
    if data_dir is None:
        return {DATA_URI: json.dumps(empty_index())}
    if not data_dir.is_dir():
        raise RenderError(f"{ENV}={data_dir}: not a directory")
    index_path = data_dir / "index.json"
    if not index_path.is_file():
        return {DATA_URI: json.dumps(empty_index())}
    try:
        index = json.loads(index_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as error:
        raise RenderError(f"{index_path}: cannot read the perf index: {error}") from error
    if not isinstance(index, dict) or index.get("format") != INDEX_FORMAT:
        raise RenderError(f"{index_path}: expected format {INDEX_FORMAT!r}")

    files: dict[str, str] = {}
    if latest := index.get("latest"):
        root = data_dir.resolve()
        latest_path = (root / latest).resolve()
        if not latest_path.is_relative_to(root / "runs") or not latest_path.is_file():
            raise RenderError(f"{index_path}: latest run {latest!r} is not a stored run")
        files[LATEST_URI] = latest_path.read_text(encoding="utf-8")
        index = index | {"latest_url": "latest.json"}
    files[DATA_URI] = json.dumps(index, separators=(",", ":"))
    return files


def data_dir_from_env(env: Mapping[str, str] = os.environ) -> Path | None:
    value = env.get(ENV, "").strip()
    return Path(value) if value else None


def on_files(files: Files, config: MkDocsConfig) -> Files:
    from mkdocs.structure.files import File

    for uri, content in site_files(data_dir_from_env()).items():
        files.append(File.generated(config, uri, content=content))
    return files
