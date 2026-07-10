"""Migration-state records, external-store protocol, and TypeDB default backend."""

from __future__ import annotations

import logging
import socket
import uuid
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any, Protocol, runtime_checkable

from type_bridge import _rust_runtime

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
class MigrationRunRecord:
    """Record of one migration execution attempt."""

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


@runtime_checkable
class MigrationStateStore(Protocol):
    """State operations required by :class:`MigrationExecutor`.

    Implement this protocol to keep applied migration state outside the target
    TypeDB database. Schema bootstrap and run-log reads are intentionally not
    part of the contract: the default :class:`MigrationStateManager` provides
    those TypeDB-specific capabilities, while embedding orchestrators may own
    persistence and execution-attempt logging independently.
    """

    def load_state(self) -> MigrationState:
        """Load the applied migration projection used for planning."""
        ...

    def record_applied(self, app_label: str, name: str, checksum: str) -> None:
        """Persist one successfully applied migration."""
        ...

    def record_unapplied(self, app_label: str, name: str) -> None:
        """Remove one successfully rolled-back migration."""
        ...

    def record_run_started(
        self,
        app_label: str,
        name: str,
        checksum: str,
        direction: str,
    ) -> MigrationRunRecord:
        """Record the start of a Python-hosted migration execution."""
        ...

    def record_run_finished(
        self,
        record: MigrationRunRecord,
        status: str,
        error: str | None = None,
    ) -> MigrationRunRecord:
        """Record the end of a Python-hosted migration execution."""
        ...


class MigrationStateManager:
    """Manages migration state in TypeDB.

    Applied state is stored in TypeDB as ``type_bridge_migration`` entities, and
    execution attempts are stored as ``type_bridge_migration_run`` entities. The
    storage mechanism — schema bootstrap, reads, and writes — lives in Rust
    behind the ``MigrationStateStore`` seam; this class is a thin facade that
    delegates to a Rust-owned ``PyMigrationStateManager`` and assembles Python
    dataclasses for callers.

    Example:
        manager = MigrationStateManager(db)
        state = manager.load_state()

        if not state.is_applied("myapp", "0001_initial"):
            # Apply migration...
            manager.record_applied("myapp", "0001_initial", "abc123")
    """

    # Compatibility alias for callers that referenced the original manager
    # constant; its value still comes from the canonical Rust contract.
    ENTITY_NAME = _rust_runtime.applied_migration_entity_label()

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

    def load_runs(self) -> list[MigrationRunRecord]:
        """Load the migration execution run log from TypeDB."""
        self.ensure_schema()

        runs: list[MigrationRunRecord] = []
        for row in self._manager.load_runs():
            runs.append(
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
            )
        return runs

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

    def record_run_started(
        self,
        app_label: str,
        name: str,
        checksum: str,
        direction: str,
    ) -> MigrationRunRecord:
        """Record that one migration execution attempt started."""
        if direction not in {"apply", "rollback"}:
            raise ValueError(f"Unsupported migration run direction: {direction}")

        started_at = _timestamp_now()
        executor_ip, executor_mac = _executor_info()
        record = MigrationRunRecord(
            run_id=str(uuid.uuid4()),
            app_label=app_label,
            name=name,
            checksum=checksum,
            direction=direction,
            status="started",
            started_at=started_at,
            executor_ip=executor_ip,
            executor_mac=executor_mac,
        )
        self._record_run(record)
        return record

    def record_run_finished(
        self,
        record: MigrationRunRecord,
        status: str,
        error: str | None = None,
    ) -> MigrationRunRecord:
        """Record that a migration execution attempt finished."""
        if status not in {"succeeded", "failed"}:
            raise ValueError(f"Unsupported migration run status: {status}")

        finished = MigrationRunRecord(
            run_id=record.run_id,
            app_label=record.app_label,
            name=record.name,
            checksum=record.checksum,
            direction=record.direction,
            status=status,
            started_at=record.started_at,
            finished_at=_timestamp_now(),
            error=error,
            executor_ip=record.executor_ip,
            executor_mac=record.executor_mac,
        )
        self._record_run(finished)
        return finished

    def _record_run(self, record: MigrationRunRecord) -> None:
        self.ensure_schema()
        self._manager.record_run(
            {
                "run_id": record.run_id,
                "app_label": record.app_label,
                "name": record.name,
                "checksum": record.checksum,
                "direction": record.direction,
                "status": record.status,
                "started_at": record.started_at,
                "finished_at": record.finished_at,
                "error": record.error,
                "executor_ip": record.executor_ip,
                "executor_mac": record.executor_mac,
            }
        )


def _timestamp_now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%S.%f")


def _optional_str(value: object) -> str | None:
    if value is None:
        return None
    text = str(value)
    return text or None


def _executor_info() -> tuple[str | None, str | None]:
    return _local_ip(), _local_mac()


def _local_ip() -> str | None:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.connect(("8.8.8.8", 80))
            return str(sock.getsockname()[0])
    except OSError:
        return None


def _local_mac() -> str | None:
    node = uuid.getnode()
    if (node >> 40) & 1:
        return None
    return ":".join(f"{(node >> shift) & 0xFF:02x}" for shift in range(40, -1, -8))
