"""Abstract base class for TypeDB entities and relations."""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Mapping
from typing import Any, ClassVar, dataclass_transform

from pydantic import BaseModel, ConfigDict, model_validator

from type_bridge.attribute import AttributeFlags, TypeFlags
from type_bridge.attribute.flags import format_type_name
from type_bridge.crud.utils import format_value as _format_value_impl
from type_bridge.models.utils import ModelAttrInfo, validate_type_name


@dataclass_transform(kw_only_default=True, field_specifiers=(AttributeFlags, TypeFlags))
class TypeDBType(BaseModel, ABC):
    """Abstract base class for TypeDB entities and relations.

    This class provides common functionality for both Entity and Relation types,
    including type name management, abstract/base flags, and attribute ownership.

    Subclasses must implement:
    - get_supertype(): Get parent type in TypeDB hierarchy
    - to_schema_definition(): Generate TypeQL schema definition
    - to_insert_query(): Generate TypeQL insert query for instances
    """

    # Pydantic configuration (inherited by Entity and Relation)
    model_config = ConfigDict(
        arbitrary_types_allowed=True,
        validate_assignment=True,
        extra="allow",
        revalidate_instances="always",
    )

    # Internal metadata (class-level)
    _flags: ClassVar[TypeFlags] = TypeFlags()
    _owned_attrs: ClassVar[dict[str, ModelAttrInfo]] = {}
    _iid: str | None = None  # TypeDB internal ID

    def __init_subclass__(cls) -> None:
        """Called when a TypeDBType subclass is created."""
        super().__init_subclass__()

        # Get TypeFlags if defined, otherwise create new default flags
        # Check if flags is defined directly on this class (not inherited)
        if "flags" in cls.__dict__ and isinstance(cls.__dict__["flags"], TypeFlags):
            # Explicitly set flags on this class
            cls._flags = cls.__dict__["flags"]
        else:
            # No explicit flags on this class - create new default flags
            # This ensures each subclass gets its own flags instance
            cls._flags = TypeFlags()

        # Validate type name doesn't conflict with TypeDB built-ins
        # Skip validation for:
        # 1. Base classes that won't appear in schema (base=True)
        # 2. The abstract base Entity and Relation classes themselves
        is_base_entity_or_relation = cls.__name__ in ("Entity", "Relation") and cls.__module__ in (
            "type_bridge.models",
            "type_bridge.models.entity",
            "type_bridge.models.relation",
        )
        if not cls._flags.base and not is_base_entity_or_relation:
            type_name = cls._flags.name or format_type_name(cls.__name__, cls._flags.case)

            # Determine context based on class hierarchy
            from typing import Literal

            from type_bridge.models.entity import Entity
            from type_bridge.models.relation import Relation

            if issubclass(cls, Relation):
                context: Literal["relation", "entity", "attribute", "role"] = "relation"
            elif issubclass(cls, Entity):
                context = "entity"
            else:
                # Default for TypeDBType subclasses
                context = "entity"

            validate_type_name(type_name, cls.__name__, context)

    @model_validator(mode="wrap")
    @classmethod
    def _wrap_raw_values(cls, values, handler):
        """Ensure all attribute fields are wrapped in Attribute instances.

        This catches edge cases like default values and model_copy that bypass validators.
        Uses 'wrap' mode to intercept all validation paths including model_copy.
        """
        # First, let Pydantic do its validation
        instance = handler(values)

        # Then wrap any raw values
        owned_attrs = cls.get_owned_attributes()
        for field_name, attr_info in owned_attrs.items():
            value = getattr(instance, field_name, None)
            flags = attr_info.flags
            attr_class = attr_info.typ

            # Check if the value is AttributeFlags (from Flag() default)
            # This happens when list fields with Flag(Card(...)) are not provided
            if isinstance(value, AttributeFlags):
                # For list fields (has_explicit_card), default to empty list
                if flags.has_explicit_card:
                    object.__setattr__(instance, field_name, [])
                    continue
                else:
                    # For single-value fields, this is an error
                    raise ValueError(
                        f"Field '{field_name}' received AttributeFlags as value. "
                        f"This usually means the field was not provided a value."
                    )

            if value is None:
                continue

            # Check if it's a list (multi-value attribute)
            if isinstance(value, list):
                wrapped_list = []
                for item in value:
                    if not isinstance(item, attr_class):
                        # Wrap raw value
                        wrapped_list.append(attr_class(item))
                    else:
                        wrapped_list.append(item)
                # Use object.__setattr__ to bypass validate_assignment and avoid recursion
                object.__setattr__(instance, field_name, wrapped_list)
            else:
                # Single value
                if not isinstance(value, attr_class):
                    # Wrap raw value
                    # Use object.__setattr__ to bypass validate_assignment and avoid recursion
                    object.__setattr__(instance, field_name, attr_class(value))

        return instance

    def model_copy(self, *, update: Mapping[str, Any] | None = None, deep: bool = False):
        """Override model_copy to ensure raw values are wrapped in Attribute instances.

        Pydantic's model_copy bypasses validators even with revalidate_instances='always',
        so we override it to force proper validation.
        """
        # Call parent model_copy
        copied = super().model_copy(update=update, deep=deep)

        # Force wrap any raw values in the update dict
        if update:
            owned_attrs = self.__class__.get_owned_attributes()
            for field_name, new_value in update.items():
                if field_name not in owned_attrs:
                    continue

                attr_info = owned_attrs[field_name]
                attr_class = attr_info.typ

                # Check if it's a list (multi-value attribute)
                if isinstance(new_value, list):
                    wrapped_list = []
                    for item in new_value:
                        if not isinstance(item, attr_class):
                            wrapped_list.append(attr_class(item))
                        else:
                            wrapped_list.append(item)
                    object.__setattr__(copied, field_name, wrapped_list)
                else:
                    # Single value
                    if not isinstance(new_value, attr_class):
                        object.__setattr__(copied, field_name, attr_class(new_value))

        return copied

    @classmethod
    def get_type_name(cls) -> str:
        """Get the TypeDB type name for this type.

        If name is explicitly set in TypeFlags, it is used as-is.
        Otherwise, the class name is formatted according to the case parameter.
        """
        if cls._flags.name:
            return cls._flags.name
        return format_type_name(cls.__name__, cls._flags.case)

    @classmethod
    def _get_base_type_class(cls) -> type[TypeDBType]:
        """Get the root base class for this type hierarchy.

        Override in subclasses to return Entity or Relation.
        Used by get_supertype() to correctly identify inheritance boundaries.

        Returns:
            The base type class (Entity or Relation)
        """
        return TypeDBType

    @classmethod
    def get_supertype(cls) -> str | None:
        """Get the supertype from Python inheritance, skipping base classes.

        Base classes (with base=True) are Python-only and don't appear in TypeDB schema.
        This method skips them when determining the TypeDB supertype.

        Returns:
            Type name of the parent class, or None if direct subclass
        """
        base_class = cls._get_base_type_class()
        for base in cls.__bases__:
            if base is not base_class and issubclass(base, base_class):
                # Skip base classes - they don't appear in TypeDB schema
                if base.is_base():
                    # Recursively find the first non-base parent
                    return base.get_supertype()
                return base.get_type_name()
        return None

    @classmethod
    def is_abstract(cls) -> bool:
        """Check if this is an abstract type."""
        return cls._flags.abstract

    @classmethod
    def is_base(cls) -> bool:
        """Check if this is a Python base class (not in TypeDB schema)."""
        return cls._flags.base

    @classmethod
    def get_owned_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Get attributes owned directly by this type (not inherited).

        Returns:
            Dictionary mapping field names to ModelAttrInfo (typ + flags)
        """
        return cls._owned_attrs.copy()

    @classmethod
    def get_all_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Get all attributes including inherited ones.

        Traverses the class hierarchy to collect all owned attributes,
        including those from parent Entity/Relation classes.

        Returns:
            Dictionary mapping field names to ModelAttrInfo (typ + flags)
        """
        all_attrs: dict[str, ModelAttrInfo] = {}

        # Traverse MRO in reverse to get parent attributes first
        # Child attributes will override parent attributes with same name
        for base in reversed(cls.__mro__):
            if hasattr(base, "_owned_attrs") and isinstance(base._owned_attrs, dict):
                all_attrs.update(base._owned_attrs)

        return all_attrs

    @classmethod
    def _build_owns_lines(cls) -> list[str]:
        """Build TypeQL 'owns' lines for schema definition.

        This is a shared helper used by both Entity and Relation to generate
        the attribute ownership part of their schema definitions.

        Returns:
            List of TypeQL 'owns' lines with proper formatting
        """
        lines = []
        for _field_name, attr_info in cls._owned_attrs.items():
            attr_class = attr_info.typ
            flags = attr_info.flags
            attr_name = attr_class.get_attribute_name()

            ownership = f"    owns {attr_name}"
            annotations = flags.to_typeql_annotations()
            if annotations:
                ownership += " " + " ".join(annotations)
            lines.append(ownership)
        return lines

    @classmethod
    @abstractmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this type.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        ...

    @abstractmethod
    def to_insert_query(self, var: str) -> str:
        """Generate TypeQL insert query for this instance.

        Args:
            var: Variable name to use

        Returns:
            TypeQL insert pattern
        """
        ...

    @staticmethod
    def _format_value(value: Any) -> str:
        """Format a Python value for TypeQL.

        Delegates to the shared format_value utility in crud/utils.py.
        """
        return _format_value_impl(value)
