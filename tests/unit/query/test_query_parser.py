"""Unit tests for TypeQL query parser (Rust-backed)."""

from __future__ import annotations

import pytest

from type_bridge.query.ast import (
    DeleteClause,
    DeleteThingStatement,
    EntityPattern,
    FetchAttribute,
    FetchAttributeList,
    FetchClause,
    FetchFunction,
    FetchNestedWildcard,
    FetchVariable,
    FetchWildcard,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    SubTypePattern,
    UpdateClause,
    ValueComparisonPattern,
)
from type_bridge.query.compiler import QueryCompiler
from type_bridge.query.parser import RUST_AVAILABLE, parse_typeql_query

pytestmark = pytest.mark.skipif(not RUST_AVAILABLE, reason="Rust extension not available")

compiler = QueryCompiler()


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------


def _roundtrip(typeql: str) -> str:
    """Parse TypeQL -> AST -> compile back to TypeQL."""
    clauses = parse_typeql_query(typeql)
    parts = [compiler.compile(c) for c in clauses]
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Match clause tests
# ---------------------------------------------------------------------------


class TestMatchClause:
    def test_simple_entity(self):
        tql = "match\n$p isa person;"
        clauses = parse_typeql_query(tql)
        assert len(clauses) == 1
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        patterns = clause.patterns
        assert len(patterns) == 1
        assert isinstance(patterns[0], EntityPattern)
        assert patterns[0].variable == "$p"
        assert patterns[0].type_name == "person"
        assert patterns[0].constraints == []

    def test_entity_with_has_constraint(self):
        tql = 'match\n$p isa person, has name "Alice";'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, EntityPattern)
        assert len(pat.constraints) == 1
        c = pat.constraints[0]
        assert isinstance(c, HasConstraint)
        assert c.attr_name == "name"
        assert isinstance(c.value, LiteralValue)
        assert c.value.value == "Alice"

    def test_entity_with_iid_constraint(self):
        tql = "match\n$p isa person, iid 0x1234abcd;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, EntityPattern)
        assert isinstance(pat.constraints[0], IidConstraint)
        assert pat.constraints[0].iid == "0x1234abcd"

    def test_entity_strict_isa(self):
        tql = "match\n$p isa! person;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, EntityPattern)
        assert pat.is_strict is True

    def test_relation_pattern(self):
        tql = "match\n$r isa employment (employee: $p, employer: $c);"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, RelationPattern)
        assert pat.variable == "$r"
        assert pat.type_name == "employment"
        assert len(pat.role_players) == 2
        assert pat.role_players[0].role == "employee"
        assert pat.role_players[0].player_var == "$p"

    def test_subtype_pattern(self):
        tql = "match\n$t sub person;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, SubTypePattern)
        assert pat.variable == "$t"
        assert pat.parent_type == "person"

    def test_has_pattern(self):
        tql = "match\n$p has name $n;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, HasPattern)
        assert pat.thing_var == "$p"
        assert pat.attr_type == "name"
        assert pat.attr_var == "$n"

    def test_iid_pattern(self):
        tql = "match\n$p iid 0xabc;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, IidPattern)
        assert pat.variable == "$p"
        assert pat.iid == "0xabc"

    def test_value_comparison(self):
        tql = "match\n$a > 10;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, ValueComparisonPattern)
        assert pat.var == "$a"
        assert pat.operator == ">"
        assert isinstance(pat.value, LiteralValue)

    def test_not_pattern(self):
        tql = "match\n$p isa person;\nnot { $p has name $n; };"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        assert len(clause.patterns) == 2
        not_pat = clause.patterns[1]
        assert isinstance(not_pat, NotPattern)
        assert len(not_pat.patterns) == 1

    def test_or_pattern(self):
        tql = "match\n{ $p isa person; } or { $p isa company; };"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        pat = clause.patterns[0]
        assert isinstance(pat, OrPattern)
        assert len(pat.alternatives) == 2

    def test_multiple_patterns(self):
        tql = 'match\n$p isa person, has name "Alice";\n$c isa company;'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchClause)
        assert len(clause.patterns) == 2


# ---------------------------------------------------------------------------
# Insert clause tests
# ---------------------------------------------------------------------------


class TestInsertClause:
    def test_simple_isa(self):
        tql = "insert\n$p isa person;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, InsertClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, IsaStatement)
        assert stmt.variable == "$p"
        assert stmt.type_name == "person"

    def test_has_statement(self):
        tql = 'insert\n$p has name "Alice";'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, InsertClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, HasStatement)
        assert stmt.subject_var == "$p"
        assert stmt.attr_name == "name"

    def test_relation_without_variable(self):
        tql = "insert\n(employee: $p, employer: $c) isa employment;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, InsertClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, RelationStatement)
        assert stmt.include_variable is False

    def test_relation_with_variable(self):
        tql = "insert\n$r isa employment, links (employee: $p, employer: $c);"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, InsertClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, RelationStatement)
        assert stmt.include_variable is True
        assert stmt.variable == "$r"


# ---------------------------------------------------------------------------
# Delete clause tests
# ---------------------------------------------------------------------------


