"""Migration executor for applying and rolling back migrations."""

from __future__ import annotations

import importlib
import logging
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from type_bridge import _rust_runtime
from type_bridge.migration import operations as ops
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.state import (
    MigrationRecord,
    MigrationRunRecord,
    MigrationState,
    MigrationStateManager,
)

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

        if _loaded_contains_run_python(all_migrations):
            self._preflight_python_plan(plan)
            return self._migrate_with_python_operations(plan, all_migrations)

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

    def _migrate_with_python_operations(
        self,
        plan: MigrationPlan,
        all_migrations: list[LoadedMigration],
    ) -> list[MigrationResult]:
        """Execute a plan that contains at least one ``ops.RunPython`` operation.

        Rust remains the default executor for non-Python migrations (including
        sidecar-backed migrations).  A plan containing ``RunPython`` needs
        Python call boundaries only at those migration boundaries.
        """
        from type_bridge.migration._lower import lower_execution_migration

        results: list[MigrationResult] = []
        checksums = {
            (loaded.migration.app_label, loaded.migration.name): loaded.checksum
            for loaded in all_migrations
        }
        index_by_key = {
            (loaded.migration.app_label, loaded.migration.name): i
            for i, loaded in enumerate(all_migrations)
        }

        has_non_python = any(
            not _migration_contains_run_python(loaded.migration)
            for loaded in [*plan.to_rollback, *plan.to_apply]
        )

        runner = None
        graph = None
        if has_non_python:
            runner = _rust_runtime.migration_runner_for(self.db)
            graph = self._execution_graph_for_mixed_plan(
                all_migrations,
                lower_execution_migration=lower_execution_migration,
            )

        def run_rust_segment(target: str | None) -> list[MigrationResult]:
            if runner is None or graph is None:
                raise MigrationError("Mixed migration plan has no Rust runner configured")
            state = self.state_manager.load_state()
            applied_records = [_applied_record_dict(record) for record in state.applied]
            rust_results = runner.apply(graph, applied_records, target)

            mapped: list[MigrationResult] = []
            for rust_result in rust_results:
                result = self._record_result(rust_result, checksums)
                mapped.append(result)
                if not result.success:
                    if result.action == "rolled_back":
                        raise MigrationError(f"Rollback failed: {result.error}")
                    raise MigrationError(f"Migration failed: {result.error}")
            return mapped

        non_python_segment: list[LoadedMigration] = []

        for loaded in plan.to_rollback:
            if _migration_contains_run_python(loaded.migration):
                if non_python_segment:
                    segment_target_idx = min(
                        index_by_key[(segment.migration.app_label, segment.migration.name)]
                        for segment in non_python_segment
                    )
                    target = (
                        all_migrations[segment_target_idx - 1].migration.name
                        if segment_target_idx > 0
                        else None
                    )
                    results.extend(run_rust_segment(target))
                    non_python_segment = []

                result = self._execute_loaded_python_path(loaded, reverse=True)
                results.append(result)
                if not result.success:
                    raise MigrationError(f"Rollback failed: {result.error}")
            else:
                non_python_segment.append(loaded)

        if non_python_segment:
            segment_target_idx = min(
                index_by_key[(segment.migration.app_label, segment.migration.name)]
                for segment in non_python_segment
            )
            target = (
                all_migrations[segment_target_idx - 1].migration.name
                if segment_target_idx > 0
                else None
            )
            results.extend(run_rust_segment(target))

        non_python_segment = []
        for loaded in plan.to_apply:
            if _migration_contains_run_python(loaded.migration):
                if non_python_segment:
                    target = non_python_segment[-1].migration.name
                    results.extend(run_rust_segment(target))
                    non_python_segment = []

                result = self._execute_loaded_python_path(loaded, reverse=False)
                results.append(result)
                if not result.success:
                    raise MigrationError(f"Migration failed: {result.error}")
            else:
                non_python_segment.append(loaded)

        if non_python_segment:
            target = non_python_segment[-1].migration.name
            results.extend(run_rust_segment(target))

        return results

    def _execution_graph_for_mixed_plan(
        self,
        all_migrations: list[LoadedMigration],
        *,
        lower_execution_migration: Callable[[LoadedMigration], dict[str, Any]],
    ) -> dict[str, Any]:
        """Lower all migrations into one executable graph for mixed plans.

        ``ops.RunPython`` migrations cannot be lowered into Rust operation
        specs, so they are represented as no-op placeholders in this mixed
        graph. Other migrations keep their full executable lowering path.
        """
        specifications: list[dict[str, Any]] = []
        for loaded in all_migrations:
            if _migration_contains_run_python(loaded.migration):
                migration = loaded.migration
                specifications.append(
                    {
                        "app_label": migration.app_label,
                        "name": migration.name,
                        "dependencies": [
                            {
                                "app_label": dependency.app_label,
                                "migration_name": dependency.migration_name,
                            }
                            for dependency in migration.get_dependencies()
                        ],
                        "operations": [],
                        "checksum": loaded.checksum,
                        "reversible": migration.reversible,
                    }
                )
                continue

            specifications.append(lower_execution_migration(loaded))

        return _rust_runtime.normalize_migration_graph({"migrations": specifications})

    def _preflight_python_plan(self, plan: MigrationPlan) -> None:
        """Validate declared RunPython imports/resources before any step mutates data."""
        for loaded in [*plan.to_rollback, *plan.to_apply]:
            for operation in getattr(loaded.migration, "operations", []):
                if isinstance(operation, ops.RunPython):
                    self._preflight_run_python_operation(loaded, operation)

    def _preflight_run_python_operation(
        self,
        loaded: LoadedMigration,
        operation: ops.RunPython,
    ) -> None:
        operation_label = operation.description or operation._callable_name(operation.code)

        for module_name in operation.import_checks:
            if not module_name:
                raise MigrationError(
                    f"RunPython operation {operation_label} in {loaded.migration.name} "
                    "declares an empty import check"
                )
            try:
                importlib.import_module(module_name)
            except (ImportError, AttributeError, ValueError) as exc:
                raise MigrationError(
                    f"RunPython operation {operation_label} in {loaded.migration.name} "
                    f"failed import check {module_name!r}: {exc}"
                ) from exc

        for resource in operation.resources:
            if not resource:
                raise MigrationError(
                    f"RunPython operation {operation_label} in {loaded.migration.name} "
                    "declares an empty resource path"
                )
            path = _resolve_migration_resource(loaded.path, resource)
            if not path.is_file():
                raise MigrationError(
                    f"RunPython operation {operation_label} in {loaded.migration.name} "
                    f"requires missing resource {resource!r} at {path}"
                )
            try:
                with path.open("rb"):
                    pass
            except OSError as exc:
                raise MigrationError(
                    f"RunPython operation {operation_label} in {loaded.migration.name} "
                    f"cannot read resource {resource!r} at {path}: {exc}"
                ) from exc

    def _execute_loaded_python_path(
        self,
        loaded: LoadedMigration,
        *,
        reverse: bool,
    ) -> MigrationResult:
        migration = loaded.migration
        action = "rolled_back" if reverse else "applied"
        direction = "rollback" if reverse else "apply"
        run = self.state_manager.record_run_started(
            migration.app_label,
            migration.name,
            loaded.checksum,
            direction,
        )

        try:
            if reverse:
                self._execute_migration_reverse(migration)
                self._record_state(
                    migration.name,
                    action,
                    lambda: self.state_manager.record_unapplied(
                        migration.app_label,
                        migration.name,
                    ),
                )
            else:
                self._execute_migration_forward(migration)
                self._record_state(
                    migration.name,
                    action,
                    lambda: self.state_manager.record_applied(
                        migration.app_label,
                        migration.name,
                        loaded.checksum,
                    ),
                )
        except Exception as exc:  # noqa: BLE001 - mapped to MigrationResult error
            self._record_run_finished(run, "failed", str(exc))
            return MigrationResult(
                name=migration.name,
                action=action,
                success=False,
                error=str(exc),
            )

        self._record_run_finished(run, "succeeded", None)
        return MigrationResult(name=migration.name, action=action, success=True)

    def _execute_migration_forward(self, migration: object) -> None:
        models = getattr(migration, "models", [])
        if models:
            self._execute_typeql(_schema_typeql_for_models(models), "schema")

        for operation in getattr(migration, "operations", []):
            self._execute_operation_forward(operation)

    def _execute_migration_reverse(self, migration: object) -> None:
        models = getattr(migration, "models", [])
        if models:
            raise MigrationError(
                f"Migration {getattr(migration, 'name', '<unknown>')} is not reversible"
            )

        operations = list(getattr(migration, "operations", []))
        for operation in reversed(operations):
            self._execute_operation_reverse(operation)

    def _execute_operation_forward(self, operation: ops.Operation) -> None:
        if isinstance(operation, ops.RunPython):
            operation.run(self.db)
            return

        typeql = operation.to_typeql()
        if typeql.strip():
            self._execute_typeql(typeql, _typeql_transaction_type(typeql))

    def _execute_operation_reverse(self, operation: ops.Operation) -> None:
        if isinstance(operation, ops.RunPython):
            operation.rollback(self.db)
            return

        typeql = operation.to_rollback_typeql()
        if typeql is None:
            raise MigrationError(f"Operation {type(operation).__name__} is not reversible")
        if typeql.strip():
            self._execute_typeql(typeql, _typeql_transaction_type(typeql))

    def _execute_typeql(self, typeql: str, transaction_type: str) -> None:
        execute_query = getattr(self.db, "execute_query", None)
        if execute_query is None:
            raise MigrationError(
                "Cannot execute TypeQL migration operation: database object has no execute_query()"
            )
        execute_query(typeql, transaction_type=transaction_type)

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

    def _record_run_finished(
        self,
        run: MigrationRunRecord,
        status: str,
        error: str | None,
    ) -> None:
        try:
            self.state_manager.record_run_finished(run, status, error)
        except Exception as exc:  # noqa: BLE001 - re-raised as MigrationError below
            raise MigrationError(
                f"Migration {run.name} {run.direction} finished but recording its run log failed: "
                f"{exc}"
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
        if _migration_contains_run_python(loaded.migration):
            return _preview_python_migration(loaded, reverse=reverse)

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
        if _loaded_contains_run_python(all_migrations):
            from type_bridge.migration._lower import lower_validation_graph

            graph = lower_validation_graph(all_migrations)
        else:
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


def _loaded_contains_run_python(loaded: list[LoadedMigration]) -> bool:
    return any(_migration_contains_run_python(item.migration) for item in loaded)


def _migration_contains_run_python(migration: object) -> bool:
    return any(
        isinstance(operation, ops.RunPython) for operation in getattr(migration, "operations", [])
    )


def _schema_typeql_for_models(models: list[type[object]]) -> str:
    from type_bridge.migration._lower import _schema_info_for_models

    return _rust_runtime.generate_define_block(_schema_info_for_models(models))


def _resolve_migration_resource(migration_path: Path, resource: str) -> Path:
    path = Path(resource)
    if path.is_absolute():
        return path
    return migration_path.parent / path


def _typeql_transaction_type(typeql: str) -> str:
    first_statement = next(
        (
            line.strip().lower()
            for line in typeql.splitlines()
            if line.strip() and not line.strip().startswith(("#", "//"))
        ),
        "",
    )
    if first_statement.startswith(("define", "undefine", "redefine")):
        return "schema"
    return "write"


def _preview_python_migration(loaded: LoadedMigration, *, reverse: bool) -> str | None:
    operations = list(getattr(loaded.migration, "operations", []))
    if reverse:
        previews: list[str] = []
        for operation in reversed(operations):
            if isinstance(operation, ops.RunPython):
                preview = operation.to_rollback_typeql()
                if preview is None:
                    return None
                previews.append(preview)
                continue
            typeql = operation.to_rollback_typeql()
            if typeql is None:
                return None
            previews.append(typeql)
        return "\n\n".join(previews)

    previews = []
    models = getattr(loaded.migration, "models", [])
    if models:
        previews.append(_schema_typeql_for_models(models))
    for operation in operations:
        previews.append(operation.to_typeql())
    return "\n\n".join(preview for preview in previews if preview.strip())
