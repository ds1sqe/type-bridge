"""Unit tests for Query Compiler."""

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from typing import cast

import pytest

from type_bridge.query.ast import (
    ArithmeticValue,
    AttributePattern,
    DeleteThingStatement,
    EntityPattern,
    FetchClause,
    FetchVariable,
    FunctionCallValue,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaConstraint,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    RawPattern,
    RawStatement,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    SubTypePattern,
    UpdateClause,
    Value,
    ValueComparisonPattern,
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


def test_compile_is_native_and_surfaces_core_errors_as_value_error(compiler):
    """There is no Python renderer to fall back to: a node the core cannot
    decode is refused with ``ValueError``."""
    assert not hasattr(compiler, "_compile_clause")
    assert not hasattr(compiler, "_compile_value")
    broken = RelationPattern(
        variable="$r",
        type_name="employment",
        role_players=[RolePlayer(role="employee", player_var=cast(str, None))],
    )

    with pytest.raises(ValueError, match="Failed to deserialize query node"):
        compiler.compile(broken)


@pytest.mark.parametrize("value", [2**63, -(2**63) - 1, float("nan"), float("inf")])
def test_literals_without_a_typeql_form_are_refused(compiler, value):
    with pytest.raises(ValueError, match="TypeQL"):
        compiler.compile(LiteralValue(value, "double"))


def test_compile_each_node_kind(compiler):
    """Patterns, statements, constraints, and values render on their own."""
    assert (
        compiler.compile(
            EntityPattern(
                variable="$p",
                type_name="person",
                constraints=[HasConstraint(attr_name="name", value="$n")],
                is_strict=True,
            )
        )
        == "$p isa! person, has name $n"
    )
    assert (
        compiler.compile(NotPattern(patterns=[HasPattern("$p", "age", "$a")]))
        == "not { $p has age $a; }"
    )
    assert (
        compiler.compile(
            OrPattern(alternatives=[[SubTypePattern("$t", "a")], [IidPattern("$t", "0x1")]])
        )
        == "{ $t sub a; } or { $t iid 0x1; }"
    )
    assert (
        compiler.compile(AttributePattern("$a", "age", LiteralValue(30, "long")))
        == "$a isa age; $a 30"
    )
    assert compiler.compile(ValueComparisonPattern("$a", ">", "$b")) == "$a > $b"
    assert compiler.compile(RawPattern("$x has y 1")) == "$x has y 1"
    assert (
        compiler.compile(HasStatement("$p", "name", LiteralValue("Bob", "string")))
        == '$p has name "Bob"'
    )
    assert compiler.compile(DeleteThingStatement("$p")) == "$p"
    assert compiler.compile(RawStatement("$p has x 1")) == "$p has x 1"
    assert compiler.compile(IidConstraint("0x1")) == "iid 0x1"
    assert compiler.compile(IsaConstraint("person", strict=True)) == "isa! person"
    assert (
        compiler.compile(ArithmeticValue(FunctionCallValue("abs", ["$x"]), "*", "$y"))
        == "(abs($x) * $y)"
    )


def test_compile_batch_joins_nodes_and_prefixes_the_operation(compiler):
    patterns = [EntityPattern("$p", "person"), HasPattern("$p", "age", "$a")]
    assert compiler.compile_batch(patterns) == "$p isa person\n$p has age $a"
    assert compiler.compile_batch(patterns, "match") == "match\n$p isa person\n$p has age $a"


def test_raw_fetch_items_pass_through(compiler):
    fetch = FetchClause(items=['"raw_name": $p.person-name', FetchVariable(key="p", var="$p")])
    assert compiler.compile(fetch) == 'fetch {\n  "raw_name": $p.person-name,\n  "p": $p\n};'


def test_literals_render_by_python_type(compiler):
    """Non-string values are typed by their class; strings keep their declared type."""

    class Wrapped:
        def __init__(self, value: object) -> None:
            self.value = value

    cases = [
        (LiteralValue(Decimal("123.45"), "string"), "123.45dec"),
        (LiteralValue(datetime(2024, 1, 2, 3, 4, 5), "string"), "2024-01-02T03:04:05"),
        (
            LiteralValue(datetime(2024, 1, 2, 3, 4, 5, tzinfo=UTC), "datetime-tz"),
            "2024-01-02T03:04:05+00:00",
        ),
        (LiteralValue(date(2024, 1, 2), "string"), "2024-01-02"),
        (LiteralValue(timedelta(days=1, hours=2), "string"), "P1DT2H"),
        (LiteralValue(1.5, "double"), "1.5"),
        (LiteralValue(True, "string"), "true"),
        (LiteralValue(7, "string"), "7"),
        (LiteralValue(Wrapped("w"), "string"), '"w"'),
        (LiteralValue(None, "string"), '"None"'),
        (LiteralValue("2024-01-02", "date"), "2024-01-02"),
        (LiteralValue('a "b"\n', "string"), '"a \\"b\\"\\n"'),
    ]
    for literal, expected in cases:
        assert compiler.compile(literal) == expected, literal


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
        compiler.compile(unknown)


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
