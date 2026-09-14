"""Create-new publication contract for validated four-binding report sets."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/ci"))
SPEC = importlib.util.spec_from_file_location(
    "persist_binding_reports", ROOT / "scripts/ci/persist_binding_reports.py"
)
assert SPEC is not None and SPEC.loader is not None
publisher = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = publisher
SPEC.loader.exec_module(publisher)


def reports(tmp_path: Path) -> tuple[Path, ...]:
    values = []
    for binding in publisher.BINDINGS:
        path = tmp_path / f"source-{binding}.json"
        path.write_bytes(f'{{"binding":"{binding}"}}'.encode())
        values.append(path)
    return tuple(values)


def test_publish_preserves_exact_bytes_under_binding_names(tmp_path: Path) -> None:
    sources = reports(tmp_path)
    output = tmp_path / "published"
    publisher.publish(sources, output, checkout=ROOT)
    for binding, source in zip(publisher.BINDINGS, sources, strict=True):
        assert (output / f"{binding}.json").read_bytes() == source.read_bytes()


def test_publish_rejects_existing_output(tmp_path: Path) -> None:
    output = tmp_path / "published"
    output.mkdir()
    with pytest.raises(publisher.PublishError, match="new directory"):
        publisher.publish(reports(tmp_path), output, checkout=ROOT)


def test_publish_rejects_incomplete_set(tmp_path: Path) -> None:
    with pytest.raises(publisher.PublishError, match="exactly four"):
        publisher.publish((), tmp_path / "published", checkout=ROOT)


def test_nested_publication_preserves_bytes_and_is_create_new(tmp_path: Path) -> None:
    source = reports(tmp_path)[0]
    output = tmp_path / "published"
    publisher.publish_files(
        {"v1/python.json": source, "v2/python.json": source}, output, checkout=ROOT
    )
    assert (output / "v1/python.json").read_bytes() == source.read_bytes()
    assert (output / "v2/python.json").read_bytes() == source.read_bytes()
    with pytest.raises(publisher.PublishError, match="new directory"):
        publisher.publish_files({"v1/python.json": source}, output, checkout=ROOT)


@pytest.mark.parametrize(
    "name",
    [
        "../escape.json",
        "/escape.json",
        "v1/../../escape.json",
        "v1//python.json",
        "v1/./python.json",
        "v1\\python.json",
    ],
)
def test_nested_publication_rejects_ambiguous_names(tmp_path: Path, name: str) -> None:
    source = reports(tmp_path)[0]
    with pytest.raises(publisher.PublishError, match="relative JSON path"):
        publisher.publish_files({name: source}, tmp_path / "published", checkout=ROOT)
    assert not (tmp_path / "published").exists()


def test_copy_failure_leaves_no_partial_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = reports(tmp_path)[0]
    copy = publisher.shutil.copyfile
    calls = 0

    def fail_second(source: Path, target: Path) -> None:
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("copy failed")
        copy(source, target)

    monkeypatch.setattr(publisher.shutil, "copyfile", fail_second)
    with pytest.raises(publisher.PublishError, match="could not be persisted"):
        publisher.publish_files(
            {"v1/python.json": source, "v2/python.json": source},
            tmp_path / "published",
            checkout=ROOT,
        )
    assert not (tmp_path / "published").exists()
    assert not list(tmp_path.glob(".published.*"))


@pytest.mark.parametrize("operation", ["mkstemp", "fsync", "link"])
def test_single_file_publication_cleans_up_io_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    class EvidenceError(ValueError):
        def __init__(self, code: str, message: str) -> None:
            self.code = code
            super().__init__(message)

    def fail(*_args, **_kwargs):
        raise OSError("injected I/O failure")

    module = publisher.tempfile if operation == "mkstemp" else publisher.os
    monkeypatch.setattr(module, operation, fail)
    output = tmp_path / "report.json"
    with pytest.raises(EvidenceError) as rejected:
        publisher.publish_bytes(output, b"{}\n", EvidenceError)
    assert rejected.value.code == "report_publication_failed"
    assert not list(tmp_path.iterdir())
