"""Loader-scoped imports for frozen pre-cutover migration sources.

Historical migrations and snapshot bindings keep the imports that were
written into their checksummed source. The generated-only package must not
restore those names for ordinary application code, so the migration loader
activates this context only while it executes a trusted frozen source.
"""

from __future__ import annotations

import builtins
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from contextvars import ContextVar
from importlib import import_module
from typing import Any

_ARCHIVE_IMPORT_ACTIVE: ContextVar[bool] = ContextVar(
    "type_bridge_archive_import_active",
    default=False,
)

_MODULE_ALIASES = {
    "type_bridge.migration.operations": "type_bridge.migration._operations",
    "type_bridge.migration.ref": "type_bridge.migration._ref",
}

_ATTRIBUTE_ALIASES: dict[str, dict[str, tuple[str, str | None]]] = {
    "type_bridge": {
        "Attribute": ("type_bridge.attribute.base", "_QueryAttribute"),
        "AttributeFlags": ("type_bridge.attribute.flags", "_QueryAttributeFlags"),
        "Boolean": ("type_bridge.attribute.boolean", "_QueryBoolean"),
        "Card": ("type_bridge.attribute.flags", "_QueryCard"),
        "Date": ("type_bridge.attribute.date", "_QueryDate"),
        "DateTime": ("type_bridge.attribute.datetime", "_QueryDateTime"),
        "DateTimeTZ": ("type_bridge.attribute.datetimetz", "_QueryDateTimeTZ"),
        "Decimal": ("type_bridge.attribute.decimal", "_QueryDecimal"),
        "Distinct": ("type_bridge.attribute.flags", "_QueryDistinct"),
        "Doc": ("type_bridge.attribute.flags", "_QueryDoc"),
        "Double": ("type_bridge.attribute.double", "_QueryDouble"),
        "Duration": ("type_bridge.attribute.duration", "_QueryDuration"),
        "Entity": ("type_bridge.models.entity", "_QueryEntity"),
        "Flag": ("type_bridge.attribute.flags", "_query_flag"),
        "Integer": ("type_bridge.attribute.integer", "_QueryInteger"),
        "Key": ("type_bridge.attribute.flags", "_QueryKey"),
        "Meta": ("type_bridge.attribute.flags", "_QueryMeta"),
        "Ordered": ("type_bridge.attribute.flags", "_QueryOrdered"),
        "Relation": ("type_bridge.models.relation", "_QueryRelation"),
        "Role": ("type_bridge.models.role", "_QueryRole"),
        "String": ("type_bridge.attribute.string", "_QueryString"),
        "TypeDBType": ("type_bridge.models.base", "_QueryTypeDBType"),
        "TypeFlags": ("type_bridge.attribute.flags", "_QueryTypeFlags"),
        "TypeNameCase": ("type_bridge.attribute.flags", "_QueryTypeNameCase"),
        "Unique": ("type_bridge.attribute.flags", "_QueryUnique"),
    },
    "type_bridge.attribute": {
        "Attribute": ("type_bridge.attribute.base", "_QueryAttribute"),
        "AttributeFlags": ("type_bridge.attribute.flags", "_QueryAttributeFlags"),
        "Boolean": ("type_bridge.attribute.boolean", "_QueryBoolean"),
        "Card": ("type_bridge.attribute.flags", "_QueryCard"),
        "Date": ("type_bridge.attribute.date", "_QueryDate"),
        "DateTime": ("type_bridge.attribute.datetime", "_QueryDateTime"),
        "DateTimeTZ": ("type_bridge.attribute.datetimetz", "_QueryDateTimeTZ"),
        "Decimal": ("type_bridge.attribute.decimal", "_QueryDecimal"),
        "Distinct": ("type_bridge.attribute.flags", "_QueryDistinct"),
        "Doc": ("type_bridge.attribute.flags", "_QueryDoc"),
        "Double": ("type_bridge.attribute.double", "_QueryDouble"),
        "Duration": ("type_bridge.attribute.duration", "_QueryDuration"),
        "Flag": ("type_bridge.attribute.flags", "_query_flag"),
        "Integer": ("type_bridge.attribute.integer", "_QueryInteger"),
        "Key": ("type_bridge.attribute.flags", "_QueryKey"),
        "Meta": ("type_bridge.attribute.flags", "_QueryMeta"),
        "Ordered": ("type_bridge.attribute.flags", "_QueryOrdered"),
        "String": ("type_bridge.attribute.string", "_QueryString"),
        "TypeFlags": ("type_bridge.attribute.flags", "_QueryTypeFlags"),
        "TypeNameCase": ("type_bridge.attribute.flags", "_QueryTypeNameCase"),
        "Unique": ("type_bridge.attribute.flags", "_QueryUnique"),
    },
    "type_bridge.models": {
        "Entity": ("type_bridge.models.entity", "_QueryEntity"),
        "ModelRegistry": ("type_bridge.models.registry", "_QueryModelRegistry"),
        "Relation": ("type_bridge.models.relation", "_QueryRelation"),
        "Role": ("type_bridge.models.role", "_QueryRole"),
        "SchemaScanner": ("type_bridge.models.schema_scanner", "_QuerySchemaScanner"),
        "TypeDBType": ("type_bridge.models.base", "_QueryTypeDBType"),
    },
    "type_bridge.fields": {
        "FieldDescriptor": ("type_bridge.fields.base", "_QueryFieldDescriptor"),
        "FieldRef": ("type_bridge.fields.base", "_QueryFieldRef"),
        "NumericFieldRef": ("type_bridge.fields.base", "_QueryNumericFieldRef"),
        "OrderedFieldRef": ("type_bridge.fields.base", "_QueryOrderedFieldRef"),
        "RolePlayerFieldRef": ("type_bridge.fields.role", "_QueryRolePlayerFieldRef"),
        "RolePlayerNumericFieldRef": (
            "type_bridge.fields.role",
            "_QueryRolePlayerNumericFieldRef",
        ),
        "RolePlayerStringFieldRef": (
            "type_bridge.fields.role",
            "_QueryRolePlayerStringFieldRef",
        ),
        "RoleRef": ("type_bridge.fields.role", "_QueryRoleRef"),
        "StringFieldRef": ("type_bridge.fields.base", "_QueryStringFieldRef"),
    },
    "type_bridge.migration": {
        "Migration": ("type_bridge.migration._archive_base", "_ArchivedMigration"),
        "MigrationDependency": (
            "type_bridge.migration._archive_base",
            "_ArchivedMigrationDependency",
        ),
        "operations": ("type_bridge.migration._operations", None),
        "ref": ("type_bridge.migration._ref", None),
    },
}


