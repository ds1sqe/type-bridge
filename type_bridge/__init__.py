"""TypeBridge - A Python ORM for TypeDB with Attribute-based API."""

from type_bridge.attribute import (
    Attribute,
    AttributeFlags,
    Boolean,
    Card,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Distinct,
    Double,
    Duration,
    Flag,
    Integer,
    Key,
    Ordered,
    String,
    TypeFlags,
    TypeNameCase,
    Unique,
)
from type_bridge.crud import (
    CrudEvent,
    CrudHook,
    EntityNotFoundError,
    HookCancelled,
    KeyAttributeError,
    NotUniqueError,
    RelationNotFoundError,
    TypeDBManager,
)
from type_bridge.migration import (
    BreakingChangeAnalyzer,
    ChangeCategory,
    Migration,
    MigrationError,
    MigrationExecutor,
    ModelRegistry,
    RolePlayerChange,
    SchemaConflictError,
    SchemaInfo,
    SchemaIntrospector,
    SchemaManager,
    SchemaValidationError,
)
from type_bridge.migration import operations as migration_ops
from type_bridge.migration.simple_migration import MigrationManager
from type_bridge.models import Entity, Relation, Role, TypeDBType
from type_bridge.proxy import ProxyDatabase, ProxyError
from type_bridge.query import Query, QueryBuilder
from type_bridge.session import Connection, Database, TransactionContext
from type_bridge.typedb_driver import Credentials, TransactionType, TypeDB, create_driver_options

__version__ = "1.5.3"

__all__ = [
    # Database and session
    "Connection",
    "Database",
    "TransactionContext",
    # TypeDB driver (re-exported for convenience)
    "Credentials",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
    # Models
    "TypeDBType",
    "Entity",
    "Relation",
    "Role",
    # Attributes
    "Attribute",
    "String",
    "Integer",
    "Double",
    "Boolean",
    "Date",
    "DateTime",
    "DateTimeTZ",
    "Decimal",
    "Duration",
    # Attribute annotations
    "AttributeFlags",
    "Flag",
    "Key",
    "Unique",
    "Ordered",
    "Distinct",
    # Cardinality types
    "Card",
    # Entity/Relation flags
    "TypeFlags",
    "TypeNameCase",
    # Query
    "Query",
    "QueryBuilder",
    # CRUD
    "TypeDBManager",
    # Hooks
    "CrudEvent",
    "CrudHook",
    "HookCancelled",
    # Proxy
    "ProxyDatabase",
    "ProxyError",
    # CRUD Exceptions
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
    # Schema
    "SchemaManager",
    "SchemaInfo",
    "SchemaIntrospector",
    "MigrationManager",
    # Schema analysis
    "BreakingChangeAnalyzer",
    "ChangeCategory",
    # Schema exceptions
    "SchemaConflictError",
    "SchemaValidationError",
    "RolePlayerChange",
    # Migration system
    "Migration",
    "MigrationExecutor",
    "MigrationError",
    "ModelRegistry",
    "migration_ops",
]
