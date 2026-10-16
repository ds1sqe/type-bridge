"""The micro CLI: run block from the environment, conversion and validation."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness import micro
from perf.harness.validate import check_file

FIXTURES = Path(__file__).parent / "fixtures"
SHA = "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90"
GITHUB = {
    "GITHUB_SHA": SHA,
    "GITHUB_REF_NAME": "master",
    "GITHUB_EVENT_NAME": "push",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20261012.1",
}


@pytest.fixture(autouse=True)
def _rustc(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(micro, "_rustc_version", lambda: "1.94.1")


def test_run_block_from_github_actions() -> None:
    run = micro.collect_run(GITHUB, started_at="2026-10-16T03:12:45+00:00")

    assert run.commit == SHA
    assert run.branch == "master"
    assert run.trigger == "push"
    assert run.runner.image == "ubuntu24/20261012.1"
    assert run.runner.cores >= 1
    assert run.toolchains.rust == "1.94.1"
    assert run.toolchains.python is None
    assert run.typedb is None
    assert run.started_at == "2026-10-16T03:12:45+00:00"


def test_pull_request_records_the_head_branch() -> None:
    env = GITHUB | {
        "GITHUB_EVENT_NAME": "pull_request",
        "GITHUB_REF_NAME": "42/merge",
        "GITHUB_HEAD_REF": "feat/perf",
    }

    run = micro.collect_run(env)

    assert (run.branch, run.trigger) == ("feat/perf", "pull_request")


def test_local_run_without_github() -> None:
    run = micro.collect_run({})

    assert run.trigger == "local"
    assert len(run.commit) == 40
    assert run.branch


def test_unknown_event_is_rejected() -> None:
    with pytest.raises(ValueError, match="workflow_run"):
        micro.collect_run(GITHUB | {"GITHUB_EVENT_NAME": "workflow_run"})


def test_cli_writes_a_valid_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    for name, value in GITHUB.items():
        monkeypatch.setenv(name, value)
    out = tmp_path / "out/result.json"

    code = micro.main(
        [
            "--criterion-dir",
            str(FIXTURES / "criterion"),
            "--iai-dir",
            str(FIXTURES / "iai"),
            "--started-at",
            "2026-10-16T03:12:45Z",
            "--out",
            str(out),
        ]
    )

    assert code == 0
    assert "3 metric(s)" in capsys.readouterr().out
    assert check_file(out).errors == []
    document = json.loads(out.read_text(encoding="utf-8"))
    assert [metric["id"] for metric in document["metrics"]] == [
        "micro.core.validate_type_name.simple",
        "micro.core.validate_type_name.simple.instructions",
        "micro.orm.serde_attribute_value.string_roundtrip.instructions",
    ]


def test_cli_fails_on_conversion_error(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    out = tmp_path / "result.json"

    code = micro.main(["--criterion-dir", str(tmp_path / "absent"), "--out", str(out)])

    assert code == 1
    assert "not a directory" in capsys.readouterr().err
    assert not out.exists()


def test_cli_fails_when_a_directory_is_empty(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("GITHUB_EVENT_NAME", "workflow_dispatch")
    criterion_dir = FIXTURES / "criterion"
    out = tmp_path / "result.json"

    code = micro.main(
        ["--criterion-dir", str(criterion_dir), "--iai-dir", str(tmp_path), "--out", str(out)]
    )

    assert code == 1
    assert "no gungraun summaries" in capsys.readouterr().err
    assert not out.exists()


def test_cli_needs_an_input_directory(tmp_path: Path) -> None:
    with pytest.raises(SystemExit) as raised:
        micro.main(["--out", str(tmp_path / "result.json")])
    assert raised.value.code == 2
