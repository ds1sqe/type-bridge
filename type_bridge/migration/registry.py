"""Model registry for migration auto-discovery.

This module provides a registry for tracking Entity/Relation models
that should be included in migration auto-generation.
"""

from __future__ import annotations

import importlib
import logging
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation

logger = logging.getLogger(__name__)


class ModelRegistry:
    """Registry for tracking Entity/Relation models.

    Models can be registered manually or auto-discovered from Python modules.
    The registry is used by the migration generator to determine which models
    should be tracked for schema changes.

    Example - Manual registration:
        from type_bridge.migration import ModelRegistry
        from myapp.models import Person, Company

        ModelRegistry.register(Person, Company)

    Example - Auto-discovery:
        from type_bridge.migration import ModelRegistry

        # Discover all Entity/Relation classes in module
        models = ModelRegistry.discover("myapp.models")

    Example - In models.py (recommended pattern):
        from type_bridge import Entity, String, Flag, Key, TypeFlags
        from type_bridge.migration import ModelRegistry

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)

        # Register at module load time
        ModelRegistry.register(Person)
    """

    _models: set[type[Entity | Relation]] = set()

    @classmethod
    def register(cls, *models: type[Entity | Relation]) -> None:
        """Register models for migration tracking.

        Args:
            models: Entity or Relation classes to register
        """
        from type_bridge.models import Entity, Relation

        for model in models:
            if not isinstance(model, type):
                logger.warning(f"Skipping non-class: {model}")
                continue

            if not issubclass(model, (Entity, Relation)):
                logger.warning(f"Skipping {model.__name__}: not an Entity or Relation subclass")
                continue

            if model in (Entity, Relation):
                continue

            if model not in cls._models:
                cls._models.add(model)
                logger.debug(f"Registered model: {model.__name__}")

    @classmethod
    def unregister(cls, *models: type[Entity | Relation]) -> None:
        """Unregister models from migration tracking.

        Args:
            models: Entity or Relation classes to unregister
        """
        for model in models:
            cls._models.discard(model)
            logger.debug(f"Unregistered model: {model.__name__}")

    @classmethod
    def clear(cls) -> None:
        """Clear all registered models."""
        cls._models.clear()
        logger.debug("Cleared all registered models")

    @classmethod
    def get_all(cls) -> list[type[Entity | Relation]]:
        """Get all registered models.

        Returns:
            List of registered Entity/Relation classes
        """
        return list(cls._models)

    @classmethod
    def is_registered(cls, model: type) -> bool:
        """Check if a model is registered.

        Args:
            model: Model class to check

        Returns:
            True if model is registered
        """
        return model in cls._models

    @classmethod
    def discover(cls, module_path: str, register: bool = True) -> list[type[Entity | Relation]]:
        """Auto-discover Entity/Relation classes from a module.

        Imports the module and finds all Entity/Relation subclasses defined in it.

        Args:
            module_path: Python module path (e.g., "myapp.models")
            register: If True, also register discovered models

        Returns:
            List of discovered Entity/Relation classes

        Raises:
            ImportError: If module cannot be imported
        """
        from type_bridge.models import Entity, Relation

        logger.info(f"Discovering models from: {module_path}")

        module = importlib.import_module(module_path)
        discovered: list[type[Entity | Relation]] = []

        def add_model(model: type[Entity | Relation]) -> None:
            if model not in discovered:
                discovered.append(model)
                logger.debug(f"Discovered model: {model.__name__}")

                if register:
                    cls.register(model)

        for name in dir(module):
            # Skip private/magic attributes
            if name.startswith("_"):
                continue

            obj = getattr(module, name)

            # Must be a class
            if not isinstance(obj, type):
                continue

            # Must be defined in this module (not imported)
            if obj.__module__ != module_path:
                continue

            # Must be Entity or Relation subclass (but not the base classes)
            if issubclass(obj, (Entity, Relation)) and obj not in (Entity, Relation):
                add_model(obj)

        # Generated bindgen packages expose ENTITIES and RELATIONS lists from
        # their package __init__.py while the actual classes live in generated
        # submodules. Accept those package-level lists so `--models myapp.models`
        # can discover the full generated schema in one argument.
        for collection_name in ("ENTITIES", "RELATIONS"):
            for obj in getattr(module, collection_name, ()):
                if not isinstance(obj, type):
                    continue
                if issubclass(obj, (Entity, Relation)) and obj not in (Entity, Relation):
                    add_model(obj)

        logger.info(f"Discovered {len(discovered)} models from {module_path}")
        return discovered

    @classmethod
    def discover_recursive(
        cls, package_path: str, register: bool = True
    ) -> list[type[Entity | Relation]]:
        """Recursively discover models from a package.

        Imports all modules in the package and discovers Entity/Relation classes.

        Args:
            package_path: Python package path (e.g., "myapp")
            register: If True, also register discovered models

        Returns:
            List of discovered Entity/Relation classes

        Raises:
            ImportError: If package cannot be imported
        """
        import pkgutil

        logger.info(f"Recursively discovering models from: {package_path}")

        package = importlib.import_module(package_path)
        discovered: list[type[Entity | Relation]] = []

        # First discover in the package itself
        discovered.extend(cls.discover(package_path, register=register))

        # Then discover in submodules
        if hasattr(package, "__path__"):
            for importer, modname, ispkg in pkgutil.walk_packages(
                package.__path__, prefix=f"{package_path}."
            ):
                try:
                    submodule_models = cls.discover(modname, register=register)
                    discovered.extend(submodule_models)
                except ImportError as e:
                    logger.warning(f"Could not import {modname}: {e}")
                    continue

        logger.info(f"Recursively discovered {len(discovered)} models from {package_path}")
        return discovered
