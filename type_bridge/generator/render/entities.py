"""Render entity class definitions from parsed schema."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from ..naming import to_python_name
from .template_loader import get_template

if TYPE_CHECKING:
    from ..models import Cardinality, ParsedSchema

logger = logging.getLogger(__name__)


@dataclass
class EntityContext:
    """Context for rendering a single entity class."""

    class_name: str
    base_class: str
    docstring: str
    flags_args: list[str]
    prefix: str | None = None
    plays: list[str] = field(default_factory=list)
    fields: list[str] = field(default_factory=list)
    # Coming-soon annotations (parsed but not yet available in TypeDB 3.x)
    # These will render as TODO comments in generated code
    cascade_attrs: list[str] = field(default_factory=list)
    subkey_groups: dict[str, list[str]] = field(default_factory=dict)
    # Plays cardinality constraints (e.g., plays friendship:friend @card(0..5))
    plays_cardinalities: dict[str, str] = field(default_factory=dict)


def _render_attr_field(
    attr_name: str,
    attr_class: str,
    is_key: bool,
    is_unique: bool,
    cardinality: Cardinality | None,
) -> str:
    """Render a single attribute field declaration."""
    py_name = to_python_name(attr_name)

    if is_key:
        return f"{py_name}: attributes.{attr_class} = Flag(Key)"

    if is_unique:
        return f"{py_name}: attributes.{attr_class} = Flag(Unique)"

    if cardinality is None or cardinality.is_optional_single:
        return f"{py_name}: attributes.{attr_class} | None = None"

    if cardinality.is_multi:
        if cardinality.max is None:
            return f"{py_name}: list[attributes.{attr_class}] = Flag(Card(min={cardinality.min}))"
        return f"{py_name}: list[attributes.{attr_class}] = Flag(Card({cardinality.min}, {cardinality.max}))"

    if cardinality.is_required and cardinality.is_single:
        return f"{py_name}: attributes.{attr_class}"

    return f"{py_name}: attributes.{attr_class} | None = None"


def _topological_sort_entities(schema: ParsedSchema) -> list[str]:
    """Sort entities so parents come before children."""
    result: list[str] = []
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visited or name not in schema.entities:
            return
        visited.add(name)

        entity = schema.entities[name]
        if entity.parent and entity.parent in schema.entities:
            visit(entity.parent)

        result.append(name)

    for name in schema.entities:
        visit(name)

    return result


def _needs_card_import(schema: ParsedSchema) -> bool:
    """Check if any entity uses multi-valued attributes requiring Card."""
    return any(
        card.is_multi
        for entity in schema.entities.values()
        for card in entity.cardinalities.values()
    )


def _build_entity_context(
    entity_name: str,
    schema: ParsedSchema,
    attr_class_names: dict[str, str],
    entity_class_names: dict[str, str],
    implicit_keys: set[str],
) -> EntityContext:
    """Build template context for a single entity."""
    entity = schema.entities[entity_name]
    cls_name = entity_class_names[entity_name]

    if entity.parent and entity.parent in entity_class_names:
        base_class = entity_class_names[entity.parent]
    else:
        base_class = "Entity"

    docstring = entity.docstring if entity.docstring else f"Entity generated from `{entity_name}`."

    flags_args = [f'name="{entity_name}"']
    if entity.abstract:
        flags_args.append("abstract=True")

    # Attributes - only render those not inherited from parent
    parent_owns = set()
    if entity.parent and entity.parent in schema.entities:
        parent_owns = schema.entities[entity.parent].owns

    own_attrs = [a for a in entity.owns_order if a not in parent_owns]
    key_attrs = (entity.keys | implicit_keys) & entity.owns
    unique_attrs = entity.uniques & entity.owns

    fields = []
    for attr in own_attrs:
        if attr not in attr_class_names:
            continue
        attr_class = attr_class_names[attr]
        cardinality = entity.cardinalities.get(attr)
        fields.append(
            _render_attr_field(
                attr_name=attr,
                attr_class=attr_class,
                is_key=attr in key_attrs,
                is_unique=attr in unique_attrs,
                cardinality=cardinality,
            )
        )

    # Collect coming-soon annotations for TODO comments
    cascade_attrs = sorted(entity.cascades) if entity.cascades else []
    # Group subkey attrs by their group name
    subkey_groups: dict[str, list[str]] = {}
    for attr, group in entity.subkeys.items():
        subkey_groups.setdefault(group, []).append(attr)
    for group in subkey_groups:
        subkey_groups[group] = sorted(subkey_groups[group])

    # Collect plays cardinalities for TODO comments
    plays_cardinalities: dict[str, str] = {}
    for role_ref, card in entity.plays_cardinalities.items():
        if card.max is None:
            plays_cardinalities[role_ref] = f"@card({card.min}..)"
        else:
            plays_cardinalities[role_ref] = f"@card({card.min}..{card.max})"

    return EntityContext(
        class_name=cls_name,
        base_class=base_class,
        docstring=docstring,
        flags_args=flags_args,
        prefix=entity.prefix,
        plays=sorted(entity.plays) if entity.plays else [],
        fields=fields,
        cascade_attrs=cascade_attrs,
        subkey_groups=subkey_groups,
        plays_cardinalities=plays_cardinalities,
    )


def render_entities(
    schema: ParsedSchema,
    attr_class_names: dict[str, str],
    entity_class_names: dict[str, str],
    implicit_key_attributes: set[str] | None = None,
) -> str:
    """Render the complete entities module source.

    Args:
        schema: Parsed schema containing entity definitions
        attr_class_names: Mapping from TypeDB attr names to Python class names
        entity_class_names: Mapping from TypeDB entity names to Python class names
        implicit_key_attributes: Attributes to treat as keys even without @key

    Returns:
        Complete Python source code for entities.py
    """
    logger.debug(f"Rendering {len(schema.entities)} entity classes")
    implicit_keys = implicit_key_attributes or set()
    needs_card = _needs_card_import(schema)

    imports = ["Entity", "Flag", "Key", "TypeFlags", "Unique"]
    if needs_card:
        imports.insert(1, "Card")

    entities = []
    all_names = []
    for entity_name in _topological_sort_entities(schema):
        all_names.append(entity_class_names[entity_name])
        entities.append(
            _build_entity_context(
                entity_name,
                schema,
                attr_class_names,
                entity_class_names,
                implicit_keys,
            )
        )

    template = get_template("entities.py.jinja")
    result = template.render(
        imports=imports,
        entities=entities,
        all_names=sorted(all_names),
    )

    logger.info(f"Rendered {len(all_names)} entity classes")
    return result
