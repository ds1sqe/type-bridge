"""Code generation renderers for TypeBridge models.

This package contains modules for rendering Python code from parsed schemas:
- attributes: Attribute class definitions
- entities: Entity class definitions
- relations: Relation class definitions
- registry: Pre-computed schema metadata registry
- package: Package __init__.py with exports
"""

from __future__ import annotations

from .api_dto import render_api_dto
from .attributes import render_attributes
from .entities import render_entities
from .functions import render_functions
from .package import render_package_init
from .registry import render_registry
from .relations import render_relations
from .structs import render_structs

__all__ = [
    "render_attributes",
    "render_entities",
    "render_functions",
    "render_package_init",
    "render_api_dto",
    "render_registry",
    "render_relations",
    "render_structs",
]
