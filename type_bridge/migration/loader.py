"""Migration file loader and discovery.

Discovers and loads migration files from a directory structure.
Migration files must follow the naming convention: NNNN_name.py (e.g., 0001_initial.py)
"""

from __future__ import annotations

import importlib.util
import logging
import types
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from type_bridge import _rust_runtime
from type_bridge.migration.base import Migration

logger = logging.getLogger(__name__)


@dataclass
class LoadedMigration:
    """A migration loaded from a file.

    Attributes:
        migration: The Migration instance
        path: Path to the migration file
        checksum: SHA256 hash of file content (first 16 chars)
        execution_spec: Optional pre-lowered MigrationSpec dict loaded from
            the JSON sidecar.  Present only for generated migrations that carry
            a ``.json`` sibling; ``None`` for legacy/hand-authored files.
            Keyword-only so existing positional construction sites are unaffected.
    """

    migration: Migration
    path: Path
    checksum: str
    execution_spec: dict[str, Any] | None = field(default=None, kw_only=True)

    def __repr__(self) -> str:
        return f"<LoadedMigration {self.migration.app_label}.{self.migration.name}>"


class MigrationLoadError(Exception):
    """Error loading a migration file."""

    pass


class MigrationLoader:
    """Loads migration files from a directory.

    Migration files must follow the naming pattern: NNNN_*.py
    where NNNN is a 4-digit number (e.g., 0001_initial.py, 0002_add_company.py)

    Example:
        loader = MigrationLoader(Path("migrations"))
        migrations = loader.discover()

        for loaded in migrations:
            print(f"{loaded.migration.name}: {loaded.checksum}")
    """

    MIGRATION_PATTERN = "[0-9][0-9][0-9][0-9]_*.py"

    def __init__(self, migrations_dir: Path):
        """Initialize loader.

        Args:
            migrations_dir: Directory containing migration files
        """
        self.migrations_dir = migrations_dir

    def discover(self) -> list[LoadedMigration]:
        """Discover all migration files in order.

        Returns:
            List of loaded migrations, sorted by filename
        """
        if not self.migrations_dir.exists():
            logger.debug(f"Migrations directory does not exist: {self.migrations_dir}")
            return []

        files = sorted(self.migrations_dir.glob(self.MIGRATION_PATTERN))
        migrations: list[LoadedMigration] = []

        for path in files:
            try:
                loaded = self._load_migration_file(path)
                if loaded:
                    migrations.append(loaded)
            except Exception as e:
                logger.error(f"Failed to load migration {path}: {e}")
                raise MigrationLoadError(f"Failed to load migration {path}: {e}") from e

        logger.debug(f"Discovered {len(migrations)} migration(s) in {self.migrations_dir}")
        return migrations

    def get_by_name(self, name: str) -> LoadedMigration | None:
        """Get a specific migration by name.

        Args:
            name: Migration name (e.g., "0001_initial")

        Returns:
            LoadedMigration or None if not found
        """
        for loaded in self.discover():
            if loaded.migration.name == name:
                return loaded
        return None

    def get_by_number(self, number: int) -> LoadedMigration | None:
        """Get a specific migration by number.

        Args:
            number: Migration number (e.g., 1 for 0001_initial)

        Returns:
            LoadedMigration or None if not found
        """
        prefix = f"{number:04d}_"
        for loaded in self.discover():
            if loaded.migration.name.startswith(prefix):
                return loaded
        return None

    def _load_migration_file(self, path: Path) -> LoadedMigration | None:
        """Load a single migration file.

        Args:
            path: Path to migration file

        Returns:
            LoadedMigration or None if no Migration class found
        """
        logger.debug(f"Loading migration: {path}")

        content = path.read_text()
        checksum = _rust_runtime.migration_file_checksum(content)

        # Load module dynamically
        module_name = f"migration_{path.stem}"
        spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None:
            logger.warning(f"Could not create module spec for {path}")
            return None

        module = importlib.util.module_from_spec(spec)

        # Execute the module
        try:
            spec.loader.exec_module(module)
        except Exception as e:
            raise MigrationLoadError(f"Error executing migration {path}: {e}") from e

        # Find Migration subclass in module
        migration_cls = self._find_migration_class(module)

        if migration_cls is None:
            logger.warning(f"No Migration class found in {path}")
            return None

        # Instantiate and set metadata
        migration = migration_cls()
        migration.name = path.stem
        migration.app_label = self.migrations_dir.name

        # Prefer the JSON sidecar as the pre-lowered execution spec when present.
        # The sidecar is produced at generation time from the same op list the
        # .py renders, so it is structurally identical to what lowering the .py
        # would produce.  The .py text remains the sole checksum source (04 gate).
        execution_spec = _rust_runtime.load_migration_sidecar(str(path))

        return LoadedMigration(
            migration=migration,
            path=path,
            checksum=checksum,
            execution_spec=execution_spec,
        )

    def _find_migration_class(self, module: types.ModuleType) -> type[Migration] | None:
        """Find a Migration subclass in a module.

        Args:
            module: Loaded Python module

        Returns:
            Migration subclass or None
        """
        for name in dir(module):
            obj = getattr(module, name)
            if (
                isinstance(obj, type)
                and issubclass(obj, Migration)
                and obj is not Migration
                and not name.startswith("_")
            ):
                return obj
        return None

    def get_next_number(self) -> int:
        """Get the next available migration number.

        Returns:
            Next migration number (1 if no migrations exist)
        """
        migrations = self.discover()
        if not migrations:
            return 1

        # Extract numbers from existing migrations
        numbers = []
        for loaded in migrations:
            try:
                num = int(loaded.migration.name[:4])
                numbers.append(num)
            except (ValueError, IndexError):
                pass

        return max(numbers) + 1 if numbers else 1

    def validate_dependencies(self) -> list[str]:
        """Validate that all migration dependencies are satisfied.

        Returns:
            List of error messages (empty if valid)
        """
        from type_bridge.migration._lower import lower_migration_graph

        graph = lower_migration_graph(self.discover())
        errors = _rust_runtime.validate_migration_graph(graph)
        return [str(error["message"]) for error in errors]
