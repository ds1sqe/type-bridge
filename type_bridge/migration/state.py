"""Migration state tracking for TypeDB.

Tracks applied migrations in TypeDB as the sole source of truth.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


@dataclass
class MigrationRecord:
    """Record of an applied migration."""

    app_label: str
    name: str
    applied_at: str  # ISO format datetime
    checksum: str  # Hash of migration content for change detection

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MigrationRecord):
            return NotImplemented
        return self.app_label == other.app_label and self.name == other.name


@dataclass
class MigrationState:
    """Complete state of applied migrations."""

    applied: list[MigrationRecord] = field(default_factory=list)
    version: str = "1.0"

    def is_applied(self, app_label: str, name: str) -> bool:
        """Check if a migration has been applied.

        Args:
            app_label: Application label
            name: Migration name

        Returns:
            True if migration has been applied
        """
        return any(r.app_label == app_label and r.name == name for r in self.applied)

    def add(self, record: MigrationRecord) -> None:
        """Add a migration record.

        Args:
            record: Migration record to add
        """
        if not self.is_applied(record.app_label, record.name):
            self.applied.append(record)

    def remove(self, app_label: str, name: str) -> None:
        """Remove a migration record (for rollback).

        Args:
            app_label: Application label
            name: Migration name
        """
        self.applied = [
            r for r in self.applied if not (r.app_label == app_label and r.name == name)
        ]

    def get_latest(self, app_label: str) -> MigrationRecord | None:
        """Get the most recently applied migration for an app.

        Args:
            app_label: Application label

        Returns:
            Most recent migration record, or None
        """
        app_migrations = [r for r in self.applied if r.app_label == app_label]
        return app_migrations[-1] if app_migrations else None

    def get_all_for_app(self, app_label: str) -> list[MigrationRecord]:
        """Get all applied migrations for an app.

        Args:
            app_label: Application label

        Returns:
            List of migration records in application order
        """
        return [r for r in self.applied if r.app_label == app_label]


class MigrationStateManager:
    """Manages migration state in TypeDB.

    State is stored in TypeDB as type_bridge_migration entities. The storage
    mechanism — schema bootstrap, the applied-state read, and the
    record/unrecord writes — lives in Rust behind the ``MigrationStateStore``
    seam; this class is a thin facade that delegates to a Rust-owned
    ``PyMigrationStateManager`` and assembles the ``MigrationState`` /
    ``MigrationRecord`` dataclasses Python callers consume.

    Example:
        manager = MigrationStateManager(db)
        state = manager.load_state()

        if not state.is_applied("myapp", "0001_initial"):
            # Apply migration...
            manager.record_applied("myapp", "0001_initial", "abc123")
    """

    ENTITY_NAME = "type_bridge_migration"

    def __init__(self, db: Database):
        """Initialize state manager.

        Args:
            db: Database connection
        """
        self.db = db
        self._state: MigrationState | None = None
        self._rust_manager: Any = None

    @property
    def _manager(self) -> Any:
        """Return the Rust state manager, building it on first use.

        Built lazily so subclasses that bypass ``__init__`` (e.g. test doubles
        overriding ``load_state``) never trigger a Rust connection.
        """
        if self._rust_manager is None:
            from type_bridge._rust_runtime import state_manager_for

            self._rust_manager = state_manager_for(self.db)
        return self._rust_manager

    def ensure_schema(self) -> None:
        """Ensure the migration tracking schema exists in TypeDB.

        Idempotent: the Rust state store no-ops when the schema is already
        ensured, so no second Python-side latch is kept here.
        """
        self._manager.ensure_schema()

    def load_state(self) -> MigrationState:
        """Load migration state from TypeDB.

        Returns:
            Current migration state
        """
        self.ensure_schema()

        state = MigrationState()
        # Let a real backend error surface; ensure_schema() above guarantees the
        # store exists, so load_applied returns [] (not an error) on first run.
        # Swallowing here would silently mask a Rust-side failure as empty state.
        for row in self._manager.load_applied():
            applied = row.get("applied_at")
            state.add(
                MigrationRecord(
                    app_label=str(row["app_label"]),
                    name=str(row["name"]),
                    applied_at="" if applied is None else str(applied),
                    checksum=str(row["checksum"]),
                )
            )

        self._state = state
        return state

    def record_applied(self, app_label: str, name: str, checksum: str) -> None:
        """Record that a migration was applied.

        Args:
            app_label: Application label
            name: Migration name
            checksum: Migration content hash
        """
        self.ensure_schema()

        applied_at = datetime.now(UTC)
        applied_at_str = applied_at.strftime("%Y-%m-%dT%H:%M:%S.%f")

        self._manager.record_applied(
            {
                "app_label": app_label,
                "name": name,
                "checksum": checksum,
                "applied_at": applied_at_str,
            }
        )

        logger.info(f"Recorded migration: {app_label}.{name}")

        # Update local state
        if self._state:
            self._state.add(
                MigrationRecord(
                    app_label=app_label,
                    name=name,
                    applied_at=applied_at.isoformat(),
                    checksum=checksum,
                )
            )

    def record_unapplied(self, app_label: str, name: str) -> None:
        """Record that a migration was rolled back.

        Args:
            app_label: Application label
            name: Migration name
        """
        self.ensure_schema()

        self._manager.record_unapplied(app_label, name)

        logger.info(f"Removed migration record: {app_label}.{name}")

        # Update local state
        if self._state:
            self._state.remove(app_label, name)
