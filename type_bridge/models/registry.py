"""Central registry for TypeDB models."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, TypeVar

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)

T = TypeVar("T", bound="TypeDBType")


class ModelRegistry:
    """Registry for TypeDB model classes.

    Maps TypeDB type names to Python classes for polymorphic resolution.
    """

    _registry: dict[str, type[TypeDBType]] = {}

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
    def clear(cls) -> None:
        """Clear the registry (for testing)."""
        cls._registry.clear()
