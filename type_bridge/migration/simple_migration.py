"""Migration manager for TypeDB schema migrations."""

import logging

from type_bridge.session import Database

logger = logging.getLogger(__name__)


class MigrationManager:
    """Manager for schema migrations."""

    def __init__(self, db: Database):
        """Initialize migration manager.

        Args:
            db: Database connection
        """
        self.db = db
        self.migrations: list[tuple[str, str]] = []

    def add_migration(self, name: str, schema: str) -> None:
        """Add a migration.

        Args:
            name: Migration name
            schema: TypeQL schema definition
        """
        logger.debug(f"Adding migration: {name} ({len(schema)} chars)")
        self.migrations.append((name, schema))

    def apply_migrations(self) -> None:
        """Apply all pending migrations."""
        logger.info(f"Applying {len(self.migrations)} migration(s)")
        for name, schema in self.migrations:
            logger.info(f"Applying migration: {name}")
            logger.debug(f"Migration schema:\n{schema}")

            with self.db.transaction("schema") as tx:
                tx.execute(schema)
                tx.commit()

            logger.info(f"Migration {name} applied successfully")
        logger.info("All migrations applied")

    def create_attribute_migration(self, attr_name: str, value_type: str) -> str:
        """Create a migration to add an attribute.

        Args:
            attr_name: Attribute name
            value_type: Value type

        Returns:
            TypeQL migration
        """
        return f"define\nattribute {attr_name}, value {value_type};"

    def create_entity_migration(self, entity_name: str, attributes: list[str]) -> str:
        """Create a migration to add an entity.

        Args:
            entity_name: Entity name
            attributes: List of attribute names

        Returns:
            TypeQL migration
        """
        lines = ["define", f"entity {entity_name}"]
        for attr in attributes:
            lines.append(f"    owns {attr}")
        lines.append(";")
        return "\n".join(lines)

    def create_relation_migration(
        self, relation_name: str, roles: list[tuple[str, str]], attributes: list[str] | None = None
    ) -> str:
        """Create a migration to add a relation.

        Args:
            relation_name: Relation name
            roles: List of (role_name, player_type) tuples
            attributes: Optional list of attribute names

        Returns:
            TypeQL migration
        """
        lines = ["define", f"relation {relation_name}"]

        seen_roles: set[str] = set()
        for role_name, _ in roles:
            if role_name in seen_roles:
                continue
            seen_roles.add(role_name)
            lines.append(f"    relates {role_name}")

        if attributes:
            for attr in attributes:
                lines.append(f"    owns {attr}")

        lines.append(";")
        lines.append("")

        # Add role player definitions
        for role_name, player_type in roles:
            lines.append(f"{player_type} plays {relation_name}:{role_name};")

        return "\n".join(lines)
