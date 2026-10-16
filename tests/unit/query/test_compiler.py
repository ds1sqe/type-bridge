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
    assert result == "match $p isa person;"


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
        == "not { $p has age $a; \n}"
    )
    assert (
        compiler.compile(
            OrPattern(alternatives=[[SubTypePattern("$t", "a")], [IidPattern("$t", "0x1")]])
        )
        == "{ $t sub a; } or { $t iid 0x1; }"
    )
    assert (
        compiler.compile(AttributePattern("$a", "age", LiteralValue(30, "long"))) == "$a isa age 30"
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
    assert compiler.compile_batch(patterns, "match") == "match $p isa person; $p has age $a;"


def test_raw_fetch_items_pass_through(compiler):
    fetch = FetchClause(items=['"raw_name": $p.person-name', FetchVariable(key="p", var="$p")])
    assert compiler.compile(fetch) == 'fetch { "raw_name": $p.person-name, "p": $p };'


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
    expected = 'match $p isa person, iid 0x123, has name "Alice", has age 30;'
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
    expected = "match $rel isa employment (employee: $p, employer: $c);"
    assert result == expected


def test_compile_insert(compiler):
    """Test simple insert clause."""
    statements = [
        IsaStatement(variable="$p", type_name="person"),
        HasStatement(subject_var="$p", attr_name="name", value=LiteralValue("Bob", "string")),
    ]
    insert = InsertClause(statements=statements)

    result = compiler.compile(insert)
    expected = 'insert $p isa person; $p has name "Bob";'
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
    expected = "insert $rel isa marriage, links (husband: $h, wife: $w);"
    assert result == expected


def test_compile_update(compiler):
    """Test update clause (using delete + insert typically, but testing generic UpdateClause here)."""
    # Assuming we use the 'update' syntactic sugar for simple attribute updates
    stmt = HasStatement(
        subject_var="$p", attr_name="email", value=LiteralValue("new@example.com", "string")
    )
    update = UpdateClause(statements=[stmt])

    result = compiler.compile(update)
    expected = 'update $p has email == "new@example.com";'
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
        assert result == "match let $count = count-items();"

    def test_single_variable_stream(self, compiler):
        """Test single variable assignment with 'in' operator for streams."""
        assignment = LetAssignment(
            variables=["$item"],
            expression=FunctionCallValue(function="get-items", args=[]),
            is_stream=True,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match let $item in get-items();"

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
        assert result == "match let $t, $c in count_artifacts_by_type();"
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
        assert result == "match let $type, $count in count-by-type($filter);"

    def test_let_assignment_with_string_expression(self, compiler):
        """Test let assignment with raw string expression."""
        assignment = LetAssignment(
            variables=["$x"],
            expression="my_func()",
            is_stream=False,
        )
        clause = MatchLetClause(assignments=[assignment])

        result = compiler.compile(clause)
        assert result == "match let $x = my_func();"

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
        expected = "match let $a = func-a(); let $b, $c in func-bc();"
        assert result == expected


def test_native_legacy_compiler_warns_and_rejects_dropped_relation_attributes() -> None:
    """The Python boundary propagates a checked error instead of losing a node."""
    from type_bridge_core import QueryCompiler as NativeCompiler

    clause = {
        "Insert": [
            {
                "type": "Relation",
                "data": {
                    "variable": "$r",
                    "type_name": "friendship",
                    "role_players": [{"role": "friend", "player_var": "$x"}],
                    "include_variable": True,
                    "attributes": [
                        {
                            "type": "Isa",
                            "data": {"variable": "$other", "type_name": "person"},
                        }
                    ],
                },
            }
        ],
    }
    with pytest.warns(DeprecationWarning, match="deprecated since 2.3.0"):
        with pytest.raises(ValueError, match="invalid legacy TypeQL node shape"):
            NativeCompiler().compile_dicts([clause])


def test_native_legacy_raw_escape_hatch_remains_verbatim() -> None:
    from type_bridge_core import QueryCompiler as NativeCompiler

    raw = "caller-owned  text\n"
    with pytest.warns(DeprecationWarning, match="deprecated since 2.3.0"):
        assert NativeCompiler().compile_node({"Pattern": {"type": "Raw", "data": raw}}) == raw


def test_optional_pattern_round_trips_through_python_without_becoming_negation() -> None:
    from type_bridge.query.ast import MatchClause, TryPattern
    from type_bridge.query.parser import parse_typeql_query

    source = 'match try { $p isa person, has name "Ada"; };'
    with pytest.warns(DeprecationWarning):
        clauses = parse_typeql_query(source)
    assert isinstance(clauses[0], MatchClause)
    assert isinstance(clauses[0].patterns[0], TryPattern)
    with pytest.warns(DeprecationWarning):
        rendered = QueryCompiler().compile(clauses[0])
    assert "try {" in rendered
    assert "not {" not in rendered
    with pytest.warns(DeprecationWarning):
        assert parse_typeql_query(rendered) == clauses


def test_parsed_decimal_keeps_exact_digits_while_double_keeps_numeric_value() -> None:
    from type_bridge.query.ast import MatchLetClause
    from type_bridge.query.parser import parse_typeql_query

    with pytest.warns(DeprecationWarning):
        decimal_clause = parse_typeql_query("match let $v = 0.1000000000000000001dec;")[0]
    assert isinstance(decimal_clause, MatchLetClause)
    decimal = decimal_clause.assignments[0].expression
    assert isinstance(decimal, LiteralValue)
    assert decimal.value_type == "decimal"
    assert decimal.value == "0.1000000000000000001"
    with pytest.warns(DeprecationWarning):
        double_clause = parse_typeql_query("match let $v = 1.25;")[0]
    assert isinstance(double_clause, MatchLetClause)
    double = double_clause.assignments[0].expression
    assert isinstance(double, LiteralValue)
    assert double.value_type == "double"
    assert double.value == 1.25


@pytest.mark.parametrize("expression", ["count", "count($p)", "sum($p)", "list($p)"])
def test_raw_reduce_expression_uses_official_reducer(compiler, expression):
    from type_bridge.query.ast import ReduceAssignment, ReduceClause

    rendered = compiler.compile(
        ReduceClause(assignments=[ReduceAssignment(variable="$total", expression=expression)])
    )
    assert rendered == "reduce $total = " + expression + ";"


@pytest.mark.parametrize(
    "expression", ["count; sort $p", "count, $other = count", "count groupby $p"]
)
def test_raw_reduce_expression_cannot_add_query_structure(compiler, expression):
    from type_bridge.query.ast import ReduceAssignment, ReduceClause

    with pytest.raises(ValueError):
        compiler.compile(
            ReduceClause(assignments=[ReduceAssignment(variable="$total", expression=expression)])
        )
