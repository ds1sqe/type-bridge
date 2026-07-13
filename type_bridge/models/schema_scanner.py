"""Schema scanner for inspecting and configuring TypeDB models."""

from typing import Any, get_origin, get_type_hints

from pydantic import Field

from type_bridge.attribute import Attribute, AttributeFlags
from type_bridge.models.role import Role
from type_bridge.models.utils import ModelAttrInfo, extract_metadata


class SchemaScanner:
    """Helper to inspect and configure TypeDB model classes (Entity/Relation)."""

    def __init__(self, cls: type):
        self.cls = cls

    def scan_attributes(self, is_relation: bool = False) -> dict[str, ModelAttrInfo]:
        """Scan class annotations for owned attributes.

        Modifies the class annotations in-place to ensure Pydantic compatibility.
        """
        owned_attrs: dict[str, ModelAttrInfo] = {}

        # Get direct annotations from this class
        direct_annotations = set(getattr(self.cls, "__annotations__", {}).keys())

        # Also include annotations from base=True parent classes
        # (they don't appear in TypeDB schema, so child must own their attributes)
        # Stop when we hit a non-base Model class
        # Note: cls.__mro__ includes cls itself, then parents.
        # We want parents only.

        # Determine the base class to stop at
        from type_bridge.models.entity import Entity
        from type_bridge.models.relation import Relation

        base_model_cls = Relation if is_relation else Entity

        for base in self.cls.__mro__[1:]:
            if base is base_model_cls or not issubclass(base, base_model_cls):
                continue
            if hasattr(base, "_flags") and base._flags.base:
                base_annotations = getattr(base, "__annotations__", {})
                direct_annotations.update(base_annotations.keys())
            else:
                break

        hints: dict[str, Any]
        try:
            # Use include_extras=True to preserve Annotated metadata
            all_hints = get_type_hints(self.cls, include_extras=True)
            # Filter to only include direct annotations and base=True parent annotations
            hints = {k: v for k, v in all_hints.items() if k in direct_annotations}
        except Exception:
            hints = {
                k: v
                for k, v in getattr(self.cls, "__annotations__", {}).items()
                if k in direct_annotations
            }

        new_annotations = {}

        # If relation, we need to know about roles to skip them
        role_names = getattr(self.cls, "_roles", {}).keys() if is_relation else set()

        for field_name, field_type in hints.items():
            if field_name.startswith("_") or field_name == "flags":
                new_annotations[field_name] = field_type
                continue

            if is_relation and field_name in role_names:
                new_annotations[field_name] = field_type
                continue

            # Get default value
            default_value = getattr(self.cls, field_name, None)

            # Extract metadata
            field_info = extract_metadata(field_type)
            field_origin = get_origin(field_type)
            is_list_type = field_origin is list

            # Check if it's an Attribute type
            if field_info.attr_type is not None:
                # Validation logic
                if is_list_type and not isinstance(default_value, AttributeFlags):
                    raise TypeError(
                        f"Field '{field_name}' in {self.cls.__name__}: "
                        f"list[Type] annotations must use Flag(Card(...)) or Flag(Ordered)."
                    )

                if isinstance(default_value, AttributeFlags):
                    flags = default_value
                    if flags.has_explicit_card and not is_list_type:
                        raise TypeError(
                            f"Field '{field_name}' in {self.cls.__name__}: "
                            f"Flag(Card(...)) can only be used with list[Type]."
                        )
                    if flags.is_ordered and not is_list_type:
                        raise TypeError(
                            f"Field '{field_name}' in {self.cls.__name__}: "
                            f"Flag(Ordered) declares a list attribute and requires a "
                            f"list[Type] annotation."
                        )
                    if is_list_type and not (flags.has_explicit_card or flags.is_ordered):
                        raise TypeError(
                            f"Field '{field_name}' in {self.cls.__name__}: "
                            f"list[Type] annotations must use Flag(Card(...)) or Flag(Ordered)."
                        )

                    if flags.card_min is None and flags.card_max is None:
                        flags.card_min = field_info.card_min
                        flags.card_max = field_info.card_max
                    if field_info.is_key:
                        flags.is_key = True
                    if field_info.is_unique:
                        flags.is_unique = True
                else:
                    flags = AttributeFlags(
                        is_key=field_info.is_key,
                        is_unique=field_info.is_unique,
                        card_min=field_info.card_min,
                        card_max=field_info.card_max,
                    )

                owned_attrs[field_name] = ModelAttrInfo(typ=field_info.attr_type, flags=flags)
                new_annotations[field_name] = field_type
            else:
                new_annotations[field_name] = field_type

        self.cls.__annotations__ = new_annotations

        # Replace AttributeFlags sentinels with real constructor defaults.
        # Flag(...) is a declaration-only marker: it must never survive as a
        # Pydantic default, or instances construct with the sentinel as value.
        for field_name, attr_info in owned_attrs.items():
            existing_default = self.cls.__dict__.get(field_name, None)

            # List fields (Card(...) or Ordered) default to an empty list
            if attr_info.flags.has_explicit_card or attr_info.flags.is_ordered:
                if isinstance(existing_default, AttributeFlags):
                    setattr(self.cls, field_name, Field(default_factory=list))
            # Optional single-value fields need default=None
            elif attr_info.flags.card_min == 0:
                if not isinstance(existing_default, Attribute):
                    setattr(self.cls, field_name, Field(default=None))
            # Required single-value fields must stay required: drop the
            # sentinel so Pydantic raises "Field required" on construction
            elif isinstance(existing_default, AttributeFlags):
                delattr(self.cls, field_name)

        # Also fix inherited fields from parent classes. __pydantic_init_subclass__
        # sets FieldDescriptor on parent class attributes, which Pydantic would
        # otherwise pick up as the child field's default — making required fields
        # optional and leaking FieldRef sentinels. Mirror the parent's real
        # FieldInfo (built before descriptor injection) onto the child instead.
        # MRO order: the nearest base wins; setattr adds the field to
        # self.cls.__dict__, so farther bases are skipped by the guard below.
        for base in self.cls.__mro__[1:]:
            if base is base_model_cls or not issubclass(base, base_model_cls):
                continue
            if hasattr(base, "_owned_attrs"):
                for field_name in base._owned_attrs:
                    if field_name in self.cls.__dict__:
                        continue
                    parent_field = base.model_fields.get(field_name)
                    if parent_field is None:
                        continue
                    if parent_field.is_required():
                        setattr(self.cls, field_name, Field())
                    elif parent_field.default_factory is not None:
                        setattr(
                            self.cls,
                            field_name,
                            Field(default_factory=parent_field.default_factory),
                        )
                    else:
                        setattr(self.cls, field_name, Field(default=parent_field.default))

        return owned_attrs

    def scan_roles(self) -> dict[str, Role]:
        """Scan class for Role definitions (Relation only)."""
        roles = {}
        annotations = getattr(self.cls, "__annotations__", {})
        try:
            all_hints = get_type_hints(self.cls, include_extras=True)
            hints = {key: all_hints.get(key, hint) for key, hint in annotations.items()}
        except Exception:
            hints = annotations

        for key, hint in hints.items():
            if not key.startswith("_") and key != "flags":
                origin = get_origin(hint)
                if origin is Role:
                    value = self.cls.__dict__.get(key)
                    if isinstance(value, Role):
                        if value.attr_name is None:
                            value.__set_name__(self.cls, key)
                        roles[key] = value
                elif hint is Role:
                    value = self.cls.__dict__.get(key)
                    role = value if isinstance(value, Role) else Role(key)
                    if role.attr_name is None:
                        role.__set_name__(self.cls, key)
                    roles[key] = role
        return roles
