"""Separately retained V1 query and compatibility identities.

Generated packages use their projection-owned managers and do not import this
module.  Imports remain lazy so loading :mod:`type_bridge` does not pull the
removed handwritten authoring closure into generated applications.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.crud.exceptions import (
        EntityNotFoundError,
        HydrationError,
        KeyAttributeError,
        NotFoundError,
        NotUniqueError,
        RelationNotFoundError,
    )
    from type_bridge.crud.has_lookup import has_lookup
    from type_bridge.crud.hooks import CrudEvent, CrudHook, HookCancelled
    from type_bridge.crud.rust_manager import (
        RustTypeDBGroupByQuery as GroupByQuery,
    )
    from type_bridge.crud.rust_manager import (
        RustTypeDBQuery as TypeDBQuery,
    )

_LAZY_EXPORTS: dict[str, tuple[str, str]] = {
    "has_lookup": ("type_bridge.crud.has_lookup", "has_lookup"),
    "TypeDBQuery": ("type_bridge.crud.rust_manager", "RustTypeDBQuery"),
    "GroupByQuery": ("type_bridge.crud.rust_manager", "RustTypeDBGroupByQuery"),
    "CrudEvent": ("type_bridge.crud.hooks", "CrudEvent"),
    "CrudHook": ("type_bridge.crud.hooks", "CrudHook"),
    "HookCancelled": ("type_bridge.crud.hooks", "HookCancelled"),
    "NotFoundError": ("type_bridge.crud.exceptions", "NotFoundError"),
    "EntityNotFoundError": ("type_bridge.crud.exceptions", "EntityNotFoundError"),
    "RelationNotFoundError": ("type_bridge.crud.exceptions", "RelationNotFoundError"),
    "NotUniqueError": ("type_bridge.crud.exceptions", "NotUniqueError"),
    "KeyAttributeError": ("type_bridge.crud.exceptions", "KeyAttributeError"),
    "HydrationError": ("type_bridge.crud.exceptions", "HydrationError"),
}


def __getattr__(name: str) -> Any:
    target = _LAZY_EXPORTS.get(name)
    if target is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

    from importlib import import_module

    module_name, attribute_name = target
    value = getattr(import_module(module_name), attribute_name)
    globals()[name] = value
    return value


__all__ = [
    # Cross-type lookup
    "has_lookup",
    # Separately retained V1 query facades
    "TypeDBQuery",
    "GroupByQuery",
    # Hooks
    "CrudEvent",
    "CrudHook",
    "HookCancelled",
    # Exceptions
    "NotFoundError",
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
    "HydrationError",
]
