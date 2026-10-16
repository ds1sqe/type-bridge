"""Append ``perf-result/v1`` files to the perf history and rebuild its index.

The data directory is a checkout of the ``perf-data`` branch::

    runs/<YYYY-MM-DD>/<sha>-<trigger>.json   # one result per (date, commit, trigger)
    index.json                               # rebuilt from runs/ on every store

The date is the run's ``started_at`` in UTC. A run is identified by (date,
commit, trigger), so nightly runs of an unchanged commit are kept as one
point per day. A result with the identity of a stored run is merged into it,
so every producer job of one workflow run (micro, pipeline, ...) lands in one
file: its metrics replace stored metrics with the same id and the others are
kept, its ``run`` block replaces the stored one except that ``started_at``
keeps the earlier instant and ``typedb`` and each toolchain keep a known
version over ``null``. A re-run of a producer on the same day therefore replaces its own
metrics, and storing the same result again changes nothing.

``index.json`` (format ``perf-index/v1``) holds, per metric id, its family,
its unit and a series of ``{commit, version, trigger, at, value, unit}``
points ordered by ``at``; the versions seen, in order of first appearance;
the run count; and ``latest``, the data-dir path of the newest run (``null``
when there is none).

Usage: ``python -m perf.harness.store --data-dir DIR RESULT...``. Every
result is validated first, and nothing is stored when any is invalid; the
exit code is then 1.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Literal

from perf.harness.validate import check, check_file

INDEX_FORMAT = "perf-index/v1"
RUNS = "runs"
INDEX = "index.json"

type Status = Literal["added", "merged", "unchanged"]


class StoreError(ValueError):
    """A result or a stored run is invalid."""


@dataclass(frozen=True)
class Stored:
    """Where a result went, relative to the data directory."""

    path: Path
    status: Status


def store(data_dir: Path, document: dict[str, Any]) -> Stored:
    """Write a validated result into ``data_dir``, merging it into a stored run
    with the same identity; does not touch the index."""
    if errors := check(document).errors:
        raise StoreError("; ".join(errors))
    run = document["run"]
    relative = Path(
        RUNS, _started_at(run).date().isoformat(), f"{run['commit']}-{run['trigger']}.json"
    )
    target = data_dir / relative

    existed = target.is_file()
    if existed:
        findings = check_file(target)
        if findings.errors:
            raise StoreError(f"{target}: stored run is invalid: {'; '.join(findings.errors)}")
        stored = json.loads(target.read_text(encoding="utf-8"))
        document = merge(stored, document)
        if document == stored:
            return Stored(relative, "unchanged")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return Stored(relative, "merged" if existed else "added")


def merge(stored: dict[str, Any], incoming: dict[str, Any]) -> dict[str, Any]:
    """``incoming`` merged into ``stored``, two results with the same identity.

    Metrics are united by id: an incoming metric replaces the stored one in
    place, new ids follow the stored ones. The run block is the incoming one,
    except that ``started_at`` is the earlier of the two, and ``typedb`` and
    each toolchain version are the incoming ones unless those are ``null``:
    a producer that does not use a tool reports it as ``null``.
    """
    incoming_by_id = {metric["id"]: metric for metric in incoming["metrics"]}
    stored_ids = {metric["id"] for metric in stored["metrics"]}
    metrics = [incoming_by_id.get(metric["id"], metric) for metric in stored["metrics"]]
    metrics += [metric for metric in incoming["metrics"] if metric["id"] not in stored_ids]

    old_run, new_run = stored["run"], incoming["run"]
    earlier = min(old_run, new_run, key=_started_at)
    old_tools, new_tools = old_run["toolchains"], new_run["toolchains"]
    toolchains = {name: _known(new_tools, old_tools, name) for name in old_tools | new_tools}
    run = new_run | {
        "started_at": earlier["started_at"],
        "typedb": _known(new_run, old_run, "typedb"),
        "toolchains": toolchains,
    }
    return incoming | {"run": run, "metrics": metrics}


def build_index(data_dir: Path) -> dict[str, Any]:
    """The ``perf-index/v1`` document for every run stored under ``data_dir``."""
    runs: list[tuple[datetime, str, str, Path, dict[str, Any]]] = []
    for path in sorted((data_dir / RUNS).glob("*/*.json")):
        findings = check_file(path)
        if findings.errors:
            raise StoreError(f"{path}: stored run is invalid: {'; '.join(findings.errors)}")
        document = json.loads(path.read_text(encoding="utf-8"))
        run = document["run"]
        relative = path.relative_to(data_dir)
        runs.append((_started_at(run), run["commit"], run["trigger"], relative, document))
    # Offsets may differ between runs, so order by instant, not by string.
    runs.sort(key=lambda entry: entry[:3])

    versions: list[str] = []
    metrics: dict[str, dict[str, Any]] = {}
    for _, _, _, _, document in runs:
        run = document["run"]
        if run["version"] not in versions:
            versions.append(run["version"])
        for metric in document["metrics"]:
            entry = metrics.setdefault(metric["id"], {"series": []})
            # The newest run names the family and unit the page labels.
            entry["family"] = metric["family"]
            entry["unit"] = metric["unit"]
            entry["series"].append(
                {
                    "commit": run["commit"],
                    "version": run["version"],
                    "trigger": run["trigger"],
                    "at": run["started_at"],
                    "value": metric["value"],
                    "unit": metric["unit"],
                }
            )
    return {
        "format": INDEX_FORMAT,
        "runs": len(runs),
        "versions": versions,
        "latest": runs[-1][3].as_posix() if runs else None,
        "metrics": {
            metric_id: {
                "family": entry["family"],
                "unit": entry["unit"],
                "series": entry["series"],
            }
            for metric_id, entry in sorted(metrics.items())
        },
    }


def write_index(data_dir: Path) -> dict[str, Any]:
    """Rebuild ``index.json`` in ``data_dir`` and return it."""
    index = build_index(data_dir)
    # Compact: the index grows with every run and is fetched by the page.
    text = json.dumps(index, separators=(",", ":")) + "\n"
    (data_dir / INDEX).write_text(text, encoding="utf-8")
    return index


def _known(preferred: dict[str, Any], fallback: dict[str, Any], name: str) -> Any:
    value = preferred.get(name)
    return value if value is not None else fallback.get(name)


def _started_at(run: dict[str, Any]) -> datetime:
    return datetime.fromisoformat(run["started_at"]).astimezone(UTC)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.store",
        description="Store perf-result/v1 files in the perf history and rebuild index.json.",
    )
    parser.add_argument(
        "--data-dir", type=Path, required=True, help="checkout of the perf-data branch"
    )
    parser.add_argument("results", nargs="+", type=Path, metavar="RESULT")
    arguments = parser.parse_args(argv)

    documents: list[dict[str, Any]] = []
    failed = False
    for path in arguments.results:
        findings = check_file(path)
        for message in findings.errors:
            print(f"{path}: error: {message}", file=sys.stderr)
        for message in findings.warnings:
            print(f"{path}: warning: {message}", file=sys.stderr)
        if findings.errors:
            failed = True
        else:
            documents.append(json.loads(path.read_text(encoding="utf-8")))
    if failed:
        print("nothing stored: fix the invalid result(s) first", file=sys.stderr)
        return 1

    data_dir: Path = arguments.data_dir
    data_dir.mkdir(parents=True, exist_ok=True)
    try:
        for path, document in zip(arguments.results, documents, strict=True):
            stored = store(data_dir, document)
            print(f"{path} -> {stored.path.as_posix()}: {stored.status}")
        index = write_index(data_dir)
    except StoreError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"{INDEX}: {len(index['metrics'])} metric(s) over {index['runs']} run(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
