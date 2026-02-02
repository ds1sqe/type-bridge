"""TypeBridge Migration System.

A Django-style migration framework for TypeDB schemas with state tracking,
rollback support, and CLI tools.

Example - Migration file (migrations/0001_initial.py):
    from typing import ClassVar
    from type_bridge.migration import Migration
    from type_bridge.models import Entity, Relation
    from myapp.models import Person, Company, Employment

    class InitialMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = []
        models: ClassVar[list[type[Entity | Relation]]] = [Person, Company, Employment]

Example - Operations-based migration (migrations/0002_add_phone.py):
    from typing import ClassVar
    from type_bridge.migration import Migration, operations as ops
    from type_bridge.migration.operations import Operation
    from myapp.models import Person, Phone

    class AddPhoneMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("migrations", "0001_initial")]
        operations: ClassVar[list[Operation]] = [
            ops.AddAttribute(Phone),
            ops.AddOwnership(Person, Phone, optional=True),
        ]

Example - Programmatic API:
    from pathlib import Path
    from type_bridge.migration import MigrationExecutor
    from type_bridge.session import Database

    db = Database(address="localhost:1729", database="mydb")
    db.connect()

    executor = MigrationExecutor(db, Path("migrations"))

    # Apply all pending migrations
    executor.migrate()

    # Migrate to specific version
    executor.migrate(target="0002_add_phone")

    # Show migration status
    for name, is_applied in executor.showmigrations():
        print(f"[{'X' if is_applied else ' '}] {name}")

    # Preview TypeQL
    print(executor.sqlmigrate("0002_add_phone"))

CLI Usage:
    # Apply all pending migrations
    python -m type_bridge.migration migrate

    # Migrate to specific version
    python -m type_bridge.migration migrate 0002_add_phone

    # Show migration status
    python -m type_bridge.migration showmigrations

    # Preview TypeQL
    python -m type_bridge.migration sqlmigrate 0002_add_phone

    # Generate migration from model changes
    python -m type_bridge.migration makemigrations --name add_phone --models myapp.models
"""

from type_bridge.migration import operations
from type_bridge.migration.base import Migration, MigrationDependency
from type_bridge.migration.breaking import (
    BreakingChangeAnalyzer,
    ChangeCategory,
    ClassifiedChange,
)
from type_bridge.migration.diff import (
    AttributeFlagChange,
    EntityChanges,
    RelationChanges,
    RoleCardinalityChange,
    RolePlayerChange,
    SchemaDiff,
)
from type_bridge.migration.exceptions import SchemaConflictError, SchemaValidationError
from type_bridge.migration.executor import (
    MigrationError,
    MigrationExecutor,
    MigrationPlan,
    MigrationResult,
)
from type_bridge.migration.generator import MigrationGenerator
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedRelation,
    IntrospectedRole,
    IntrospectedSchema,
    SchemaIntrospector,
)
from type_bridge.migration.loader import (
    LoadedMigration,
    MigrationLoader,
    MigrationLoadError,
)
from type_bridge.migration.registry import ModelRegistry
from type_bridge.migration.schema_manager import SchemaManager
from type_bridge.migration.simple_migration import MigrationManager as SimpleMigrationManager
from type_bridge.migration.state import (
    MigrationRecord,
    MigrationState,
    MigrationStateManager,
)
from type_bridge.migration.utils import type_exists

__all__ = [
    # Core classes
    "Migration",
    "MigrationDependency",
    # Diff classes
    "SchemaDiff",
    "EntityChanges",
    "RelationChanges",
    "AttributeFlagChange",
    "RolePlayerChange",
    "RoleCardinalityChange",
    # Breaking change analysis
    "BreakingChangeAnalyzer",
    "ChangeCategory",
    "ClassifiedChange",
    # Exceptions
    "SchemaConflictError",
    "SchemaValidationError",
    # Executor
    "MigrationExecutor",
    "MigrationError",
    "MigrationPlan",
    "MigrationResult",
    # State management
    "MigrationState",
    "MigrationStateManager",
    "MigrationRecord",
    # Loader
    "MigrationLoader",
    "LoadedMigration",
    "MigrationLoadError",
    # Generator
    "MigrationGenerator",
    # Model registry
    "ModelRegistry",
    # Schema info
    "SchemaInfo",
    # Introspection
    "SchemaIntrospector",
    "IntrospectedSchema",
    "IntrospectedEntity",
    "IntrospectedRelation",
    "IntrospectedAttribute",
    "IntrospectedOwnership",
    "IntrospectedRole",
    # Schema Manager
    "SchemaManager",
    "SimpleMigrationManager",
    # Utils
    "type_exists",
    # Operations module
    "operations",
]
