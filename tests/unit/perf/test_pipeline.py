"""The pipeline converter: report parsing, percentiles, stages and the CLI.

``fixtures/pipeline-report/rust.json`` is real ``type-bridge-perf-pipelines``
output: three pipelines, three measured iterations each (two for the
migration), against TypeDB 3.13.6.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness import micro, pipeline
from perf.harness.validate import check_file

FIXTURES = Path(__file__).parent / "fixtures"
REPORT = FIXTURES / "pipeline-report/rust.json"
SHA = "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90"
GITHUB = {
    "GITHUB_SHA": SHA,
    "GITHUB_REF_NAME": "master",
    "GITHUB_EVENT_NAME": "schedule",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20261012.1",
}


@pytest.fixture(autouse=True)
def _rustc(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(micro, "_rustc_version", lambda: "1.94.1")


def write_report(tmp_path: Path, pipelines: list[dict[str, object]]) -> Path:
    path = tmp_path / "report.json"
    document = {"format": pipeline.REPORT_FORMAT, "binding": "rust", "pipelines": pipelines}
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def test_percentiles_interpolate_between_ranks() -> None:
    values = [40.0, 10.0, 30.0, 20.0]

    assert pipeline.percentile(values, 0.5) == 25.0
    assert pipeline.percentile(values, 0.9) == pytest.approx(37.0)
    assert pipeline.percentile([7.0], 0.99) == 7.0
    with pytest.raises(ValueError):
        pipeline.percentile([], 0.5)


def test_each_pipeline_becomes_a_staged_metric_and_a_throughput() -> None:
    binding, pipelines = pipeline.read_report(REPORT)
    metrics, summaries = pipeline.convert(binding, pipelines)

    ids = [metric.id for metric in metrics]
    assert ids == [
        "pipeline.rust.insert_one",
        "pipeline.rust.insert_one.throughput",
        "pipeline.rust.batch_given_rows_1k",
        "pipeline.rust.batch_given_rows_1k.throughput",
        "pipeline.rust.migration_apply",
        "pipeline.rust.migration_apply.throughput",
    ]
    insert = metrics[0]
    totals = [sample.total_ns for sample in pipelines[0].samples]
    assert insert.unit == "ns" and insert.family == "pipeline"
    assert insert.value == insert.p50 == sorted(totals)[1]
    assert insert.n == 3 and insert.samples == totals
    assert insert.p90 is not None and insert.p99 is not None
    assert insert.value <= insert.p90 <= insert.p99 <= max(totals)
    assert insert.stages is not None
    assert set(insert.stages) == {
        "tb.commit",
        "tb.decode",
        "tb.orm.plan",
        "tb.query.compile",
        "tb.runtime.execute",
    }
    assert sum(insert.stages.values()) < insert.value
    # A batch iteration is 1,000 row operations.
    batch, batch_rate = metrics[2], metrics[3]
    assert batch.samples is not None
    assert batch_rate.unit == "ops_per_s"
    assert batch_rate.value == pytest.approx(1000 * 3 * 1e9 / sum(batch.samples))
    assert [summary.metric for summary in summaries] == ids[::2]
    assert all(0 < summary.attributed < 1 for summary in summaries)


def test_a_stage_missing_from_an_iteration_counts_as_zero(tmp_path: Path) -> None:
    report = write_report(
        tmp_path,
        [
            {
                "name": "fetch",
                "operations": 1,
                "samples": [
                    {"total_ns": 100, "stages": {"tb.runtime.execute": 60, "tb.decode": 10}},
                    {"total_ns": 120, "stages": {"tb.runtime.execute": 70}},
                    {"total_ns": 110, "stages": {"tb.runtime.execute": 65, "tb.decode": 30}},
                ],
            }
        ],
    )

    metrics, [summary] = pipeline.convert(*pipeline.read_report(report))

    assert metrics[0].stages == {"tb.decode": 10, "tb.runtime.execute": 65}
    # Unattributed medians 30, 50, 15 -> 30; stages 75 + 30 = 105 vs p50 110.
    assert summary.gap == pytest.approx(5 / 110)
    assert summary.attributed == pytest.approx(75 / 110)


def test_a_pipeline_without_stages_has_no_stage_map(tmp_path: Path) -> None:
    report = write_report(
        tmp_path,
        [{"name": "ping", "operations": 1, "samples": [{"total_ns": 5, "stages": {}}]}],
    )

    metrics, _ = pipeline.convert(*pipeline.read_report(report))

    assert metrics[0].stages is None


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ({"format": "other/v1"}, "expected format"),
        ({"binding": ""}, "binding"),
        ({"pipelines": []}, "pipelines must be"),
        ({"pipelines": [{"name": "x", "operations": 0, "samples": []}]}, "operations"),
        ({"pipelines": [{"name": "x", "operations": 1, "samples": []}]}, "samples"),
        (
            {"pipelines": [{"name": "x", "operations": 1, "samples": [{"total_ns": -1}]}]},
            "total_ns",
        ),
        (
            {
                "pipelines": [
                    {
                        "name": "x",
                        "operations": 1,
                        "samples": [{"total_ns": 1, "stages": {"compile": 1}}],
                    }
                ]
            },
            "dotted name",
        ),
    ],
)
def test_malformed_reports_are_rejected(
    tmp_path: Path, change: dict[str, object], message: str
) -> None:
    document = json.loads(REPORT.read_text(encoding="utf-8")) | change
    path = tmp_path / "report.json"
    path.write_text(json.dumps(document), encoding="utf-8")

    with pytest.raises(ValueError, match=message):
        pipeline.read_report(path)


def test_cli_writes_a_valid_result_with_the_typedb_version(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    out = tmp_path / "out/pipeline.json"

    code = pipeline.main(
        [
            "--report",
            str(REPORT),
            "--typedb",
            "3.13.6",
            "--out",
            str(out),
            "--started-at",
            "2026-10-17T03:17:00+00:00",
        ],
        env=GITHUB,
    )

    assert code == 0
    assert check_file(out).errors == []
    document = json.loads(out.read_text(encoding="utf-8"))
    assert document["run"]["typedb"] == "3.13.6"
    assert document["run"]["trigger"] == "schedule"
    assert document["run"]["toolchains"]["rust"] == "1.94.1"
    printed = capsys.readouterr().out
    assert "pipeline.rust.insert_one: p50" in printed
    assert "stage-sum gap" in printed


def test_cli_fails_without_writing_on_a_bad_report(tmp_path: Path) -> None:
    bad = tmp_path / "bad.json"
    bad.write_text("{}", encoding="utf-8")
    out = tmp_path / "pipeline.json"

    code = pipeline.main(
        ["--report", str(bad), "--typedb", "3.13.6", "--out", str(out)], env=GITHUB
    )

    assert code == 1
    assert not out.exists()


def test_workflow_runs_the_pipelines_on_a_tmpfs_typedb_outside_pull_requests() -> None:
    import yaml

    workflow = yaml.safe_load((micro.REPO_ROOT / ".github/workflows/perf.yml").read_text())
    job = workflow["jobs"]["pipeline"]

    assert job["if"] == "github.event_name != 'pull_request'"
    service = job["services"]["typedb"]
    assert service["image"] == "typedb/typedb:3.13.6"
    assert service["options"] == "--tmpfs /var/lib/typedb/data:rw"
    commands = "\n".join(step.get("run", "") for step in job["steps"])
    assert "-p type-bridge-perf --bin type-bridge-perf-pipelines" in commands
    assert "python -m perf.harness.pipeline" in commands
    assert '--typedb "${{ steps.typedb.outputs.version }}"' in commands


def _report_as(tmp_path: Path, binding: str) -> Path:
    document = json.loads(REPORT.read_text(encoding="utf-8")) | {"binding": binding}
    path = tmp_path / f"{binding}.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def test_cli_merges_one_report_per_sdk(tmp_path: Path) -> None:
    out = tmp_path / "pipeline.json"
    reports = [_report_as(tmp_path, sdk) for sdk in ("rust", "python", "node", "c")]
    arguments = [part for report in reports for part in ("--report", str(report))]
    sdks = [part for sdk in pipeline.SDKS for part in ("--sdk", sdk)]

    code = pipeline.main([*arguments, *sdks, "--typedb", "3.13.6", "--out", str(out)], env=GITHUB)

    assert code == 0
    assert check_file(out).errors == []
    ids = {metric["id"] for metric in json.loads(out.read_text(encoding="utf-8"))["metrics"]}
    assert {f"pipeline.{sdk}.insert_one" for sdk in pipeline.SDKS} <= ids


@pytest.mark.parametrize(
    ("bindings", "sdks", "message"),
    [
        (["rust", "rust"], [], "a second report for rust"),
        (["rust"], ["rust", "python"], "expected"),
        (["ruby"], [], "binding must be one of"),
    ],
)
def test_cli_rejects_reports_that_do_not_cover_the_sdks(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    bindings: list[str],
    sdks: list[str],
    message: str,
) -> None:
    out = tmp_path / "pipeline.json"
    arguments: list[str] = []
    for index, binding in enumerate(bindings):
        directory = tmp_path / str(index)
        directory.mkdir()
        path = _report_as(directory, binding)
        arguments += ["--report", str(path)]
    sdk_arguments = [part for sdk in sdks for part in ("--sdk", sdk)]

    code = pipeline.main(
        [*arguments, *sdk_arguments, "--typedb", "3.13.6", "--out", str(out)], env=GITHUB
    )

    assert code == 1
    assert message in capsys.readouterr().err
    assert not out.exists()
