"""Raw TypeQL setup helpers for tests of retained runtime behavior."""

from __future__ import annotations

from type_bridge import Database


def define_schema(database: Database, body: str) -> None:
    """Apply schema text without invoking a removed model-authoring facade."""
    schema = body.strip()
    if not schema.startswith("define"):
        schema = f"define\n{schema}"
    database.execute_query(schema, transaction_type="schema")
