"""Unit tests for Query Compiler."""

from datetime import datetime

import pytest

from type_bridge.query.ast import (
    EntityPattern,
    HasConstraint,
    HasStatement,
    IidConstraint,
    InsertClause,
    IsaStatement,
    LiteralValue,
    MatchClause,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    UpdateClause,
    Value,
)
from type_bridge.query.compiler import QueryCompiler


@pytest.fixture
def compiler():
    return QueryCompiler()


def test_compile_simple_match(compiler):
    """Test matching a simple entity."""
    pattern = EntityPattern(variable="$p", type_name="person", constraints=[])
    match = MatchClause(patterns=[pattern])

    result = compiler.compile(match)
    assert result == "match\n$p isa person;"


def test_compile_match_with_constraints(compiler):
    """Test matching entity with IID and attributes."""
    pattern = EntityPattern(
        variable="$p",
        type_name="person",
        constraints=[
            IidConstraint(iid="0x123"),
            HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
            HasConstraint(attr_name="age", value=LiteralValue(30, "long")),
        ],
    )
    match = MatchClause(patterns=[pattern])

    result = compiler.compile(match)
    expected = 'match\n$p isa person, iid 0x123, has name "Alice", has age 30;'
    assert result == expected


def test_compile_relation_match(compiler):
    """Test matching a relation with role players."""
    pattern = RelationPattern(
        variable="$rel",
        type_name="employment",
        role_players=[
            RolePlayer(role="employee", player_var="$p"),
            RolePlayer(role="employer", player_var="$c"),
        ],
    )
    match = MatchClause(patterns=[pattern])

    result = compiler.compile(match)
    # TypeDB 3.x syntax: isa type comes before role players
    expected = "match\n$rel isa employment (employee: $p, employer: $c);"
    assert result == expected


def test_compile_insert(compiler):
    """Test simple insert clause."""
    statements = [
        IsaStatement(variable="$p", type_name="person"),
        HasStatement(subject_var="$p", attr_name="name", value=LiteralValue("Bob", "string")),
    ]
    insert = InsertClause(statements=statements)

    result = compiler.compile(insert)
    expected = 'insert\n$p isa person;\n$p has name "Bob";'
    assert result == expected


def test_compile_relation_insert(compiler):
    """Test inserting a relation."""
    stmt = RelationStatement(
        variable="$rel",
        type_name="marriage",
        role_players=[
            RolePlayer(role="husband", player_var="$h"),
            RolePlayer(role="wife", player_var="$w"),
        ],
    )
    insert = InsertClause(statements=[stmt])

    result = compiler.compile(insert)
    expected = "insert\n$rel (husband: $h, wife: $w) isa marriage;"
    assert result == expected


def test_compile_update(compiler):
    """Test update clause (using delete + insert typically, but testing generic UpdateClause here)."""
    # Assuming we use the 'update' syntactic sugar for simple attribute updates
    stmt = HasStatement(
        subject_var="$p", attr_name="email", value=LiteralValue("new@example.com", "string")
    )
    update = UpdateClause(statements=[stmt])

    result = compiler.compile(update)
    expected = 'update\n$p has email "new@example.com";'
    assert result == expected


def test_escaping_behavior(compiler):
    """Test that values are properly escaped via the imported formatter."""
    # Dates
    dt = datetime(2023, 10, 27, 12, 0, 0)
    stmt = HasStatement(
        subject_var="$x", attr_name="created_at", value=LiteralValue(dt, "datetime")
    )
    result = compiler.compile(InsertClause(statements=[stmt]))
    assert "2023-10-27T12:00:00" in result

    # Strings with quotes
    stmt = HasStatement(
        subject_var="$x", attr_name="bio", value=LiteralValue('He said "Hello"', "string")
    )
    result = compiler.compile(InsertClause(statements=[stmt]))
    # The formatter should handle escaping.
    # Standard formatter usually wraps in double quotes and escapes internal double quotes
    assert "'He said \"Hello\"'" in result or '"He said \\"Hello\\""' in result


def test_compile_value_unknown_type_raises_error(compiler):
    """Test that compiling an unknown Value subclass raises an error."""
    from dataclasses import dataclass

    @dataclass
    class UnknownValue(Value):
        """A custom Value subclass not handled by the compiler."""

        data: str

    unknown = UnknownValue(data="test")

    with pytest.raises(ValueError, match="Unknown value type: UnknownValue"):
        compiler._compile_value(unknown)