class TestDeleteClause:
    def test_delete_thing(self):
        tql = "delete\n$p;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, DeleteClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, DeleteThingStatement)
        assert stmt.variable == "$p"

    def test_delete_has(self):
        tql = 'delete\n$p has name "Alice";'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, DeleteClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, HasStatement)


# ---------------------------------------------------------------------------
# Update clause tests
# ---------------------------------------------------------------------------


class TestUpdateClause:
    def test_update_has(self):
        tql = 'update\n$p has name "Bob";'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, UpdateClause)
        stmt = clause.statements[0]
        assert isinstance(stmt, HasStatement)


# ---------------------------------------------------------------------------
# Fetch clause tests
# ---------------------------------------------------------------------------


class TestFetchClause:
    def test_fetch_wildcard(self):
        tql = 'fetch {\n  "data": $p.*\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchWildcard)
        assert item.key == "data"
        assert item.var == "$p"

    def test_fetch_attribute(self):
        tql = 'fetch {\n  "name": $p.name\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchAttribute)
        assert item.attr_name == "name"

    def test_fetch_variable(self):
        tql = 'fetch {\n  "person": $p\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchVariable)

    def test_fetch_attribute_list(self):
        tql = 'fetch {\n  "names": [$p.name]\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchAttributeList)

    def test_fetch_function(self):
        tql = 'fetch {\n  "_iid": iid($p)\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchFunction)
        assert item.func_name == "iid"

    def test_fetch_nested_wildcard(self):
        tql = 'fetch {\n  "data": { $p.* }\n};'
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, FetchClause)
        item = clause.items[0]
        assert isinstance(item, FetchNestedWildcard)


# ---------------------------------------------------------------------------
# Reduce clause tests
# ---------------------------------------------------------------------------


class TestReduceClause:
    def test_simple_reduce(self):
        tql = "reduce $count = count($p);"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, ReduceClause)
        assert len(clause.assignments) == 1
        assign = clause.assignments[0]
        assert isinstance(assign, ReduceAssignment)
        assert assign.variable == "$count"

    def test_reduce_with_groupby(self):
        tql = "reduce $count = count($p) groupby $type;"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, ReduceClause)
        assert clause.group_by == "$type"


# ---------------------------------------------------------------------------
# MatchLet clause tests
# ---------------------------------------------------------------------------


class TestMatchLetClause:
    def test_simple_let(self):
        tql = "match\nlet $x = count($p);"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchLetClause)
        assign = clause.assignments[0]
        assert isinstance(assign, LetAssignment)
        assert assign.variables == ["$x"]
        assert assign.is_stream is False

    def test_let_stream(self):
        tql = "match\nlet $x in func($p);"
        clauses = parse_typeql_query(tql)
        clause = clauses[0]
        assert isinstance(clause, MatchLetClause)
        assign = clause.assignments[0]
        assert assign.is_stream is True


# ---------------------------------------------------------------------------
# Multi-clause tests
# ---------------------------------------------------------------------------


class TestMultiClause:
    def test_match_insert(self):
        tql = 'match\n$p isa person;\ninsert\n$p has name "Alice";'
        clauses = parse_typeql_query(tql)
        assert len(clauses) == 2
        assert isinstance(clauses[0], MatchClause)
        assert isinstance(clauses[1], InsertClause)

    def test_match_delete(self):
        tql = "match\n$p isa person;\ndelete\n$p;"
        clauses = parse_typeql_query(tql)
        assert len(clauses) == 2
        assert isinstance(clauses[0], MatchClause)
        assert isinstance(clauses[1], DeleteClause)

    def test_match_fetch(self):
        tql = 'match\n$p isa person;\nfetch {\n  "data": $p.*\n};'
        clauses = parse_typeql_query(tql)
        assert len(clauses) == 2
        assert isinstance(clauses[0], MatchClause)
        assert isinstance(clauses[1], FetchClause)


# ---------------------------------------------------------------------------
# Roundtrip tests: compile(parse(tql)) == tql
# ---------------------------------------------------------------------------


class TestRoundtrip:
    def test_simple_match(self):
        tql = "match\n$p isa person;"
        assert _roundtrip(tql) == tql

    def test_match_with_constraints(self):
        tql = 'match\n$p isa person, has name "Alice", has age 30;'
        assert _roundtrip(tql) == tql

    def test_relation_match(self):
        tql = "match\n$r isa employment (employee: $p, employer: $c);"
        assert _roundtrip(tql) == tql

    def test_insert_isa(self):
        tql = "insert\n$p isa person;"
        assert _roundtrip(tql) == tql

    def test_insert_has(self):
        tql = 'insert\n$p has name "Alice";'
        assert _roundtrip(tql) == tql

    def test_delete_thing(self):
        tql = "delete\n$p;"
        assert _roundtrip(tql) == tql

    def test_match_insert_roundtrip(self):
        tql = 'match\n$p isa person;\ninsert\n$p has name "Bob";'
        assert _roundtrip(tql) == tql

    def test_fetch_roundtrip(self):
        tql = 'fetch {\n  "data": $p.*\n};'
        assert _roundtrip(tql) == tql

    def test_reduce_roundtrip(self):
        tql = "reduce $count = count($p);"
        assert _roundtrip(tql) == tql

    def test_subtype_roundtrip(self):
        tql = "match\n$t sub person;"
        assert _roundtrip(tql) == tql

    def test_has_pattern_roundtrip(self):
        tql = "match\n$p has name $n;"
        assert _roundtrip(tql) == tql

    def test_not_pattern_roundtrip(self):
        tql = "match\nnot { $p has name $n; };"
        assert _roundtrip(tql) == tql

    def test_or_pattern_roundtrip(self):
        tql = "match\n{ $p isa person; } or { $p isa company; };"
        assert _roundtrip(tql) == tql

    def test_iid_pattern_roundtrip(self):
        tql = "match\n$p iid 0xabc;"
        assert _roundtrip(tql) == tql

    def test_value_comparison_roundtrip(self):
        tql = "match\n$a > 10;"
        assert _roundtrip(tql) == tql


