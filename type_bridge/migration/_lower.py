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


def lower_validation_graph(loaded: Sequence[LoadedMigration]) -> dict[str, Any]:
    """Lower loaded migrations into the graph shape needed for validation.

    Dependency and checksum validation only needs migration metadata, not
    executable operations. This keeps validation available for Python-only
    operations such as ``ops.RunPython`` that cannot be serialized into Rust
    ``OperationSpec`` values.
    """
    migrations: list[dict[str, Any]] = []
    for item in loaded:
        migration = item.migration
        migrations.append(
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
                "operations": [],
                "checksum": item.checksum,
                "reversible": migration.reversible,
            }
        )
    return _rust_runtime.normalize_migration_graph({"migrations": migrations})


def lower_execution_migration(loaded: LoadedMigration) -> dict[str, Any]:
    """Lower one loaded migration into the Rust migration IR used for execution.

    Generated migrations normally carry a JSON sidecar.  When present, that
    sidecar is the executable contract and is returned after Rust serde
    normalization.  Legacy or hand-authored ``.py`` files fall back to the same
    structured lowering as :func:`lower_migration`, preserving typed
    ``OperationSpec`` variants so Rust remains the execution-lowering source of
    truth.  Explicit ``ops.RunTypeQL`` operations still lower to ``run_typeql``.
    """
    if loaded.execution_spec is not None:
        return _rust_runtime.normalize_migration_spec(loaded.execution_spec)

    return lower_migration(loaded.migration, checksum=loaded.checksum)


def lower_execution_graph(loaded: Sequence[LoadedMigration]) -> dict[str, Any]:
    """Lower loaded migrations into a Rust-normalized execution graph."""
    return _rust_runtime.normalize_migration_graph(
        {"migrations": [lower_execution_migration(migration) for migration in loaded]}
    )


