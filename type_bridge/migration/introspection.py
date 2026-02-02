"""TypeDB schema introspection for migration auto-generation.

This module provides functionality to introspect a TypeDB database schema
and convert it to a format comparable with Python model definitions.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from type_bridge.migration.utils import type_exists

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


@dataclass
class IntrospectedAttribute:
    """An attribute type from the database schema."""

    name: str
    value_type: str  # string, integer, double, boolean, datetime, etc.


@dataclass
class IntrospectedOwnership:
    """An ownership relationship between a type and an attribute."""

    owner_name: str
    attribute_name: str
    annotations: list[str] = field(default_factory=list)  # @key, @unique, @card


@dataclass
class IntrospectedRole:
    """A role in a relation."""

    name: str
    player_types: list[str] = field(default_factory=list)


@dataclass
class IntrospectedRelation:
    """A relation type from the database schema."""

    name: str
    roles: dict[str, IntrospectedRole] = field(default_factory=dict)


@dataclass
class IntrospectedEntity:
    """An entity type from the database schema."""

    name: str
    supertype: str | None = None


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
        from type_bridge.models import Entity, Relation

        schema = IntrospectedSchema()

        if not self.db.database_exists():
            logger.debug("Database does not exist, returning empty schema")
            return schema

        logger.info(f"Introspecting database schema for {len(models)} model types")

        # Collect unique attribute types from all models
        attr_types: set[type] = set()
        for model in models:
            if hasattr(model, "get_owned_attributes"):
                for attr_info in model.get_owned_attributes().values():
                    attr_types.add(attr_info.typ)

        # Check each attribute type
        for attr_type in attr_types:
            attr_name = attr_type.get_attribute_name()
            if type_exists(self.db, attr_name):
                schema.attributes[attr_name] = IntrospectedAttribute(
                    name=attr_name,
                    value_type=getattr(attr_type, "_value_type", "string"),
                )
                logger.debug(f"Found existing attribute: {attr_name}")

        # Check each model type
        for model in models:
            type_name = model.get_type_name()

            if issubclass(model, Entity) and model is not Entity:
                if type_exists(self.db, type_name):
                    schema.entities[type_name] = IntrospectedEntity(name=type_name)
                    logger.debug(f"Found existing entity: {type_name}")

                    # Check ownerships (pass model for fallback)
                    self._introspect_ownerships_for_type(schema, type_name, model)

            elif issubclass(model, Relation) and model is not Relation:
                if type_exists(self.db, type_name):
                    schema.relations[type_name] = IntrospectedRelation(name=type_name)
                    logger.debug(f"Found existing relation: {type_name}")

                    # Check roles and role players
                    self._introspect_roles_for_relation(schema, type_name, model)

        logger.info(
            f"Introspected: {len(schema.entities)} entities, "
            f"{len(schema.relations)} relations, "
            f"{len(schema.attributes)} attributes"
        )

        return schema

    def _introspect_ownerships_for_type(
        self,
        schema: IntrospectedSchema,
        type_name: str,
        model: type[Entity] | type[Relation] | None = None,
    ) -> None:
        """Introspect ownerships for a specific type.

        Uses schema query to check ownership definitions, not instances.
        Falls back to model definition if schema query fails.
        """
        # Try to query ownership from schema definition
        query = f"""
        match {type_name} owns $a;
        fetch {{ "attribute": $a }};
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    attr_info = result.get("attribute", {})
                    if isinstance(attr_info, dict):
                        attr_name = attr_info.get("label")
                    else:
                        attr_name = str(attr_info) if attr_info else None

                    if attr_name:
                        schema.ownerships.append(
                            IntrospectedOwnership(
                                owner_name=type_name,
                                attribute_name=attr_name,
                            )
                        )
                        logger.debug(f"Found ownership from schema: {type_name} owns {attr_name}")
        except Exception as e:
            logger.debug(f"Schema query failed for {type_name} ownerships: {e}")
            # Fall back to model definition if provided
            if model and hasattr(model, "get_owned_attributes"):
                for attr_name, attr_info in model.get_owned_attributes().items():
                    attr_type_name = attr_info.typ.get_attribute_name()
                    if type_exists(self.db, attr_type_name):
                        schema.ownerships.append(
                            IntrospectedOwnership(
                                owner_name=type_name,
                                attribute_name=attr_type_name,
                            )
                        )
                        logger.debug(
                            f"Found ownership from model: {type_name} owns {attr_type_name}"
                        )

    def _introspect_roles_for_relation(
        self, schema: IntrospectedSchema, rel_name: str, model: type[Relation]
    ) -> None:
        """Introspect roles for a relation using model definition.

        Since we can't easily query roles from TypeDB 3.x schema,
        we use the model's role definitions as the source of truth.
        """
        if hasattr(model, "_roles"):
            for role_name, role in model._roles.items():
                schema.relations[rel_name].roles[role_name] = IntrospectedRole(
                    name=role_name,
                    player_types=list(role.player_types),
                )
                logger.debug(f"Found role from model: {rel_name}:{role_name}")

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

        # Query all schema information
        self._introspect_entities(schema)
        self._introspect_relations(schema)
        self._introspect_attributes(schema)
        self._introspect_ownerships(schema)
        self._introspect_role_players(schema)

        logger.info(
            f"Introspected: {len(schema.entities)} entities, "
            f"{len(schema.relations)} relations, "
            f"{len(schema.attributes)} attributes"
        )

        return schema

    def _introspect_entities(self, schema: IntrospectedSchema) -> None:
        """Query all entity types."""
        query = """
            match $t sub entity;
            fetch { "type": $t };
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    type_info = result.get("type", {})
                    if isinstance(type_info, dict):
                        type_name = type_info.get("label")
                    else:
                        type_name = str(type_info) if type_info else None

                    if type_name and type_name != "entity":
                        schema.entities[type_name] = IntrospectedEntity(name=type_name)
                        logger.debug(f"Found entity: {type_name}")
        except Exception as e:
            logger.warning(f"Failed to introspect entities: {e}")

    def _introspect_relations(self, schema: IntrospectedSchema) -> None:
        """Query all relation types with their roles."""
        # First get relation types
        query = """
            match $r sub relation;
            fetch { "relation": $r };
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    rel_info = result.get("relation", {})
                    if isinstance(rel_info, dict):
                        rel_name = rel_info.get("label")
                    else:
                        rel_name = str(rel_info) if rel_info else None

                    if rel_name and rel_name != "relation":
                        schema.relations[rel_name] = IntrospectedRelation(name=rel_name)
                        logger.debug(f"Found relation: {rel_name}")
        except Exception as e:
            logger.warning(f"Failed to introspect relations: {e}")

        # Then get roles for each relation
        for rel_name in list(schema.relations.keys()):
            self._introspect_relation_roles(schema, rel_name)

    def _introspect_relation_roles(self, schema: IntrospectedSchema, rel_name: str) -> None:
        """Query roles for a specific relation."""
        query = f"""
            match $r type {rel_name}; $r relates $role;
            fetch {{ "role": $role }};
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    role_info = result.get("role", {})
                    if isinstance(role_info, dict):
                        role_name = role_info.get("label")
                    else:
                        role_name = str(role_info) if role_info else None

                    if role_name:
                        # Extract just the role name (after colon if present)
                        if ":" in role_name:
                            role_name = role_name.split(":")[-1]
                        schema.relations[rel_name].roles[role_name] = IntrospectedRole(
                            name=role_name
                        )
                        logger.debug(f"Found role: {rel_name}:{role_name}")
        except Exception as e:
            logger.warning(f"Failed to introspect roles for {rel_name}: {e}")

    def _introspect_attributes(self, schema: IntrospectedSchema) -> None:
        """Query all attribute types with value types."""
        query = """
            match $a sub attribute;
            fetch { "attribute": $a };
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    attr_info = result.get("attribute", {})
                    if isinstance(attr_info, dict):
                        attr_name = attr_info.get("label")
                        value_type = attr_info.get("value_type", "string")
                    else:
                        attr_name = str(attr_info) if attr_info else None
                        value_type = "string"

                    if attr_name and attr_name != "attribute":
                        schema.attributes[attr_name] = IntrospectedAttribute(
                            name=attr_name, value_type=str(value_type)
                        )
                        logger.debug(f"Found attribute: {attr_name} ({value_type})")
        except Exception as e:
            logger.warning(f"Failed to introspect attributes: {e}")

    def _introspect_ownerships(self, schema: IntrospectedSchema) -> None:
        """Query all ownership relationships."""
        query = """
            match $t owns $a;
            fetch { "owner": $t, "attribute": $a };
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    owner_info = result.get("owner", {})
                    attr_info = result.get("attribute", {})

                    if isinstance(owner_info, dict):
                        owner_name = owner_info.get("label")
                    else:
                        owner_name = str(owner_info) if owner_info else None

                    if isinstance(attr_info, dict):
                        attr_name = attr_info.get("label")
                    else:
                        attr_name = str(attr_info) if attr_info else None

                    if owner_name and attr_name:
                        # Skip built-in types
                        if owner_name in ("entity", "relation", "attribute"):
                            continue
                        if attr_name == "attribute":
                            continue

                        schema.ownerships.append(
                            IntrospectedOwnership(
                                owner_name=owner_name,
                                attribute_name=attr_name,
                            )
                        )
                        logger.debug(f"Found ownership: {owner_name} owns {attr_name}")
        except Exception as e:
            logger.warning(f"Failed to introspect ownerships: {e}")

    def _introspect_role_players(self, schema: IntrospectedSchema) -> None:
        """Query role player types for each relation."""
        query = """
            match $t plays $rel:$role;
            fetch { "player": $t, "relation": $rel, "role": $role };
        """

        try:
            with self.db.transaction("read") as tx:
                results = list(tx.execute(query))
                for result in results:
                    player_info = result.get("player", {})
                    rel_info = result.get("relation", {})
                    role_info = result.get("role", {})

                    if isinstance(player_info, dict):
                        player_name = player_info.get("label")
                    else:
                        player_name = str(player_info) if player_info else None

                    if isinstance(rel_info, dict):
                        rel_name = rel_info.get("label")
                    else:
                        rel_name = str(rel_info) if rel_info else None

                    if isinstance(role_info, dict):
                        role_name = role_info.get("label")
                    else:
                        role_name = str(role_info) if role_info else None

                    if player_name and rel_name and role_name:
                        # Extract just the role name
                        if ":" in role_name:
                            role_name = role_name.split(":")[-1]

                        # Skip built-in types
                        if rel_name == "relation":
                            continue

                        if rel_name in schema.relations:
                            if role_name in schema.relations[rel_name].roles:
                                schema.relations[rel_name].roles[role_name].player_types.append(
                                    player_name
                                )
                                logger.debug(
                                    f"Found role player: {player_name} plays {rel_name}:{role_name}"
                                )
        except Exception as e:
            logger.warning(f"Failed to introspect role players: {e}")


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
