"""Read-only snapshot metadata recovery tests."""

from __future__ import annotations

import json

from type_bridge.migration.snapshots import get_snapshot_metadata


def test_reads_existing_snapshot_metadata_without_writing(tmp_path) -> None:
    snapshot = tmp_path / "v0001"
    snapshot.mkdir()
    metadata = {
        "version": "v0001",
        "source_migration": "0001_initial",
        "schema_hash": "a" * 64,
        "file_hashes": {"schema.tql": "b" * 64},
    }
    manifest = snapshot / "snapshot.json"
    manifest.write_text(json.dumps(metadata), encoding="utf-8")
    before = {path.name: path.read_bytes() for path in snapshot.iterdir()}

    assert get_snapshot_metadata(snapshot) == metadata
    assert {path.name: path.read_bytes() for path in snapshot.iterdir()} == before


def test_missing_or_malformed_snapshot_metadata_is_not_repaired(tmp_path) -> None:
    snapshot = tmp_path / "v0001"
    snapshot.mkdir()
    assert get_snapshot_metadata(snapshot) is None
    assert list(snapshot.iterdir()) == []

    manifest = snapshot / "snapshot.json"
    manifest.write_text("not-json", encoding="utf-8")
    assert get_snapshot_metadata(snapshot) is None
    assert manifest.read_text(encoding="utf-8") == "not-json"
