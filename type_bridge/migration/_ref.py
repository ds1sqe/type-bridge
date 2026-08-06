"""Private schema-label references for frozen migration recovery.

Active application model classes can disappear when bindgen regenerates a
package after a schema deletion.  Migration refs are the historical authoring
surface for generated migrations: they carry only the TypeDB label needed to
review or remove schema objects, while sidecars carry full executable payloads
for schema-bearing add operations.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal


def _validate_label(label: str) -> str:
    if not isinstance(label, str) or not label:
        raise ValueError("Migration ref labels must be non-empty strings")
    return label


@dataclass(frozen=True)
class EntityRef:
    """Historical reference to an entity type label."""

    label: str
    kind: Literal["entity"] = "entity"

    def __post_init__(self) -> None:
        object.__setattr__(self, "label", _validate_label(self.label))

    def get_type_name(self) -> str:
        return self.label


@dataclass(frozen=True)
class RelationRef:
    """Historical reference to a relation type label."""

    label: str
    kind: Literal["relation"] = "relation"

    def __post_init__(self) -> None:
        object.__setattr__(self, "label", _validate_label(self.label))

    def get_type_name(self) -> str:
        return self.label


@dataclass(frozen=True)
class AttributeRef:
    """Historical reference to an attribute type label."""

    label: str
    kind: Literal["attribute"] = "attribute"

    def __post_init__(self) -> None:
        object.__setattr__(self, "label", _validate_label(self.label))

    def get_attribute_name(self) -> str:
        return self.label


def entity(label: str) -> EntityRef:
    """Return a stable migration reference to an entity label."""
    return EntityRef(label)


def relation(label: str) -> RelationRef:
    """Return a stable migration reference to a relation label."""
    return RelationRef(label)


def attribute(label: str) -> AttributeRef:
    """Return a stable migration reference to an attribute label."""
    return AttributeRef(label)
