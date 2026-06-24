"""Migration snapshot bindings and time-state package management."""

from __future__ import annotations

import ast
import hashlib
import json
import logging
from pathlib import Path
from typing import Any

from type_bridge import __version__ as type_bridge_version
from type_bridge.generator import generate_models

logger = logging.getLogger(__name__)


class SnapshotError(ValueError):
    """Raised when an existing migration snapshot is missing or stale."""


def generate_snapshot(
    migrations_dir: Path,
    version: str,
    migration_name: str,
    schema_text: str,
) -> Path:
    """Generate a schema snapshot package in migrations/snapshots/vNNNN/.

    Args:
        migrations_dir: Directory containing migrations (e.g. Path("migrations"))
        version: Snapshot version string, e.g. "v0001"
        migration_name: The suffix name of the migration, e.g. "0001_initial"
        schema_text: The canonical TypeQL schema text

    Returns:
        Path to the created snapshot directory
    """
    # 1. Establish directory layout
    snapshots_dir = migrations_dir / "snapshots"
    snapshots_dir.mkdir(parents=True, exist_ok=True)

    # Write empty __init__.py in snapshots/ if not exists
    init_file = snapshots_dir / "__init__.py"
    if not init_file.exists():
        init_file.write_text("# TypeBridge migration snapshots package\n")

    schema_hash = _schema_hash(schema_text)
    snapshot_dir = snapshots_dir / version
    # If the snapshot already exists, we do NOT overwrite it (append-only contract)
    if snapshot_dir.exists():
        _validate_existing_snapshot(snapshot_dir, schema_hash)
        return snapshot_dir

    snapshot_dir.mkdir(parents=True, exist_ok=False)

    try:
        # 2. Render Python classes
        generate_models(
            schema=schema_text,
            output_dir=snapshot_dir,
            copy_schema=True,
            schema_path="schema.tql",
        )

        _rewrite_snapshot_init(snapshot_dir)

        # 3. Compute canonical schema hash
        file_hashes = _file_hashes(snapshot_dir)

        # 5. Core version info
        try:
            import type_bridge_core

            core_version = getattr(type_bridge_core, "__version__", None) or getattr(
                type_bridge_core, "VERSION", "unknown"
            )
        except ImportError:
            core_version = "unknown"

        # 6. Render snapshot.json metadata
        metadata = {
            "version": version,
            "source_migration": migration_name,
            "schema_hash": schema_hash,
            "file_hashes": file_hashes,
            "type_bridge_version": type_bridge_version,
            "type_bridge_core_version": core_version,
        }
        (snapshot_dir / "snapshot.json").write_text(
            json.dumps(metadata, indent=2) + "\n",
            encoding="utf-8",
        )

        logger.info(f"Successfully generated migration snapshot in {snapshot_dir}")
        return snapshot_dir

    except Exception as e:
        # Clean up directory on failure so we don't leave corrupted state
        import shutil

        if snapshot_dir.exists():
            try:
                shutil.rmtree(snapshot_dir)
            except Exception:
                pass
        raise e


def _schema_hash(schema_text: str) -> str:
    return hashlib.sha256(schema_text.encode("utf-8")).hexdigest()


