"""Migration executor for applying and rolling back migrations."""

from __future__ import annotations

import logging
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from type_bridge import _rust_runtime
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.state import MigrationRecord, MigrationState, MigrationStateManager

if TYPE_CHECKING:
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


class MigrationError(Exception):
    """Error during migration execution."""

    pass


@dataclass
class MigrationPlan:
    """Plan for migration execution.

    Attributes:
        to_apply: Migrations to apply (forward)
        to_rollback: Migrations to rollback (reverse)
    """

    to_apply: list[LoadedMigration]
    to_rollback: list[LoadedMigration]

    def is_empty(self) -> bool:
        """Check if plan has no operations."""
        return not self.to_apply and not self.to_rollback


@dataclass
class MigrationResult:
    """Result of a migration operation.

    Attributes:
        name: Migration name
        action: "applied" or "rolled_back"
        success: Whether the operation succeeded
        error: Error message if failed
        backfill: Per-step backfill counts when the migration contained a
            CopyAttribute op; ``None`` for pure-schema migrations (no bloat).
    """

    name: str
    action: str
    success: bool
    error: str | None = None
    backfill: list[dict[str, object]] | None = None


class MigrationExecutor:
    """Executes migrations against a TypeDB database.

    Handles:
    - Applying pending migrations
    - Rolling back applied migrations
    - Previewing migration TypeQL
    - Listing migration status

    Example:
        executor = MigrationExecutor(db, Path("migrations"))

        # Apply all pending migrations
        results = executor.migrate()

        # Migrate to specific version
        results = executor.migrate(target="0002_add_company")

        # Show migration status
        status = executor.showmigrations()
        for name, is_applied in status:
            print(f"[{'X' if is_applied else ' '}] {name}")

        # Preview TypeQL
        typeql = executor.sqlmigrate("0002_add_company")
        print(typeql)
    """

    def __init__(
        self,
        db: Database,
        migrations_dir: Path,
        dry_run: bool = False,
    ):
        """Initialize executor.

        Args:
            db: Database connection
            migrations_dir: Directory containing migration files
            dry_run: If True, preview operations without executing
        """
        self.db = db
        self.migrations_dir = migrations_dir
        self.dry_run = dry_run
        self.loader = MigrationLoader(migrations_dir)
        self.state_manager = MigrationStateManager(db)

    def migrate(self, target: str | None = None) -> list[MigrationResult]:
        """Apply pending migrations.

        Args:
            target: Optional target migration name (e.g., "0002_add_company")
                   If None, apply all pending migrations.
                   If specified, migrate to that exact state (may rollback).

        Returns:
            List of migration results

        Raises:
            MigrationError: If migration fails
        """
        from type_bridge.migration._lower import lower_execution_graph

        state = self.state_manager.load_state()
        all_migrations = self.loader.discover()
        self._preflight_migrations(all_migrations, state)
        plan = self._create_plan(state, all_migrations, target)

        if plan.is_empty():
            logger.info("No migrations to apply")
            return []

        if self.dry_run:
            return self._dry_run(plan)

        graph = lower_execution_graph(all_migrations)
        applied_records = [_applied_record_dict(record) for record in state.applied]

        runner = _rust_runtime.migration_runner_for(self.db)
        rust_results = runner.apply(graph, applied_records, target)

        checksums = {
            (loaded.migration.app_label, loaded.migration.name): loaded.checksum
            for loaded in all_migrations
        }

        results: list[MigrationResult] = []
        for rust_result in rust_results:
            result = self._record_result(rust_result, checksums)
            results.append(result)
            if not result.success:
                if result.action == "rolled_back":
                    raise MigrationError(f"Rollback failed: {result.error}")
                raise MigrationError(f"Migration failed: {result.error}")

        return results

    def _record_result(
        self,
        rust_result: dict,
        checksums: dict[tuple[str, str], str],
    ) -> MigrationResult:
        """Record state for one Rust execution result and map it to a dataclass.

        On a successful Rust apply/rollback the matching state record is written
        via the unchanged `MigrationStateManager`. The schema change is already
        durable in TypeDB at this point; a failure to record state is surfaced as
        a `MigrationError` naming the migration rather than swallowed (full
        desync-window closure is sub-plan 06).
        """
        app_label = rust_result["app_label"]
        name = rust_result["name"]
        is_apply = rust_result["action"] == "apply"
        action = "applied" if is_apply else "rolled_back"
        success = bool(rust_result["success"])
        error = rust_result.get("error")

        if not success:
            return MigrationResult(name=name, action=action, success=False, error=error)

        if is_apply:
            checksum = checksums.get((app_label, name), "")
            self._record_state(
                name,
                action,
                lambda: self.state_manager.record_applied(app_label, name, checksum),
            )
        else:
            self._record_state(
                name,
                action,
                lambda: self.state_manager.record_unapplied(app_label, name),
            )

        return MigrationResult(
            name=name,
            action=action,
            success=True,
            backfill=rust_result.get("backfill"),
        )

    def _record_state(self, name: str, action: str, record: Callable[[], None]) -> None:
        """Run a state-recording call, surfacing failures as a MigrationError.

        The schema change already landed in Rust; a state-record failure leaves
        applied history out of sync with the database, so it must not be
        silently swallowed.
        """
        try:
            record()
        except Exception as exc:  # noqa: BLE001 - re-raised as MigrationError below
            verb = "apply" if action == "applied" else "rollback"
            raise MigrationError(
                f"Migration {name} {verb} succeeded but recording its state failed: {exc}"
            ) from exc

    def _dry_run(self, plan: MigrationPlan) -> list[MigrationResult]:
        """Log the TypeQL each migration would run without touching the database."""
        results: list[MigrationResult] = []
        for loaded in plan.to_rollback:
            typeql = self._preview_typeql(loaded, reverse=True)
            if typeql is None:
                results.append(
                    MigrationResult(
                        name=loaded.migration.name,
                        action="rolled_back",
                        success=False,
                        error=f"Migration {loaded.migration.name} is not reversible",
                    )
                )
                continue
            logger.info(f"[DRY RUN] Would roll back {loaded.migration.name}:\n{typeql}")
            results.append(
                MigrationResult(name=loaded.migration.name, action="rolled_back", success=True)
            )
        for loaded in plan.to_apply:
            typeql = self._preview_typeql(loaded, reverse=False)
            logger.info(f"[DRY RUN] Would apply {loaded.migration.name}:\n{typeql}")
            results.append(
                MigrationResult(name=loaded.migration.name, action="applied", success=True)
            )
        return results

    def showmigrations(self) -> list[tuple[str, bool]]:
        """List all migrations with their applied status.

        Returns:
            List of (migration_name, is_applied) tuples
        """
        state = self.state_manager.load_state()
        all_migrations = self.loader.discover()

        result: list[tuple[str, bool]] = []
        for loaded in all_migrations:
            is_applied = state.is_applied(loaded.migration.app_label, loaded.migration.name)
            result.append((loaded.migration.name, is_applied))

        return result

    def sqlmigrate(self, migration_name: str, reverse: bool = False) -> str:
        """Preview TypeQL for a migration without executing.

        Args:
            migration_name: Name of the migration
            reverse: If True, show rollback TypeQL

        Returns:
            TypeQL string that would be executed

        Raises:
            MigrationError: If migration not found or not reversible
        """
        loaded = self.loader.get_by_name(migration_name)
        if loaded is None:
            raise MigrationError(f"Migration not found: {migration_name}")

        typeql = self._preview_typeql(loaded, reverse=reverse)
        if reverse and typeql is None:
            raise MigrationError(f"Migration {migration_name} is not reversible")
        return typeql or ""

    def _preview_typeql(self, loaded: LoadedMigration, *, reverse: bool) -> str | None:
        """Render a migration's lowered execution TypeQL for preview/dry-run.

        Routes through the Rust planner so preview and execution share one
        TypeQL source even when the migration carries typed ``OperationSpec``
        sidecars. Returns forward TypeQL when ``reverse`` is ``False``; reverse
        TypeQL in reverse step order when ``reverse`` is ``True``, or ``None``
        when any step is non-reversible.
        """
        from type_bridge.migration._lower import lower_execution_migration

        spec = lower_execution_migration(loaded)
        # Preview is scoped to one migration file, matching the historical
        # Python sqlmigrate behavior. Dependencies are irrelevant for rendering
        # this migration's steps and would fail graph validation without the
        # rest of the migration directory.
        spec = {**spec, "dependencies": []}
        graph = _rust_runtime.normalize_migration_graph({"migrations": [spec]})
        execution_plan = _rust_runtime.plan_migration_graph(graph, [], loaded.migration.name)
        executions = execution_plan["to_apply"]
        if not executions:
            return ""
        steps = executions[0]["steps"]

        if not reverse:
            return "\n\n".join(_step_forward(step) for step in steps)

        reverses: list[str] = []
        for step in reversed(steps):
            rollback = _step_reverse(step)
            if rollback is None:
                return None
            reverses.append(rollback)
        return "\n\n".join(reverses)

    def plan(self, target: str | None = None) -> MigrationPlan:
        """Get the migration plan without executing.

        Args:
            target: Optional target migration name

        Returns:
            MigrationPlan showing what would be applied/rolled back
        """
        state = self.state_manager.load_state()
        all_migrations = self.loader.discover()
        return self._create_plan(state, all_migrations, target)

    def _create_plan(
        self,
        state: MigrationState,
        all_migrations: list[LoadedMigration],
        target: str | None,
    ) -> MigrationPlan:
        """Create execution plan.

        Args:
            state: Current migration state
            all_migrations: All discovered migrations
            target: Optional target migration

        Returns:
            MigrationPlan
        """
        to_apply: list[LoadedMigration] = []
        to_rollback: list[LoadedMigration] = []

        if target is None:
            # Apply all pending
            for loaded in all_migrations:
                if not state.is_applied(loaded.migration.app_label, loaded.migration.name):
                    to_apply.append(loaded)
        else:
            # Find target index
            target_idx = -1
            for i, loaded in enumerate(all_migrations):
                if loaded.migration.name == target:
                    target_idx = i
                    break

            if target_idx == -1:
                raise MigrationError(f"Target migration not found: {target}")

            # Calculate what to apply/rollback
            for i, loaded in enumerate(all_migrations):
                is_applied = state.is_applied(loaded.migration.app_label, loaded.migration.name)

                if i <= target_idx and not is_applied:
                    to_apply.append(loaded)
                elif i > target_idx and is_applied:
                    to_rollback.append(loaded)

            # Rollbacks go in reverse order
            to_rollback.reverse()

        return MigrationPlan(to_apply=to_apply, to_rollback=to_rollback)

    def _preflight_migrations(
        self,
        all_migrations: list[LoadedMigration],
        state: MigrationState,
    ) -> None:
        """Validate graph and checksum drift before any migration step runs."""
        from type_bridge.migration._lower import lower_migration_graph

        graph = lower_migration_graph(all_migrations)
        applied_records = [_applied_record_dict(record) for record in state.applied]
        errors = _rust_runtime.validate_migration_graph(graph, applied_records)
        if errors:
            messages = [str(error["message"]) for error in errors]
            raise MigrationError("Migration graph validation failed: " + "; ".join(messages))
        try:
            _rust_runtime.check_migration_drift(graph, applied_records)
        except ValueError as exc:
            raise MigrationError(f"Migration checksum drift detected: {exc}") from exc


def _step_forward(step: dict) -> str:
    """Return the forward TypeQL a lowered execution step would run.

    A ``define_schema`` step carries a ``SchemaInfo`` rather than a TypeQL
    string; its forward TypeQL is produced by the canonical Rust generator (the
    same engine the planner uses), keeping one TypeQL source.
    """
    if step["kind"] == "define_schema":
        return _rust_runtime.generate_define_block(step["schema"])
    return step["forward"]


def _step_reverse(step: dict) -> str | None:
    """Return the reverse TypeQL of a lowered step, or ``None`` if irreversible."""
    if step["kind"] == "define_schema":
        return None
    return step.get("reverse")


def _applied_record_dict(record: MigrationRecord) -> dict[str, str]:
    return {
        "app_label": record.app_label,
        "name": record.name,
        "checksum": record.checksum,
        "applied_at": record.applied_at,
    }
