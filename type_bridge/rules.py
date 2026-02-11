"""Validation rule builder for type-bridge.

Provides a Pythonic API for defining custom validation rules that are
evaluated by the Rust core engine via a portable JSON DSL.

Example::

    from type_bridge.rules import RuleBuilder

    rules = (
        RuleBuilder()
        .required("name-req", entity="person", attribute="name")
        .regex("email-fmt", attribute="email", pattern=r"^.+@.+\\..+$")
        .range("age-bounds", attribute="age", min=0, max=150)
    )

    json_str = rules.to_json()
"""

from __future__ import annotations

import json
from typing import Any, Self


class RuleBuilder:
    """Fluent builder for constructing validation rules as JSON DSL."""

    def __init__(self) -> None:
        self._rules: list[dict[str, Any]] = []

    def _target(self, attribute: str, entity: str | None) -> dict[str, Any]:
        if entity is not None:
            return {"type": "EntityAttribute", "data": {"entity": entity, "attribute": attribute}}
        return {"type": "Attribute", "data": {"attribute": attribute}}

    def _add(
        self,
        rule_id: str,
        target: dict[str, Any],
        rule_type: dict[str, Any],
        message: str | None,
    ) -> Self:
        rule: dict[str, Any] = {
            "id": rule_id,
            "target": target,
            "rule_type": rule_type,
        }
        if message is not None:
            rule["error_message"] = message
        self._rules.append(rule)
        return self

    def required(
        self,
        rule_id: str,
        *,
        entity: str,
        attribute: str,
        message: str | None = None,
    ) -> Self:
        """Add a Required rule (scoped to an entity type)."""
        target = self._target(attribute, entity)
        return self._add(rule_id, target, {"type": "Required"}, message)

    def regex(
        self,
        rule_id: str,
        *,
        attribute: str,
        pattern: str,
        entity: str | None = None,
        message: str | None = None,
    ) -> Self:
        """Add a Regex rule."""
        target = self._target(attribute, entity)
        return self._add(rule_id, target, {"type": "Regex", "data": {"pattern": pattern}}, message)

    def range(
        self,
        rule_id: str,
        *,
        attribute: str,
        min: float | None = None,
        max: float | None = None,
        entity: str | None = None,
        message: str | None = None,
    ) -> Self:
        """Add a Range rule."""
        target = self._target(attribute, entity)
        return self._add(
            rule_id, target, {"type": "Range", "data": {"min": min, "max": max}}, message
        )

    def values(
        self,
        rule_id: str,
        *,
        attribute: str,
        allowed: list[Any],
        entity: str | None = None,
        message: str | None = None,
    ) -> Self:
        """Add a Values (allowlist) rule."""
        target = self._target(attribute, entity)
        return self._add(rule_id, target, {"type": "Values", "data": {"allowed": allowed}}, message)

    def cardinality(
        self,
        rule_id: str,
        *,
        entity: str,
        attribute: str,
        min: int = 0,
        max: int | None = None,
        message: str | None = None,
    ) -> Self:
        """Add a Cardinality rule (scoped to an entity type)."""
        target = self._target(attribute, entity)
        return self._add(
            rule_id, target, {"type": "Cardinality", "data": {"min": min, "max": max}}, message
        )

    def length(
        self,
        rule_id: str,
        *,
        attribute: str,
        min: int | None = None,
        max: int | None = None,
        entity: str | None = None,
        message: str | None = None,
    ) -> Self:
        """Add a Length rule."""
        target = self._target(attribute, entity)
        return self._add(
            rule_id, target, {"type": "Length", "data": {"min": min, "max": max}}, message
        )

    def to_json(self) -> str:
        """Serialize rules to a portable JSON DSL string."""
        return json.dumps({"rules": self._rules}, indent=2)

    def build(self) -> list[dict[str, Any]]:
        """Return rules as a list of dicts."""
        return list(self._rules)
