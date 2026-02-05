"""CLI entry point for the TypeBridge schema generator.

Usage:
    python -m type_bridge.generator schema.tql -o ./myapp/models/
    python -m type_bridge.generator schema.tql --output ./myapp/models/ --version 2.0.0
    python -m type_bridge.generator schema.tql -o ./models/ --dto --dto-config myapp.config:dto_config
"""

from __future__ import annotations

import importlib
import logging
from pathlib import Path
from typing import Annotated

import typer

from . import DTOConfig, generate_models

logger = logging.getLogger(__name__)

app = typer.Typer(
    name="type_bridge.generator",
    help="Generate TypeBridge Python models from a TypeDB schema file.",
    epilog=(
        "The --output directory is required. We recommend a dedicated "
        "directory like './myapp/models/' or './src/schema/' to keep "
        "generated code separate from hand-written code."
    ),
    no_args_is_help=True,
)


def _load_dto_config(config_path: str) -> DTOConfig:
    """Load a DTOConfig from a Python module path.

    Args:
        config_path: Module path in format 'module.path:attribute'
            e.g., 'myapp.config:dto_config'

    Returns:
        The loaded DTOConfig instance

    Raises:
        typer.Exit: If the config cannot be loaded
    """
    if ":" not in config_path:
        typer.echo(
            f"Error: --dto-config must be in format 'module.path:attribute', got '{config_path}'",
            err=True,
        )
        raise typer.Exit(1)

    module_path, attr_name = config_path.rsplit(":", 1)

    try:
        module = importlib.import_module(module_path)
    except ImportError as e:
        typer.echo(f"Error: Could not import module '{module_path}': {e}", err=True)
        raise typer.Exit(1)

    try:
        config = getattr(module, attr_name)
    except AttributeError:
        typer.echo(f"Error: Module '{module_path}' has no attribute '{attr_name}'", err=True)
        raise typer.Exit(1)

    if not isinstance(config, DTOConfig):
        typer.echo(
            f"Error: '{config_path}' is not a DTOConfig instance (got {type(config).__name__})",
            err=True,
        )
        raise typer.Exit(1)

    return config


@app.command()
def main(
    schema: Annotated[
        Path,
        typer.Argument(help="Path to the TypeDB schema file (.tql)"),
    ],
    output: Annotated[
        Path,
        typer.Option("--output", "-o", help="Output directory for generated package"),
    ],
    version: Annotated[
        str,
        typer.Option(help="Schema version string"),
    ] = "1.0.0",
    no_copy_schema: Annotated[
        bool,
        typer.Option("--no-copy-schema", help="Don't copy the schema file to the output directory"),
    ] = False,
    schema_path: Annotated[
        str | None,
        typer.Option(
            "--schema-path",
            help=(
                "Custom path for the schema file. Relative paths are resolved "
                "against the output directory. Absolute paths write to that location."
            ),
        ),
    ] = None,
    implicit_keys: Annotated[
        list[str] | None,
        typer.Option("--implicit-keys", help="Attribute names to treat as @key even if not marked"),
    ] = None,
    dto: Annotated[
        bool,
        typer.Option("--dto", help="Generate Pydantic API DTOs (api_dto.py)"),
    ] = False,
    dto_config: Annotated[
        str | None,
        typer.Option(
            "--dto-config",
            help=(
                "Python module path to DTOConfig instance in format 'module.path:attribute'. "
                "Example: 'myapp.config:dto_config'"
            ),
        ),
    ] = None,
) -> None:
    """Generate TypeBridge Python models from a TypeDB schema file."""
    logger.debug(f"CLI arguments: schema={schema}, output={output}, version={version}")

    if not schema.exists():
        logger.error(f"Schema file not found: {schema}")
        typer.echo(f"Error: Schema file not found: {schema}", err=True)
        raise typer.Exit(1)

    if not schema.is_file():
        logger.error(f"Not a file: {schema}")
        typer.echo(f"Error: Not a file: {schema}", err=True)
        raise typer.Exit(1)

    # Load DTO config if specified
    loaded_config: DTOConfig | None = None
    if dto_config:
        if not dto:
            typer.echo(
                "Warning: --dto-config specified but --dto not enabled. Config will be ignored.",
                err=True,
            )
        else:
            loaded_config = _load_dto_config(dto_config)

    try:
        logger.info(f"Generating models from {schema} to {output}")
        generate_models(
            schema=schema,
            output_dir=output,
            implicit_key_attributes=implicit_keys or None,
            schema_version=version,
            copy_schema=not no_copy_schema,
            schema_path=schema_path,
            generate_dto=dto,
            dto_config=loaded_config,
        )
    except Exception as e:
        logger.error(f"Generation failed: {e}", exc_info=True)
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(1)

    logger.info(f"Successfully generated models in: {output}")
    typer.echo(f"Generated TypeBridge models in: {output}")


if __name__ == "__main__":
    app()
