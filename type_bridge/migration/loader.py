"""Migration file loader and discovery.

Discovers and loads migration files from a directory structure.
Migration files must follow the naming convention: NNNN_name.py (e.g., 0001_initial.py)
"""

from __future__ import annotations

import errno
import hashlib
import importlib.util
import logging
import types
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from type_bridge import _rust_runtime
from type_bridge.migration._adoption_authority import (
    AdoptionDirectoryAuthority,
    AdoptionDirectoryEntry,
    AdoptionDirectoryError,
)
from type_bridge.migration._archive_base import _ArchivedMigration as Migration

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
            a ``.json`` sibling; ``None`` for archived hand-authored files.
            Keyword-only so existing positional construction sites are unaffected.
    """

    migration: Migration
    path: Path
    checksum: str
    execution_spec: dict[str, Any] | None = field(default=None, kw_only=True)
    source_sha256: str | None = field(default=None, kw_only=True)
    execution_sidecar_sha256: str | None = field(default=None, kw_only=True)
    execution_sidecar_json: str | None = field(default=None, kw_only=True, repr=False)
    execution_sidecar_entry: AdoptionDirectoryEntry | None = field(
        default=None,
        kw_only=True,
        repr=False,
    )

    def __repr__(self) -> str:
        return f"<LoadedMigration {self.migration.app_label}.{self.migration.name}>"


@dataclass(frozen=True)
class IgnoredMigrationSource:
    """Checksum evidence for a migration-shaped source ignored by V1 discovery.

    The released loader skips ``NNNN_*.py`` files that do not expose a public
    :class:`Migration` subclass. Adoption conversion retains that exact
    classification so the native reader can verify, then deliberately omit,
    the same source without treating a missing archive as authority loss.
    """

    path: Path
    checksum: str
    source_sha256: str


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
    MAX_MIGRATION_FILES = 65_536
    MAX_MIGRATION_FILE_BYTES = 16 * 1024 * 1024
    MAX_MIGRATION_HISTORY_BYTES = 256 * 1024 * 1024

    def __init__(
        self,
        migrations_dir: Path,
        *,
        use_sidecars: bool = True,
        adoption_limits: bool = False,
        directory_authority: AdoptionDirectoryAuthority | None = None,
        adoption_import_dir: Path | None = None,
    ):
        """Initialize loader.

        Args:
            migrations_dir: Directory containing migration files
            use_sidecars: Prefer checked execution sidecars when present.
                Adoption metadata generation preserves this released behavior:
                a retained valid sidecar is authoritative and prevents Python
                execution; only a source with no sidecar is imported.
            adoption_limits: Apply the bounded, no-follow reader used only by
                adoption metadata generation. The released loader path stays
                byte-for-byte compatible with its unbounded ``glob`` and
                ``Path.read_text()`` behavior.
            directory_authority: Retained adoption-only directory capability.
                When absent, an adoption-limited discovery retains one for the
                duration of that discovery. Ordinary V1 discovery ignores it.
            adoption_import_dir: Private retained mirror of ``migrations_dir``.
                Required for adoption discovery so checksum decoding, module
                execution, and package imports all consume the same captured
                bytes without writing bytecode into the retained authority.
        """
        self.migrations_dir = migrations_dir
        self.use_sidecars = use_sidecars
        self.adoption_limits = adoption_limits
        self._directory_authority = directory_authority
        self._adoption_import_dir = adoption_import_dir
        self._history_bytes = 0
        self._ignored_sources: list[IgnoredMigrationSource] = []
        self._adoption_entries: dict[str, AdoptionDirectoryEntry] = {}

    @property
    def ignored_sources(self) -> tuple[IgnoredMigrationSource, ...]:
        """Return ignored-source evidence captured by the last discovery.

        This is populated only by the adoption-limited trusted-reader path.
        Ordinary released discovery retains its historical return contract.
        """

        return tuple(self._ignored_sources)

    @property
    def adoption_entries(self) -> dict[str, AdoptionDirectoryEntry]:
        """Return captured source revisions from the last adoption discovery."""
        return dict(self._adoption_entries)

    def discover(self) -> list[LoadedMigration]:
        """Discover all migration files in order.

        Returns:
            List of loaded migrations, sorted by filename
        """
        self._ignored_sources = []
        self._adoption_entries = {}
        if self.adoption_limits and self._adoption_import_dir is None:
            if not self.migrations_dir.exists():
                logger.debug(f"Migrations directory does not exist: {self.migrations_dir}")
                return []
            from type_bridge.migration._adoption_import import (
                RetainedImportError,
                retained_import_mirror,
            )

            owned = self._directory_authority is None
            authority = self._directory_authority
            if authority is None:
                try:
                    authority = AdoptionDirectoryAuthority.open(self.migrations_dir)
                except OSError as error:
                    raise MigrationLoadError(
                        "Failed to retain migrations directory authority"
                    ) from error
            revision = authority.directory_revision()
            try:
                try:
                    with retained_import_mirror(
                        authority,
                        self.migrations_dir,
                        revision,
                    ) as mirror:
                        retained_loader = MigrationLoader(
                            self.migrations_dir,
                            use_sidecars=self.use_sidecars,
                            adoption_limits=True,
                            directory_authority=authority,
                            adoption_import_dir=mirror.package_dir,
                        )
                        migrations = retained_loader.discover()
                        self._ignored_sources = list(retained_loader.ignored_sources)
                        self._adoption_entries = retained_loader.adoption_entries
                        return migrations
                except RetainedImportError as error:
                    raise MigrationLoadError(str(error)) from error
            finally:
                if owned:
                    authority.close()

        owned_authority: AdoptionDirectoryAuthority | None = None
        if self.adoption_limits and self._directory_authority is None:
            if not self.migrations_dir.exists():
                logger.debug(f"Migrations directory does not exist: {self.migrations_dir}")
                return []
            try:
                owned_authority = AdoptionDirectoryAuthority.open(self.migrations_dir)
            except OSError as error:
                raise MigrationLoadError(
                    "Failed to retain migrations directory authority"
                ) from error
            self._directory_authority = owned_authority
        elif not self.migrations_dir.exists() and not self.adoption_limits:
            logger.debug(f"Migrations directory does not exist: {self.migrations_dir}")
            return []

        try:
            authority_revision = (
                self._require_directory_authority().directory_revision()
                if self.adoption_limits
                else None
            )
            if self.adoption_limits:
                files = self._discover_adoption_sources()
            else:
                # This is the released 1.5.x discovery contract. In particular,
                # unrelated directory entries are filtered by glob rather than
                # counted toward a new resource ceiling.
                files = sorted(self.migrations_dir.glob(self.MIGRATION_PATTERN))
            migrations: list[LoadedMigration] = []
            self._history_bytes = 0

            for path in files:
                try:
                    loaded = self._load_migration_file(path)
                    if loaded:
                        migrations.append(loaded)
                except Exception as e:
                    logger.error(f"Failed to load migration {path}: {e}")
                    raise MigrationLoadError(f"Failed to load migration {path}: {e}") from e

            if authority_revision is not None:
                self._require_directory_authority().require_directory_revision(authority_revision)
            logger.debug(f"Discovered {len(migrations)} migration(s) in {self.migrations_dir}")
            return migrations
        except AdoptionDirectoryError as error:
            raise MigrationLoadError(
                "Migration authority changed during bounded adoption discovery"
            ) from error
        finally:
            if owned_authority is not None:
                owned_authority.close()
                self._directory_authority = None

    def _discover_adoption_sources(self) -> list[Path]:
        """Discover archive sources through the bounded adoption trust boundary."""
        files: list[Path] = []
        try:
            entries = self._require_directory_authority().entries(
                maximum_entries=self.MAX_MIGRATION_FILES
            )
            for entry in entries:
                name = entry.name
                if (
                    len(name) >= 10
                    and name[:4].isascii()
                    and name[:4].isdigit()
                    and name[4] == "_"
                    and name.endswith(".json")
                    and not name.endswith(".adoption.json")
                ):
                    self._adoption_entries[name] = entry
                if not (
                    len(name) >= 8
                    and name[:4].isascii()
                    and name[:4].isdigit()
                    and name[4] == "_"
                    and name.endswith(".py")
                ):
                    continue
                if entry.is_symlink() or not entry.is_file():
                    raise MigrationLoadError(
                        f"Migration source {name} must be a regular file, "
                        "not a link or special entry"
                    )
                files.append(self.migrations_dir / name)
                self._adoption_entries[name] = entry
        except OSError as error:
            if error.errno == errno.E2BIG:
                raise MigrationLoadError(
                    f"Migration directory exceeds {self.MAX_MIGRATION_FILES} file entries"
                ) from error
            raise MigrationLoadError(
                f"Failed to enumerate migrations directory {self.migrations_dir}: {error}"
            ) from error
        files.sort()
        return files

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

        if self.adoption_limits:
            raw_content = self._read_adoption_source(path)
            try:
                import_path = self._require_adoption_import_dir() / path.name
                # This deliberately retains the released checksum contract:
                # Path.read_text() uses the runtime's default encoding and
                # universal-newline handling. The private mirror contains the
                # exact raw bytes captured above.
                content = import_path.read_text()
            except (OSError, UnicodeError) as error:
                raise MigrationLoadError(
                    f"Migration file {path} cannot be decoded with the released loader encoding"
                ) from error
            source_sha256 = hashlib.sha256(raw_content).hexdigest()
        else:
            # Preserve the released loader's encoding, universal-newline,
            # symlink-following, and error behavior.
            content = path.read_text()
            raw_content = None
            source_sha256 = None
        checksum = _rust_runtime.migration_file_checksum(content)

        execution_sidecar_sha256: str | None = None
        execution_sidecar_json: str | None = None
        execution_sidecar_entry: AdoptionDirectoryEntry | None = None
        if self.use_sidecars and self.adoption_limits:
            retained_sidecar = self._load_adoption_sidecar(path)
            if retained_sidecar is None:
                execution_spec = None
            else:
                (
                    execution_spec,
                    execution_sidecar_json,
                    execution_sidecar_sha256,
                    execution_sidecar_entry,
                ) = retained_sidecar
        else:
            execution_spec = (
                _rust_runtime.load_migration_sidecar(str(path)) if self.use_sidecars else None
            )
        if execution_spec is not None:
            sidecar_checksum = execution_spec.get("checksum")
            if sidecar_checksum is not None and sidecar_checksum != checksum:
                raise MigrationLoadError(
                    f"sidecar drift detected for {path}: sidecar checksum "
                    f"{sidecar_checksum} does not match current .py checksum {checksum}"
                )
            migration = self._migration_from_sidecar(execution_spec, path)
            return LoadedMigration(
                migration=migration,
                path=path,
                checksum=checksum,
                execution_spec=execution_spec,
                source_sha256=source_sha256,
                execution_sidecar_sha256=execution_sidecar_sha256,
                execution_sidecar_json=execution_sidecar_json,
                execution_sidecar_entry=execution_sidecar_entry,
            )

        # Load module dynamically
        module_name = f"migration_{path.stem}"
        import_path = (
            self._require_adoption_import_dir() / path.name if self.adoption_limits else path
        )
        spec = importlib.util.spec_from_file_location(module_name, import_path)
        if spec is None or spec.loader is None:
            logger.warning(f"Could not create module spec for {path}")
            return None

        module = importlib.util.module_from_spec(spec)
        from type_bridge.migration._archive_imports import (
            archive_builtins,
            archive_import_context,
        )

        module.__dict__["__builtins__"] = archive_builtins()

        # Execute the module
        import sys

        parent_path = str(self.migrations_dir.parent.resolve())
        added_to_path = False
        if not self.adoption_limits and parent_path not in sys.path:
            sys.path.insert(0, parent_path)
            added_to_path = True
        try:
            # Adoption points this standard loader at a private mirror made
            # from the retained raw bytes. Ordinary discovery keeps the
            # released ambient path unchanged.
            with archive_import_context():
                spec.loader.exec_module(module)
        except Exception as e:
            raise MigrationLoadError(f"Error executing migration {path}: {e}") from e
        finally:
            if added_to_path:
                try:
                    sys.path.remove(parent_path)
                except ValueError:
                    pass

        # Find Migration subclass in module
        migration_cls = self._find_migration_class(module)

        if migration_cls is None:
            if self.adoption_limits:
                assert source_sha256 is not None
                self._ignored_sources.append(
                    IgnoredMigrationSource(
                        path=path,
                        checksum=checksum,
                        source_sha256=source_sha256,
                    )
                )
                logger.debug(f"V1 ignored migration-shaped source {path}")
            else:
                logger.warning(f"No Migration class found in {path}")
            return None

        # Instantiate and set metadata
        migration = migration_cls()
        migration.name = path.stem
        migration.app_label = self.migrations_dir.name

        return LoadedMigration(
            migration=migration,
            path=path,
            checksum=checksum,
            execution_spec=None,
            source_sha256=source_sha256,
        )

    def _read_adoption_source(self, path: Path) -> bytes:
        """Read one checksum authority with adoption-only resource limits."""
        try:
            raw = self._require_directory_authority().read_bounded(
                Path(path.name),
                self.MAX_MIGRATION_FILE_BYTES,
                expected=self._adoption_entries.get(path.name),
            )
        except AdoptionDirectoryError as error:
            if error.errno == errno.EFBIG:
                raise MigrationLoadError(
                    f"Migration file {path.name} exceeds the 16 MiB byte ceiling"
                ) from error
            raise MigrationLoadError(
                f"Migration source {path.name} changed or is not a regular file"
            ) from error
        self._history_bytes += len(raw)
        if self._history_bytes > self.MAX_MIGRATION_HISTORY_BYTES:
            raise MigrationLoadError("Migration history exceeds the 256 MiB byte ceiling")
        return raw

    def _load_adoption_sidecar(
        self,
        source_path: Path,
    ) -> tuple[dict[str, Any], str, str, AdoptionDirectoryEntry] | None:
        """Load one released sidecar through retained no-follow authority."""
        relative = Path(source_path.with_suffix(".json").name)
        entry = self._adoption_entries.get(relative.name)
        if entry is None:
            try:
                entry = self._require_directory_authority().inspect_direct(relative.name)
            except AdoptionDirectoryError as error:
                raise MigrationLoadError(
                    f"Failed to inspect migration sidecar {relative.name}"
                ) from error
        if entry is None:
            return None
        if entry.is_symlink() or not entry.is_file():
            raise MigrationLoadError(
                f"Migration sidecar {relative.name} must be a regular file, "
                "not a link or special entry"
            )
        try:
            body = self._require_directory_authority().read_bounded(
                relative,
                self.MAX_MIGRATION_FILE_BYTES,
                expected=entry,
            )
        except AdoptionDirectoryError as error:
            if error.errno == errno.EFBIG:
                raise MigrationLoadError(
                    f"Migration sidecar {relative.name} exceeds the 16 MiB byte ceiling"
                ) from error
            raise MigrationLoadError(
                f"Migration sidecar {relative.name} changed or is not a regular file"
            ) from error
        self._history_bytes += len(body)
        if self._history_bytes > self.MAX_MIGRATION_HISTORY_BYTES:
            raise MigrationLoadError("Migration history exceeds the 256 MiB byte ceiling")
        try:
            exact_json = body.decode("utf-8")
            normalized = _rust_runtime.migration_spec_from_json(exact_json)
        except (UnicodeDecodeError, TypeError, ValueError) as error:
            raise MigrationLoadError(
                f"Failed to parse migration sidecar {relative.name}: {error}"
            ) from error
        return normalized, exact_json, hashlib.sha256(body).hexdigest(), entry

    def _require_directory_authority(self) -> AdoptionDirectoryAuthority:
        authority = self._directory_authority
        if authority is None:
            raise MigrationLoadError("Adoption directory authority is unavailable")
        return authority

    def _require_adoption_import_dir(self) -> Path:
        directory = self._adoption_import_dir
        if directory is None:
            raise MigrationLoadError(
                "Adoption import mirror is unavailable; use checked sidecar conversion"
            )
        return directory

    def _migration_from_sidecar(self, execution_spec: dict[str, Any], path: Path) -> Migration:
        """Create lightweight migration metadata without executing sidecar-backed .py."""
        dependencies = [
            (str(dependency["app_label"]), str(dependency["migration_name"]))
            for dependency in execution_spec.get("dependencies", [])
        ]
        migration_cls = type(
            "SidecarMigration",
            (Migration,),
            {
                "dependencies": dependencies,
                "operations": [],
                "models": [],
                "reversible": bool(execution_spec.get("reversible", True)),
            },
        )
        migration = migration_cls()
        migration.name = str(execution_spec.get("name") or path.stem)
        migration.app_label = str(execution_spec.get("app_label") or self.migrations_dir.name)
        return migration

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
        from type_bridge.migration._lower import lower_validation_graph

        graph = lower_validation_graph(self.discover())
        errors = _rust_runtime.validate_migration_graph(graph)
        return [str(error["message"]) for error in errors]
