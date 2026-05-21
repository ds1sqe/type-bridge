"""Python helpers for the experimental Rust ORM backend."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from functools import lru_cache
from typing import TYPE_CHECKING, Any

import isodate  # pyright: ignore[reportMissingModuleSource]

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType


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
        import type_bridge_core  # type: ignore[import-not-found]
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
        return registry.register_relation(descriptor)
    return registry.register_entity(descriptor)


def entity_descriptor(model_cls: type[TypeDBType]) -> dict[str, Any]:
    """Build an entity descriptor dict from Python class metadata."""
    return {
        "type_name": model_cls.get_type_name(),
        "is_abstract": model_cls.is_abstract(),
        "parent_type": model_cls.get_supertype(),
        "owned_attributes": attribute_descriptors(model_cls),
    }


def relation_descriptor(model_cls: type[TypeDBType]) -> dict[str, Any]:
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


def _relation_roles(model_cls: type[TypeDBType]) -> list[Any]:
    roles = list(model_cls.get_roles().values())  # type: ignore[attr-defined]
    if roles:
        return [
            _RoleMetadata(
                role_name=role.role_name,
                player_types=role.player_entity_types,
                cardinality=role.cardinality,
            )
            for role in roles
        ]

    fallback_roles = []
    for field in getattr(model_cls, "model_fields", {}).values():
        default = getattr(field, "default", None)
        role_name = getattr(default, "role_name", None)
        player_types = getattr(default, "player_types", None)
        if role_name is None or player_types is None:
            continue
        fallback_roles.append(
            _RoleMetadata(role_name=role_name, player_types=player_types, cardinality=None)
        )
    return fallback_roles


def attribute_descriptors(model_cls: type[TypeDBType]) -> list[dict[str, Any]]:
    """Build Rust owned-attribute descriptors from Python model metadata."""
    descriptors = []
    for field_name, attr_info in model_cls.get_all_attributes().items():
        value_type = rust_value_type(attr_info.typ)
        descriptors.append(
            {
                "field_name": field_name,
                "attr_name": attr_info.typ.get_attribute_name(),
                "value_type": value_type,
                "annotations": _annotations(attr_info.flags),
                "is_optional": _is_optional(attr_info.flags),
            }
        )
    return descriptors


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
    from type_bridge.session import Database

    if not isinstance(connection, Database):
        raise NotImplementedError(
            "TYPE_BRIDGE_BACKEND=rust currently supports Database connections only; "
            "Transaction and TransactionContext parity is Phase 3 scope"
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


def rust_manager_for_entity(connection: Any, descriptor: dict[str, Any]) -> Any:
    """Create a PyO3 dynamic entity manager."""
    return rust_core().PyDynamicEntityManager(rust_database_for(connection), descriptor)


def rust_manager_for_relation(connection: Any, descriptor: dict[str, Any]) -> Any:
    """Create a PyO3 dynamic relation manager."""
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
        if role_name is None or player_types is None:
            continue
        fields.append(
            (
                field_name,
                _RoleMetadata(role_name=role_name, player_types=player_types, cardinality=None),
            )
        )
    return fields


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
