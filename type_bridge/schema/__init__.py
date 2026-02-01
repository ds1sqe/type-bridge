"""TypeDB schema management - modular schema utilities.

This package provides schema management functionality for TypeBridge, including:
- Schema comparison and diff (SchemaDiff, EntityChanges, RelationChanges)
- Schema information container (SchemaInfo)
- Schema management (SchemaManager)
- Schema migrations (MigrationManager)
- Breaking change analysis (BreakingChangeAnalyzer)
- Schema exceptions (SchemaConflictError, SchemaValidationError)
"""

from type_bridge.schema.breaking import (
    BreakingChangeAnalyzer,
    ChangeCategory,
    ClassifiedChange,
)
from type_bridge.schema.diff import (
    AttributeFlagChange,
    EntityChanges,
    RelationChanges,
    RoleCardinalityChange,
    RolePlayerChange,
    SchemaDiff,
)
from type_bridge.schema.exceptions import SchemaConflictError, SchemaValidationError
from type_bridge.schema.info import SchemaInfo
from type_bridge.schema.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedRelation,
    IntrospectedRole,
    IntrospectedSchema,
    SchemaIntrospector,
)
from type_bridge.schema.manager import SchemaManager
from type_bridge.schema.migration import MigrationManager
from type_bridge.schema.utils import type_exists

__all__ = [
    # Diff classes
    "SchemaDiff",
    "EntityChanges",
    "RelationChanges",
    "AttributeFlagChange",
    "RolePlayerChange",
    "RoleCardinalityChange",
    # Info container
    "SchemaInfo",
    # Managers
    "SchemaManager",
    "MigrationManager",
    # Breaking change analysis
    "BreakingChangeAnalyzer",
    "ChangeCategory",
    "ClassifiedChange",
    # Introspection
    "SchemaIntrospector",
    "IntrospectedSchema",
    "IntrospectedEntity",
    "IntrospectedRelation",
    "IntrospectedAttribute",
    "IntrospectedOwnership",
    "IntrospectedRole",
    # Exceptions
    "SchemaConflictError",
    "SchemaValidationError",
    # Utils
    "type_exists",
]