# ---------------------------------------------------------------------------
# Parity tests: Python AST -> compile -> parse -> assert equal
# ---------------------------------------------------------------------------


class TestParity:
    def test_entity_pattern_parity(self):
        original = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="person",
                    constraints=[
                        HasConstraint(
                            attr_name="name",
                            value=LiteralValue("Alice", "string"),
                        ),
                    ],
                ),
            ],
        )
        tql = compiler.compile(original)
        parsed_clauses = parse_typeql_query(tql)
        assert len(parsed_clauses) == 1
        parsed = parsed_clauses[0]
        assert isinstance(parsed, MatchClause)
        pat = parsed.patterns[0]
        assert isinstance(pat, EntityPattern)
        assert pat.variable == "$p"
        assert pat.type_name == "person"
        assert len(pat.constraints) == 1
        c = pat.constraints[0]
        assert isinstance(c, HasConstraint)
        assert c.attr_name == "name"

    def test_relation_pattern_parity(self):
        original = MatchClause(
            patterns=[
                RelationPattern(
                    variable="$r",
                    type_name="employment",
                    role_players=[
                        RolePlayer(role="employee", player_var="$p"),
                        RolePlayer(role="employer", player_var="$c"),
                    ],
                ),
            ],
        )
        tql = compiler.compile(original)
        parsed_clauses = parse_typeql_query(tql)
        parsed = parsed_clauses[0]
        assert isinstance(parsed, MatchClause)
        pat = parsed.patterns[0]
        assert isinstance(pat, RelationPattern)
        assert len(pat.role_players) == 2

    def test_insert_statement_parity(self):
        original = InsertClause(
            statements=[
                HasStatement(
                    subject_var="$p",
                    attr_name="age",
                    value=LiteralValue(30, "long"),
                ),
            ],
        )
        tql = compiler.compile(original)
        parsed_clauses = parse_typeql_query(tql)
        parsed = parsed_clauses[0]
        assert isinstance(parsed, InsertClause)
        stmt = parsed.statements[0]
        assert isinstance(stmt, HasStatement)
        assert stmt.subject_var == "$p"
        assert stmt.attr_name == "age"

    def test_relation_statement_parity(self):
        original = InsertClause(
            statements=[
                RelationStatement(
                    variable="$r",
                    type_name="employment",
                    role_players=[
                        RolePlayer(role="employee", player_var="$p"),
                        RolePlayer(role="employer", player_var="$c"),
                    ],
                    include_variable=False,
                ),
            ],
        )
        tql = compiler.compile(original)
        parsed_clauses = parse_typeql_query(tql)
        parsed = parsed_clauses[0]
        assert isinstance(parsed, InsertClause)
        stmt = parsed.statements[0]
        assert isinstance(stmt, RelationStatement)
        assert stmt.include_variable is False

    def test_delete_thing_parity(self):
        original = DeleteClause(
            statements=[DeleteThingStatement(variable="$p")],
        )
        tql = compiler.compile(original)
        parsed_clauses = parse_typeql_query(tql)
        parsed = parsed_clauses[0]
        assert isinstance(parsed, DeleteClause)
        stmt = parsed.statements[0]
        assert isinstance(stmt, DeleteThingStatement)
        assert stmt.variable == "$p"


# ---------------------------------------------------------------------------
# Error tests
# ---------------------------------------------------------------------------


class TestErrors:
    def test_invalid_input_raises_value_error(self):
        with pytest.raises(ValueError):
            parse_typeql_query("this is not valid typeql at all!!!")

    def test_empty_input(self):
        clauses = parse_typeql_query("")
        assert clauses == []

    def test_not_implemented_without_rust(self):
        """Verify the NotImplementedError path exists."""
        from type_bridge.query import parser as parser_mod

        old_available = parser_mod.RUST_AVAILABLE
        old_fn = parser_mod._rust_parse_typeql_query
        try:
            parser_mod.RUST_AVAILABLE = False
            with pytest.raises(NotImplementedError):
                parser_mod.parse_typeql_query("match\n$p isa person;")
        finally:
            parser_mod.RUST_AVAILABLE = old_available
            parser_mod._rust_parse_typeql_query = old_fn
