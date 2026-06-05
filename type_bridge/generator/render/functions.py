"""Render function definitions from parsed schema."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING

from ..naming import to_python_name

if TYPE_CHECKING:
    from ..models import FunctionSpec, ParsedSchema

logger = logging.getLogger(__name__)

# TypeDB type -> Python type hint
TYPE_MAPPING = {
    "string": "str",
    "integer": "int",
    "int": "int",
    "long": "int",
    "double": "float",
    "boolean": "bool",
    "bool": "bool",
    "date": "date",
    "datetime": "datetime",
    "datetime-tz": "datetime",
    "decimal": "Decimal",
    "duration": "Duration",
}


@dataclass
class FunctionParamContext:
    """Context for a single function parameter."""

    name: str  # TypeDB name (e.g., "$target_id")
    py_name: str  # Python name (e.g., "target_id")
    type_hint: str  # Python type hint (e.g., "str | Expression")
    typedb_type: str  # Original TypeDB type


@dataclass
class FunctionContext:
    """Context for rendering a single function."""

    name: str  # TypeDB name (e.g., "count-artifacts")
    py_name: str  # Python name (e.g., "count_artifacts")
    params: list[FunctionParamContext]
    return_types: list[str]  # List of TypeDB return types
    is_stream: bool  # True if returns { }
    docstring: str | None
    # Derived fields
    param_signature: str = ""  # Full Python signature string
    return_hint: str = ""  # Python return type hint


def _get_python_type(type_name: str) -> str:
    """Get Python type hint for TypeDB type."""
    is_optional = type_name.endswith("?")
    if is_optional:
        type_name = type_name[:-1]

    base = TYPE_MAPPING.get(type_name, type_name)

    if is_optional:
        return f"{base} | None"
    return base


def _get_return_type_hint(is_stream: bool, types: list[str]) -> str:
    """Generate Python return type hint for the function.

    Args:
        is_stream: Whether function returns multiple rows
        types: List of TypeDB return types

    Returns:
        Python type hint like "FunctionQuery[int]" or "FunctionQuery[tuple[str, int]]"
    """
    py_types = [_get_python_type(t) for t in types]

    if len(py_types) == 1:
        inner_type = py_types[0]
    else:
        inner_type = f"tuple[{', '.join(py_types)}]"

    if is_stream:
        return f"FunctionQuery[Iterator[{inner_type}]]"
    return f"FunctionQuery[{inner_type}]"


def _build_function_context(name: str, spec: FunctionSpec) -> FunctionContext:
    """Build template context for a single function."""
    py_name = to_python_name(name)

    # Parse parameters
    params = []
    for p in spec.parameters:
        p_py_name = to_python_name(p.name)
        p_type = _get_python_type(p.type)
        params.append(
            FunctionParamContext(
                name=f"${p.name}",
                py_name=p_py_name,
                type_hint=f"{p_type} | str",  # Allow variable references
                typedb_type=p.type,
            )
        )

    # Reconstruct ?-suffixed string list from structured IR
    is_stream = spec.return_type.is_stream
    return_types = [item.name + ("?" if item.optional else "") for item in spec.return_type.types]
    return_hint = _get_return_type_hint(is_stream, return_types)

    # Build parameter signature
    param_parts = [f"{p.py_name}: {p.type_hint}" for p in params]
    param_signature = ", ".join(param_parts)

    ctx = FunctionContext(
        name=name,
        py_name=py_name,
        params=params,
        return_types=return_types,
        is_stream=is_stream,
        docstring=spec.docstring,
        param_signature=param_signature,
        return_hint=return_hint,
    )

    return ctx


def _get_required_imports(contexts: list[FunctionContext]) -> set[str]:
    """Determine which imports are needed based on function signatures."""
    imports: set[str] = set()

    for ctx in contexts:
        for return_type in ctx.return_types:
            clean_type = return_type.rstrip("?")
            if clean_type in ("datetime", "datetime-tz"):
                imports.add("datetime")
            elif clean_type == "date":
                imports.add("date")
            elif clean_type == "decimal":
                imports.add("Decimal")
            elif clean_type == "duration":
                imports.add("Duration")

        for param in ctx.params:
            clean_type = param.typedb_type.rstrip("?")
            if clean_type in ("datetime", "datetime-tz"):
                imports.add("datetime")
            elif clean_type == "date":
                imports.add("date")
            elif clean_type == "decimal":
                imports.add("Decimal")
            elif clean_type == "duration":
                imports.add("Duration")

    return imports


def render_functions(schema: ParsedSchema) -> str:
    """Render the complete functions module.

    Generates Python function wrappers that return FunctionQuery objects
    for each TypeDB function defined in the schema.

    Args:
        schema: Parsed schema containing function definitions

    Returns:
        Complete Python source code for functions.py, or empty string if no functions
    """
    if not schema.functions:
        return ""

    logger.debug(f"Rendering {len(schema.functions)} function wrappers")

    contexts = []
    all_names = []
    for name, spec in sorted(schema.functions.items()):
        py_name = to_python_name(name)
        all_names.append(py_name)
        contexts.append(_build_function_context(name, spec))

    imports = _get_required_imports(contexts)
    datetime_imports = sorted(imports & {"datetime", "date"})
    has_decimal = "Decimal" in imports
    has_duration = "Duration" in imports

    # Generate the source code directly (template is simple enough)
    lines = [
        '"""Function wrappers generated from a TypeDB schema.',
        "",
        "These functions return FunctionQuery objects that can generate TypeQL queries",
        "for calling the corresponding TypeDB schema functions.",
        '"""',
        "",
        "from __future__ import annotations",
        "",
    ]

    # Add imports
    if datetime_imports:
        lines.append(f"from datetime import {', '.join(datetime_imports)}")
    if has_decimal:
        lines.append("from decimal import Decimal")
    if has_duration:
        lines.append("from isodate import Duration")
    lines.append("from typing import Iterator")
    lines.append("")
    lines.append("from type_bridge.expressions import FunctionQuery, ReturnType")
    lines.append("")
    lines.append("")

    # Generate each function
    for ctx in contexts:
        # Function signature
        if ctx.param_signature:
            lines.append(f"def {ctx.py_name}({ctx.param_signature}) -> {ctx.return_hint}:")
        else:
            lines.append(f"def {ctx.py_name}() -> {ctx.return_hint}:")

        # Docstring
        if ctx.docstring:
            lines.append(f'    """{ctx.docstring}')
        else:
            lines.append(f'    """Call TypeDB function `{ctx.name}`.')

        # Add return type info to docstring
        stream_info = "stream of " if ctx.is_stream else ""
        type_info = ", ".join(ctx.return_types)
        lines.append("")
        lines.append(f"    Returns: {stream_info}{type_info}")
        lines.append('    """')

        # Build args list
        if ctx.params:
            args_items = [f'("{p.name}", {p.py_name})' for p in ctx.params]
            args_str = f"[{', '.join(args_items)}]"
        else:
            args_str = "[]"

        # Build return type
        return_types_str = ", ".join(f'"{t}"' for t in ctx.return_types)
        is_stream_str = "True" if ctx.is_stream else "False"

        lines.append("    return FunctionQuery(")
        lines.append(f'        name="{ctx.name}",')
        lines.append(f"        args={args_str},")
        lines.append(
            f"        return_type=ReturnType([{return_types_str}], is_stream={is_stream_str}),"
        )
        lines.append("    )")
        lines.append("")
        lines.append("")

    # Generate __all__
    lines.append("__all__ = [")
    for name in sorted(all_names):
        lines.append(f'    "{name}",')
    lines.append("]")
    lines.append("")

    result = "\n".join(lines)
    logger.info(f"Rendered {len(all_names)} function wrappers")
    return result
