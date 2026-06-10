from . import type_bridge_core
from .type_bridge_core import *  # noqa: F403  (re-export the compiled extension's public API)

__doc__ = type_bridge_core.__doc__
if hasattr(type_bridge_core, "__all__"):
    __all__ = type_bridge_core.__all__
