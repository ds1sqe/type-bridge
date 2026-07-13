"""Offline migration artifact authoring (#166).

Authors the complete checked artifact set — reviewable ``NNNN_name.py``,
executable ``NNNN_name.json`` sidecar, and immutable ``snapshots/vNNNN/``
files — from two serialized ``SchemaInfo`` dictionaries. No TypeDB
connection, no ``Database``, no ``ModelRegistry``, and no live model
classes are involved: this module only marshals inputs into the Rust
authoring core and offers filesystem convenience around its result.

The live ``makemigrations`` flow delegates to the same Rust core, so both
paths share one canonical ``SchemaDiff -> ordered operations`` mapping.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any

from type_bridge._rust_runtime import rust_core

if TYPE_CHECKING:
    from pathlib import Path

__all__ = ["AuthoredMigration", "author_migration"]


class AuthoredMigration:
    """A complete authored migration artifact set, held in memory.

    Wraps the Rust authoring result. ``files`` contains every artifact as
    ``(relative_path, bytes)`` pairs; :meth:`write_to` persists them through
    the validated all-or-nothing writer.
    """

    def __init__(self, inner: Any) -> None:
        self._inner = inner

    @property
    def migration_name(self) -> str:
        """Full migration stem, e.g. ``0003_add_assignment``."""
        return str(self._inner.migration_name)

    @property
    def python_source(self) -> str:
        """The reviewable ``.py`` source text (sole checksum source)."""
        return str(self._inner.python_source)

    @property
    def spec(self) -> dict[str, Any]:
        """The canonical ``MigrationSpec`` as a normalized dict."""
        return dict(self._inner.spec)

    @property
    def files(self) -> list[tuple[str, bytes]]:
        """Every artifact file as ``(relative_path, contents)``."""
        return [(str(path), bytes(contents)) for path, contents in self._inner.files]

    def write_to(
        self,
        migrations_dir: Path,
        *,
        on_existing: str = "validate_identical",
    ) -> Path:
        """Write all artifacts under ``migrations_dir``.

        Existing files must be byte-identical (``validate_identical``) or
        absent (``fail``); collisions and drift abort before anything is
        written. Snapshots stay append-only.

        Returns:
            Path to the written migration ``.py`` file.
        """
        self._inner.write_to(str(migrations_dir), on_existing)
        return migrations_dir / f"{self.migration_name}.py"


def author_migration(
    base: dict[str, Any],
    target: dict[str, Any],
    *,
    app_label: str,
    name: str,
    snapshot_version: str,
    dependencies: list[tuple[str, str]] | None = None,
    previous_snapshot_version: str | None = None,
    before_schema: list[dict[str, Any]] | None = None,
    after_schema: list[dict[str, Any]] | None = None,
    generated_at: str | None = None,
) -> AuthoredMigration | None:
    """Author the complete migration artifact set from serialized schemas.

    Args:
        base: Serialized ``SchemaInfo`` of the current (remote) schema.
        target: Serialized ``SchemaInfo`` the migration should produce.
        app_label: Migrations package name (the migrations directory name).
        name: Full migration stem, e.g. ``0003_add_assignment``.
        snapshot_version: Snapshot version to generate, e.g. ``v0003``.
        dependencies: ``(app_label, migration_name)`` dependency pairs.
        previous_snapshot_version: The prior snapshot version (``v0002``)
            whose bindings removal operations reference, if one exists.
        before_schema: Serialized operations executed before the schema
            change set (e.g. destructive data cleanup).
        after_schema: Serialized operations executed after the schema
            change set (e.g. backfills).
        generated_at: Explicit timestamp text for the generated ``.py``
            docstring. Defaults to now; pass a fixed value for
            byte-deterministic output.

    Returns:
        The authored artifact set, or ``None`` when the diff is empty and
        no explicit operations were supplied.
    """
    from type_bridge import __version__ as type_bridge_version

    core = rust_core()
    core_version = getattr(core, "__version__", None) or getattr(core, "VERSION", "unknown")
    inner = core.author_migration(
        base,
        target,
        app_label=app_label,
        name=name,
        dependencies=list(dependencies or []),
        snapshot_version=snapshot_version,
        generated_at=generated_at or datetime.now(UTC).isoformat(),
        type_bridge_version=type_bridge_version,
        type_bridge_core_version=str(core_version),
        previous_snapshot_version=previous_snapshot_version,
        before_schema=before_schema,
        after_schema=after_schema,
    )
    if inner is None:
        return None
    return AuthoredMigration(inner)
