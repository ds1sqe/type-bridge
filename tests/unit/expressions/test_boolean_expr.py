"""Unit tests for BooleanExpr flattening behavior."""

import pytest

from type_bridge import Integer, String
from type_bridge.expressions import BooleanExpr, ComparisonExpr


class Name(String):
    pass


class Age(Integer):
    pass


# ============================================================
# Basic BooleanExpr tests
# ============================================================


def test_boolean_expr_requires_two_operands_for_and():
    """Test AND operation requires at least 2 operands."""
    expr1 = Name.eq(Name("Alice"))
    with pytest.raises(ValueError, match="at least 2 operands"):
        BooleanExpr("and", [expr1])


def test_boolean_expr_requires_two_operands_for_or():
    """Test OR operation requires at least 2 operands."""
    expr1 = Name.eq(Name("Alice"))
    with pytest.raises(ValueError, match="at least 2 operands"):
        BooleanExpr("or", [expr1])


def test_boolean_expr_not_requires_one_operand():
    """Test NOT operation requires exactly 1 operand."""
    expr1 = Name.eq(Name("Alice"))
    expr2 = Name.eq(Name("Bob"))
    with pytest.raises(ValueError, match="exactly 1 operand"):
        BooleanExpr("not", [expr1, expr2])


# ============================================================
# Flattening tests for .or_()
# ============================================================


def test_or_flattens_when_chained():
    """Test that chaining .or_() creates flat structure, not nested binary tree.

    This is critical for avoiding TypeDB query planner stack overflow.
    See: https://github.com/ds1sqe/type-bridge/issues/76
    """
    a = Name.eq(Name("Alice"))
    b = Name.eq(Name("Bob"))
    c = Name.eq(Name("Charlie"))
    d = Name.eq(Name("David"))

    # Chain multiple .or_() calls
    result = a.or_(b).or_(c).or_(d)

    # Should be flat: (a OR b OR c OR d) with 4 operands at same level
    assert isinstance(result, BooleanExpr)
    assert result.operation == "or"
    assert len(result.operands) == 4

    # All operands should be ComparisonExpr, not nested BooleanExpr
    for operand in result.operands:
        assert isinstance(operand, ComparisonExpr)


def test_or_flattens_many_operands():
    """Test flattening works with many operands (the original issue scenario)."""
    expressions = [Name.eq(Name(f"name_{i}")) for i in range(100)]

    # Build using chained .or_() calls
    result = expressions[0]
    for expr in expressions[1:]:
        result = result.or_(expr)

    # Should be flat with 100 operands
    assert isinstance(result, BooleanExpr)
    assert result.operation == "or"
    assert len(result.operands) == 100


def test_or_wraps_different_operation():
    """Test that OR wraps AND expression (doesn't flatten different operations)."""
    a = Name.eq(Name("Alice"))
    b = Name.eq(Name("Bob"))
    c = Name.eq(Name("Charlie"))

    # (a AND b) OR c - should NOT flatten
    and_expr = BooleanExpr("and", [a, b])
    result = and_expr.or_(c)

    assert isinstance(result, BooleanExpr)
    assert result.operation == "or"
    assert len(result.operands) == 2
    assert isinstance(result.operands[0], BooleanExpr)
    assert result.operands[0].operation == "and"


# ============================================================
# Flattening tests for .and_()
# ============================================================


def test_and_flattens_when_chained():
    """Test that chaining .and_() creates flat structure."""
    a = Name.eq(Name("Alice"))
    b = Age.gt(Age(18))
    c = Age.lt(Age(65))
    d = Name.eq(Name("Bob"))

    # Chain multiple .and_() calls
    result = a.and_(b).and_(c).and_(d)

    # Should be flat: (a AND b AND c AND d) with 4 operands at same level
    assert isinstance(result, BooleanExpr)
    assert result.operation == "and"
    assert len(result.operands) == 4


def test_and_wraps_different_operation():
    """Test that AND wraps OR expression (doesn't flatten different operations)."""
    a = Name.eq(Name("Alice"))
    b = Name.eq(Name("Bob"))
    c = Age.gt(Age(18))

    # (a OR b) AND c - should NOT flatten
    or_expr = BooleanExpr("or", [a, b])
    result = or_expr.and_(c)

    assert isinstance(result, BooleanExpr)
    assert result.operation == "and"
    assert len(result.operands) == 2
    assert isinstance(result.operands[0], BooleanExpr)
    assert result.operands[0].operation == "or"


# ============================================================
# Mixed operation tests
# ============================================================


def test_complex_expression_preserves_structure():
    """Test that complex expressions preserve logical structure."""
    a = Name.eq(Name("Alice"))
    b = Name.eq(Name("Bob"))
    c = Age.gt(Age(18))
    d = Age.lt(Age(65))

    # (a OR b) AND (c AND d)
    or_expr = a.or_(b)  # BooleanExpr("or", [a, b])
    and_expr = c.and_(d)  # BooleanExpr("and", [c, d])
    result = or_expr.and_(and_expr)

    assert result.operation == "and"
    assert len(result.operands) == 2
    # Check nested operands are the expected BooleanExpr types
    assert isinstance(result.operands[0], BooleanExpr)
    assert isinstance(result.operands[1], BooleanExpr)
    assert result.operands[0].operation == "or"
    assert result.operands[1].operation == "and"


def test_not_followed_by_or_wraps():
    """Test NOT expression followed by OR wraps correctly."""
    a = Name.eq(Name("Alice"))
    b = Name.eq(Name("Bob"))

    # NOT(a) OR b
    not_expr = a.not_()
    result = not_expr.or_(b)

    assert result.operation == "or"
    assert len(result.operands) == 2
    assert isinstance(result.operands[0], BooleanExpr)
    assert result.operands[0].operation == "not"
