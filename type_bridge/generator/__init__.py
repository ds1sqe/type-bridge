"""TypeBridge code generator - Generate Python models from TypeDB schemas.

This module provides tools to parse TypeDB TQL schema files and generate
corresponding type-bridge Python model classes.

Example usage:

    from type_bridge.generator import generate_models

    # Generate from a schema file
    generate_models("schema.tql", "myapp/models/")

    # Or from schema text
    schema_text = '''
    define
    entity person,
        owns name @key;
    attribute name, value string;
    '''
    generate_models(schema_text, "myapp/models/")

The generated package structure:

    myapp/models/
    ├── __init__.py      # Package exports, ATTRIBUTES/ENTITIES/RELATIONS lists
    ├── attributes.py    # Attribute class definitions
    ├── entities.py      # Entity class definitions
    ├── relations.py     # Relation class definitions
    └── schema.tql       # Copy of the source schema
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal

from .annotations import extract_annotations
from .api_dto import render_api_dto
from .dto_config import (
    BaseClassConfig,
    CompositeEntityConfig,
    CompositeFieldConfig,
    DTOConfig,
    EntityFieldOverride,
    FieldOverride,
    FieldSyncConfig,
    ValidatorConfig,
)
from .models import ParsedSchema
from .parser import parse_tql_schema

try:
    from type_bridge_core import render_models_json as _rust_render_models_json

    _RUST_BINDGEN_AVAILABLE = True
except ImportError:
    _rust_render_models_json = None
    _RUST_BINDGEN_AVAILABLE = False

if TYPE_CHECKING:
    from collections.abc import Iterable

__all__ = [
    "BaseClassConfig",
    "CompositeEntityConfig",
    "CompositeFieldConfig",
    "DTOConfig",
    "EntityFieldOverride",
    "FieldOverride",
    "FieldSyncConfig",
    "ParsedSchema",
    "ValidatorConfig",
    "generate_models",
    "parse_tql_schema",
]


def generate_models(
    schema: str | Path,
    output_dir: str | Path,
    *,
    implicit_key_attributes: Iterable[str] | None = None,
    schema_version: str = "1.0.0",
    copy_schema: bool = True,
    schema_path: str | Path | None = None,
    generate_dto: bool = False,
    dto_config: DTOConfig | None = None,
    format: Literal["tql", "toml"] | None = None,  # noqa: A002
    target: Literal["python", "typescript", "rust"] = "python",
) -> None:
    """Generate TypeBridge models from a TypeDB schema.

    Args:
        schema: Either a path to a .tql file, or the schema text directly
        output_dir: Directory to write the generated package to
        implicit_key_attributes: Attribute names to treat as @key even if not marked
        schema_version: Version string for SCHEMA_VERSION constant
        copy_schema: Whether to copy the schema file to the output directory
        schema_path: Custom path for the schema file. If relative, resolved against
            output_dir. If None and copy_schema=True, uses "schema.tql" in output_dir.
        generate_dto: Whether to generate Pydantic API DTOs
        dto_config: Configuration for DTO generation (custom base classes, validators, etc.)
        format: Explicit schema format override. ``"toml"`` routes through the TOML
            transpiler; ``"tql"`` or ``None`` keeps the default TQL path. When
            ``None``, a ``.toml`` file suffix also triggers transpilation.

            .. deprecated::
                TOML desired-schema authoring (``format="toml"`` and ``.toml``
                auto-routing) is scheduled for removal in 2.1.0; author split
                YAML instead. Both TOML routes emit a
                :class:`DeprecationWarning`. The read-only
                ``type_bridge_core.toml_to_typeql`` converter is permanent.
        target: Output model language. Defaults to ``"python"`` for the historical
            Python package generator; ``"typescript"`` and ``"rust"`` use the same
            Rust-hosted bindgen engine for cross-target generation.
    """
    if not _RUST_BINDGEN_AVAILABLE or _rust_render_models_json is None:
        raise RuntimeError(
            "type_bridge_core is required to generate models but is not "
            "available. Reinstall type-bridge with its native core."
        )
    if generate_dto and target != "python":
        raise ValueError("generate_dto is only supported for the python generation target")

    # Resolve schema text
    schema_source_path: Path | None = None
    if isinstance(schema, Path):
        schema_source_path = schema
    elif isinstance(schema, str):
        # Check if it looks like a file path (short string, no newlines)
        if len(schema) < 500 and "\n" not in schema:
            try:
                candidate = Path(schema)
                if candidate.exists() and candidate.is_file():
                    schema_source_path = candidate
            except OSError:
                pass  # Not a valid path

    if schema_source_path:
        schema_text = schema_source_path.read_text(encoding="utf-8")
    else:
        schema_text = str(schema)

    # TOML routing — TOML is authoring sugar; it is transpiled to canonical TypeQL
    # and fed to the single existing parse path (invariant 1: one parse path).
    # Route when `format="toml"` is explicit, or when the resolved source path has
    # a `.toml` suffix and `format` is not overriding to "tql".  A raw-string input
    # with `format=None` stays on the TQL path — no content-sniffing.
    _is_toml = (format == "toml") or (
        format is None and schema_source_path is not None and schema_source_path.suffix == ".toml"
    )
    if _is_toml:
        import warnings

        from type_bridge_core import toml_to_typeql

        warnings.warn(
            "TOML desired-schema authoring (generate_models format='toml' and "
            ".toml auto-routing) is deprecated and scheduled for removal in "
            "type-bridge 2.1.0.  Author split YAML schema documents instead; "
            "the read-only type_bridge_core.toml_to_typeql converter remains "
            "permanent for rendering existing TOML schemas during migration.",
            DeprecationWarning,
            stacklevel=2,
        )
        schema_text = toml_to_typeql(schema_text)

    # Create output directory
    output = Path(output_dir)
    output.mkdir(parents=True, exist_ok=True)

    # Determine schema output location
    schema_filename: str | None = None
    schema_output_path: Path | None = None
    if copy_schema:
        if schema_path is None:
            # Default: schema.tql in output directory
            schema_output_path = output / "schema.tql"
            schema_filename = "schema.tql"
        else:
            resolved_path = Path(schema_path)
            if resolved_path.is_absolute():
                schema_output_path = resolved_path
                # Only include loader if schema is in the output directory
                try:
                    resolved_path.relative_to(output.resolve())
                    schema_filename = resolved_path.name
                except ValueError:
                    schema_filename = None  # Outside output dir, no loader
            else:
                # Relative path - resolve against output dir
                schema_output_path = output / resolved_path
                # Only include loader if it's a simple filename (no subdirs)
                if resolved_path.parent == Path("."):
                    schema_filename = str(resolved_path)
                else:
                    schema_filename = None  # In subdir, loader won't work

    entity_annots, attr_annots, rel_annots, role_annots = extract_annotations(schema_text)
    options: dict[str, Any] = {
        "schema_version": schema_version,
        "schema_filename": schema_filename,
        "schema_text": schema_text,
        "implicit_key_attributes": sorted(implicit_key_attributes or ()),
        "python_metadata": {
            "entity_annotations": entity_annots,
            "attribute_annotations": attr_annots,
            "relation_annotations": rel_annots,
            "role_annotations": role_annots,
        },
    }
    package = json.loads(_rust_render_models_json(schema_text, target, json.dumps(options)))
    for file_info in package["files"]:
        relative_path = Path(file_info["path"])
        path = output / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(file_info["contents"], encoding="utf-8")

    # Render Pydantic DTOs if requested. DTOs remain a Python-only wrapper feature.
    if generate_dto:
        parsed = parse_tql_schema(schema_text)
        (output / "api_dto.py").write_text(
            render_api_dto(parsed, config=dto_config),
            encoding="utf-8",
        )

    # Copy schema file if requested
    if schema_output_path:
        schema_output_path.parent.mkdir(parents=True, exist_ok=True)
        schema_output_path.write_text(schema_text, encoding="utf-8")
