"""Separately retained V1 query aliases.

Generated packages use projection-owned managers. The handwritten manager
identity is intentionally absent from this compatibility module.
"""

from __future__ import annotations

from type_bridge.crud.rust_manager import RustTypeDBGroupByQuery as GroupByQuery
from type_bridge.crud.rust_manager import RustTypeDBQuery as TypeDBQuery

__all__ = ["GroupByQuery", "TypeDBQuery"]
