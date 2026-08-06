"""Read-only records and reader for a frozen archive migration ledger."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from type_bridge import _rust_runtime

if TYPE_CHECKING:
    from type_bridge.session import Database


@dataclass
class MigrationRecord:
    """One applied record read from the frozen ledger."""

    app_label: str
    name: str
    applied_at: str
    checksum: str

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MigrationRecord):
            return NotImplemented
        return self.app_label == other.app_label and self.name == other.name


@dataclass
class MigrationRunRecord:
    """One historical migration execution record."""

    run_id: str
    app_label: str
    name: str
    checksum: str
    direction: str
    status: str
    started_at: str
    finished_at: str | None = None
    error: str | None = None
    executor_ip: str | None = None
    executor_mac: str | None = None


@dataclass
class MigrationState:
    """In-memory projection of applied archived migration records."""

    applied: list[MigrationRecord] = field(default_factory=list)
    version: str = "1.0"

    def is_applied(self, app_label: str, name: str) -> bool:
        return any(row.app_label == app_label and row.name == name for row in self.applied)

    def get_latest(self, app_label: str) -> MigrationRecord | None:
        rows = [row for row in self.applied if row.app_label == app_label]
        return rows[-1] if rows else None

    def get_all_for_app(self, app_label: str) -> list[MigrationRecord]:
        return [row for row in self.applied if row.app_label == app_label]


class MigrationStateManager:
    """Read an existing archive ledger without bootstrapping or mutating it."""

    ENTITY_NAME = _rust_runtime.applied_migration_entity_label()

    def __init__(self, db: Database):
        self.db = db
        self._rust_reader: Any = None

    @property
    def _reader(self) -> Any:
        if self._rust_reader is None:
            from type_bridge._rust_runtime import state_reader_for

            self._rust_reader = state_reader_for(self.db)
        return self._rust_reader

    def load_state(self) -> MigrationState:
        rows = [
            MigrationRecord(
                app_label=str(row["app_label"]),
                name=str(row["name"]),
                applied_at=_optional_str(row.get("applied_at")) or "",
                checksum=str(row["checksum"]),
            )
            for row in self._reader.load_applied()
        ]
        return MigrationState(applied=rows)

    def load_runs(self) -> list[MigrationRunRecord]:
        return [
            MigrationRunRecord(
                run_id=str(row["run_id"]),
                app_label=str(row["app_label"]),
                name=str(row["name"]),
                checksum=str(row["checksum"]),
                direction=str(row["direction"]),
                status=str(row["status"]),
                started_at=str(row["started_at"]),
                finished_at=_optional_str(row.get("finished_at")),
                error=_optional_str(row.get("error")),
                executor_ip=_optional_str(row.get("executor_ip")),
                executor_mac=_optional_str(row.get("executor_mac")),
            )
            for row in self._reader.load_runs()
        ]


def _optional_str(value: object) -> str | None:
    if value is None:
        return None
    text = str(value)
    return text or None


__all__ = [
    "MigrationRecord",
    "MigrationRunRecord",
    "MigrationState",
    "MigrationStateManager",
]
