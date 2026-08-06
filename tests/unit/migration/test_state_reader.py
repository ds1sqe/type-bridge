"""The retained Python ledger facade is observably read-only."""

from __future__ import annotations

from typing import cast

from type_bridge.migration.state import MigrationStateManager
from type_bridge.session import Database


class _Reader:
    def load_applied(self):
        return [
            {
                "app_label": "archive",
                "name": "0001_initial",
                "applied_at": "2025-01-01T00:00:00",
                "checksum": "abc",
            }
        ]

    def load_runs(self):
        return []


def test_state_manager_exposes_reads_only() -> None:
    manager = MigrationStateManager.__new__(MigrationStateManager)
    manager.db = cast(Database, object())
    manager._rust_reader = _Reader()

    state = manager.load_state()
    assert state.is_applied("archive", "0001_initial")
    assert manager.load_runs() == []
    for name in (
        "ensure_schema",
        "record_applied",
        "record_unapplied",
        "record_run_started",
        "record_run_finished",
    ):
        assert not hasattr(manager, name), name
