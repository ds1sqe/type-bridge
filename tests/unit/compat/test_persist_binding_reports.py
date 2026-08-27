"""Create-new publication contract for validated four-binding report sets."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/ci"))
import persist_binding_reports as publisher  # noqa: E402


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
