"""Render relation class definitions from parsed schema."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from ..models import Cardinality, minimal_role_players
from ..naming import to_class_name, to_python_name
from .entities import _render_attr_field
from .template_loader import get_template

if TYPE_CHECKING:
    from ..models import AnnotationValue, ParsedSchema

logger = logging.getLogger(__name__)


@dataclass
class RelationContext:
    """Context for rendering a single relation class."""

    class_name: str
    base_class: str
    docstring: str
    flags_args: list[str]
    attr_fields: list[str] = field(default_factory=list)
    role_fields: list[str] = field(default_factory=list)
    # Coming-soon annotations (parsed but not yet available in TypeDB 3.x)
    # These will render as TODO comments in generated code
    cascade_attrs: list[str] = field(default_factory=list)
    subkey_groups: dict[str, list[str]] = field(default_factory=dict)
    distinct_roles: list[str] = field(default_factory=list)
    # Custom annotations from TQL comments (e.g., # @api(public))
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)


def _render_card_arg(cardinality: Cardinality | None) -> str:
    """Render Card argument for role cardinality, if needed."""
    if cardinality is None:
        return ""
    # Default single cardinality (1, 1) doesn't need explicit Card
    if cardinality.min == 1 and cardinality.max == 1:
        return ""
    if cardinality.max is None:
        return f", cardinality=Card({cardinality.min})"
    return f", cardinality=Card({cardinality.min}, {cardinality.max})"


def _render_role_field(
    role_name: str,
    player_classes: list[str],
    cardinality: Cardinality | None = None,
) -> str | None:
    """Render a single role field declaration."""
    if not player_classes:
        return None

    py_name = to_python_name(role_name)
    card_arg = _render_card_arg(cardinality)

    if len(player_classes) == 1:
        player = player_classes[0]
        return (
            f'{py_name}: Role[entities.{player}] = Role("{role_name}", entities.{player}{card_arg})'
        )

    primary, *rest = player_classes
    extras = ", ".join(f"entities.{p}" for p in rest)
    union_type = " | ".join(f"entities.{p}" for p in player_classes)

    return (
        f"{py_name}: Role[{union_type}] = "
        f'_multi(Role.multi("{role_name}", entities.{primary}, {extras}{card_arg}))'
    )


def _topological_sort_relations(schema: ParsedSchema) -> list[str]:
    """Sort relations so parents come before children."""
    result: list[str] = []
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visited or name not in schema.relations:
            return
        visited.add(name)

        relation = schema.relations[name]
        if relation.parent and relation.parent in schema.relations:
            visit(relation.parent)

        result.append(name)

    for name in schema.relations:
        visit(name)

    return result


def _build_relation_context(
    relation_name: str,
    schema: ParsedSchema,
    attr_class_names: dict[str, str],
    entity_class_names: dict[str, str],
    relation_class_names: dict[str, str],
) -> RelationContext:
    """Build template context for a single relation."""
    relation = schema.relations[relation_name]
    cls_name = relation_class_names[relation_name]

    if relation.parent and relation.parent in relation_class_names:
        base_class = relation_class_names[relation.parent]
    else:
        base_class = "Relation"

    docstring = (
        relation.docstring if relation.docstring else f"Relation generated from `{relation_name}`."
    )

    flags_args = [f'name="{relation_name}"']
    if relation.abstract:
        flags_args.append("abstract=True")

    # Attributes - only render those not inherited
    parent_owns = set()
    if relation.parent and relation.parent in schema.relations:
        parent_owns = schema.relations[relation.parent].owns

    attr_fields = []
    own_attrs = [a for a in relation.owns_order if a not in parent_owns]
    key_attrs = relation.keys & relation.owns
    unique_attrs = relation.uniques & relation.owns

    for attr in own_attrs:
        if attr not in attr_class_names:
            continue
        attr_class = attr_class_names[attr]
        cardinality = relation.cardinalities.get(attr)
        attr_fields.append(
            _render_attr_field(
                attr_name=attr,
                attr_class=attr_class,
                is_key=attr in key_attrs,
                is_unique=attr in unique_attrs,
                cardinality=cardinality,
            )
        )

    # Roles - only render those not inherited
    parent_roles = set()
    if relation.parent and relation.parent in schema.relations:
        parent_roles = {r.name for r in schema.relations[relation.parent].roles}

    role_fields = []
    for role in relation.roles:
        if role.name in parent_roles and not role.overrides:
            continue

        players = minimal_role_players(schema, relation_name, role.name)
        player_classes = [entity_class_names[p] for p in players if p in entity_class_names]

        role_line = _render_role_field(role.name, player_classes, role.cardinality)
        if role_line:
            role_fields.append(role_line)

    # Collect coming-soon annotations for TODO comments
    cascade_attrs = sorted(relation.cascades) if relation.cascades else []
    # Group subkey attrs by their group name
    subkey_groups: dict[str, list[str]] = {}
    for attr, group in relation.subkeys.items():
        subkey_groups.setdefault(group, []).append(attr)
    for group in subkey_groups:
        subkey_groups[group] = sorted(subkey_groups[group])
    # Collect roles with @distinct annotation
    distinct_roles = [r.name for r in relation.roles if r.distinct]

    # Collect custom annotations (excluding internal ones like _docstring)
    annotations = {k: v for k, v in relation.annotations.items() if not k.startswith("_")}

    return RelationContext(
        class_name=cls_name,
        base_class=base_class,
        docstring=docstring,
        flags_args=flags_args,
        attr_fields=attr_fields,
        role_fields=role_fields,
        cascade_attrs=cascade_attrs,
        subkey_groups=subkey_groups,
        distinct_roles=sorted(distinct_roles),
        annotations=annotations,
    )


def _needs_card_for_attributes(schema: ParsedSchema) -> bool:
    """Check if any relation uses multi-valued attributes requiring Card."""
    return any(
        card.is_multi
        for relation in schema.relations.values()
        for card in relation.cardinalities.values()
    )


def _needs_card_for_roles(schema: ParsedSchema) -> bool:
    """Check if any role has non-default cardinality requiring Card."""
    for relation in schema.relations.values():
        for role in relation.roles:
            if role.cardinality is not None:
                # Default (1, 1) doesn't need Card import
                if not (role.cardinality.min == 1 and role.cardinality.max == 1):
                    return True
    return False


def _needs_card_import(schema: ParsedSchema) -> bool:
    """Check if any relation uses multi-valued attributes or role cardinalities requiring Card."""
    return _needs_card_for_attributes(schema) or _needs_card_for_roles(schema)


def _needs_key_import(schema: ParsedSchema) -> bool:
    """Check if any relation has @key attributes."""
    return any(relation.keys for relation in schema.relations.values())


def _needs_unique_import(schema: ParsedSchema) -> bool:
    """Check if any relation has @unique attributes."""
    return any(relation.uniques for relation in schema.relations.values())


def render_relations(
    schema: ParsedSchema,
    attr_class_names: dict[str, str],
    entity_class_names: dict[str, str],
    relation_class_names: dict[str, str] | None = None,
) -> str:
    """Render the complete relations module source.

    Args:
        schema: Parsed schema containing relation definitions
        attr_class_names: Mapping from TypeDB attr names to Python class names
        entity_class_names: Mapping from TypeDB entity names to Python class names
        relation_class_names: Mapping from TypeDB relation names to Python class names

    Returns:
        Complete Python source code for relations.py
    """
    logger.debug(f"Rendering {len(schema.relations)} relation classes")
    if relation_class_names is None:
        relation_class_names = {name: to_class_name(name) for name in schema.relations}

    # Build imports list
    imports = ["Relation", "Role", "TypeFlags"]
    # Flag is needed for @key, @unique, or multi-value attributes (Card used with Flag)
    needs_flag = (
        _needs_key_import(schema)
        or _needs_unique_import(schema)
        or _needs_card_for_attributes(schema)
    )
    if _needs_card_import(schema):
        imports.insert(0, "Card")
    if needs_flag:
        imports.insert(0, "Flag")
    if _needs_key_import(schema):
        imports.append("Key")
    if _needs_unique_import(schema):
        imports.append("Unique")

    relations = []
    all_names = []
    for relation_name in _topological_sort_relations(schema):
        all_names.append(relation_class_names[relation_name])
        relations.append(
            _build_relation_context(
                relation_name,
                schema,
                attr_class_names,
                entity_class_names,
                relation_class_names,
            )
        )

    template = get_template("relations.py.jinja")
    result = template.render(
        imports=imports,
        relations=relations,
        all_names=sorted(all_names),
    )

    logger.info(f"Rendered {len(all_names)} relation classes")
    return result
