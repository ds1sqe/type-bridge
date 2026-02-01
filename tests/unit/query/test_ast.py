"""Unit tests for AST nodes."""

from type_bridge.query.ast import (
    EntityPattern,
    HasConstraint,
    IidConstraint,
    LiteralValue,
    MatchClause,
    RelationPattern,
    RolePlayer,
)


def test_entity_pattern_creation():
    pattern = EntityPattern(
        variable="$p",
        type_name="person",
        constraints=[
            IidConstraint(iid="0x123"),
            HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
        ],
    )
    assert pattern.variable == "$p"
    assert pattern.type_name == "person"
    assert len(pattern.constraints) == 2
    assert isinstance(pattern.constraints[0], IidConstraint)


def test_relation_pattern_creation():
    pattern = RelationPattern(
        variable="$rel",
        type_name="employment",
        role_players=[
            RolePlayer(role="employee", player_var="$p"),
            RolePlayer(role="employer", player_var="$c"),
        ],
    )
    assert pattern.variable == "$rel"
    assert len(pattern.role_players) == 2
    assert pattern.role_players[0].role == "employee"
    assert pattern.role_players[0].player_var == "$p"


def test_match_clause_structure():
    e_pat = EntityPattern(variable="$x", type_name="person")
    match = MatchClause(patterns=[e_pat])
    assert len(match.patterns) == 1
    assert match.patterns[0] == e_pat
