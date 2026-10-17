"""The perf history store: run files, merging, idempotency and ``index.json``."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from perf.harness.store import INDEX, INDEX_FORMAT, StoreError, build_index, main, store

REPO_ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).parent / "fixtures"
DATA = FIXTURES / "data"
SHA = "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90"
OTHER_SHA = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c"
INSERT = "micro.core.compilation.insert_entity"

type Document = dict[str, Any]


def _micro() -> Document:
    return json.loads((FIXTURES / "micro.json").read_text(encoding="utf-8"))


def _older() -> Document:
    """Two days before the micro fixture, on an earlier release."""
    document = _micro()
    document["run"].update(
        commit=OTHER_SHA,
        version="2.2.6",
        trigger="schedule",
        started_at="2026-10-14T03:17:02Z",
    )
    document["metrics"][0]["value"] = 1902.6
    return document


def _write(path: Path, document: Document) -> Path:
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def _files(data_dir: Path) -> list[str]:
    return sorted(path.relative_to(data_dir).as_posix() for path in data_dir.rglob("*.json"))


def test_first_run_on_an_empty_branch(tmp_path: Path) -> None:
    assert build_index(tmp_path)["runs"] == 0

    stored = store(tmp_path, _micro())

    assert stored.status == "added"
    assert stored.path.as_posix() == f"runs/2026-10-16/{SHA}-push.json"
    assert json.loads((tmp_path / stored.path).read_text(encoding="utf-8")) == _micro()
    index = build_index(tmp_path)
    assert index["format"] == INDEX_FORMAT
    assert index["runs"] == 1
    assert index["versions"] == ["2.3.0.dev0"]
    assert index["latest"] == stored.path.as_posix()
    assert index["metrics"][INSERT] == {
        "family": "micro",
        "unit": "ns",
        "series": [
            {
                "commit": SHA,
                "version": "2.3.0.dev0",
                "trigger": "push",
                "at": "2026-10-16T03:12:45Z",
                "value": 1843.2,
                "unit": "ns",
            }
        ],
    }


def test_restoring_the_same_result_changes_nothing(tmp_path: Path) -> None:
    store(tmp_path, _micro())
    before = (tmp_path / f"runs/2026-10-16/{SHA}-push.json").stat().st_mtime_ns

    again = store(tmp_path, _micro())

    assert again.status == "unchanged"
    assert (tmp_path / again.path).stat().st_mtime_ns == before
    assert _files(tmp_path) == [f"runs/2026-10-16/{SHA}-push.json"]


def test_same_day_rerun_replaces_its_own_metrics(tmp_path: Path) -> None:
    store(tmp_path, _micro())
    rerun = _micro()
    rerun["run"]["started_at"] = "2026-10-16T23:55:00Z"
    rerun["metrics"][0]["value"] = 1850.0

    stored = store(tmp_path, rerun)

    assert stored.status == "merged"
    assert _files(tmp_path) == [f"runs/2026-10-16/{SHA}-push.json"]
    run_file = json.loads((tmp_path / stored.path).read_text(encoding="utf-8"))
    assert [metric["value"] for metric in run_file["metrics"]] == [1850.0, 21544]
    # The earlier start of the same identity is kept.
    assert run_file["run"]["started_at"] == "2026-10-16T03:12:45Z"
    [point] = build_index(tmp_path)["metrics"][INSERT]["series"]
    assert point["value"] == 1850.0


def _pipeline() -> Document:
    """A second producer of the micro fixture's run: same commit, day and trigger."""
    document = json.loads((FIXTURES / "pipeline.json").read_text(encoding="utf-8"))
    document["run"].update(trigger="push", started_at="2026-10-16T03:40:00Z")
    document["run"]["toolchains"]["rust"] = None
    return document


def test_two_producers_of_one_run_are_merged_into_one_file(tmp_path: Path) -> None:
    micro, pipeline = _micro(), _pipeline()
    store(tmp_path, micro)

    stored = store(tmp_path, pipeline)

    assert stored.status == "merged"
    assert _files(tmp_path) == [f"runs/2026-10-16/{SHA}-push.json"]
    run_file = json.loads((tmp_path / stored.path).read_text(encoding="utf-8"))
    assert run_file["metrics"] == micro["metrics"] + pipeline["metrics"]
    assert run_file["run"] == pipeline["run"] | {
        "started_at": micro["run"]["started_at"],
        # Each producer knows only its own tools; null never wins.
        "toolchains": {"rust": "1.88.0", "python": "3.13.7", "node": "20.19.5", "cc": "gcc 13.3.0"},
    }
    assert store(tmp_path, pipeline).status == "unchanged"
    assert store(tmp_path, micro).status == "unchanged"

    index = build_index(tmp_path)
    assert index["runs"] == 1
    metrics = micro["metrics"] + pipeline["metrics"]
    assert sorted(index["metrics"]) == sorted(metric["id"] for metric in metrics)
    for metric in metrics:
        entry = index["metrics"][metric["id"]]
        assert entry["family"] == metric["family"]
        [point] = entry["series"]
        assert (point["at"], point["value"]) == (micro["run"]["started_at"], metric["value"])


