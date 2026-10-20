"""The offline binding-overhead converter: ``binding.<sdk>.<op>`` and the
host-minus-Rust overhead, from the drivers' ``--binding`` reports."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness import binding, micro
from perf.harness.validate import check_file

GITHUB = {
    "GITHUB_SHA": "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90",
    "GITHUB_REF_NAME": "master",
    "GITHUB_EVENT_NAME": "schedule",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20261012.1",
}


@pytest.fixture(autouse=True)
def _rustc(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(micro, "_rustc_version", lambda: "1.94.1")


def write_report(tmp_path: Path, sdk: str, medians: tuple[int, int, int]) -> Path:
    ops = [
        {"name": name, "samples_ns": [median - 10, median, median + 10]}
        for name, median in zip(binding.OPS, medians, strict=True)
    ]
    path = tmp_path / f"{sdk}.json"
    document = {"format": binding.REPORT_FORMAT, "binding": sdk, "warmup": 1, "ops": ops}
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def test_each_op_is_a_metric_and_hosts_carry_their_overhead_over_rust(tmp_path: Path) -> None:
    rust = binding.read_report(write_report(tmp_path, "rust", (100, 1000, 500)))
    node = binding.read_report(write_report(tmp_path, "node", (250, 3000, 400)))

    metrics, lines = binding.convert(dict([rust, node]))

    by_id = {metric.id: metric for metric in metrics}
    assert by_id["binding.rust.build_purchase"].p50 == 100
    assert by_id["binding.rust.build_purchase"].n == 3
    assert "binding.rust.build_purchase.overhead" not in by_id
    assert by_id["binding.node.build_purchase.overhead"].value == 150
    assert by_id["binding.node.build_rows_1k.overhead"].value == 2000
    # A host path cheaper than Rust's is a 0 measure; the line keeps the sign.
    assert by_id["binding.node.encode_rows_1k.overhead"].value == 0
    assert "binding.node.encode_rows_1k: p50 0.4 us, -0.1 us over rust" in lines
    assert all(metric.family == "binding" and metric.unit == "ns" for metric in metrics)


def test_the_rust_baseline_is_required(tmp_path: Path) -> None:
    node = binding.read_report(write_report(tmp_path, "node", (100, 200, 300)))
    with pytest.raises(ValueError, match="baseline"):
        binding.convert(dict([node]))


@pytest.mark.parametrize(
    ("document", "message"),
    [
        ({"format": "other"}, "expected format"),
        ({"format": binding.REPORT_FORMAT, "binding": "go", "ops": []}, "binding must be"),
        ({"format": binding.REPORT_FORMAT, "binding": "c", "ops": []}, "ops must be"),
        (
            {
                "format": binding.REPORT_FORMAT,
                "binding": "c",
                "ops": [{"name": "build_purchase", "samples_ns": [-1]}],
            },
            "non-negative",
        ),
    ],
)
def test_malformed_reports_are_rejected(
    tmp_path: Path, document: dict[str, object], message: str
) -> None:
    path = tmp_path / "bad.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(ValueError, match=message):
        binding.read_report(path)


def test_cli_merges_every_sdk_into_a_valid_result(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    reports = []
    for offset, sdk in enumerate(("rust", "python", "node", "c")):
        path = write_report(tmp_path, sdk, (100 + offset, 1000 + offset, 500 + offset))
        reports += ["--report", str(path), "--sdk", sdk]
    out = tmp_path / "out/binding.json"

    assert binding.main([*reports, "--out", str(out)], env=GITHUB) == 0

    assert check_file(out).errors == []
    ids = {metric["id"] for metric in json.loads(out.read_text(encoding="utf-8"))["metrics"]}
    assert len(ids) == 4 * 3 + 3 * 3
    assert "binding.c.encode_rows_1k.overhead" in ids
    assert "binding.python.build_rows_1k: p50 1.0 us, +0.0 us over rust" in capsys.readouterr().out


def test_cli_rejects_reports_that_do_not_cover_the_sdks(tmp_path: Path) -> None:
    rust = write_report(tmp_path, "rust", (100, 200, 300))
    out = tmp_path / "binding.json"
    code = binding.main(
        ["--report", str(rust), "--sdk", "rust", "--sdk", "c", "--out", str(out)], env=GITHUB
    )
    assert code == 1
    assert not out.exists()
