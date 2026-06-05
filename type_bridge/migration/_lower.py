"""Internal lowering from Python migrations to Rust migration IR."""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

from type_bridge import _rust_runtime
from type_bridge.migration import operations as ops
from type_bridge.migration.base import Migration
from type_bridge.migration.loader import LoadedMigration


def lower_operation(operation: ops.Operation) -> dict[str, Any]:
    """Lower one Python operation object into a Rust-normalized operation spec."""
    spec = _operation_spec(operation)
    normalized = _rust_runtime.normalize_migration_spec(
        {
            "app_label": "_lower",
            "name": "operation",
            "dependencies": [],
            "operations": [spec],
            "checksum": None,
            "reversible": operation.reversible,
        }
    )
    return normalized["operations"][0]


def lower_migration(migration: Migration, *, checksum: str | None = None) -> dict[str, Any]:
    """Lower a Python ``Migration`` instance into a Rust-normalized spec dict."""
    operations: list[dict[str, Any]] = []
    if migration.models:
        operations.append(
            {
                "kind": "define_schema",
                "schema": _schema_info_for_models(migration.models),
            }
        )
    operations.extend(_operation_spec(operation) for operation in migration.operations)

    return _rust_runtime.normalize_migration_spec(
        {
            "app_label": migration.app_label,
            "name": migration.name,
            "dependencies": [
                {
                    "app_label": dependency.app_label,
                    "migration_name": dependency.migration_name,
                }
                for dependency in migration.get_dependencies()
            ],
            "operations": operations,
            "checksum": checksum,
            "reversible": migration.reversible,
        }
    )


def lower_loaded_migration(loaded: LoadedMigration) -> dict[str, Any]:
    """Lower a loaded migration and carry its file checksum."""
    return lower_migration(loaded.migration, checksum=loaded.checksum)


def lower_migration_graph(loaded: Sequence[LoadedMigration]) -> dict[str, Any]:
    """Lower loaded migrations into an ordered Rust-normalized graph dict."""
    return _rust_runtime.normalize_migration_graph(
        {"migrations": [lower_loaded_migration(migration) for migration in loaded]}
    )


def _operation_spec(operation: ops.Operation) -> dict[str, Any]:
    if isinstance(operation, ops.AddAttribute):
        return {
            "kind": "add_attribute",
            "attribute": _attribute_entry(operation.attribute),
        }
    if isinstance(operation, ops.RemoveAttribute):
        return {
            "kind": "remove_attribute",
            "attr_name": operation.attribute.get_attribute_name(),
        }
    if isinstance(operation, ops.AddEntity):
        return {
            "kind": "add_entity",
            "entity": _schema_info_for_models([operation.entity])["entities"][
                operation.entity.get_type_name()
            ],
        }
    if isinstance(operation, ops.RemoveEntity):
        return {
            "kind": "remove_entity",
            "type_name": operation.entity.get_type_name(),
        }
    if isinstance(operation, ops.AddOwnership):
        return {
            "kind": "add_ownership",
            "owner_type": operation.owner.get_type_name(),
            "attribute": _owned_attribute_entry(
                operation.attribute,
                optional=operation.optional,
                key=operation.key,
                unique=operation.unique,
                card_min=operation.card_min,
                card_max=operation.card_max,
            ),
        }
    if isinstance(operation, ops.RemoveOwnership):
        return {
            "kind": "remove_ownership",
            "owner_type": operation.owner.get_type_name(),
            "attr_name": operation.attribute.get_attribute_name(),
        }
    if isinstance(operation, ops.ModifyOwnership):
        return {
            "kind": "modify_ownership",
            "owner_type": operation.owner.get_type_name(),
            "attr_name": operation.attribute.get_attribute_name(),
            "old_annotations": operation.old_annotations,
            "new_annotations": operation.new_annotations,
        }
    if isinstance(operation, ops.AddRelation):
        return {
            "kind": "add_relation",
            "relation": _schema_info_for_models([operation.relation])["relations"][
                operation.relation.get_type_name()
            ],
        }
    if isinstance(operation, ops.RemoveRelation):
        return {
            "kind": "remove_relation",
            "type_name": operation.relation.get_type_name(),
        }
    if isinstance(operation, ops.AddRole):
        return {
            "kind": "add_role",
            "relation_type": operation.relation.get_type_name(),
            "role": {
                "role_name": operation.role_name,
                "player_type_names": operation.player_types,
                "cardinality": None,
            },
        }
    if isinstance(operation, ops.RemoveRole):
        return {
            "kind": "remove_role",
            "relation_type": operation.relation.get_type_name(),
            "role_name": operation.role_name,
        }
    if isinstance(operation, ops.AddRolePlayer):
        return {
            "kind": "add_role_player",
            "relation_type": operation.relation.get_type_name(),
            "role_name": operation.role_name,
            "player_type_name": operation.player_type,
        }
    if isinstance(operation, ops.RemoveRolePlayer):
        return {
            "kind": "remove_role_player",
            "relation_type": operation.relation.get_type_name(),
            "role_name": operation.role_name,
            "player_type_name": operation.player_type,
        }
    if isinstance(operation, ops.RunTypeQL):
        return {
            "kind": "run_typeql",
            "forward": operation.forward,
            "reverse": operation.reverse,
        }
    if isinstance(operation, ops.RenameAttribute):
        return {
            "kind": "rename_attribute",
            "old_name": operation.old_name,
            "new_name": operation.new_name,
            "value_type": operation.value_type,
        }
    raise TypeError(f"Unsupported migration operation type: {type(operation).__name__}")


def _schema_info_for_models(models: Sequence[type[Any]]) -> dict[str, Any]:
    from type_bridge.models import Relation

    registry = _rust_runtime.rust_core().PyDescriptorRegistry()
    for model in models:
        descriptor = _rust_runtime.descriptor_for_model(model)
        if issubclass(model, Relation):
            registry.register_relation(descriptor)
        else:
            registry.register_entity(descriptor)
    return registry.schema_info()


def _attribute_entry(attribute: type[Any]) -> dict[str, Any]:
    return {
        "attr_name": attribute.get_attribute_name(),
        "value_type": _rust_runtime.rust_value_type(attribute),
    }


def _owned_attribute_entry(
    attribute: type[Any],
    *,
    optional: bool,
    key: bool,
    unique: bool,
    card_min: int | None,
    card_max: int | None,
) -> dict[str, Any]:
    return {
        **_attribute_entry(attribute),
        "annotations": _ownership_annotations(
            optional=optional,
            key=key,
            unique=unique,
            card_min=card_min,
            card_max=card_max,
        ),
    }


def _ownership_annotations(
    *,
    optional: bool,
    key: bool,
    unique: bool,
    card_min: int | None,
    card_max: int | None,
) -> list[Any]:
    if key:
        return ["Key"]
    if unique:
        return ["Unique"]
    if card_min is not None or card_max is not None:
        return [{"Card": [card_min if card_min is not None else 0, card_max]}]
    if optional:
        return [{"Card": [0, 1]}]
    return []