@contextmanager
def archive_import_context() -> Iterator[None]:
    """Resolve frozen authoring imports only for this execution context."""
    token = _ARCHIVE_IMPORT_ACTIVE.set(True)
    try:
        yield
    finally:
        _ARCHIVE_IMPORT_ACTIVE.reset(token)


def archive_attribute(module_name: str, name: str) -> Any:
    """Return one frozen-source alias or raise ordinary ``AttributeError``."""
    if not _ARCHIVE_IMPORT_ACTIVE.get():
        raise AttributeError(f"module {module_name!r} has no attribute {name!r}")
    target = _ATTRIBUTE_ALIASES.get(module_name, {}).get(name)
    if target is None:
        raise AttributeError(f"module {module_name!r} has no attribute {name!r}")
    target_module, target_name = target
    module = import_module(target_module)
    return module if target_name is None else getattr(module, target_name)


def archive_builtins() -> Mapping[str, Any]:
    """Return module builtins with exact retired module paths redirected."""
    namespace = vars(builtins).copy()
    regular_import = builtins.__import__

    def archive_import(
        name: str,
        globals: Mapping[str, Any] | None = None,
        locals: Mapping[str, Any] | None = None,
        fromlist: tuple[str, ...] | list[str] = (),
        level: int = 0,
    ) -> Any:
        target = _MODULE_ALIASES.get(name) if level == 0 else None
        if target is None:
            return regular_import(name, globals, locals, fromlist, level)
        return regular_import(target, globals, locals, fromlist, 0)

    namespace["__import__"] = archive_import
    return namespace
