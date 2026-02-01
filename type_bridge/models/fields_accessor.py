"""Fields accessor for TypeDB models."""

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType


class FieldsAccessor:
    """
    Accessor for model fields that provides FieldRef/RoleRef objects for query building.

    This replaces the magic __getattribute__ interception on model classes.
    Usage:
        Person.c.age.gt(30)
        Employment.c.employee.eq(alice)
    """

    def __init__(self, model_cls: type["TypeDBType"]):
        self._model_cls = model_cls

    def __getattr__(self, name: str) -> Any:
        # Check owned attributes (Entity/Relation)
        # We access _owned_attrs directly from the class
        if hasattr(self._model_cls, "_owned_attrs"):
            owned_attrs = getattr(self._model_cls, "_owned_attrs")
            if name in owned_attrs:
                from type_bridge.fields import FieldDescriptor

                attr_info = owned_attrs[name]
                # Create descriptor and get class-level value (FieldRef)
                descriptor = FieldDescriptor(field_name=name, attr_type=attr_info.typ)
                return descriptor.__get__(None, self._model_cls)

        # Check roles (Relation)
        if hasattr(self._model_cls, "_roles"):
            roles = getattr(self._model_cls, "_roles")
            if name in roles:
                from type_bridge.fields.role import RoleRef

                role = roles[name]
                return RoleRef(
                    role_name=role.role_name,
                    player_types=role.player_entity_types,
                )

        raise AttributeError(f"'{self._model_cls.__name__}' has no field or role '{name}'")
