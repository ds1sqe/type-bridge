"""Render api_dto.py with strictly-typed API Data Transfer Objects."""

from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

from .template_loader import get_template
from ..naming import to_class_name

if TYPE_CHECKING:
    from ..models import ParsedSchema

TYPE_MAP = {
    "string": "str",
    "integer": "int",
    "double": "float",
    "boolean": "bool",
    "datetime": "datetime",
}

@dataclass
class AttrContext:
    name: str
    type: str
    default: Any

@dataclass
class EntityContext:
    name: str
    class_name: str
    class_prefix: str
    parent_base: str
    abstract: bool
    internal: bool
    own_attributes: list[AttrContext]

@dataclass
class RelationContext:
    name: str
    class_name: str
    class_prefix: str
    own_attributes: list[AttrContext]

def render_api_dto(schema: "ParsedSchema") -> str:
    entities = []
    for name, spec in schema.entities.items():
        # Determine parent base for DTO inheritance
        # If it's artifact or a subtype of artifact, use BaseArtifact
        parent_base = "BaseDTO"
        curr = name
        while curr:
            if curr == "artifact":
                parent_base = "BaseArtifact"
                break
            curr = schema.entities[curr].parent

        own_attrs = []
        for attr_name in sorted(spec.owns):
            # Skip attributes already in BaseArtifact if we are inheriting from it
            if parent_base == "BaseArtifact" and attr_name in {
                "display_id", "name", "description", "status", 
                "priority", "created_at", "updated_at"
            }:
                continue
            
            attr_spec = schema.attributes[attr_name]
            default_val = attr_spec.annotations.get("default", "None")
            if isinstance(default_val, str) and default_val != "None":
                default_val = f'"{default_val}"'
            
            own_attrs.append(AttrContext(
                name=attr_name,
                type=TYPE_MAP.get(attr_spec.value_type, "Any"),
                default=default_val
            ))

        internal = spec.annotations.get("internal", False)

        entities.append(EntityContext(
            name=name,
            class_name=to_class_name(name),
            class_prefix=to_class_name(name),
            parent_base=parent_base,
            abstract=spec.abstract,
            internal=internal,
            own_attributes=own_attrs
        ))

    relations = []
    for name, spec in schema.relations.items():
        own_attrs = []
        for attr_name in sorted(spec.owns):
            attr_spec = schema.attributes[attr_name]
            default_val = attr_spec.annotations.get("default", "None")
            if isinstance(default_val, str) and default_val != "None":
                default_val = f'"{default_val}"'
                
            own_attrs.append(AttrContext(
                name=attr_name,
                type=TYPE_MAP.get(attr_spec.value_type, "Any"),
                default=default_val
            ))
        
        relations.append(RelationContext(
            name=name,
            class_name=to_class_name(name),
            class_prefix=to_class_name(name),
            own_attributes=own_attrs
        ))

    template = get_template("api_dto.py.jinja")
    return template.render(
        entities=entities,
        relations=relations
    )