def test_rerun_of_one_producer_keeps_the_other_producers_metrics(tmp_path: Path) -> None:
    store(tmp_path, _micro())
    store(tmp_path, _pipeline())
    rerun = _micro()
    rerun["metrics"][0]["value"] = 1850.0

    store(tmp_path, rerun)

    index = build_index(tmp_path)
    [point] = index["metrics"][INSERT]["series"]
    assert point["value"] == 1850.0
    [pipeline_point] = index["metrics"]["pipeline.python.insert"]["series"]
    assert pipeline_point["value"] == 912000


def _nightly(started_at: str, value: float) -> Document:
    document = _micro()
    document["run"].update(trigger="schedule", started_at=started_at)
    document["metrics"][0]["value"] = value
    return document


def test_nightly_runs_of_one_commit_on_different_days_are_kept(tmp_path: Path) -> None:
    store(tmp_path, _nightly("2026-10-16T03:17:00Z", 1843.2))

    stored = store(tmp_path, _nightly("2026-10-17T03:17:00Z", 1851.0))

    assert stored.status == "added"
    assert _files(tmp_path) == [
        f"runs/2026-10-16/{SHA}-schedule.json",
        f"runs/2026-10-17/{SHA}-schedule.json",
    ]
    index = build_index(tmp_path)
    assert index["runs"] == 2
    assert index["latest"] == f"runs/2026-10-17/{SHA}-schedule.json"
    series = index["metrics"][INSERT]["series"]
    assert [(point["at"], point["value"]) for point in series] == [
        ("2026-10-16T03:17:00Z", 1843.2),
        ("2026-10-17T03:17:00Z", 1851.0),
    ]


def test_nightly_runs_of_one_commit_on_the_same_day_are_one_point(tmp_path: Path) -> None:
    store(tmp_path, _nightly("2026-10-16T03:17:00Z", 1843.2))

    stored = store(tmp_path, _nightly("2026-10-16T09:00:00Z", 1851.0))

    assert stored.status == "merged"
    [point] = build_index(tmp_path)["metrics"][INSERT]["series"]
    assert (point["at"], point["value"]) == ("2026-10-16T03:17:00Z", 1851.0)


def test_another_trigger_of_the_same_commit_is_a_separate_run(tmp_path: Path) -> None:
    store(tmp_path, _micro())
    nightly = _micro()
    nightly["run"].update(trigger="schedule", started_at="2026-10-16T03:17:00Z")

    store(tmp_path, nightly)

    assert build_index(tmp_path)["runs"] == 2


def test_two_versions_in_one_series_ordered_by_time(tmp_path: Path) -> None:
    # Stored newest first: the index orders by ``at``, not by store order.
    store(tmp_path, _micro())
    store(tmp_path, _older())

    index = build_index(tmp_path)

    assert index["versions"] == ["2.2.6", "2.3.0.dev0"]
    assert index["latest"] == f"runs/2026-10-16/{SHA}-push.json"
    series = index["metrics"][INSERT]["series"]
    assert [(point["version"], point["value"]) for point in series] == [
        ("2.2.6", 1902.6),
        ("2.3.0.dev0", 1843.2),
    ]


def test_runs_are_ordered_by_instant_across_offsets(tmp_path: Path) -> None:
    late = _micro()
    late["run"]["started_at"] = "2026-10-16T05:00:00+09:00"  # 2026-10-15T20:00Z
    store(tmp_path, late)
    store(tmp_path, _older() | {"run": _older()["run"] | {"started_at": "2026-10-15T21:00:00Z"}})

    series = build_index(tmp_path)["metrics"][INSERT]["series"]

    assert [point["commit"] for point in series] == [SHA, OTHER_SHA]
    assert (tmp_path / f"runs/2026-10-15/{SHA}-push.json").is_file()


def test_invalid_result_is_rejected(tmp_path: Path) -> None:
    document = _micro()
    document["metrics"][0]["unit"] = "ms"

    with pytest.raises(StoreError, match=r"metrics\[0\]\.unit"):
        store(tmp_path, document)
    assert not (tmp_path / "runs").exists()


def test_invalid_stored_run_fails_the_index_and_a_merge(tmp_path: Path) -> None:
    stored = store(tmp_path, _micro())
    (tmp_path / stored.path).write_text("{}", encoding="utf-8")

    with pytest.raises(StoreError, match="stored run is invalid"):
        build_index(tmp_path)
    with pytest.raises(StoreError, match="stored run is invalid"):
        store(tmp_path, _micro())


