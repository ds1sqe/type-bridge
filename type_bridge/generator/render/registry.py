"""Render registry.py with pre-computed schema metadata.

The registry provides static, type-safe access to schema metadata without
runtime introspection. It includes:

- Type name collections (tuples and StrEnums)
- Type-to-class mappings
- Relation role metadata
- Entity attribute ownership
- Attribute value types
- Custom annotations from TQL comments
- Convenience lookup functions
- JSON schema fragments for LLM tools
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from typing import TYPE_CHECKING

from ..models import minimal_role_players
from .template_loader import get_template

if TYPE_CHECKING:
    from ..models import ParsedSchema


@dataclass
class RoleContext:
    """Context for rendering a single role."""

    name: str
    players: list[str]


def render_registry(
    schema: ParsedSchema,
    attr_class_names: dict[str, str],
    entity_class_names: dict[str, str],
    relation_class_names: dict[str, str],
    *,
    schema_version: str = "1.0.0",
    schema_text: str | None = None,
) -> str:
    """Render the registry.py module source.

    Args:
        schema: Parsed schema with all type information
        attr_class_names: Mapping from TypeDB attr names to Python class names
        entity_class_names: Mapping from TypeDB entity names to Python class names
        relation_class_names: Mapping from TypeDB relation names to Python class names
        schema_version: Version string for SCHEMA_VERSION constant
        schema_text: Original schema text for hash computation

    Returns:
        Complete Python source code for registry.py
    """
    entity_names = sorted(schema.entities.keys())
    relation_names = sorted(schema.relations.keys())
    attribute_names = sorted(schema.attributes.keys())

    # Compute schema hash
    if schema_text:
        schema_hash = f"sha256:{hashlib.sha256(schema_text.encode('utf-8')).hexdigest()[:16]}"
    else:
        schema_hash = ""

    # Build type-to-class mappings
    entity_map = {name: entity_class_names[name] for name in entity_names}
    relation_map = {name: relation_class_names[name] for name in relation_names}
    attribute_map = {name: attr_class_names[name] for name in attribute_names}

    # Build relation roles
    relation_roles = {}
    for rel_name in relation_names:
        relation = schema.relations[rel_name]
        if not relation.roles:
            continue
        roles = []
        for role in relation.roles:
            players = list(minimal_role_players(schema, rel_name, role.name))
            roles.append(RoleContext(name=role.name, players=players))
        relation_roles[rel_name] = roles

    # Build relation attributes
    relation_attributes = {}
    for name in relation_names:
        relation = schema.relations[name]
        relation_attributes[name] = sorted(relation.owns)

    # Build entity attributes
    entity_attributes = {}
    for name in entity_names:
        entity = schema.entities[name]
        entity_attributes[name] = sorted(entity.owns)

    # Build entity keys
    entity_keys = {}
    for name in entity_names:
        entity = schema.entities[name]
        if entity.keys:
            entity_keys[name] = sorted(entity.keys)

    # Build attribute value types
    attribute_value_types = {}
    for name in attribute_names:
        attr = schema.attributes[name]
        if attr.value_type:
            attribute_value_types[name] = attr.value_type

    # Build inheritance metadata
    entity_parents = {name: schema.entities[name].parent for name in entity_names}
    relation_parents = {name: schema.relations[name].parent for name in relation_names}

    entity_abstract = [n for n in entity_names if schema.entities[n].abstract]
    relation_abstract = [n for n in relation_names if schema.relations[n].abstract]

    # Collect annotations
    entity_annotations = {
        name: dict(spec.annotations) for name, spec in schema.entities.items() if spec.annotations
    }
    attribute_annotations = {
        name: dict(spec.annotations) for name, spec in schema.attributes.items() if spec.annotations
    }
    relation_annotations = {
        name: dict(spec.annotations) for name, spec in schema.relations.items() if spec.annotations
    }

    template = get_template("registry.py.jinja")
    return template.render(
        schema_version=schema_version,
        schema_hash=schema_hash,
        entity_names=entity_names,
        relation_names=relation_names,
        attribute_names=attribute_names,
        entity_map=entity_map,
        relation_map=relation_map,
        attribute_map=attribute_map,
        relation_roles=relation_roles,
        relation_attributes=relation_attributes,
        entity_attributes=entity_attributes,
        entity_keys=entity_keys,
        attribute_value_types=attribute_value_types,
        entity_parents=entity_parents,
        relation_parents=relation_parents,
        entity_abstract=entity_abstract,
        relation_abstract=relation_abstract,
        entity_annotations=entity_annotations,
        attribute_annotations=attribute_annotations,
        relation_annotations=relation_annotations,
    )
