"""Migration generator for auto-generating migrations from model changes."""

from __future__ import annotations

import logging
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any

from type_bridge import _rust_runtime
from type_bridge.attribute.base import Attribute
from type_bridge.migration import operations as ops
from type_bridge.migration import ref
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import IntrospectedSchema, SchemaIntrospector
from type_bridge.migration.loader import MigrationLoader
from type_bridge.migration.schema_manager import SchemaManager
from type_bridge.migration.state_schema import without_migration_state_schema

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

    attr_info = _owned_attribute_info(owner, attr_name)
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


def _owned_attribute_info(owner: type[Entity | Relation], attr_name: str) -> Any | None:
    """Return ownership metadata for a TypeDB attribute label.

    Model metadata is keyed by Python field name, while Rust schema diffs use
    TypeDB labels. Bindgen-generated models commonly map ``smoke152-email`` to
    the Python field ``smoke152_email``, so lookup must compare the attribute
    class label rather than only the dict key.
    """
    owned_attributes = owner.get_owned_attributes()
    if attr_name in owned_attributes:
        return owned_attributes[attr_name]

    for attr_info in owned_attributes.values():
        typ = getattr(attr_info, "typ", None)
        get_attribute_name = getattr(typ, "get_attribute_name", None)
        if get_attribute_name is not None and get_attribute_name() == attr_name:
            return attr_info

    return None


def _role_player_type_names(role: object) -> list[str]:
    return [
        player.get_type_name() if hasattr(player, "get_type_name") else str(player)
        for player in getattr(role, "player_types", [])
    ]


def _attribute_type_change_keyword(changes: dict) -> str:
    """Choose TypeDB schema keyword for an attribute type definition change."""
    for key in ("value_type_changed", "parent_changed"):
        if changes.get(key) is not None:
            return "redefine"

    for key in ("regex_changed", "allowed_values_changed", "range_changed"):
        old_new = changes.get(key)
        if old_new is not None:
            old, new = old_new
            if old is not None or new is None:
                return "redefine"

    for key in ("abstract_changed", "independent_changed"):
        old_new = changes.get(key)
        if old_new is not None:
            old, new = old_new
            if old is not False or new is not True:
                return "redefine"

    return "define"


def _type_label(value: Any) -> str:
    return str(value.get_type_name())


def _attribute_label(value: Any) -> str:
    return str(value.get_attribute_name())


def _player_type_label(value: object) -> str:
    get_type_name = getattr(value, "get_type_name", None)
    if callable(get_type_name):
        return str(get_type_name())
    return str(value)


def _type_ref(value: object) -> str:
    from type_bridge.models import Relation

    label = _type_label(value)
    if getattr(value, "kind", None) == "relation":
        return f"ref.relation({label!r})"
    if isinstance(value, type) and issubclass(value, Relation):
        return f"ref.relation({label!r})"
    return f"ref.entity({label!r})"


