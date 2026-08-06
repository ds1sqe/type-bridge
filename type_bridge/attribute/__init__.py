"""Private compatibility implementation for retained V1 query execution.

Application attribute classes are emitted by ``type-bridge schema generate``.
This package intentionally has no public authoring exports.
"""

from __future__ import annotations

from typing import Any


def __getattr__(name: str) -> Any:
    from type_bridge.migration._archive_imports import archive_attribute

    return archive_attribute(__name__, name)


__all__: list[str] = []
