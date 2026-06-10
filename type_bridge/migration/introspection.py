"""TypeDB schema introspection for migration auto-generation.

This module provides functionality to introspect a TypeDB database schema
and convert it to a format comparable with Python model definitions.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


@dataclass
class IntrospectedAttribute:
    """An attribute type from the database schema."""

    name: str
    value_type: str  # string, integer, double, boolean, datetime, etc.
    parent_type: str | None = None
    is_abstract: bool = False
    is_independent: bool = False
    regex: str | None = None
    allowed_values: list[str] | None = None
    range: list[str | None] | tuple[str | None, str | None] | None = None


@dataclass
class IntrospectedOwnership:
    """An ownership relationship between a type and an attribute."""

    owner_name: str
    attribute_name: str
    # Live-introspected schemas carry annotation DTOs serialized by the Rust engine;
    # older/hand-built paths may still deliver legacy strings (@key, @unique, @card).
    # Both forms are accepted so downstream comparison is form-agnostic.
    annotations: list[object] = field(default_factory=list)


@dataclass
class IntrospectedRole:
    """A role in a relation."""

    name: str
    player_types: list[str] = field(default_factory=list)
    cardinality: object | None = None


@dataclass
class IntrospectedRelation:
    """A relation type from the database schema."""

    name: str
    roles: dict[str, IntrospectedRole] = field(default_factory=dict)
    supertype: str | None = None
    is_abstract: bool = False


@dataclass
class IntrospectedEntity:
    """An entity type from the database schema."""

    name: str
    supertype: str | None = None
    is_abstract: bool = False


@dataclass
class IntrospectedSchema:
    """Complete introspected schema from TypeDB database.

    This is a database-centric view of the schema that can be compared
    against Python model definitions.
    """

    entities: dict[str, IntrospectedEntity] = field(default_factory=dict)
    relations: dict[str, IntrospectedRelation] = field(default_factory=dict)
    attributes: dict[str, IntrospectedAttribute] = field(default_factory=dict)
    ownerships: list[IntrospectedOwnership] = field(default_factory=list)

    def is_empty(self) -> bool:
        """Check if the schema is empty (no custom types)."""
        # Filter out built-in types
        custom_entities = {k: v for k, v in self.entities.items() if k not in ("entity",)}
        custom_relations = {k: v for k, v in self.relations.items() if k not in ("relation",)}
        custom_attrs = {k: v for k, v in self.attributes.items() if k not in ("attribute",)}

        return not (custom_entities or custom_relations or custom_attrs)

    def get_entity_names(self) -> set[str]:
        """Get names of all custom entity types."""
        return {k for k in self.entities.keys() if k != "entity"}

    def get_relation_names(self) -> set[str]:
        """Get names of all custom relation types."""
        return {k for k in self.relations.keys() if k != "relation"}

    def get_attribute_names(self) -> set[str]:
        """Get names of all custom attribute types."""
        return {k for k in self.attributes.keys() if k != "attribute"}

    def get_ownerships_for(self, owner_name: str) -> list[IntrospectedOwnership]:
        """Get all ownerships for a specific owner type."""
        return [o for o in self.ownerships if o.owner_name == owner_name]

    @classmethod
    def from_rust_schema_info(cls, info: dict) -> IntrospectedSchema:
        """Build the compatibility DTO from Rust ``SchemaInfo`` live introspection."""
        schema = cls()

        for attr_name, attr in info.get("attributes", {}).items():
            schema.attributes[attr_name] = IntrospectedAttribute(
                name=attr_name,
                value_type=attr.get("value_type", "string"),
                parent_type=attr.get("parent_type"),
                is_abstract=bool(attr.get("is_abstract", False)),
                is_independent=bool(attr.get("is_independent", False)),
                regex=attr.get("regex"),
                allowed_values=list(attr["allowed_values"])
                if attr.get("allowed_values") is not None
                else None,
                range=attr.get("range"),
            )

        for entity_name, entity in info.get("entities", {}).items():
            schema.entities[entity_name] = IntrospectedEntity(
                name=entity_name,
                supertype=entity.get("parent_type"),
                is_abstract=bool(entity.get("is_abstract", False)),
            )
            schema._add_ownerships_from_rust_entry(entity_name, entity)

        for relation_name, relation in info.get("relations", {}).items():
            schema.relations[relation_name] = IntrospectedRelation(
                name=relation_name,
                supertype=relation.get("parent_type"),
                is_abstract=bool(relation.get("is_abstract", False)),
            )
            for role in relation.get("roles", []):
                role_name = role["role_name"]
                schema.relations[relation_name].roles[role_name] = IntrospectedRole(
                    name=role_name,
                    player_types=list(role.get("player_type_names", [])),
                    cardinality=role.get("cardinality"),
                )
            schema._add_ownerships_from_rust_entry(relation_name, relation)

        return schema

    def _add_ownerships_from_rust_entry(self, owner_name: str, entry: dict) -> None:
        for attr in entry.get("owned_attributes", []):
            self.ownerships.append(
                IntrospectedOwnership(
                    owner_name=owner_name,
                    attribute_name=attr["attr_name"],
                    annotations=list(attr.get("annotations", [])),
                )
            )

    def to_rust_schema_info(self) -> dict:
        """Serialize introspected database schema to the Rust ``SchemaInfo`` dict shape."""
        info: dict = {"entities": {}, "relations": {}, "attributes": {}}

        for attr_name, attr in self.attributes.items():
            if attr_name == "attribute":
                continue
            info["attributes"][attr_name] = {
                "attr_name": attr_name,
                "value_type": _rust_value_type(attr.value_type),
            }
            _add_attribute_metadata(info["attributes"][attr_name], attr)

        for entity_name, entity in self.entities.items():
            if entity_name == "entity":
                continue
            info["entities"][entity_name] = {
                "type_name": entity_name,
                "is_abstract": entity.is_abstract,
                "parent_type": entity.supertype,
                "owned_attributes": self._owned_attributes_for(entity_name, info),
            }

        for relation_name, relation in self.relations.items():
            if relation_name == "relation":
                continue
            info["relations"][relation_name] = {
                "type_name": relation_name,
                "is_abstract": relation.is_abstract,
                "parent_type": relation.supertype,
                "owned_attributes": self._owned_attributes_for(relation_name, info),
                "roles": [
                    {
                        "role_name": role.name,
                        "player_type_names": [
                            _player_type_name(player) for player in role.player_types
                        ],
                        "cardinality": role.cardinality,
                    }
                    for role in relation.roles.values()
                ],
            }

        return info

    def _owned_attributes_for(self, owner_name: str, info: dict) -> list[dict]:
        attrs = []
        for ownership in self.get_ownerships_for(owner_name):
            attr = self.attributes.get(ownership.attribute_name)
            value_type = _rust_value_type(attr.value_type) if attr is not None else "string"
            info["attributes"].setdefault(
                ownership.attribute_name,
                {"attr_name": ownership.attribute_name, "value_type": value_type},
            )
            if attr is not None:
                _add_attribute_metadata(info["attributes"][ownership.attribute_name], attr)
            attrs.append(
                {
                    "attr_name": ownership.attribute_name,
                    "value_type": value_type,
                    "annotations": [_rust_annotation(ann) for ann in ownership.annotations],
                }
            )
        return attrs


def _rust_value_type(value_type: str) -> str:
    return {
        "integer": "long",
        "long": "long",
        "string": "string",
        "double": "double",
        "boolean": "boolean",
        "date": "date",
        "datetime": "datetime",
        "datetime-tz": "datetime-tz",
        "decimal": "decimal",
        "duration": "duration",
    }.get(value_type, "string")


def _add_attribute_metadata(entry: dict, attr: IntrospectedAttribute) -> None:
    if attr.parent_type is not None:
        entry["parent_type"] = attr.parent_type
    if attr.is_abstract:
        entry["is_abstract"] = True
    if attr.is_independent:
        entry["is_independent"] = True
    if attr.regex is not None:
        entry["regex"] = attr.regex
    if attr.allowed_values is not None:
        entry["allowed_values"] = list(attr.allowed_values)
    if attr.range is not None:
        bounds = list(attr.range)
        if len(bounds) == 2:
            entry["range"] = [bounds[0], bounds[1]]


def _player_type_name(player: object) -> str:
    getter = getattr(player, "get_type_name", None)
    if callable(getter):
        return str(getter())
    return str(player)


def _rust_annotation(annotation: object) -> object:
    if annotation in ("@key", "key", "Key"):
        return "Key"
    if annotation in ("@unique", "unique", "Unique"):
        return "Unique"
    return annotation


def _filter_schema_for_models(
    schema: IntrospectedSchema,
    models: list[type[Entity] | type[Relation]],
    entity_base: type[Entity],
    relation_base: type[Relation],
) -> IntrospectedSchema:
    filtered = IntrospectedSchema()
    owner_names: set[str] = set()
    attribute_names: set[str] = set()

    for model in models:
        type_name = model.get_type_name()
        if issubclass(model, entity_base) and model is not entity_base:
            if type_name in schema.entities:
                filtered.entities[type_name] = schema.entities[type_name]
                owner_names.add(type_name)
        elif issubclass(model, relation_base) and model is not relation_base:
            if type_name in schema.relations:
                filtered.relations[type_name] = schema.relations[type_name]
                owner_names.add(type_name)

        if hasattr(model, "get_all_attributes"):
            for attr_info in model.get_all_attributes().values():
                attribute_names.add(attr_info.typ.get_attribute_name())

    filtered.ownerships = [
        ownership for ownership in schema.ownerships if ownership.owner_name in owner_names
    ]
    attribute_names.update(ownership.attribute_name for ownership in filtered.ownerships)
    filtered.attributes = {
        attr_name: attr
        for attr_name, attr in schema.attributes.items()
        if attr_name in attribute_names
    }
    return filtered


class SchemaIntrospector:
    """Introspects TypeDB database schema.

    Queries the database to discover all types, attributes, ownerships,
    and relations defined in the schema.

    Example:
        introspector = SchemaIntrospector(db)
        schema = introspector.introspect()

        print(f"Found {len(schema.entities)} entities")
        print(f"Found {len(schema.relations)} relations")
        print(f"Found {len(schema.attributes)} attributes")
    """

    def __init__(self, db: Database):
        """Initialize introspector.

        Args:
            db: Database connection
        """
        self.db = db

    def introspect_for_models(
        self, models: list[type[Entity] | type[Relation]]
    ) -> IntrospectedSchema:
        """Introspect database schema for specific model types.

        This is the TypeDB 3.x compatible approach that checks each
        model type individually instead of enumerating all types.

        Args:
            models: List of model classes to check

        Returns:
            IntrospectedSchema with info about existing types
        """
        from type_bridge._rust_runtime import introspect_schema
        from type_bridge.models import Entity, Relation

        schema = IntrospectedSchema()

        if not self.db.database_exists():
            logger.debug("Database does not exist, returning empty schema")
            return schema

        logger.info(f"Introspecting database schema for {len(models)} model types")

        live_schema = IntrospectedSchema.from_rust_schema_info(introspect_schema(self.db))
        schema = _filter_schema_for_models(live_schema, models, Entity, Relation)

        logger.info(
            f"Introspected: {len(schema.entities)} entities, "
            f"{len(schema.relations)} relations, "
            f"{len(schema.attributes)} attributes"
        )

        return schema

    def introspect(self) -> IntrospectedSchema:
        """Query TypeDB schema and return structured info.

        Returns:
            IntrospectedSchema with all discovered types
        """
        schema = IntrospectedSchema()

        if not self.db.database_exists():
            logger.debug("Database does not exist, returning empty schema")
            return schema

        logger.info("Introspecting database schema")

        from type_bridge._rust_runtime import introspect_schema

        schema = IntrospectedSchema.from_rust_schema_info(introspect_schema(self.db))

        logger.info(
            f"Introspected: {len(schema.entities)} entities, "
            f"{len(schema.relations)} relations, "
            f"{len(schema.attributes)} attributes"
        )

        return schema


def compare_schemas(
    db_schema: IntrospectedSchema, model_names: dict[str, str]
) -> dict[str, list[str]]:
    """Compare database schema against model type names.

    Args:
        db_schema: Introspected database schema
        model_names: Dict mapping model class names to TypeDB type names

    Returns:
        Dict with 'added', 'removed', 'unchanged' lists
    """
    db_types = (
        db_schema.get_entity_names()
        | db_schema.get_relation_names()
        | db_schema.get_attribute_names()
    )

    model_types = set(model_names.values())

    return {
        "added": list(model_types - db_types),
        "removed": list(db_types - model_types),
        "unchanged": list(db_types & model_types),
    }