def _attribute_ref(value: object) -> str:
    return f"ref.attribute({_attribute_label(value)!r})"


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

        operations: list[ops.Operation] = []
        if empty:
            operations_code = "    operations: ClassVar[list[Operation]] = []"
            models_code = ""
            imports_code = self._generate_empty_imports()
            description = "empty migration"
            self._pre_version = f"v{next_num - 1:04d}" if next_num > 1 else None
            self._post_version = f"v{next_num:04d}"
            self._import_records = {}
            self._symbol_aliases = {}
        else:
            # Detect changes - now always returns operations
            operations, _ = self._detect_changes(models, existing)

            if not operations:
                logger.info("No changes detected")
                return None

            # Phase 2: Generate snapshot first
            from type_bridge.migration.snapshots import generate_snapshot

            schema_mgr = SchemaManager(self.db)
            schema_mgr.register(*models)
            schema_text = schema_mgr.generate_schema()

            migration_name = f"{next_num:04d}_{name}"
            version_str = f"v{next_num:04d}"

            generate_snapshot(
                migrations_dir=self.migrations_dir,
                version=version_str,
                migration_name=migration_name,
                schema_text=schema_text,
            )

            self._pre_version = f"v{next_num - 1:04d}" if next_num > 1 else None
            self._post_version = f"v{next_num:04d}"
            self._import_records = {}
            self._symbol_aliases = {}

            # Collect imports and render
            self._collect_imports_and_aliases(operations)
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

        # Write the JSON sidecar carrying the lowered MigrationSpec execution IR.
        # The sidecar is produced from the SAME operations list the .py renders so
        # both artifacts are byte-identical in execution semantics.  The checksum
        # is computed over the just-written .py text so it matches what the loader
        # will compute via migration_file_checksum (04 drift gate invariant).
        if not empty:
            self._write_sidecar(filepath, operations, dependencies, migration_name)

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

        # Introspect the full application schema so removed target types are
        # still visible to the diff. TypeBridge's own migration state schema is
        # filtered out because it is storage infrastructure, not app schema.
        introspector = SchemaIntrospector(self.db)
        db_schema = without_migration_state_schema(introspector.introspect())

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

        for attr_name, changes in rust_diff.get("modified_attributes", {}).items():
            if attr := attributes.get(attr_name):
                keyword = _attribute_type_change_keyword(changes)
                operations.append(
                    ops.RunTypeQL(forward=f"{keyword}\n{attr.to_schema_definition()}")
                )
                logger.debug(f"Will {keyword} attribute type: {attr_name}")

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
                attr = attributes.get(attr_name) or ref.attribute(attr_name)
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
                attr = attributes.get(attr_name) or ref.attribute(attr_name)
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

        for relation_name in rust_diff.get("removed_relations", []):
            relation_ref = ref.relation(relation_name)
            if relation := db_schema.relations.get(relation_name):
                for role in relation.roles.values():
                    for player_type in role.player_types:
                        operations.append(
                            ops.RemoveRolePlayer(
                                relation_ref,
                                role.name,
                                _player_type_label(player_type),
                            )
                        )
                    operations.append(ops.RemoveRole(relation_ref, role.name))
                for ownership in db_schema.get_ownerships_for(relation_name):
                    operations.append(
                        ops.RemoveOwnership(
                            relation_ref,
                            ref.attribute(ownership.attribute_name),
                        )
                    )
            operations.append(ops.RemoveRelation(ref.relation(relation_name)))
            logger.debug(f"Will remove relation: {relation_name}")

        for entity_name in rust_diff.get("removed_entities", []):
            entity_ref = ref.entity(entity_name)
            for ownership in db_schema.get_ownerships_for(entity_name):
                operations.append(
                    ops.RemoveOwnership(
                        entity_ref,
                        ref.attribute(ownership.attribute_name),
                    )
                )
            operations.append(ops.RemoveEntity(entity_ref))
            logger.debug(f"Will remove entity: {entity_name}")

        for attr_name in rust_diff.get("removed_attributes", []):
            operations.append(ops.RemoveAttribute(ref.attribute(attr_name)))
            logger.debug(f"Will remove attribute: {attr_name}")

        return operations

    def _render_operations(self, operations: list[ops.Operation]) -> str:
        """Render operations list as Python code.

        Generated migrations keep the operation-object authoring surface
        (`ops.AddAttribute(...)`, `ops.AddEntity(...)`, etc.) so users can review
        schema changes at the same abstraction level as their models. Custom
        `ops.RunTypeQL` operations are still rendered as `ops.RunTypeQL`.

        Args:
            operations: List of operations

        Returns:
            Python code string
        """
        if not operations:
            return "    operations: ClassVar[list[Operation]] = []"

        lines = ["    operations: ClassVar[list[Operation]] = ["]
        for op in operations:
            lines.append(f"        {self._render_operation(op)},")
        lines.append("    ]")
        return "\n".join(lines)

    def _collect_imports_and_aliases(self, operations: list[ops.Operation]) -> None:
        """Scan operations to collect all needed snapshot imports and compute aliases."""
        self._import_records = {}
        self._symbol_aliases = {}

        # 1. Collect imports by scanning each operation
        for op in operations:
            self._collect_op_imports(op)

        # 2. Compute aliases for colliding symbol names
        # Count occurrences of each symbol name across all versions
        symbol_counts: dict[str, int] = {}
        symbol_versions: dict[str, list[str]] = {}
        for version, submodules in self._import_records.items():
            for submodule, symbols in submodules.items():
                for symbol in symbols:
                    symbol_counts[symbol] = symbol_counts.get(symbol, 0) + 1
                    symbol_versions.setdefault(symbol, []).append(version)

        # For symbols with count > 1, latest version gets the unaliased symbol,
        # and others get aliased to Symbol{Version}
        for symbol, count in symbol_counts.items():
            if count > 1:
                versions = sorted(symbol_versions[symbol])
                latest_version = versions[-1]
                # Register aliases
                for version in versions:
                    if version != latest_version:
                        alias = f"{symbol}{version.upper()}"
                        self._symbol_aliases[(version, symbol)] = alias

    def _collect_op_imports(self, op: ops.Operation) -> None:
        # Determine the version (pre or post) for the operation
        if isinstance(
            op,
            (
                ops.AddEntity,
                ops.AddRelation,
                ops.AddAttribute,
                ops.AddOwnership,
                ops.AddRole,
                ops.AddRolePlayer,
            ),
        ):
            version = self._post_version
        else:
            version = self._pre_version

        if not version:
            return

        # Helper to try to resolve type references
        def record_ref(value: object):
            if not isinstance(value, str):
                self._resolve_snapshot_symbol(value, version)

        if isinstance(op, (ops.AddEntity, ops.RemoveEntity)):
            record_ref(op.entity)
        elif isinstance(op, (ops.AddRelation, ops.RemoveRelation)):
            record_ref(op.relation)
        elif isinstance(op, (ops.AddAttribute, ops.RemoveAttribute)):
            record_ref(op.attribute)
        elif isinstance(op, (ops.AddOwnership, ops.RemoveOwnership)):
            record_ref(op.owner)
            record_ref(op.attribute)
        elif isinstance(op, ops.ModifyOwnership):
            record_ref(op.owner)
            record_ref(op.attribute)
        elif isinstance(op, (ops.AddRole, ops.RemoveRole)):
            record_ref(op.relation)
        elif isinstance(op, (ops.AddRolePlayer, ops.RemoveRolePlayer)):
            record_ref(op.relation)
        elif isinstance(op, ops.CopyAttribute):
            record_ref(op.owner)

    def _resolve_snapshot_symbol(self, value: Any, version: str) -> str | None:
        if not version:
            return None

        # Extract label
        from type_bridge.migration.snapshots import get_snapshot_class_map

        # Get label and kind
        try:
            label, kind = self._get_label_and_kind(value)
        except Exception:
            return None

        class_map = get_snapshot_class_map(self.migrations_dir, version)
        if label not in class_map:
            return None

        cls = class_map[label]
        class_name = cls.__name__

        # Determine submodule
        from type_bridge.attribute.base import Attribute
        from type_bridge.models import Relation

        if issubclass(cls, Relation):
            submodule = "relations"
        elif issubclass(cls, Attribute):
            submodule = "attributes"
        else:
            submodule = "entities"

        # Record the import
        self._import_records.setdefault(version, {}).setdefault(submodule, set()).add(class_name)
        return class_name

    def _get_label_and_kind(self, value: Any) -> tuple[str, str]:
        if isinstance(value, type):
            from type_bridge.attribute.base import Attribute
            from type_bridge.models import Relation

            if issubclass(value, Relation):
                return value.get_type_name(), "relation"
            elif issubclass(value, Attribute):
                return value.get_attribute_name(), "attribute"
            else:
                return value.get_type_name(), "entity"
        elif hasattr(value, "get_type_name"):
            kind = getattr(value, "kind", "entity")
            return str(value.get_type_name()), kind
        elif hasattr(value, "get_attribute_name"):
            return str(value.get_attribute_name()), "attribute"
        elif hasattr(value, "label"):
            kind = getattr(value, "kind", "entity")
            return str(value.label), kind
        else:
            raise ValueError(f"Cannot get label from {value}")

    def _type_ref(self, value: Any, version: str | None) -> str:
        if version:
            symbol = self._resolve_snapshot_symbol(value, version)
            if symbol:
                resolved_symbol = self._symbol_aliases.get((version, symbol), symbol)
                return resolved_symbol

        # Fallback to old behavior
        from type_bridge.models import Relation

        try:
            label, kind = self._get_label_and_kind(value)
        except Exception:
            label = getattr(value, "label", str(value))
            kind = "entity"

        if kind == "relation":
            return f"ref.relation({label!r})"
        if isinstance(value, type) and issubclass(value, Relation):
            return f"ref.relation({label!r})"
        return f"ref.entity({label!r})"

    def _attribute_ref(self, value: Any, version: str | None) -> str:
        if version:
            symbol = self._resolve_snapshot_symbol(value, version)
            if symbol:
                resolved_symbol = self._symbol_aliases.get((version, symbol), symbol)
                return resolved_symbol

        # Fallback to old behavior
        try:
            label, _ = self._get_label_and_kind(value)
        except Exception:
            label = getattr(value, "label", str(value))
        return f"ref.attribute({label!r})"

    def _render_operation(self, operation: ops.Operation) -> str:
        """Render one operation object as Python source."""
        pre_version = getattr(self, "_pre_version", None)
        post_version = getattr(self, "_post_version", None)

        if isinstance(operation, ops.AddAttribute):
            return f"ops.AddAttribute({self._attribute_ref(operation.attribute, post_version)})"
        if isinstance(operation, ops.RemoveAttribute):
            return f"ops.RemoveAttribute({self._attribute_ref(operation.attribute, pre_version)})"
        if isinstance(operation, ops.AddEntity):
            return f"ops.AddEntity({self._type_ref(operation.entity, post_version)})"
        if isinstance(operation, ops.RemoveEntity):
            return f"ops.RemoveEntity({self._type_ref(operation.entity, pre_version)})"
        if isinstance(operation, ops.AddOwnership):
            args = [
                self._type_ref(operation.owner, post_version),
                self._attribute_ref(operation.attribute, post_version),
            ]
            kwargs = self._render_add_ownership_kwargs(operation)
            return self._render_call("AddOwnership", args, kwargs)
        if isinstance(operation, ops.RemoveOwnership):
            return (
                "ops.RemoveOwnership("
                f"{self._type_ref(operation.owner, pre_version)}, {self._attribute_ref(operation.attribute, pre_version)})"
            )
        if isinstance(operation, ops.ModifyOwnership):
            return self._render_call(
                "ModifyOwnership",
                [
                    self._type_ref(operation.owner, pre_version),
                    self._attribute_ref(operation.attribute, pre_version),
                ],
                {
                    "old_annotations": repr(operation.old_annotations),
                    "new_annotations": repr(operation.new_annotations),
                },
            )
        if isinstance(operation, ops.AddRelation):
            return f"ops.AddRelation({self._type_ref(operation.relation, post_version)})"
        if isinstance(operation, ops.RemoveRelation):
            return f"ops.RemoveRelation({self._type_ref(operation.relation, pre_version)})"
        if isinstance(operation, ops.AddRole):
            return self._render_call(
                "AddRole",
                [
                    self._type_ref(operation.relation, post_version),
                    repr(operation.role_name),
                    repr(operation.player_types),
                ],
                {},
            )
        if isinstance(operation, ops.RemoveRole):
            return f"ops.RemoveRole({self._type_ref(operation.relation, pre_version)}, {operation.role_name!r})"
        if isinstance(operation, ops.AddRolePlayer):
            return (
                "ops.AddRolePlayer("
                f"{self._type_ref(operation.relation, post_version)}, {operation.role_name!r}, "
                f"{operation.player_type!r})"
            )
        if isinstance(operation, ops.RemoveRolePlayer):
            return (
                "ops.RemoveRolePlayer("
                f"{self._type_ref(operation.relation, pre_version)}, {operation.role_name!r}, "
                f"{operation.player_type!r})"
            )
        if isinstance(operation, ops.RunTypeQL):
            kwargs = {"forward": repr(operation.forward)}
            if operation.reverse is not None:
                kwargs["reverse"] = repr(operation.reverse)
            return self._render_call("RunTypeQL", [], kwargs)
        if isinstance(operation, ops.RenameAttribute):
            return self._render_call(
                "RenameAttribute",
                [repr(operation.old_name), repr(operation.new_name), repr(operation.value_type)],
                {},
            )
        if isinstance(operation, ops.CopyAttribute):
            kwargs = {
                "owner": self._type_ref(operation.owner, pre_version),
                "source": repr(operation.source),
                "dest": repr(operation.dest),
            }
            if operation.filter is not None:
                kwargs["filter"] = repr(operation.filter)
            return self._render_call("CopyAttribute", [], kwargs)
        raise TypeError(f"Unsupported migration operation type: {type(operation).__name__}")

    def _render_add_ownership_kwargs(self, operation: ops.AddOwnership) -> dict[str, str]:
        """Render non-default AddOwnership keyword args without redundant cardinality."""
        kwargs: dict[str, str] = {}
        if operation.key:
            kwargs["key"] = "True"
            return kwargs
        if operation.unique:
            kwargs["unique"] = "True"
            return kwargs
        if operation.optional and operation.card_min == 0 and operation.card_max == 1:
            kwargs["optional"] = "True"
            return kwargs
        if operation.optional:
            kwargs["optional"] = "True"
        if operation.card_min is not None:
            kwargs["card_min"] = repr(operation.card_min)
        if operation.card_max is not None:
            kwargs["card_max"] = repr(operation.card_max)
        return kwargs

    def _render_call(self, op_name: str, args: list[str], kwargs: dict[str, str]) -> str:
        """Render an operation constructor call."""
        parts = [*args, *(f"{key}={value}" for key, value in kwargs.items())]
        return f"ops.{op_name}({', '.join(parts)})"

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

    def _write_sidecar(
        self,
        py_path: Path,
        operations: list[ops.Operation],
        dependencies: list[tuple[str, str]],
        migration_name: str,
    ) -> None:
        """Write the JSON sidecar for a generated migration alongside its .py file.

        Builds the structured MigrationSpec from the original typed operation
        list, while generated .py source renders stable snapshot bindings. Typed
        operations stay typed in the sidecar; only explicit ops.RunTypeQL
        instances become run_typeql.
        The checksum is computed over the .py text so the drift gate reads a
        consistent value regardless of which path (sidecar or exec_module) is
        used.

        The sidecar is omitted if migration_spec_to_json or any serialization step
        raises — we log the error rather than failing the whole generate() call,
        since the .py is already written and is fully usable without a sidecar.
        """
        try:
            from type_bridge.migration._lower import lower_migration
            from type_bridge.migration.base import Migration

            app_label = self.migrations_dir.name

            # Compute the .py checksum over the just-written file so the sidecar
            # carries the same value the loader will produce via
            # _rust_runtime.migration_file_checksum.
            py_content = py_path.read_text()
            checksum = _rust_runtime.migration_file_checksum(py_content)

            migration_cls = type(
                "GeneratedSidecarMigration",
                (Migration,),
                {
                    "dependencies": dependencies,
                    "operations": operations,
                    "reversible": True,
                },
            )
            migration = migration_cls()
            migration.app_label = app_label
            migration.name = migration_name

            spec = lower_migration(migration, checksum=checksum)
            json_text = _rust_runtime.migration_spec_to_json(spec)
            sidecar_path = py_path.with_suffix(".json")
            sidecar_path.write_text(json_text)
            logger.info(f"Created migration sidecar: {sidecar_path}")
        except Exception as exc:
            logger.warning(f"Could not write migration sidecar for {py_path}: {exc}")

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
            "from type_bridge.migration import operations as ops, ref",
        ]

        if hasattr(self, "_import_records") and self._import_records:
            lines.append("")
            app_name = self.migrations_dir.name
            for version in sorted(self._import_records.keys()):
                submodules = self._import_records[version]
                symbols = sorted({symbol for symbols in submodules.values() for symbol in symbols})
                import_parts = []
                for sym in symbols:
                    alias = self._symbol_aliases.get((version, sym))
                    if alias:
                        import_parts.append(f"{sym} as {alias}")
                    else:
                        import_parts.append(sym)

                lines.append(
                    f"from {app_name}.snapshots.{version} import {', '.join(import_parts)}"
                )

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