def _file_hashes(snapshot_dir: Path) -> dict[str, str]:
    file_hashes: dict[str, str] = {}
    for path in sorted(snapshot_dir.glob("*")):
        if path.is_file() and path.name != "snapshot.json":
            file_hashes[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return file_hashes


def _validate_existing_snapshot(snapshot_dir: Path, expected_schema_hash: str) -> None:
    metadata = get_snapshot_metadata(snapshot_dir)
    if metadata is None:
        raise SnapshotError(
            f"snapshot {snapshot_dir} already exists but has no readable snapshot.json"
        )

    actual_schema_hash = metadata.get("schema_hash")
    if actual_schema_hash != expected_schema_hash:
        raise SnapshotError(
            f"snapshot {snapshot_dir} schema hash mismatch: "
            f"expected {expected_schema_hash}, found {actual_schema_hash}"
        )

    expected_hashes = metadata.get("file_hashes")
    if not isinstance(expected_hashes, dict):
        raise SnapshotError(f"snapshot {snapshot_dir} has no file hash manifest")

    current_hashes = _file_hashes(snapshot_dir)
    for filename, expected_hash in expected_hashes.items():
        actual_hash = current_hashes.get(str(filename))
        if actual_hash != expected_hash:
            raise SnapshotError(
                f"snapshot {snapshot_dir} file hash mismatch for {filename}: "
                f"expected {expected_hash}, found {actual_hash}"
            )

    init_exports = (snapshot_dir / "__init__.py").read_text(encoding="utf-8")
    if (
        "from .entities import" not in init_exports
        and "from .attributes import" not in init_exports
        and "from .relations import" not in init_exports
    ):
        raise SnapshotError(f"snapshot {snapshot_dir} does not expose top-level binding imports")


def _rewrite_snapshot_init(snapshot_dir: Path) -> None:
    attributes = _class_names(snapshot_dir / "attributes.py")
    entities = _class_names(snapshot_dir / "entities.py")
    relations = _class_names(snapshot_dir / "relations.py")

    lines = [
        '"""TypeBridge migration snapshot bindings generated from a TypeDB schema.',
        "",
        "AUTO-GENERATED FILE - DO NOT EDIT MANUALLY",
        '"""',
        "",
        "from __future__ import annotations",
        "",
        "from importlib import resources",
        "",
        "from . import attributes, entities, registry, relations",
    ]
    for module, names in (
        ("attributes", attributes),
        ("entities", entities),
        ("relations", relations),
    ):
        import_block = _render_class_import(module, names)
        if import_block:
            lines.extend(["", import_block])

    lines.extend(
        [
            "",
            'SCHEMA_VERSION = "1.0.0"',
            "",
            "",
            "def schema_text() -> str:",
            '    """Return the canonical TypeDB schema text bundled with the package."""',
            "    return (",
            "        resources.files(__package__)",
            '        .joinpath("schema.tql")',
            '        .read_text(encoding="utf-8")',
            "    )",
            "",
            _render_class_list("ATTRIBUTES", attributes),
            "",
            _render_class_list("ENTITIES", entities),
            "",
            _render_class_list("RELATIONS", relations),
            "",
            _render_all([*attributes, *entities, *relations]),
        ]
    )
    (snapshot_dir / "__init__.py").write_text("\n".join(lines) + "\n", encoding="utf-8")


def _class_names(path: Path) -> list[str]:
    if not path.exists():
        return []
    tree = ast.parse(path.read_text(encoding="utf-8"))
    return [node.name for node in tree.body if isinstance(node, ast.ClassDef)]


def _render_class_import(module: str, names: list[str]) -> str:
    if not names:
        return ""
    joined = ", ".join(names)
    if len(f"from .{module} import {joined}") <= 88:
        return f"from .{module} import {joined}"
    lines = [f"from .{module} import ("]
    lines.extend(f"    {name}," for name in names)
    lines.append(")")
    return "\n".join(lines)


def _render_class_list(name: str, class_names: list[str]) -> str:
    lines = [f"{name} = ["]
    lines.extend(f"    {class_name}," for class_name in class_names)
    lines.append("]")
    return "\n".join(lines)


def _render_all(class_names: list[str]) -> str:
    exported = [
        *class_names,
        "ATTRIBUTES",
        "ENTITIES",
        "RELATIONS",
        "SCHEMA_VERSION",
        "attributes",
        "entities",
        "registry",
        "relations",
        "schema_text",
    ]
    lines = ["__all__ = ["]
    lines.extend(f'    "{name}",' for name in exported)
    lines.append("]")
    return "\n".join(lines)


def get_snapshot_metadata(snapshot_dir: Path) -> dict[str, Any] | None:
    """Read metadata from a snapshot.json file."""
    metadata_path = snapshot_dir / "snapshot.json"
    if not metadata_path.exists():
        return None
    try:
        return json.loads(metadata_path.read_text(encoding="utf-8"))
    except Exception as e:
        logger.warning(f"Failed to read snapshot metadata from {metadata_path}: {e}")
        return None


def get_snapshot_class_map(migrations_dir: Path, version: str) -> dict[str, type]:
    """Return a mapping from TypeDB type label to class for a snapshot version.

    For example, maps 'user' to migrations.snapshots.v0001.entities.User class.
    """
    import importlib
    import sys

    # Invalidate import caches so newly generated snapshot files are discoverable
    importlib.invalidate_caches()

    app_name = migrations_dir.name
    module_name = f"{app_name}.snapshots.{version}.registry"

    # Clean up sys.modules cache for the migrations package so Python imports it fresh from the new path
    for key in list(sys.modules.keys()):
        if key == app_name or key.startswith(f"{app_name}."):
            try:
                del sys.modules[key]
            except KeyError:
                pass

    parent_path = str(migrations_dir.parent.resolve())
    added_to_path = False
    if parent_path not in sys.path:
        sys.path.insert(0, parent_path)
        added_to_path = True
    try:
        reg_mod = importlib.import_module(module_name)
    except Exception as e:
        logger.warning(f"Could not load registry for snapshot {version}: {e}")
        return {}
    finally:
        if added_to_path:
            try:
                sys.path.remove(parent_path)
            except ValueError:
                pass

    class_map = {}
    if hasattr(reg_mod, "ENTITY_MAP"):
        class_map.update(reg_mod.ENTITY_MAP)
    if hasattr(reg_mod, "RELATION_MAP"):
        class_map.update(reg_mod.RELATION_MAP)
    if hasattr(reg_mod, "ATTRIBUTE_MAP"):
        class_map.update(reg_mod.ATTRIBUTE_MAP)
    return class_map
