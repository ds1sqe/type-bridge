"""Compatibility exports for the retired Python ORM manager.

The Python TypeQL-building manager was removed in #125 Phase 4. Import paths
remain stable, but the names now point at the Rust-backed runtime facade.
"""

from __future__ import annotations

from type_bridge.crud.rust_manager import (
    RustTypeDBGroupByQuery as GroupByQuery,
)
from type_bridge.crud.rust_manager import (
    RustTypeDBManager as TypeDBManager,
)
from type_bridge.crud.rust_manager import (
    RustTypeDBQuery as TypeDBQuery,
)

__all__ = ["GroupByQuery", "TypeDBManager", "TypeDBQuery"]
