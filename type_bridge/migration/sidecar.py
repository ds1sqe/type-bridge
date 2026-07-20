"""Checked JSON sidecar generation for Python-only migration histories.

The native (Rust) migration path executes only ``NNNN_name.json`` sidecars;
a ``.py``-only migration that 1.5.x ran through dynamic import has no
executable representation there. This module converts such histories by
loading each migration through the released :class:`MigrationLoader`
(dynamic import fallback intact) and lowering it with the released
:func:`~type_bridge.migration._lower.lower_migration` — the exact lowering
the Python executor uses — so the generated sidecar and the historical
execution semantics agree by construction.

Conversion is all-or-nothing: every convertible migration is lowered
before any sidecar is written, and a history containing an unconvertible
migration (for example ``ops.RunPython``, which has no serializable
execution spec) fails with a report naming each blocker and writes
nothing.

Usage::

    python -m type_bridge.migration.sidecar path/to/migrations
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from type_bridge import _rust_runtime
from type_bridge.migration._lower import lower_migration
from type_bridge.migration.loader import LoadedMigration, MigrationLoader

__all__ = ["SidecarConversionError", "generate_sidecars"]


class SidecarConversionError(Exception):
    """A migration history contains migrations that cannot be lowered.

    Raised before any sidecar is written. ``blockers`` maps each
    unconvertible migration name to the reason its operations cannot be
    represented as a checked execution spec.
    """

    def __init__(self, blockers: dict[str, str]) -> None:
        self.blockers = dict(blockers)
        details = "; ".join(f"{name}: {reason}" for name, reason in sorted(self.blockers.items()))
        super().__init__(
            "cannot generate sidecars for this history; no files were written. "
            f"Unconvertible migrations: {details}. Rewrite Python-only operations "
            "(e.g. ops.RunPython) as ops.RunTypeQL, or keep executing this history "
            "through the Python migration engine."
        )


@dataclass
class _PendingSidecar:
    loaded: LoadedMigration
    spec_json: str

    @property
    def sidecar_path(self) -> Path:
        return self.loaded.path.with_suffix(".json")


def generate_sidecars(migrations_dir: Path) -> list[Path]:
    """Generate checked JSON sidecars for every py-only migration in a directory.

    Discovers the history through the released loader (sidecar-backed
    migrations are skipped; drift between an existing sidecar and its
    ``.py`` fails discovery), lowers each remaining migration with the
    released execution lowering, and writes one ``NNNN_name.json`` per
    converted migration with the ``.py`` checksum embedded for the native
    drift guard.

    Returns:
        Paths of the sidecars written, in history order. Empty when every
        migration already carries a sidecar.

    Raises:
        SidecarConversionError: When any migration cannot be lowered.
            Nothing is written in that case.
        MigrationLoadError: When discovery itself fails (unreadable file,
            stale existing sidecar, broken migration module).
    """
    loader = MigrationLoader(migrations_dir)
    pending: list[_PendingSidecar] = []
    blockers: dict[str, str] = {}

    for loaded in loader.discover():
        if loaded.execution_spec is not None:
            continue
        try:
            spec = lower_migration(loaded.migration, checksum=loaded.checksum)
            spec_json = _rust_runtime.migration_spec_to_json(spec)
        except (TypeError, ValueError) as error:
            blockers[loaded.migration.name] = str(error)
            continue
        pending.append(_PendingSidecar(loaded=loaded, spec_json=spec_json))

    if blockers:
        raise SidecarConversionError(blockers)

    written: list[Path] = []
    for item in pending:
        with item.sidecar_path.open("x", encoding="utf-8") as sidecar:
            sidecar.write(item.spec_json)
        written.append(item.sidecar_path)
    return written


def _main() -> int:
    import argparse

    parser = argparse.ArgumentParser(
        prog="python -m type_bridge.migration.sidecar",
        description=(
            "Generate checked JSON execution sidecars for Python-only "
            "migrations so the native migration path can adopt the history."
        ),
    )
    parser.add_argument(
        "migrations_dir",
        type=Path,
        help="Directory containing NNNN_name.py migration files",
    )
    args = parser.parse_args()

    try:
        written = generate_sidecars(args.migrations_dir)
    except Exception as error:  # argparse-style CLI boundary: report, nonzero exit
        print(f"error: {error}")
        return 1

    if not written:
        print("all migrations already carry sidecars; nothing to do")
    else:
        for path in written:
            print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
