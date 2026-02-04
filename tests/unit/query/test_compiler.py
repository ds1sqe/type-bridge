"""Unit tests for Query Compiler."""

from datetime import datetime

import pytest
from type_bridge.query.ast import (
    EntityPattern,
    FunctionCallValue,
    HasConstraint,
    HasStatement,
    IidConstraint,
    InsertClause,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
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
    """Test inserting a relation with variable.

    TypeDB 3.x syntax: $var isa type, links (role: $player);
    """
    stmt = RelationStatement(
        variable="$rel",
        type_name="marriage",
        role_players=[
            RolePlayer(role="husband", player_var="$h"),
            RolePlayer(role="wife", player_var="$w"),
        ],
        include_variable=True,
    )
    insert = InsertClause(statements=[stmt])

    result = compiler.compile(insert)
    expected = "insert\n$rel isa marriage, links (husband: $h, wife: $w);"
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


class TestLetAssignment:
    """Tests for let assignment compilation in match clauses."""

    def test_single_variable_assignment(self, compiler):
        """Test single variable assignment with = operator."""
        assignment = LetAssignment(
            variables=["$count"],
            expression=FunctionCallValue(function="count-items", args=[]),
            is_stream=False,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match\nlet $count = count-items();"

    def test_single_variable_stream(self, compiler):
        """Test single variable assignment with 'in' operator for streams."""
        assignment = LetAssignment(
            variables=["$item"],
            expression=FunctionCallValue(function="get-items", args=[]),
            is_stream=True,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match\nlet $item in get-items();"

    def test_multi_variable_assignment_no_parentheses(self, compiler):
        """Test multi-variable assignment does NOT wrap in parentheses.

        TypeQL 3.x syntax: let $t, $c in func();
        NOT: let ($t, $c) in func();
        """
        assignment = LetAssignment(
            variables=["$t", "$c"],
            expression=FunctionCallValue(function="count_artifacts_by_type", args=[]),
            is_stream=True,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        # Critical: no parentheses around variables
        assert result == "match\nlet $t, $c in count_artifacts_by_type();"
        # Explicitly verify no parentheses
        assert "($t" not in result
        assert "$c)" not in result

    def test_multi_variable_assignment_with_args(self, compiler):
        """Test multi-variable stream function with arguments."""
        assignment = LetAssignment(
            variables=["$type", "$count"],
            expression=FunctionCallValue(
                function="count-by-type",
                args=["$filter"],
            ),
            is_stream=True,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match\nlet $type, $count in count-by-type($filter);"

    def test_let_assignment_with_string_expression(self, compiler):
        """Test let assignment with raw string expression."""
        assignment = LetAssignment(
            variables=["$x"],
            expression="my_func()",
            is_stream=False,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match\nlet $x = my_func();"

    def test_multiple_let_assignments(self, compiler):
        """Test multiple assignments in a single MatchLetClause."""
        assignments = [
            LetAssignment(
                variables=["$a"],
                expression=FunctionCallValue(function="func-a", args=[]),
                is_stream=False,
            ),
            LetAssignment(
                variables=["$b", "$c"],
                expression=FunctionCallValue(function="func-bc", args=[]),
                is_stream=True,
            ),
        ]
        clause = MatchLetClause(assignments=assignments)

        result = compiler.compile(clause)
        expected = "match\nlet $a = func-a();\nlet $b, $c in func-bc();"
        assert result == expected
