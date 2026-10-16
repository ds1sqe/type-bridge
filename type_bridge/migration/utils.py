"""Shared utilities for schema operations."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.session import Database


def type_exists(db: "Database", type_name: str) -> bool:
    """Check if a type exists in the database schema.

    Uses a simple query to check if the type name is valid in the schema.
    If the type doesn't exist, the query will raise an error.

    Args:
        db: Database connection
        type_name: Name of the type to check (entity, relation, or attribute)

    Returns:
        True if type exists in schema, False otherwise

    Example:
        >>> type_exists(db, "person")
        True
        >>> type_exists(db, "nonexistent")
        False
    """
    try:
        from type_bridge.query import Query
        from type_bridge.query.ast import EntityPattern

        query = Query()
        query.match_clause.patterns.append(EntityPattern(variable="$t", type_name=type_name))
        query.fetch("$t")
        query_text = query.build()
        with db.transaction("read") as tx:
            # If type exists, query succeeds (even with 0 results)
            # If type doesn't exist, query raises an error
            list(tx.execute(query_text))
            return True
    except Exception:
        return False
