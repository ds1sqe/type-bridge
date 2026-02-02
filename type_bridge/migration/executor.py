"""Migration executor for applying and rolling back migrations."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from type_bridge.migration.base import Migration
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.schema_manager import SchemaManager
from type_bridge.migration.state import MigrationState, MigrationStateManager

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
    """

    name: str
    action: str
    success: bool
    error: str | None = None


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
        state = self.state_manager.load_state()
        all_migrations = self.loader.discover()
        plan = self._create_plan(state, all_migrations, target)

        if plan.is_empty():
            logger.info("No migrations to apply")
            return []

        results: list[MigrationResult] = []

        # Rollback if needed (migrating backwards)
        for loaded in plan.to_rollback:
            result = self._rollback_one(loaded)
            results.append(result)
            if not result.success:
                raise MigrationError(f"Rollback failed: {result.error}")

        # Apply forward migrations
        for loaded in plan.to_apply:
            result = self._apply_one(loaded)
            results.append(result)
            if not result.success:
                raise MigrationError(f"Migration failed: {result.error}")

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

        if reverse:
            typeql = self._generate_rollback_typeql(loaded.migration)
            if typeql is None:
                raise MigrationError(f"Migration {migration_name} is not reversible")
            return typeql
        else:
            return self._generate_apply_typeql(loaded.migration)

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

    def _apply_one(self, loaded: LoadedMigration) -> MigrationResult:
        """Apply a single migration.

        Args:
            loaded: Migration to apply

        Returns:
            MigrationResult
        """
        migration = loaded.migration
        logger.info(f"Applying migration: {migration.name}")

        try:
            typeql_statements = self._generate_apply_typeql_statements(migration)

            if self.dry_run:
                for typeql in typeql_statements:
                    logger.info(f"[DRY RUN] Would execute:\n{typeql}")
            else:
                # Execute each operation separately in schema transaction
                # (TypeDB doesn't allow multiple define blocks in one query)
                for typeql in typeql_statements:
                    with self.db.transaction("schema") as tx:
                        tx.execute(typeql)
                        tx.commit()

                # Record as applied
                self.state_manager.record_applied(
                    migration.app_label,
                    migration.name,
                    loaded.checksum,
                )

            logger.info(f"Applied: {migration.name}")
            return MigrationResult(
                name=migration.name,
                action="applied",
                success=True,
            )

        except Exception as e:
            error_msg = self._extract_error_message(e)
            logger.error(f"Failed to apply {migration.name}: {error_msg}")
            return MigrationResult(
                name=migration.name,
                action="applied",
                success=False,
                error=error_msg,
            )

    def _rollback_one(self, loaded: LoadedMigration) -> MigrationResult:
        """Rollback a single migration.

        Args:
            loaded: Migration to rollback

        Returns:
            MigrationResult
        """
        migration = loaded.migration
        logger.info(f"Rolling back migration: {migration.name}")

        try:
            typeql_statements = self._generate_rollback_typeql_statements(migration)
            if typeql_statements is None:
                return MigrationResult(
                    name=migration.name,
                    action="rolled_back",
                    success=False,
                    error=f"Migration {migration.name} is not reversible",
                )

            if self.dry_run:
                for typeql in typeql_statements:
                    logger.info(f"[DRY RUN] Would execute:\n{typeql}")
            else:
                # Execute each operation separately in schema transaction
                for typeql in typeql_statements:
                    with self.db.transaction("schema") as tx:
                        tx.execute(typeql)
                        tx.commit()

                # Remove from applied
                self.state_manager.record_unapplied(
                    migration.app_label,
                    migration.name,
                )

            logger.info(f"Rolled back: {migration.name}")
            return MigrationResult(
                name=migration.name,
                action="rolled_back",
                success=True,
            )

        except Exception as e:
            error_msg = self._extract_error_message(e)
            logger.error(f"Failed to rollback {migration.name}: {error_msg}")
            return MigrationResult(
                name=migration.name,
                action="rolled_back",
                success=False,
                error=error_msg,
            )

    def _extract_error_message(self, e: Exception) -> str:
        """Extract error message from exception.

        TypeDB exceptions may have blank str() output, so we try
        multiple approaches to get the actual error message.
        """
        error_msg = str(e)
        if not error_msg:
            error_msg = getattr(e, "message", "")
        if not error_msg:
            error_msg = repr(e)
        return error_msg

    def _generate_apply_typeql_statements(self, migration: Migration) -> list[str]:
        """Generate forward TypeQL statements for a migration.

        Returns a list of individual TypeQL statements to execute separately.

        Args:
            migration: Migration instance

        Returns:
            List of TypeQL strings
        """
        statements: list[str] = []

        # Initial migration with models
        if migration.models:
            schema_mgr = SchemaManager(self.db)
            schema_mgr.register(*migration.models)
            schema_info = schema_mgr.collect_schema_info()
            statements.append(schema_info.to_typeql())

        # Operations-based migration
        for op in migration.operations:
            typeql = op.to_typeql()
            if typeql:
                statements.append(typeql)

        return statements

    def _generate_apply_typeql(self, migration: Migration) -> str:
        """Generate forward TypeQL for a migration (for preview).

        Args:
            migration: Migration instance

        Returns:
            TypeQL string (all statements joined)
        """
        return "\n\n".join(self._generate_apply_typeql_statements(migration))

    def _generate_rollback_typeql_statements(self, migration: Migration) -> list[str] | None:
        """Generate rollback TypeQL statements for a migration.

        Returns a list of individual TypeQL statements to execute separately.

        Args:
            migration: Migration instance

        Returns:
            List of TypeQL strings or None if not reversible
        """
        # Initial migrations with models are not reversible
        if migration.models:
            return None

        if not migration.reversible:
            return None

        statements: list[str] = []

        # Reverse operations in reverse order
        for op in reversed(migration.operations):
            rollback = op.to_rollback_typeql()
            if rollback is None:
                # One non-reversible op makes whole migration non-reversible
                return None
            statements.append(rollback)

        return statements

    def _generate_rollback_typeql(self, migration: Migration) -> str | None:
        """Generate rollback TypeQL for a migration (for preview).

        Args:
            migration: Migration instance

        Returns:
            TypeQL string or None if not reversible
        """
        statements = self._generate_rollback_typeql_statements(migration)
        if statements is None:
            return None

        return "\n\n".join(statements)
