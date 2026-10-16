"""Raw TypeQL setup helpers for tests of retained runtime behavior."""

from __future__ import annotations

import type_bridge_core as core

from type_bridge import Database


def define_schema(database: Database, body: str) -> None:
    """Apply schema text without invoking a removed model-authoring facade."""
    schema = body.strip()
    if not schema.startswith("define"):
        schema = f"define\n{schema}"
    database.execute_query(schema, transaction_type="schema")


def assert_typeql_equivalent(
    actual: str | list[str] | None, expected: str | list[str] | None
) -> None:
    if isinstance(actual, list):
        assert isinstance(expected, list)
        assert len(actual) == len(expected)
        for left, right in zip(actual, expected, strict=True):
            assert_typeql_equivalent(left, right)
    elif actual is None:
        assert expected is None
    else:
        assert isinstance(expected, str)
        assert core._typeql_queries_equivalent(actual, expected), (actual, expected)