def _operation_spec(operation: ops.Operation) -> dict[str, Any]:
    if isinstance(operation, ops.AddAttribute):
        return {
            "kind": "add_attribute",
            "attribute": _attribute_entry(_attribute_class(operation.attribute)),
        }
    if isinstance(operation, ops.RemoveAttribute):
        return {
            "kind": "remove_attribute",
            "attr_name": _attribute_name(operation.attribute),
        }
    if isinstance(operation, ops.AddEntity):
        entity = _model_class(operation.entity)
        return {
            "kind": "add_entity",
            "entity": _schema_info_for_models([entity])["entities"][entity.get_type_name()],
        }
    if isinstance(operation, ops.RemoveEntity):
        return {
            "kind": "remove_entity",
            "type_name": _type_name(operation.entity),
        }
    if isinstance(operation, ops.AddOwnership):
        return {
            "kind": "add_ownership",
            "owner_type": operation.owner.get_type_name(),
            "attribute": _owned_attribute_entry(
                _attribute_class(operation.attribute),
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
            "owner_type": _type_name(operation.owner),
            "attr_name": _attribute_name(operation.attribute),
        }
    if isinstance(operation, ops.ModifyOwnership):
        return {
            "kind": "modify_ownership",
            "owner_type": _type_name(operation.owner),
            "attr_name": _attribute_name(operation.attribute),
            "old_annotations": operation.old_annotations,
            "new_annotations": operation.new_annotations,
        }
    if isinstance(operation, ops.ModifyTypeAnnotations):
        return {
            "kind": "modify_type_annotations",
            "type_name": _subject_name(operation.subject),
            "old_doc": operation.old_doc,
            "new_doc": operation.new_doc,
            "old_meta": dict(operation.old_meta),
            "new_meta": dict(operation.new_meta),
        }
    if isinstance(operation, ops.ModifyRoleAnnotations):
        return {
            "kind": "modify_role_annotations",
            "relation_type": _type_name(operation.relation),
            "role_name": operation.role_name,
            "old_doc": operation.old_doc,
            "new_doc": operation.new_doc,
            "old_meta": dict(operation.old_meta),
            "new_meta": dict(operation.new_meta),
        }
    if isinstance(operation, ops.AddRelation):
        relation = _model_class(operation.relation)
        return {
            "kind": "add_relation",
            "relation": _schema_info_for_models([relation])["relations"][relation.get_type_name()],
        }
    if isinstance(operation, ops.RemoveRelation):
        return {
            "kind": "remove_relation",
            "type_name": _type_name(operation.relation),
        }
    if isinstance(operation, ops.AddRole):
        return {
            "kind": "add_role",
            "relation_type": _type_name(operation.relation),
            "role": {
                "role_name": operation.role_name,
                "player_type_names": operation.player_types,
                "cardinality": None,
            },
        }
    if isinstance(operation, ops.RemoveRole):
        return {
            "kind": "remove_role",
            "relation_type": _type_name(operation.relation),
            "role_name": operation.role_name,
        }
    if isinstance(operation, ops.AddRolePlayer):
        return {
            "kind": "add_role_player",
            "relation_type": _type_name(operation.relation),
            "role_name": operation.role_name,
            "player_type_name": operation.player_type,
        }
    if isinstance(operation, ops.RemoveRolePlayer):
        return {
            "kind": "remove_role_player",
            "relation_type": _type_name(operation.relation),
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
    if isinstance(operation, ops.CopyAttribute):
        # The executor runs the carried TypeQL (invariant 2: a single TypeQL
        # source); the structured fields ride along so the op stays portable
        # through the offline authoring surface, whose Rust synthesis is
        # pinned byte-identical to `to_typeql()` by a parity test.
        return {
            "kind": "copy_attribute",
            "owner": _type_name(operation.owner),
            "source": operation.source,
            "dest": operation.dest,
            "filter": operation.filter,
            "forward": operation.to_typeql(),
            "reverse": operation.to_rollback_typeql(),
        }
    raise TypeError(f"Unsupported migration operation type: {type(operation).__name__}")


def _schema_info_for_models(models: Sequence[type[Any]]) -> dict[str, Any]:
    """Lower a model list to a Rust SchemaInfo dict via the registry path.

    Rust ``SchemaInfo::from_descriptors`` handles plays_cardinalities overlays
    (from each role's ``plays_cardinality`` field) and foreign parent_type nulling.
    The attributes section is merged on the Python side to preserve full attribute-class
    metadata (regex, range, allowed_values, etc.) not represented in the descriptor layer.
    """
    from type_bridge.models import Relation

    registry = _rust_runtime.rust_core().PyDescriptorRegistry()
    for model in models:
        if not isinstance(model, type):
            raise TypeError(
                "Schema-bearing migration operations require full model classes "
                "when no sidecar execution spec is present"
            )
        descriptor = _rust_runtime.descriptor_for_model(model)
        if issubclass(model, Relation):
            registry.register_relation(descriptor)
        else:
            registry.register_entity(descriptor)
    schema = registry.schema_info()
    for model in models:
        for attr_info in model.get_all_attributes().values():
            entry = _rust_runtime.attribute_schema_entry(attr_info.typ)
            schema["attributes"][entry["attr_name"]] = entry
    return schema


def _attribute_entry(attribute: type[Any]) -> dict[str, Any]:
    return _rust_runtime.attribute_schema_entry(attribute)


def _model_class(value: object) -> type[Any]:
    if not isinstance(value, type):
        raise TypeError(
            "Schema-bearing migration operations require full model classes "
            "when no sidecar execution spec is present"
        )
    return value


def _attribute_class(value: object) -> type[Any]:
    if not isinstance(value, type):
        raise TypeError(
            "Schema-bearing migration operations require full attribute classes "
            "when no sidecar execution spec is present"
        )
    return value


def _subject_name(value: object) -> str:
    """Resolve a type name from an entity/relation model, attribute class, or ref."""
    if getattr(value, "get_type_name", None) is not None:
        return _type_name(value)
    return _attribute_name(value)


def _type_name(value: object) -> str:
    get_type_name = getattr(value, "get_type_name", None)
    if get_type_name is None:
        raise TypeError(f"{value!r} is not a TypeBridge type or migration type ref")
    return str(get_type_name())


def _attribute_name(value: object) -> str:
    get_attribute_name = getattr(value, "get_attribute_name", None)
    if get_attribute_name is None:
        raise TypeError(f"{value!r} is not a TypeBridge attribute or migration attribute ref")
    return str(get_attribute_name())


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
