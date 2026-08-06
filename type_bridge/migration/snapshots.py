"""Read-only metadata access for frozen archive migration snapshots.

Snapshot creation belonged to the retired Python/TypeQL authoring lane.  The
canonical Split-YAML migration workflow owns every new snapshot.  This module
intentionally reads existing ``snapshot.json`` files only; verification and
one-way adoption of their bound files are handled by the checked sidecar and
native adoption readers.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)


def get_snapshot_metadata(snapshot_dir: Path) -> dict[str, Any] | None:
    """Read metadata from an existing archived ``snapshot.json`` file."""
    metadata_path = snapshot_dir / "snapshot.json"
    if not metadata_path.exists():
        return None
    try:
        value = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        logger.warning("Failed to read snapshot metadata from %s: %s", metadata_path, error)
        return None
    if not isinstance(value, dict):
        logger.warning("Snapshot metadata at %s is not a JSON object", metadata_path)
        return None
    return value


__all__ = ["get_snapshot_metadata"]
