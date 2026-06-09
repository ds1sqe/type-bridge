"""Python helpers for the experimental Rust ORM backend."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from functools import lru_cache
from typing import TYPE_CHECKING, Any

import isodate

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType
    from type_bridge.models.relation import Relation


@dataclass(frozen=True)
class _RoleMetadata:
    role_name: str
    player_types: tuple[type[Any], ...]
    cardinality: Any


PYTHON_TO_RUST_VALUE_TYPE = {
    "string": "string",
    "integer": "long",
    "long": "long",
    "double": "double",
    "boolean": "boolean",
    "date": "date",
    "datetime": "datetime",
    "datetime-tz": "datetime-tz",
    "decimal": "decimal",
    "duration": "duration",
}


def rust_core() -> Any:
    """Import the optional Rust extension or raise a backend-specific error."""
    try:
        import type_bridge_core
    except ImportError as exc:
        raise RuntimeError(
            "TYPE_BRIDGE_BACKEND=rust requires the type_bridge_core Rust extension"
        ) from exc
    return type_bridge_core


@lru_cache(maxsize=1)
def descriptor_registry() -> Any:
    """Return the process-local PyO3 descriptor registry wrapper."""
    return rust_core().PyDescriptorRegistry()


def descriptor_for_model(model_cls: type[TypeDBType]) -> dict[str, Any]:
    """Translate a Python Entity/Relation class into a Rust descriptor dict."""
    from type_bridge.models import Relation

    if issubclass(model_cls, Relation):
        return relation_descriptor(model_cls)
    return entity_descriptor(model_cls)


def register_model_descriptor(model_cls: type[TypeDBType]) -> dict[str, Any]:
    """Register and return the canonical Rust descriptor for a Python model class."""
    from type_bridge.models import Relation

    registry = descriptor_registry()
    descriptor = descriptor_for_model(model_cls)
    if issubclass(model_cls, Relation):
        return _register_or_project_descriptor(
            descriptor,
            register=registry.register_relation,
            lookup=registry.relation,
        )
    return _register_or_project_descriptor(
        descriptor,
        register=registry.register_entity,
        lookup=registry.entity,
    )


def model_schema_info() -> dict[str, Any]:
    """Schema-IR view of the registered models for the migration diff/introspection path."""
    return descriptor_registry().schema_info()


def compute_schema_diff(current: dict[str, Any], target: dict[str, Any]) -> dict[str, Any]:
    """Compute schema diff through the Rust schema engine."""
    return rust_core().compute_schema_diff(current, target)


def classify_schema_diff(diff: dict[str, Any]) -> list[dict[str, Any]]:
    """Classify schema diff changes through the Rust schema engine."""
    return rust_core().classify_schema_diff(diff)


def schema_diff_is_breaking(diff: dict[str, Any]) -> bool:
    """Return whether the Rust schema diff contains breaking changes."""
    return bool(rust_core().schema_diff_is_breaking(diff))


def generate_define_block(info: dict[str, Any]) -> str:
    """Generate TypeQL through the Rust schema generator."""
    return rust_core().generate_define_block(info)


def normalize_migration_spec(spec: dict[str, Any]) -> dict[str, Any]:
    """Normalize a migration spec dict through Rust serde."""
    return rust_core().normalize_migration_spec(spec)


def normalize_migration_graph(graph: dict[str, Any]) -> dict[str, Any]:
    """Normalize a migration graph dict through Rust serde."""
    return rust_core().normalize_migration_graph(graph)


def migration_spec_to_json(spec: dict[str, Any]) -> str:
    """Serialize a migration spec dict through Rust serde."""
    return rust_core().migration_spec_to_json(spec)


def migration_spec_from_json(json: str) -> dict[str, Any]:
    """Deserialize a migration spec JSON string through Rust serde."""
    return rust_core().migration_spec_from_json(json)


def migration_graph_to_json(graph: dict[str, Any]) -> str:
    """Serialize a migration graph dict through Rust serde."""
    return rust_core().migration_graph_to_json(graph)


def migration_graph_from_json(json: str) -> dict[str, Any]:
    """Deserialize a migration graph JSON string through Rust serde."""
    return rust_core().migration_graph_from_json(json)


def migration_file_checksum(content: str) -> str:
    """Calculate the migration-file checksum through Rust."""
    return rust_core().calculate_migration_file_checksum(content)


def load_migration_sidecar(py_path: str) -> dict[str, Any] | None:
    """Load the JSON sidecar for a migration .py path through Rust.

    Derives the sidecar path by replacing the .py extension with .json.
    Returns the deserialized MigrationSpec as a dict when a valid sidecar
    exists, or None when no sidecar is present.  Raises ValueError if the
    sidecar exists but cannot be read or deserialized.
    """
    return rust_core().load_migration_sidecar(py_path)


def validate_migration_graph(
    graph: dict[str, Any],
    applied_records: list[dict[str, Any]] | None = None,
) -> list[dict[str, Any]]:
    """Validate a migration graph through Rust."""
    return rust_core().validate_migration_graph(graph, applied_records)


def check_migration_drift(
    graph: dict[str, Any],
    applied_records: list[dict[str, Any]],
) -> None:
    """Fail if applied migration checksums drifted from the loaded graph."""
    rust_core().check_migration_drift(graph, applied_records)


def introspect_schema(connection: Any) -> dict[str, Any]:
    """Introspect the live TypeDB schema through the Rust schema manager."""
    return rust_database_for(connection).introspect_schema()


def schema_text(connection: Any) -> str:
    """Export the live TypeDB schema as TypeQL text through the Rust driver."""
    return rust_database_for(connection).schema_text()


def _register_or_project_descriptor(
    descriptor: dict[str, Any],
    *,
    register: Any,
    lookup: Any,
) -> dict[str, Any]:
    """Register a descriptor, or use a same-kind local projection on shape conflict.

    Integration tests and user code can define narrower local Python classes for
    the same TypeDB label. The shared Rust registry stays strict, while dynamic
    managers can still use the local descriptor directly because they receive an
    owned descriptor from PyO3 rather than a registry key.
    """
    try:
        return register(descriptor)
    except ValueError as exc:
        message = str(exc)
        if "descriptor shape differs from registered descriptor" not in message:
            raise
        lookup(descriptor["type_name"])
        return descriptor


def entity_descriptor(model_cls: type[TypeDBType]) -> dict[str, Any]:
    """Build an entity descriptor dict from Python class metadata."""
    return {
        "type_name": model_cls.get_type_name(),
        "is_abstract": model_cls.is_abstract(),
        "parent_type": model_cls.get_supertype(),
        "owned_attributes": attribute_descriptors(model_cls),
    }


def relation_descriptor(model_cls: type[Relation]) -> dict[str, Any]:
    """Build a relation descriptor dict from Python class metadata."""
    roles = []
    for role in _relation_roles(model_cls):
        roles.append(
            {
                "role_name": role.role_name,
                "player_type_names": [typ.get_type_name() for typ in role.player_types],
                "cardinality": _cardinality_tuple(role.cardinality),
            }
        )

    descriptor = entity_descriptor(model_cls)
    descriptor["roles"] = roles
    return descriptor


def _relation_roles(model_cls: type[Relation]) -> list[Any]:
    roles = list(model_cls.get_roles().values())
    if roles:
        return [
            _RoleMetadata(
                role_name=role.role_name,
                player_types=role.player_entity_types,
                cardinality=role.cardinality,
            )
            for role in roles
        ]

    # Only own-declared fields contribute roles. A subtype relation inherits its
    # parent's role fields through ``model_fields``, but those roles belong to the
    # declaring relation; emitting them here would scope a player's ``plays`` edge
    # to the subtype (e.g. ``plays collaboration:participant``) which TypeDB rejects.
    own_fields = model_cls.__dict__.get("__annotations__", {})
    fallback_roles = []
    for field_name, field in getattr(model_cls, "model_fields", {}).items():
        if field_name not in own_fields:
            continue
        default = getattr(field, "default", None)
        role_name = getattr(default, "role_name", None)
        player_types = getattr(default, "player_types", None)
        cardinality = getattr(default, "cardinality", None)
        if role_name is None or player_types is None:
            continue
        fallback_roles.append(
            _RoleMetadata(role_name=role_name, player_types=player_types, cardinality=cardinality)
        )
    return fallback_roles


def attribute_descriptors(model_cls: type[TypeDBType]) -> list[dict[str, Any]]:
    """Build Rust owned-attribute descriptors from Python model metadata."""
    descriptors = []
    for field_name, attr_info in model_cls.get_all_attributes().items():
        attr_entry = attribute_schema_entry(attr_info.typ)
        descriptors.append(
            {
                "field_name": field_name,
                "attr_name": attr_entry["attr_name"],
                "value_type": attr_entry["value_type"],
                "annotations": _annotations(attr_info.flags),
                "is_optional": _is_optional(attr_info.flags),
            }
        )
    return descriptors


def attribute_schema_entry(attr_cls: type[Any]) -> dict[str, Any]:
    """Build a Rust ``AttributeSchemaEntry`` dict from a Python Attribute class."""
    entry: dict[str, Any] = {
        "attr_name": attr_cls.get_attribute_name(),
        "value_type": rust_value_type(attr_cls),
    }

    parent_type = attr_cls.get_supertype()
    if parent_type is not None:
        entry["parent_type"] = parent_type
    if attr_cls.is_abstract():
        entry["is_abstract"] = True
    if attr_cls.is_independent():
        entry["is_independent"] = True

    regex_pattern = getattr(attr_cls, "regex_pattern", None)
    if isinstance(regex_pattern, str):
        entry["regex"] = regex_pattern

    allowed_values = getattr(attr_cls, "allowed_values", None)
    if isinstance(allowed_values, (tuple, list)):
        entry["allowed_values"] = [str(value) for value in allowed_values]

    range_constraint = getattr(attr_cls, "range_constraint", None)
    if range_constraint is not None:
        range_min, range_max = range_constraint
        entry["range"] = [_range_bound(range_min), _range_bound(range_max)]

    return entry


def _range_bound(value: Any) -> str | None:
    if value is None:
        return None
    return str(value)


def rust_value_type(attr_cls: type[Any]) -> str:
    """Return the Rust/TypeDB value type string for a Python Attribute class."""
    value_type = PYTHON_TO_RUST_VALUE_TYPE.get(attr_cls.get_value_type())
    if value_type is None:
        raise ValueError(
            f"Unsupported Rust backend attribute value type {attr_cls.get_value_type()!r}"
        )
    return value_type


def normalize_value(value: Any, value_type: str) -> Any:
    """Normalize Python values into JSON-safe Rust backend values."""
    if hasattr(value, "value"):
        value = value.value

    if value is None:
        return None
    if value_type == "date":
        if isinstance(value, datetime):
            return value.date().isoformat()
        if isinstance(value, date):
            return value.isoformat()
    if value_type in {"datetime", "datetime-tz"}:
        if isinstance(value, datetime):
            return value.isoformat()
    if value_type == "decimal":
        if isinstance(value, Decimal):
            return str(value)
    if value_type == "duration":
        if isinstance(value, str):
            return value
        if isinstance(value, timedelta):
            return isodate.duration_isoformat(value)
        return isodate.duration_isoformat(value)
    return value


def normalize_attributes(model_cls: type[TypeDBType], data: dict[str, Any]) -> dict[str, Any]:
    """Normalize field-name keyed model data for PyO3 manager calls."""
    attrs = model_cls.get_all_attributes()
    normalized: dict[str, Any] = {}
    for field_name, value in data.items():
        attr_info = attrs.get(field_name)
        if attr_info is None:
            continue
        value_type = rust_value_type(attr_info.typ)
        if isinstance(value, list):
            normalized[field_name] = [normalize_value(item, value_type) for item in value]
        else:
            normalized[field_name] = normalize_value(value, value_type)
    return normalized


def rust_database_for(connection: Any) -> Any:
    """Return or create a Rust database handle for a Python Database object."""
    from type_bridge.session import Database, TransactionContext

    if isinstance(connection, TransactionContext):
        return rust_database_for(connection.database)
    if not isinstance(connection, Database):
        raise NotImplementedError(
            "TYPE_BRIDGE_BACKEND=rust supports Database and TransactionContext connections; "
            "raw Python Transaction handles cannot be shared with the Rust backend"
        )

    cached = getattr(connection, "_rust_backend_database", None)
    if cached is not None:
        return cached

    rust_db = rust_core().PyRustDatabase.connect(
        connection.address,
        connection.database_name,
        connection.username or "admin",
        connection.password or "password",
    )
    setattr(connection, "_rust_backend_database", rust_db)
    return rust_db


def migration_runner_for(connection: Any) -> Any:
    """Return a PyMigrationRunner bound to the Rust database for a connection.

    The runner shares the connection's `Arc<Database>` and `Arc<Runtime>`, so
    migration execution runs on the same Rust connection as the rest of the ORM
    path — no second runtime is created.
    """
    return rust_core().PyMigrationRunner(rust_database_for(connection))


def state_manager_for(connection: Any) -> Any:
    """Return a PyMigrationStateManager bound to the Rust database.

    Resolves the unconfigured default migration-state backend to the
    TypeDB-backed manager built from the live `PyRustDatabase`, sharing the
    connection's `Arc<Database>` and `Arc<Runtime>` — the same default path as
    `migration_runner_for`.
    """
    return rust_core().PyMigrationStateManager(rust_database_for(connection))


def rust_transaction_for(connection: Any) -> Any | None:
    """Return the Rust transaction adapter for a Python TransactionContext."""
    from type_bridge.session import TransactionContext

    if not isinstance(connection, TransactionContext):
        return None

    rust_tx = getattr(connection, "_rust_tx", None)
    if rust_tx is None:
        raise RuntimeError("Rust transaction context is not active")
    return rust_tx


def rust_manager_for_entity(connection: Any, descriptor: dict[str, Any]) -> Any:
    """Create a PyO3 dynamic entity manager."""
    rust_tx = rust_transaction_for(connection)
    if rust_tx is not None:
        return rust_core().PyDynamicEntityManager.for_transaction(rust_tx, descriptor)
    return rust_core().PyDynamicEntityManager(rust_database_for(connection), descriptor)


def rust_manager_for_relation(connection: Any, descriptor: dict[str, Any]) -> Any:
    """Create a PyO3 dynamic relation manager."""
    rust_tx = rust_transaction_for(connection)
    if rust_tx is not None:
        return rust_core().PyDynamicRelationManager.for_transaction(rust_tx, descriptor)
    return rust_core().PyDynamicRelationManager(rust_database_for(connection), descriptor)


def role_player_inputs(instance: Any) -> list[dict[str, Any]]:
    """Build Rust dynamic relation role-player inputs from a Python relation."""
    inputs: list[dict[str, Any]] = []
    for field_name, role in _relation_role_fields(instance.__class__):
        value = getattr(instance, field_name, None)
        if value is None:
            continue
        players = value if isinstance(value, list) else [value]
        for player in players:
            player_type = player.__class__
            item = {
                "role_name": role.role_name,
                "player_type_name": player_type.get_type_name(),
            }
            iid = getattr(player, "_iid", None)
            if iid:
                item["iid"] = iid
            else:
                key = key_filter_for_entity(player)
                if key is None:
                    raise ValueError(
                        f"Role player for role '{role.role_name}' needs _iid or a key attribute"
                    )
                item.update(key)
            inputs.append(item)
    return inputs


def _relation_role_fields(model_cls: type[Any]) -> list[tuple[str, _RoleMetadata]]:
    roles = model_cls.get_roles()
    if roles:
        return [
            (
                field_name,
                _RoleMetadata(
                    role_name=role.role_name,
                    player_types=role.player_entity_types,
                    cardinality=role.cardinality,
                ),
            )
            for field_name, role in roles.items()
        ]

    fields = []
    for field_name, field in getattr(model_cls, "model_fields", {}).items():
        default = getattr(field, "default", None)
        role_name = getattr(default, "role_name", None)
        player_types = getattr(default, "player_types", None)
        cardinality = getattr(default, "cardinality", None)
        if role_name is None or player_types is None:
            continue
        fields.append(
            (
                field_name,
                _RoleMetadata(
                    role_name=role_name, player_types=player_types, cardinality=cardinality
                ),
            )
        )
    return fields


def relation_role_fields(model_cls: type[Any]) -> list[tuple[str, _RoleMetadata]]:
    """Return Python relation field names with Rust backend role metadata."""
    return _relation_role_fields(model_cls)


def key_filter_for_entity(instance: Any) -> dict[str, Any] | None:
    """Return key fields for a role-player entity, if one is available."""
    for field_name, attr_info in instance.__class__.get_all_attributes().items():
        if not attr_info.flags.is_key:
            continue
        value = getattr(instance, field_name, None)
        if value is None:
            continue
        value_type = rust_value_type(attr_info.typ)
        return {
            "key_attr": attr_info.typ.get_attribute_name(),
            "key_value": normalize_value(value, value_type),
            "key_value_type": value_type,
        }
    return None


def _annotations(flags: Any) -> list[Any]:
    annotations: list[Any] = []
    if flags.is_key:
        annotations.append("Key")
    if flags.is_unique:
        annotations.append("Unique")

    should_emit_card = flags.card_min is not None or flags.card_max is not None
    default_unique_card = flags.is_unique and flags.card_min == 1 and flags.card_max == 1
    if should_emit_card and not flags.is_key and not default_unique_card:
        annotations.append(
            {"Card": [flags.card_min if flags.card_min is not None else 0, flags.card_max]}
        )
    return annotations


def _is_optional(flags: Any) -> bool:
    return flags.card_min == 0


def _cardinality_tuple(cardinality: Any) -> list[Any] | None:
    if cardinality is None:
        return None
    return [cardinality.min if cardinality.min is not None else 0, cardinality.max]
