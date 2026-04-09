"""Central registry for TypeDB models."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, TypeVar

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType
    from type_bridge.models.entity import Entity

logger = logging.getLogger(__name__)

T = TypeVar("T", bound="TypeDBType")


class ModelRegistry:
    """Registry for TypeDB model classes.

    Maps TypeDB type names to Python classes for polymorphic resolution.
    """

    _registry: dict[str, type[TypeDBType]] = {}
    # Cache for resolved polymorphic classes (type_label -> class)
    _resolution_cache: dict[str, type[TypeDBType]] = {}
    # Reverse index: attribute class → set of models that own it
    _attribute_owners: dict[type, set[type[TypeDBType]]] = {}

    @classmethod
    def register(cls, model: type[TypeDBType]) -> None:
        """Register a model class.

        Args:
            model: The model class to register
        """
        # Skip base classes like Entity/Relation
        if model.is_base():
            return

        type_name = model.get_type_name()
        if type_name in cls._registry and cls._registry[type_name] != model:
            logger.warning(
                f"Overwriting model registration for '{type_name}': "
                f"{cls._registry[type_name].__name__} -> {model.__name__}"
            )

        logger.debug(f"Registered model '{type_name}' -> {model.__name__}")
        cls._registry[type_name] = model

    @classmethod
    def get(cls, type_name: str) -> type[TypeDBType] | None:
        """Get model class by type name.

        Args:
            type_name: TypeDB type name

        Returns:
            The registered model class or None
        """
        return cls._registry.get(type_name)

    @classmethod
    def resolve(
        cls,
        type_label: str | None,
        allowed_classes: tuple[type[Entity], ...],
    ) -> type[Entity]:
        """Resolve a TypeDB type label to a Python model class.

        This is the unified method for polymorphic class resolution. It:
        1. First tries the central registry (O(1) lookup)
        2. Falls back to subclass search if not found
        3. Caches results for repeated lookups

        Args:
            type_label: The TypeDB type label (from label() function)
            allowed_classes: Tuple of allowed entity classes for this context

        Returns:
            The matching Python entity class, or the first allowed class as fallback
        """
        if not type_label:
            return allowed_classes[0]

        # Check resolution cache first
        if type_label in cls._resolution_cache:
            cached = cls._resolution_cache[type_label]
            # Verify the cached class is compatible with allowed_classes
            for allowed in allowed_classes:
                if issubclass(cached, allowed):
                    return cached  # type: ignore[return-value]

        # Try central registry
        registered = cls._registry.get(type_label)
        if registered is not None:
            # Verify it's compatible with allowed_classes
            for allowed in allowed_classes:
                if issubclass(registered, allowed):
                    cls._resolution_cache[type_label] = registered
                    return registered  # type: ignore[return-value]

        # Quick check: direct match against allowed classes
        for allowed_cls in allowed_classes:
            if allowed_cls.get_type_name() == type_label:
                cls._resolution_cache[type_label] = allowed_cls
                return allowed_cls

        # Fallback: search through subclasses
        def find_in_subclasses(search_cls: type[Entity]) -> type[Entity] | None:
            """Recursively search subclasses for matching type name."""
            for subclass in search_cls.__subclasses__():
                if subclass.get_type_name() == type_label:
                    return subclass
                found = find_in_subclasses(subclass)
                if found:
                    return found
            return None

        for allowed_cls in allowed_classes:
            found = find_in_subclasses(allowed_cls)
            if found:
                # Cache for future lookups and register in main registry
                cls._resolution_cache[type_label] = found
                cls._registry[type_label] = found
                return found

        # Fallback to first allowed class
        return allowed_classes[0]

    @classmethod
    def register_attribute_owners(cls, model: type[TypeDBType]) -> None:
        """Populate the reverse index for a model's owned attributes.

        Must be called **after** ``_owned_attrs`` has been populated
        (i.e. after SchemaScanner runs in Entity/Relation ``__init_subclass__``).
        """
        if model.is_base():
            return
        for attr_info in model._owned_attrs.values():
            cls._attribute_owners.setdefault(attr_info.typ, set()).add(model)

    @classmethod
    def get_attribute_owners(cls, attr_class: type) -> set[type[TypeDBType]]:
        """Get all model classes that own the given attribute type.

        Args:
            attr_class: Attribute subclass to look up

        Returns:
            Set of Entity/Relation classes that own this attribute
        """
        return set(cls._attribute_owners.get(attr_class, set()))

    @classmethod
    def clear(cls) -> None:
        """Clear the registry (for testing)."""
        cls._registry.clear()
        cls._resolution_cache.clear()
        cls._attribute_owners.clear()
