"""Migration generator for auto-generating migrations from model changes."""

from __future__ import annotations

import logging
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING

from type_bridge.attribute.base import Attribute
from type_bridge.migration import operations as ops
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import IntrospectedSchema, SchemaIntrospector
from type_bridge.migration.loader import MigrationLoader
from type_bridge.migration.schema_manager import SchemaManager

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


def _add_ownership_operation(
    owner: type[Entity | Relation],
    attr_name: str,
    attributes: dict[str, type[Attribute]],
) -> ops.AddOwnership | None:
    attr = attributes.get(attr_name)
    if attr is None:
        return None

    attr_info = owner.get_owned_attributes().get(attr_name)
    if attr_info is None:
        return ops.AddOwnership(owner, attr)

    flags = attr_info.flags
    return ops.AddOwnership(
        owner,
        attr,
        optional=flags.card_min == 0,
        key=flags.is_key,
        unique=flags.is_unique,
        card_min=flags.card_min,
        card_max=flags.card_max,
    )


def _role_player_type_names(role: object) -> list[str]:
    return [
        player.get_type_name() if hasattr(player, "get_type_name") else str(player)
        for player in getattr(role, "player_types", [])
    ]


class MigrationGenerator:
    """Generates migration files from model changes.

    Compares current models against the last migration state and generates
    appropriate operations for the detected changes.

    Example:
        generator = MigrationGenerator(db, Path("migrations"))

        # Generate migration from models
        path = generator.generate([Person, Company, Employment], name="initial")
        # Creates: migrations/0001_initial.py

        # Generate empty migration for manual editing
        path = generator.generate([], name="custom_changes", empty=True)
    """

    def __init__(self, db: Database, migrations_dir: Path):
        """Initialize generator.

        Args:
            db: Database connection
            migrations_dir: Directory to write migration files
        """
        self.db = db
        self.migrations_dir = migrations_dir
        self.loader = MigrationLoader(migrations_dir)

    def generate(
        self,
        models: list[type[Entity | Relation]],
        name: str = "auto",
        empty: bool = False,
    ) -> Path | None:
        """Generate a migration file.

        Args:
            models: Model classes to check for changes
            name: Migration name suffix (e.g., "initial", "add_company")
            empty: Create empty migration for manual editing

        Returns:
            Path to created file, or None if no changes detected
        """
        # Get current state
        existing = self.loader.discover()

        # Determine next migration number
        next_num = self.loader.get_next_number()

        # Determine dependencies
        dependencies: list[tuple[str, str]] = []
        if existing:
            last = existing[-1]
            dependencies.append((last.migration.app_label, last.migration.name))

        if empty:
            operations_code = "    operations: ClassVar[list[Operation]] = []"
            models_code = ""
            imports_code = self._generate_empty_imports()
            description = "empty migration"
        else:
            # Detect changes - now always returns operations
            operations, _ = self._detect_changes(models, existing)

            if not operations:
                logger.info("No changes detected")
                return None

            # Always use operations-based migration
            operations_code = self._render_operations(operations)
            models_code = ""
            imports_code = self._generate_operations_imports(operations)
            description = self._describe_operations(operations)

        # Generate filename
        migration_name = f"{next_num:04d}_{name}"
        filename = f"{migration_name}.py"
        filepath = self.migrations_dir / filename

        # Ensure directory exists
        self.migrations_dir.mkdir(parents=True, exist_ok=True)

        # Generate content
        content = self._render_migration(
            class_name=self._to_class_name(name),
            dependencies=dependencies,
            operations_code=operations_code,
            models_code=models_code,
            imports_code=imports_code,
            description=description,
        )

        filepath.write_text(content)
        logger.info(f"Created migration: {filepath}")

        return filepath

    def _detect_changes(
        self,
        models: list[type[Entity | Relation]],
        existing_migrations: list,
    ) -> tuple[list[ops.Operation], list[type[Entity | Relation]]]:
        """Detect changes between models and current database schema.

        Compares Python models against the actual TypeDB database schema
        and generates operations for the detected differences.

        Args:
            models: New models to compare
            existing_migrations: Existing migrations (used for fallback)

        Returns:
            Tuple of (operations, empty list) - always returns operations now
        """
        if not models:
            return [], []

        # Collect schema info from models
        schema_mgr = SchemaManager(self.db)
        schema_mgr.register(*models)
        new_info = schema_mgr.collect_schema_info()

        # Introspect actual database schema using model-aware approach
        introspector = SchemaIntrospector(self.db)
        db_schema = introspector.introspect_for_models(models)

        # Generate operations - this works for both initial and incremental migrations
        # For empty database, all model types will be "new" and get Add operations
        operations = self._introspected_to_operations(db_schema, new_info)

        if not operations:
            logger.info("No changes detected between models and database")
        elif db_schema.is_empty():
            logger.info("Empty database detected, generating initial migration")
        else:
            logger.info(f"Detected {len(operations)} schema changes")

        return operations, []

    def _introspected_to_operations(
        self, db_schema: IntrospectedSchema, model_info: SchemaInfo
    ) -> list[ops.Operation]:
        """Generate operations from comparing introspected schema to models.

        Args:
            db_schema: Introspected database schema
            model_info: Schema info from Python models

        Returns:
            List of operations to apply
        """
        from type_bridge._rust_runtime import compute_schema_diff

        operations: list[ops.Operation] = []

        current_schema = db_schema.to_rust_schema_info()
        target_schema = model_info.to_rust_schema_info()
        rust_diff = compute_schema_diff(current_schema, target_schema)

        entities = {entity.get_type_name(): entity for entity in model_info.entities}
        relations = {relation.get_type_name(): relation for relation in model_info.relations}
        attributes = {
            attribute.get_attribute_name(): attribute for attribute in model_info.attribute_classes
        }

        for attr_name in rust_diff.get("added_attributes", []):
            if attr := attributes.get(attr_name):
                operations.append(ops.AddAttribute(attr))
                logger.debug(f"Will add attribute: {attr_name}")

        for entity_name in rust_diff.get("added_entities", []):
            if entity := entities.get(entity_name):
                operations.append(ops.AddEntity(entity))
                logger.debug(f"Will add entity: {entity_name}")

        for relation_name in rust_diff.get("added_relations", []):
            if relation := relations.get(relation_name):
                operations.append(ops.AddRelation(relation))
                logger.debug(f"Will add relation: {relation_name}")

        for entity_name, changes in rust_diff.get("modified_entities", {}).items():
            entity = entities.get(entity_name)
            if entity is None:
                continue
            for attr_name in changes.get("added_attributes", []):
                if operation := _add_ownership_operation(entity, attr_name, attributes):
                    operations.append(operation)
                    logger.debug(f"Will add ownership: {entity_name} owns {attr_name}")
            for attr_name in changes.get("removed_attributes", []):
                if attr := attributes.get(attr_name):
                    operations.append(ops.RemoveOwnership(entity, attr))
            for attr_name, old_flags, new_flags in changes.get("modified_attributes", []):
                if attr := attributes.get(attr_name):
                    operations.append(ops.ModifyOwnership(entity, attr, old_flags, new_flags))

        for relation_name, changes in rust_diff.get("modified_relations", {}).items():
            relation = relations.get(relation_name)
            if relation is None:
                continue
            for attr_name in changes.get("added_attributes", []):
                if operation := _add_ownership_operation(relation, attr_name, attributes):
                    operations.append(operation)
                    logger.debug(f"Will add ownership: {relation_name} owns {attr_name}")
            for attr_name in changes.get("removed_attributes", []):
                if attr := attributes.get(attr_name):
                    operations.append(ops.RemoveOwnership(relation, attr))
            for attr_name, old_flags, new_flags in changes.get("modified_attributes", []):
                if attr := attributes.get(attr_name):
                    operations.append(ops.ModifyOwnership(relation, attr, old_flags, new_flags))
            for role_name in changes.get("added_roles", []):
                role = relation._roles.get(role_name)
                player_types = _role_player_type_names(role) if role is not None else []
                operations.append(ops.AddRole(relation, role_name, player_types))
                logger.debug(f"Will add role: {relation_name}:{role_name}")
            for role_name in changes.get("removed_roles", []):
                operations.append(ops.RemoveRole(relation, role_name))
            for player_change in changes.get("modified_role_players", []):
                role_name = player_change["role_name"]
                for player_type in player_change.get("added_player_types", []):
                    operations.append(ops.AddRolePlayer(relation, role_name, player_type))
                for player_type in player_change.get("removed_player_types", []):
                    operations.append(ops.RemoveRolePlayer(relation, role_name, player_type))

        return operations

    def _render_operations(self, operations: list[ops.Operation]) -> str:
        """Render operations list as Python code.

        Converts class-based operations to RunTypeQL operations so that
        the generated migration file is self-contained and doesn't need
        to import model classes.

        Args:
            operations: List of operations

        Returns:
            Python code string
        """
        if not operations:
            return "    operations: ClassVar[list[Operation]] = []"

        lines = ["    operations: ClassVar[list[Operation]] = ["]
        for op in operations:
            # Convert to RunTypeQL to make migrations self-contained
            forward_tql = op.to_typeql()
            reverse_tql = op.to_rollback_typeql()

            if reverse_tql:
                lines.append(
                    f"        ops.RunTypeQL(forward={forward_tql!r}, reverse={reverse_tql!r}),"
                )
            else:
                lines.append(f"        ops.RunTypeQL(forward={forward_tql!r}),")
        lines.append("    ]")
        return "\n".join(lines)

    def _describe_operations(self, operations: list[ops.Operation]) -> str:
        """Generate description of operations.

        Args:
            operations: List of operations

        Returns:
            Description string
        """
        parts: list[str] = []
        for op in operations[:3]:  # First 3 operations
            if isinstance(op, ops.AddEntity):
                parts.append(f"add {op.entity.get_type_name()}")
            elif isinstance(op, ops.AddAttribute):
                parts.append(f"add {op.attribute.get_attribute_name()}")
            elif isinstance(op, ops.AddRelation):
                parts.append(f"add {op.relation.get_type_name()}")
            elif isinstance(op, ops.AddOwnership):
                parts.append(
                    f"add {op.attribute.get_attribute_name()} to {op.owner.get_type_name()}"
                )

        if len(operations) > 3:
            parts.append(f"and {len(operations) - 3} more")

        return ", ".join(parts) or "schema changes"

    def _to_class_name(self, name: str) -> str:
        """Convert migration name to class name.

        Args:
            name: Migration name (e.g., "add_company")

        Returns:
            Class name (e.g., "AddCompanyMigration")
        """
        return "".join(word.capitalize() for word in name.split("_")) + "Migration"

    def _generate_empty_imports(self) -> str:
        """Generate imports for empty migration."""
        return """from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops"""

    def _generate_operations_imports(self, operations: list[ops.Operation]) -> str:
        """Generate imports for operations-based migration."""
        lines = [
            "from typing import ClassVar",
            "",
            "from type_bridge.migration import Migration",
            "from type_bridge.migration.operations import Operation",
            "from type_bridge.migration import operations as ops",
        ]

        # Collect types used in operations
        types_needed: set[str] = set()
        for op in operations:
            if isinstance(op, (ops.AddAttribute, ops.RemoveAttribute)):
                types_needed.add(op.attribute.__name__)
            elif isinstance(op, (ops.AddEntity, ops.RemoveEntity)):
                types_needed.add(op.entity.__name__)
            elif isinstance(op, (ops.AddRelation, ops.RemoveRelation)):
                types_needed.add(op.relation.__name__)
            elif isinstance(op, ops.AddOwnership):
                types_needed.add(op.owner.__name__)
                types_needed.add(op.attribute.__name__)
            elif isinstance(op, (ops.AddRole, ops.AddRolePlayer, ops.RemoveRolePlayer)):
                types_needed.add(op.relation.__name__)

        if types_needed:
            lines.append("")
            lines.append("# TODO: Update these imports to match your model locations")
            for type_name in sorted(types_needed):
                lines.append(f"# from your_app.models import {type_name}")

        return "\n".join(lines)

    def _render_migration(
        self,
        class_name: str,
        dependencies: list[tuple[str, str]],
        operations_code: str,
        models_code: str,
        imports_code: str,
        description: str,
    ) -> str:
        """Render migration file content.

        Args:
            class_name: Migration class name
            dependencies: List of dependencies
            operations_code: Operations as Python code
            models_code: Models as Python code
            imports_code: Import statements
            description: Migration description

        Returns:
            Complete migration file content
        """
        deps_str = repr(dependencies)
        timestamp = datetime.now(UTC).isoformat()

        # Build class body
        body_parts = [
            f'    """Migration: {description}"""',
            "",
            f"    dependencies: ClassVar[list[tuple[str, str]]] = {deps_str}",
        ]

        if models_code:
            body_parts.append("")
            body_parts.append(models_code)

        if operations_code:
            body_parts.append("")
            body_parts.append(operations_code)

        body = "\n".join(body_parts)

        return f'''"""Migration: {description}

Auto-generated by type_bridge on {timestamp}
"""

{imports_code}


class {class_name}(Migration):
{body}
'''
