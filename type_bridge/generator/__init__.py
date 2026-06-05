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

from pathlib import Path
from typing import TYPE_CHECKING, Literal

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
from .naming import build_class_name_map
from .parser import parse_tql_schema
from .render import (
    render_api_dto,
    render_attributes,
    render_entities,
    render_functions,
    render_package_init,
    render_registry,
    render_relations,
    render_structs,
)

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
    """
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
        from type_bridge_core import toml_to_typeql  # type: ignore[import-not-found]

        schema_text = toml_to_typeql(schema_text)

    # Create output directory
    output = Path(output_dir)
    output.mkdir(parents=True, exist_ok=True)

    # Parse schema
    parsed = parse_tql_schema(schema_text)
    implicit_keys = set(implicit_key_attributes or set())

    # Build class name mappings
    attr_class_names = build_class_name_map(parsed.attributes)
    entity_class_names = build_class_name_map(parsed.entities)
    relation_class_names = build_class_name_map(parsed.relations)
    struct_class_names = build_class_name_map(parsed.structs)

    # Generate and write files
    (output / "attributes.py").write_text(
        render_attributes(parsed, attr_class_names),
        encoding="utf-8",
    )

    (output / "entities.py").write_text(
        render_entities(parsed, attr_class_names, entity_class_names, implicit_keys),
        encoding="utf-8",
    )

    (output / "relations.py").write_text(
        render_relations(parsed, attr_class_names, entity_class_names, relation_class_names),
        encoding="utf-8",
    )

    # Render functions if present
    functions_content = render_functions(parsed)
    functions_present = False
    if functions_content:
        (output / "functions.py").write_text(functions_content, encoding="utf-8")
        functions_present = True

    # Render structs if present
    structs_content = render_structs(parsed, struct_class_names)
    if structs_content:
        (output / "structs.py").write_text(structs_content, encoding="utf-8")

    # Render registry with pre-computed metadata
    (output / "registry.py").write_text(
        render_registry(
            parsed,
            attr_class_names,
            entity_class_names,
            relation_class_names,
            schema_version=schema_version,
            schema_text=schema_text,
        ),
        encoding="utf-8",
    )

    # Render Pydantic DTOs if requested
    if generate_dto:
        (output / "api_dto.py").write_text(
            render_api_dto(parsed, config=dto_config),
            encoding="utf-8",
        )

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

    (output / "__init__.py").write_text(
        render_package_init(
            attr_class_names,
            entity_class_names,
            relation_class_names,
            schema_version=schema_version,
            include_schema_loader=schema_filename is not None,
            schema_filename=schema_filename,
            functions_present=functions_present,
        ),
        encoding="utf-8",
    )

    # Copy schema file if requested
    if schema_output_path:
        schema_output_path.parent.mkdir(parents=True, exist_ok=True)
        schema_output_path.write_text(schema_text, encoding="utf-8")
