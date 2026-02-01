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
                        f"list[Type] annotations must use Flag(Card(...))."
                    )

                if isinstance(default_value, AttributeFlags):
                    flags = default_value
                    if flags.has_explicit_card and not is_list_type:
                        raise TypeError(
                            f"Field '{field_name}' in {self.cls.__name__}: "
                            f"Flag(Card(...)) can only be used with list[Type]."
                        )
                    if is_list_type and not flags.has_explicit_card:
                        raise TypeError(
                            f"Field '{field_name}' in {self.cls.__name__}: "
                            f"list[Type] annotations must use Flag(Card(...))."
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
        
        # Set explicit None defaults for optional fields
        for field_name, attr_info in owned_attrs.items():
            existing_default = self.cls.__dict__.get(field_name, None)
            if attr_info.flags.card_min == 0 and not attr_info.flags.has_explicit_card:
                if not isinstance(existing_default, Attribute):
                    setattr(self.cls, field_name, Field(default=None))
                    
        return owned_attrs

    def scan_roles(self) -> dict[str, Role]:
        """Scan class for Role definitions (Relation only)."""
        roles = {}
        annotations = getattr(self.cls, "__annotations__", {})
        
        for key, hint in annotations.items():
            if not key.startswith("_") and key != "flags":
                origin = get_origin(hint)
                if origin is Role:
                    value = self.cls.__dict__.get(key)
                    if isinstance(value, Role):
                        roles[key] = value
        return roles
