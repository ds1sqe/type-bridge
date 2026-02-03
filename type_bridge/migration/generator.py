"""Migration generator for auto-generating migrations from model changes."""

from __future__ import annotations

import logging
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING

from type_bridge.migration import operations as ops
from type_bridge.migration.diff import SchemaDiff
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import IntrospectedSchema, SchemaIntrospector
from type_bridge.migration.loader import MigrationLoader
from type_bridge.migration.schema_manager import SchemaManager

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation
    from type_bridge.session import Database

logger = logging.getLogger(__name__)


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
        operations: list[ops.Operation] = []

        # Get type names from both sources
        db_entities = db_schema.get_entity_names()
        db_relations = db_schema.get_relation_names()
        db_attributes = db_schema.get_attribute_names()

        model_entities = {e.get_type_name() for e in model_info.entities}
        model_relations = {r.get_type_name() for r in model_info.relations}
        model_attributes = {a.get_attribute_name() for a in model_info.attribute_classes}

        # Find new attributes (must be added first)
        new_attrs = model_attributes - db_attributes
        for attr in model_info.attribute_classes:
            if attr.get_attribute_name() in new_attrs:
                operations.append(ops.AddAttribute(attr))
                logger.debug(f"Will add attribute: {attr.get_attribute_name()}")

        # Find new entities
        new_entities = model_entities - db_entities
        for entity in model_info.entities:
            if entity.get_type_name() in new_entities:
                operations.append(ops.AddEntity(entity))
                logger.debug(f"Will add entity: {entity.get_type_name()}")

        # Find new relations
        new_relations = model_relations - db_relations
        for relation in model_info.relations:
            if relation.get_type_name() in new_relations:
                operations.append(ops.AddRelation(relation))
                logger.debug(f"Will add relation: {relation.get_type_name()}")

        # Check for new ownerships on existing entities
        for entity in model_info.entities:
            entity_name = entity.get_type_name()
            if entity_name in db_entities:
                # Entity exists, check for new attributes
                db_ownerships = {
                    o.attribute_name for o in db_schema.get_ownerships_for(entity_name)
                }
                model_ownerships = entity.get_owned_attributes()

                for attr_name, attr_info in model_ownerships.items():
                    attr_type_name = attr_info.typ.get_attribute_name()
                    if attr_type_name not in db_ownerships:
                        # Need to add this attribute first if it doesn't exist
                        if attr_type_name not in db_attributes and attr_type_name not in new_attrs:
                            operations.append(ops.AddAttribute(attr_info.typ))
                            new_attrs.add(attr_type_name)

                        operations.append(
                            ops.AddOwnership(
                                entity,
                                attr_info.typ,
                                optional=attr_info.flags.card_min == 0,
                                key=attr_info.flags.is_key,
                                unique=attr_info.flags.is_unique,
                            )
                        )
                        logger.debug(f"Will add ownership: {entity_name} owns {attr_type_name}")

        # Check for new roles/role players on existing relations
        for relation in model_info.relations:
            rel_name = relation.get_type_name()
            if rel_name in db_relations:
                db_rel = db_schema.relations.get(rel_name)
                if db_rel:
                    db_role_names = set(db_rel.roles.keys())
                    model_roles = relation._roles

                    # New roles
                    for role_name, role in model_roles.items():
                        if role_name not in db_role_names:
                            operations.append(
                                ops.AddRole(
                                    relation,
                                    role_name,
                                    list(role.player_types),
                                )
                            )
                            logger.debug(f"Will add role: {rel_name}:{role_name}")
                        else:
                            # Role exists, check for new player types
                            db_role = db_rel.roles.get(role_name)
                            if db_role:
                                db_players = set(db_role.player_types)
                                model_players = set(role.player_types)
                                new_players = model_players - db_players

                                for player in new_players:
                                    operations.append(
                                        ops.AddRolePlayer(relation, role_name, player)
                                    )
                                    logger.debug(
                                        f"Will add role player: {player} plays {rel_name}:{role_name}"
                                    )

        # Log warnings for removed types (breaking changes)
        removed_entities = db_entities - model_entities
        if removed_entities:
            logger.warning(
                f"Detected entities in DB but not in models: {removed_entities}. "
                "Manual migration may be required for data cleanup."
            )

        removed_relations = db_relations - model_relations
        if removed_relations:
            logger.warning(
                f"Detected relations in DB but not in models: {removed_relations}. "
                "Manual migration may be required for data cleanup."
            )

        removed_attrs = db_attributes - model_attributes
        if removed_attrs:
            logger.warning(
                f"Detected attributes in DB but not in models: {removed_attrs}. "
                "Manual migration may be required for data cleanup."
            )

        return operations

    def _diff_to_operations(self, diff: SchemaDiff, new_info: SchemaInfo) -> list[ops.Operation]:
        """Convert SchemaDiff to operations.

        Args:
            diff: Schema diff
            new_info: New schema info (for looking up types)

        Returns:
            List of operations
        """
        operations: list[ops.Operation] = []

        # Add new attributes first (they may be needed by entities/relations)
        for attr in diff.added_attributes:
            operations.append(ops.AddAttribute(attr))

        # Add new entities
        for entity in diff.added_entities:
            operations.append(ops.AddEntity(entity))

        # Add new relations
        for relation in diff.added_relations:
            operations.append(ops.AddRelation(relation))

        # Handle modified entities
        for entity, changes in diff.modified_entities.items():
            # Add ownership for new attributes
            owned_attrs = entity.get_owned_attributes()
            for attr_name in changes.added_attributes:
                if attr_name in owned_attrs:
                    attr_info = owned_attrs[attr_name]
                    operations.append(
                        ops.AddOwnership(
                            entity,
                            attr_info.typ,
                            optional=attr_info.flags.card_min == 0,
                            key=attr_info.flags.is_key,
                            unique=attr_info.flags.is_unique,
                        )
                    )

            # Note: Removed attributes would need RemoveOwnership
            # but that's a breaking change requiring careful handling

        # Handle modified relations
        for relation, changes in diff.modified_relations.items():
            # Add new roles
            for role_name in changes.added_roles:
                if role_name in relation._roles:
                    role = relation._roles[role_name]
                    operations.append(
                        ops.AddRole(
                            relation,
                            role_name,
                            list(role.player_types),
                        )
                    )

            # Handle role player changes
            for rpc in changes.modified_role_players:
                for player_type in rpc.added_player_types:
                    operations.append(ops.AddRolePlayer(relation, rpc.role_name, player_type))

        # Removed types are breaking changes - generate warnings but don't auto-remove
        if diff.removed_entities:
            logger.warning(
                f"Detected removed entities: {[e.__name__ for e in diff.removed_entities]}. "
                "Manual migration required for data cleanup."
            )

        if diff.removed_relations:
            logger.warning(
                f"Detected removed relations: {[r.__name__ for r in diff.removed_relations]}. "
                "Manual migration required for data cleanup."
            )

        if diff.removed_attributes:
            logger.warning(
                f"Detected removed attributes: {[a.get_attribute_name() for a in diff.removed_attributes]}. "
                "Manual migration required for data cleanup."
            )

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

    def _render_models(self, models: list[type[Entity | Relation]]) -> str:
        """Render models list as Python code.

        Args:
            models: List of model classes

        Returns:
            Python code string
        """
        if not models:
            return ""

        model_names = [m.__name__ for m in models]
        return f"    models: ClassVar[list[type[Entity | Relation]]] = [{', '.join(model_names)}]"

    def _op_to_code(self, op: ops.Operation) -> str:
        """Convert operation to Python code string.

        Args:
            op: Operation instance

        Returns:
            Python code
        """
        if isinstance(op, ops.AddAttribute):
            return f"ops.AddAttribute({op.attribute.__name__})"
        elif isinstance(op, ops.RemoveAttribute):
            return f"ops.RemoveAttribute({op.attribute.__name__})"
        elif isinstance(op, ops.AddEntity):
            return f"ops.AddEntity({op.entity.__name__})"
        elif isinstance(op, ops.RemoveEntity):
            return f"ops.RemoveEntity({op.entity.__name__})"
        elif isinstance(op, ops.AddRelation):
            return f"ops.AddRelation({op.relation.__name__})"
        elif isinstance(op, ops.AddOwnership):
            parts = [f"{op.owner.__name__}", f"{op.attribute.__name__}"]
            if op.optional:
                parts.append("optional=True")
            if op.key:
                parts.append("key=True")
            if op.unique:
                parts.append("unique=True")
            return f"ops.AddOwnership({', '.join(parts)})"
        elif isinstance(op, ops.AddRole):
            return f"ops.AddRole({op.relation.__name__}, {op.role_name!r}, {op.player_types!r})"
        elif isinstance(op, ops.AddRolePlayer):
            return (
                f"ops.AddRolePlayer({op.relation.__name__}, {op.role_name!r}, {op.player_type!r})"
            )
        elif isinstance(op, ops.RunTypeQL):
            return f"ops.RunTypeQL(forward={op.forward!r}, reverse={op.reverse!r})"
        else:
            return repr(op)

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

    def _describe_models(self, models: list[type[Entity | Relation]]) -> str:
        """Generate description of models.

        Args:
            models: List of model classes

        Returns:
            Description string
        """
        model_names = [m.__name__ for m in models[:3]]
        if len(models) > 3:
            model_names.append(f"and {len(models) - 3} more")
        return f"initial migration with {', '.join(model_names)}"

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

    def _generate_model_imports(self, models: list[type[Entity | Relation]]) -> str:
        """Generate imports for model-based migration."""
        lines = [
            "from typing import ClassVar",
            "",
            "from type_bridge.migration import Migration",
            "from type_bridge.models import Entity, Relation",
        ]

        # Add model imports (user needs to adjust these)
        lines.append("")
        lines.append("# TODO: Update these imports to match your model locations")
        for model in models:
            lines.append(f"# from your_app.models import {model.__name__}")

        return "\n".join(lines)

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
