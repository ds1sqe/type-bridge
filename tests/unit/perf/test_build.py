"""Cold-build runner and conversion of captured stable Cargo HTML timings."""

from pathlib import Path

import pytest

from perf.harness import build
from perf.harness.validate import check_file

FIXTURE = Path(__file__).parent / "fixtures/cargo-timings/cargo-timing.html"


def test_captured_cargo_units() -> None:
    units = build.convert(FIXTURE)
    assert units
    assert all(duration >= 0 for duration in units.values())
    assert "type-bridge-toml-transpiler" in units


def test_only_twenty_slowest_units_are_reported() -> None:
    result = build.metrics({"public": 42.0}, {f"crate-{i}": float(i) for i in range(25)})
    assert result[0].id == "build.public.wall"
    assert len(result) == 21
    assert result[1].id == "build.unit.crate_24"
    assert result[-1].value == 5


@pytest.mark.parametrize(
    "body", ["no data", "const UNIT_DATA = [];", 'const UNIT_DATA = [{"name":"x","duration":-1}];']
)
def test_bad_timings_fail(tmp_path: Path, body: str) -> None:
    report = tmp_path / "bad.html"
    report.write_text(body)
    with pytest.raises(ValueError):
        build.convert(report)


def test_cold_runner_writes_valid_result(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    def command(argv: list[str], log: Path, *, env: dict[str, str]) -> float:
        target = Path(env["CARGO_TARGET_DIR"])
        assert not any(target.iterdir())
        assert "--release" in argv and "--locked" in argv and "--timings" in argv
        timing = target / "cargo-timings/cargo-timing.html"
        timing.parent.mkdir()
        timing.write_bytes(FIXTURE.read_bytes())
        return 12.5

    monkeypatch.setattr(build, "timed", command)
    out = tmp_path / "build.json"
    arguments = [
        "--group",
        "public",
        "--package",
        "type-bridge-toml-transpiler",
        "--target-dir",
        str(tmp_path / "target"),
        "--out",
        str(out),
    ]
    assert build.main(arguments) == 0
    assert check_file(out).errors == []
    assert build.main(arguments) == 1  # Refuse the now-warm target.


def test_command_failure_does_not_publish(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import subprocess

    def fail(*args: object, **kwargs: object) -> float:
        raise subprocess.CalledProcessError(1, "cargo")

    monkeypatch.setattr(build, "timed", fail)
    out = tmp_path / "result.json"
    assert (
        build.main(["--group", "node", "--target-dir", str(tmp_path / "target"), "--out", str(out)])
        == 1
    )
    assert not out.exists()
