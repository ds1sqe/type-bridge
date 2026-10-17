"""Byte measurements of an actual npm pack fixture and invalid artifacts."""

import json
from pathlib import Path

import pytest

from perf.harness import size
from perf.harness.validate import check_file

FIXTURE = Path(__file__).parent / "fixtures/artifact-size"


def test_captured_npm_pack_size_matches_archive() -> None:
    captured = json.loads((FIXTURE / "npm-pack.json").read_text())["perf-size-fixture"]
    [metric] = size.convert({"npm": FIXTURE / "package.npm"})
    assert metric.value == captured["size"]
    assert (metric.id, metric.unit, metric.family) == ("size.npm.bytes", "bytes", "size")


def test_cli_writes_valid_result(tmp_path: Path) -> None:
    out = tmp_path / "size.json"
    assert size.main(["--artifact", f"npm={FIXTURE / 'package.npm'}", "--out", str(out)]) == 0
    assert not check_file(out).errors


@pytest.mark.parametrize("kind", ["missing", "empty", "directory"])
def test_invalid_artifact_fails(tmp_path: Path, kind: str) -> None:
    artifact = tmp_path / kind
    if kind == "empty":
        artifact.touch()
    elif kind == "directory":
        artifact.mkdir()
    with pytest.raises(ValueError):
        size.convert({kind: artifact})


@pytest.mark.parametrize("names", [("npm", "npm"), ("npm-a", "npm_a")])
def test_duplicate_metric_identity_fails(tmp_path: Path, names: tuple[str, str]) -> None:
    out = tmp_path / "size.json"
    args = [arg for name in names for arg in ["--artifact", f"{name}={FIXTURE / 'package.npm'}"]]
    assert size.main([*args, "--out", str(out)]) == 1
    assert not out.exists()


def test_output_cannot_destroy_artifact(tmp_path: Path) -> None:
    artifact = tmp_path / "archive"
    artifact.write_bytes(b"keep")
    assert size.main(["--artifact", f"npm={artifact}", "--out", str(artifact)]) == 1
    assert artifact.read_bytes() == b"keep"