def test_cli_stores_and_writes_the_index(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    data_dir = tmp_path / "perf-data"
    older = _write(tmp_path / "older.json", _older())

    assert main(["--data-dir", str(data_dir), str(FIXTURES / "micro.json"), str(older)]) == 0

    out = capsys.readouterr().out
    assert f"runs/2026-10-16/{SHA}-push.json: added" in out
    assert "2 metric(s) over 2 run(s)" in out
    index = json.loads((data_dir / INDEX).read_text(encoding="utf-8"))
    assert index == build_index(data_dir)

    assert main(["--data-dir", str(data_dir), str(FIXTURES / "micro.json")]) == 0
    assert ": unchanged" in capsys.readouterr().out


def test_cli_stores_nothing_when_any_result_is_invalid(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    data_dir = tmp_path / "perf-data"
    broken = copy.deepcopy(_older())
    broken["run"]["commit"] = "c7ee52d3"
    broken_path = _write(tmp_path / "broken.json", broken)

    code = main(["--data-dir", str(data_dir), str(FIXTURES / "micro.json"), str(broken_path)])

    assert code == 1
    assert f"{broken_path}: error: run.commit:" in capsys.readouterr().err
    assert not data_dir.exists()


def test_cli_module_entry_point(tmp_path: Path) -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "perf.harness.store",
            "--data-dir",
            str(tmp_path),
            str(FIXTURES / "micro.json"),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert (tmp_path / INDEX).is_file()


def test_fixture_data_dir_index_is_current() -> None:
    """``fixtures/data`` is what the store wrote; rebuild it if this fails."""
    stored = json.loads((DATA / INDEX).read_text(encoding="utf-8"))
    assert stored == build_index(DATA)
    assert stored["versions"] == ["2.2.6", "2.3.0.dev0"]
    # Two nightly runs of one commit on different days, then a push.
    assert stored["runs"] == 3


def _release(sdk: str, version: str, *, head: bool = False, value: float = 3.2e6) -> Document:
    """One ``release-matrix`` result of the micro fixture's commit."""
    document = _micro()
    document["run"].update(
        version=version,
        trigger="release-matrix",
        source="tree" if head else "registry",
        typedb="3.12.3",
        started_at="2026-10-16T04:00:00Z",
    )
    document["metrics"] = [
        {"id": f"release.{sdk}.{op}", "family": "release", "unit": "ns", "value": value}
        for op in ("insert_one", "update")
    ]
    return document


def test_release_matrix_keeps_every_sdk_and_version_of_one_run(tmp_path: Path) -> None:
    # One workflow run: shared commit, trigger and day, two SDKs, three series.
    matrix = [
        _release(sdk, version, head=version == "2.2.6")
        for sdk in ("python", "node")
        for version in ("2.0.2", "2.2.5", "2.2.6")
    ]
    paths = [store(tmp_path, document).path.as_posix() for document in matrix]
    store(tmp_path, _micro())

    day = f"runs/2026-10-16/{SHA}"
    assert paths == [
        f"{day}-release-matrix-{sdk}-{series}.json"
        for sdk in ("python", "node")
        for series in ("2.0.2", "2.2.5", "head")
    ]
    index = build_index(tmp_path)
    assert index["runs"] == 7
    # The tree's own runs name the versions and the latest run.
    assert index["versions"] == ["2.3.0.dev0"]
    assert index["latest"] == f"{day}-push.json"
    for sdk in ("python", "node"):
        series = index["metrics"][f"release.{sdk}.update"]["series"]
        assert [(point["series"], point["version"]) for point in series] == [
            ("2.0.2", "2.0.2"),
            ("2.2.5", "2.2.5"),
            ("head", "2.2.6"),
        ]
    assert all("series" not in point for point in index["metrics"][INSERT]["series"])


def test_release_matrix_rerun_merges_only_its_own_series(tmp_path: Path) -> None:
    store(tmp_path, _release("python", "2.2.5"))
    store(tmp_path, _release("python", "2.2.5", head=True))

    stored = store(tmp_path, _release("python", "2.2.5", value=4.0e6))

    assert stored.status == "merged"
    series = build_index(tmp_path)["metrics"]["release.python.update"]["series"]
    # Head built at 2.2.5 stays apart from the 2.2.5 release.
    assert [(point["series"], point["value"]) for point in series] == [
        ("2.2.5", 4.0e6),
        ("head", 3.2e6),
    ]


@pytest.mark.parametrize(
    "ids",
    [["release.python.update", "release.node.update"], ["release.python.update", "micro.x.y"]],
)
def test_release_matrix_result_must_name_one_sdk(tmp_path: Path, ids: list[str]) -> None:
    document = _release("python", "2.2.5")
    for metric, metric_id in zip(document["metrics"], ids, strict=True):
        metric["id"] = metric_id

    with pytest.raises(StoreError, match="release.<sdk>.<op> of one SDK"):
        store(tmp_path, document)
    assert _files(tmp_path) == []
