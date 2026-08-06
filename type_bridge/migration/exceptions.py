"""Retained exception identities without a schema-authoring dependency."""

from __future__ import annotations

from typing import Any


class SchemaValidationError(Exception):
    """A retained schema-validation diagnostic."""


class SchemaConflictError(Exception):
    """A retained conflict diagnostic for existing compatibility callers."""

    def __init__(self, diff: Any, message: str | None = None) -> None:
        self.diff = diff
        super().__init__(message or "Schema conflict detected")

    def has_breaking_changes(self) -> bool:
        """Report whether the supplied historical diff has breaking members."""
        return any(
            bool(getattr(self.diff, name, None))
            for name in (
                "removed_entities",
                "removed_relations",
                "removed_attributes",
                "modified_attributes",
                "modified_entities",
                "modified_relations",
            )
        )


__all__ = ["SchemaConflictError", "SchemaValidationError"]
