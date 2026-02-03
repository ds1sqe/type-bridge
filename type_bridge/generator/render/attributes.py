"""Render attribute class definitions from parsed schema."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING

from ..utils import TYPEDB_TO_BRIDGE, topological_sort
from .template_loader import get_template

if TYPE_CHECKING:
    from ..models import ParsedSchema

logger = logging.getLogger(__name__)

# Alias for internal use
VALUE_TYPE_MAP = TYPEDB_TO_BRIDGE


@dataclass
class AttributeContext:
    """Context for rendering a single attribute class."""

    class_name: str
    base_class: str
    docstring: str
    flags_args: list[str]
    regex: str | None = None
    allowed_values: list[str] | None = None
    range_min: str | None = None
    range_max: str | None = None
    default: object = None
    transform: object = None
    independent: bool = False


def _resolve_value_type(
    attr_name: str,
    schema: ParsedSchema,
    visited: set[str] | None = None,
) -> str:
    """Resolve the value type for an attribute, following inheritance."""
    if visited is None:
        visited = set()

    if attr_name in visited:
        return "String"

    visited.add(attr_name)
    attr = schema.attributes.get(attr_name)

    if attr is None:
        return "String"

    if attr.value_type:
        return attr.value_type

    if attr.parent:
        return _resolve_value_type(attr.parent, schema, visited)

    return "String"


def _resolve_base_class(
    attr_name: str,
    schema: ParsedSchema,
    class_names: dict[str, str],
) -> str:
    """Determine the base class for an attribute."""
    attr = schema.attributes[attr_name]

    if attr.parent and attr.parent in schema.attributes:
        return class_names[attr.parent]

    value_type = _resolve_value_type(attr_name, schema)
    return VALUE_TYPE_MAP.get(value_type, "String")


def _get_required_imports(schema: ParsedSchema, class_names: dict[str, str]) -> set[str]:
    """Determine which type-bridge imports are needed."""
    imports: set[str] = {"AttributeFlags"}

    for attr in schema.attributes.values():
        base = _resolve_base_class(attr.name, schema, class_names)
        if base in VALUE_TYPE_MAP.values():
            imports.add(base)

    if any(attr.case for attr in schema.attributes.values()):
        imports.add("TypeNameCase")

    return imports


def _topological_sort_attributes(schema: ParsedSchema) -> list[str]:
    """Sort attributes so parents come before children."""
    return topological_sort(schema.attributes, lambda attr: attr.parent)


def _build_attribute_context(
    attr_name: str,
    schema: ParsedSchema,
    class_names: dict[str, str],
) -> AttributeContext:
    """Build template context for a single attribute."""
    attr = schema.attributes[attr_name]
    cls_name = class_names[attr_name]
    base_class = _resolve_base_class(attr_name, schema, class_names)

    docstring = attr.docstring if attr.docstring else f"Attribute for `{attr_name}`."

    flags_args = [f'name="{attr_name}"']
    if attr.case:
        flags_args.append(f"case=TypeNameCase.{attr.case}")

    return AttributeContext(
        class_name=cls_name,
        base_class=base_class,
        docstring=docstring,
        flags_args=flags_args,
        regex=attr.regex,
        allowed_values=list(attr.allowed_values) if attr.allowed_values else None,
        range_min=attr.range_min,
        range_max=attr.range_max,
        default=attr.default,
        transform=attr.transform,
        independent=attr.independent,
    )


def render_attributes(schema: ParsedSchema, class_names: dict[str, str]) -> str:
    """Render the complete attributes module source.

    Args:
        schema: Parsed schema containing attribute definitions
        class_names: Mapping from TypeDB names to Python class names

    Returns:
        Complete Python source code for attributes.py
    """
    logger.debug(f"Rendering {len(schema.attributes)} attribute classes")

    imports = sorted(_get_required_imports(schema, class_names))
    uses_classvar = any(
        attr.allowed_values or attr.regex or attr.range_min or attr.range_max
        for attr in schema.attributes.values()
    )

    attributes = []
    all_names = []
    for attr_name in _topological_sort_attributes(schema):
        all_names.append(class_names[attr_name])
        attributes.append(_build_attribute_context(attr_name, schema, class_names))

    template = get_template("attributes.py.jinja")
    result = template.render(
        imports=imports,
        uses_classvar=uses_classvar,
        attributes=attributes,
        all_names=sorted(all_names),
    )

    logger.info(f"Rendered {len(all_names)} attribute classes")
    return result
