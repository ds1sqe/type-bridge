"""Unit tests for role-player lookup filter parsing."""

import pytest
from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.crud.lookup import build_lookup_expression
from type_bridge.crud.role_lookup import parse_role_lookup_filters
from type_bridge.expressions import (
    AttributeExistsExpr,
    BooleanExpr,
    ComparisonExpr,
    RolePlayerExpr,
    StringExpr,
)


# Test models
class Name(String):
    pass


class Age(Integer):
    pass


class Industry(String):
    pass


class Salary(Integer):
    pass


class Position(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)
    industry: Industry | None = None


class Bot(Entity):
    flags = TypeFlags(name="bot")
    name: Name = Flag(Key)


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position | None = None
    salary: Salary | None = None


class Interaction(Relation):
    """Relation with Role.multi for testing polymorphic roles."""

    flags = TypeFlags(name="interaction")
    actor: Role[Person | Bot] = Role.multi("actor", Person, Bot)
    target: Role[Person] = Role("target", Person)


# ============================================================
# Basic parsing tests
# ============================================================


def test_parse_exact_role_player_filter():
    """Test entity instance filter passes through to role_player_filters."""
    person = Person(name=Name("Alice"))
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"employee": person}
    )
    assert attr_filters == {}
    assert role_filters == {"employee": person}
    assert role_exprs == {}
    assert attr_exprs == []


def test_parse_exact_attribute_filter():
    """Test relation attribute filter passes through to attr_filters."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"position": "Engineer"}
    )
    assert attr_filters == {"position": "Engineer"}
    assert role_filters == {}
    assert role_exprs == {}
    assert attr_exprs == []


def test_parse_role_attribute_exact():
    """Test role__attr pattern creates exact match expression wrapped in RolePlayerExpr."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"employee__name": "Alice"}
    )
    assert attr_filters == {}
    assert role_filters == {}
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 1
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert role_expr.role_name == "employee"
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert attr_exprs == []


def test_parse_role_attribute_with_lookup():
    """Test role__attr__lookup pattern creates expression wrapped in RolePlayerExpr."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"employee__age__gt": 30}
    )
    assert attr_filters == {}
    assert role_filters == {}
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 1
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert role_expr.role_name == "employee"
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert role_expr.inner_expr.operator == ">"
    assert attr_exprs == []


# ============================================================
# Comparison operator tests
# ============================================================


def test_parse_role_lookup_gt():
    """Test __gt lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__gt": 30})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert role_expr.inner_expr.operator == ">"


def test_parse_role_lookup_gte():
    """Test __gte lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__gte": 30})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert role_expr.inner_expr.operator == ">="


def test_parse_role_lookup_lt():
    """Test __lt lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__lt": 50})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert role_expr.inner_expr.operator == "<"


def test_parse_role_lookup_lte():
    """Test __lte lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__lte": 50})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, ComparisonExpr)
    assert role_expr.inner_expr.operator == "<="


# ============================================================
# String lookup tests
# ============================================================


def test_parse_role_lookup_contains():
    """Test __contains lookup on string attribute wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employer__name__contains": "Tech"}
    )
    role_expr = role_exprs["employer"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, StringExpr)
    assert role_expr.inner_expr.operation == "contains"


def test_parse_role_lookup_startswith():
    """Test __startswith lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employer__name__startswith": "Tech"}
    )
    role_expr = role_exprs["employer"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, StringExpr)
    assert role_expr.inner_expr.operation == "regex"  # startswith uses regex internally


def test_parse_role_lookup_endswith():
    """Test __endswith lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employer__name__endswith": "Corp"}
    )
    role_expr = role_exprs["employer"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, StringExpr)


def test_parse_role_lookup_regex():
    """Test __regex lookup wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employer__name__regex": "^Tech.*"}
    )
    role_expr = role_exprs["employer"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, StringExpr)
    assert role_expr.inner_expr.operation == "regex"


def test_string_lookup_on_non_string_raises():
    """Test string lookups on non-string attribute raises error."""
    with pytest.raises(ValueError, match="Unsupported lookup operator 'contains' for Age"):
        parse_role_lookup_filters(Employment, {"employee__age__contains": "30"})


# ============================================================
# Membership and null tests
# ============================================================


def test_parse_role_lookup_in():
    """Test __in lookup creates OR expression wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employee__name__in": ["Alice", "Bob"]}
    )
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, BooleanExpr)
    assert role_expr.inner_expr.operation == "or"


def test_parse_role_lookup_in_creates_flat_or_not_nested():
    """Test that __in lookup creates a flat OR with all operands at same level.

    This is critical for avoiding TypeDB query planner stack overflow with
    many values. A flat OR like (a or b or c or d) is safe, but a nested
    binary tree like (((a or b) or c) or d) causes stack overflow with 75+ values.

    See: https://github.com/ds1sqe/type-bridge/issues/76
    """
    values = [f"name_{i}" for i in range(100)]  # 100 values
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__name__in": values})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, BooleanExpr)
    assert role_expr.inner_expr.operation == "or"
    # All operands should be at the same level (flat structure)
    assert len(role_expr.inner_expr.operands) == 100
    # Each operand should be a ComparisonExpr, not a nested BooleanExpr
    for operand in role_expr.inner_expr.operands:
        assert isinstance(operand, ComparisonExpr)


def test_parse_role_lookup_in_single_value_no_boolean_expr():
    """Test that __in lookup with a single value returns just the comparison."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__name__in": ["Alice"]})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    # Single value should not wrap in BooleanExpr
    assert isinstance(role_expr.inner_expr, ComparisonExpr)


def test_parse_role_lookup_in_requires_iterable():
    """Test __in lookup requires iterable."""
    with pytest.raises(ValueError, match="requires an iterable"):
        parse_role_lookup_filters(Employment, {"employee__name__in": "Alice"})


def test_parse_role_lookup_in_requires_non_empty():
    """Test __in lookup requires non-empty iterable."""
    with pytest.raises(ValueError, match="non-empty"):
        parse_role_lookup_filters(Employment, {"employee__name__in": []})


def test_parse_role_lookup_isnull_true():
    """Test __isnull=True creates AttributeExistsExpr wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__isnull": True})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, AttributeExistsExpr)
    assert role_expr.inner_expr.present is False


def test_parse_role_lookup_isnull_false():
    """Test __isnull=False creates AttributeExistsExpr wrapped in RolePlayerExpr."""
    _, _, role_exprs, _ = parse_role_lookup_filters(Employment, {"employee__age__isnull": False})
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, AttributeExistsExpr)
    assert role_expr.inner_expr.present is True


def test_parse_role_lookup_isnull_requires_bool():
    """Test __isnull lookup requires boolean value."""
    with pytest.raises(ValueError, match="expects a boolean"):
        parse_role_lookup_filters(Employment, {"employee__age__isnull": "yes"})


# ============================================================
# Combined filter tests
# ============================================================


def test_parse_mixed_filters():
    """Test parsing mix of role lookups, role instances, and attribute filters."""
    person = Person(name=Name("Alice"))
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment,
        {
            "position": "Engineer",
            "employer": Company(name=Name("TechCorp")),
            "employee__age__gt": 25,
        },
    )
    assert attr_filters == {"position": "Engineer"}
    assert "employer" in role_filters
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 1
    assert attr_exprs == []


def test_parse_multiple_lookups_same_role():
    """Test multiple lookups on the same role create multiple expressions."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employee__age__gt": 25, "employee__age__lt": 50}
    )
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 2


def test_parse_lookups_multiple_roles():
    """Test lookups on different roles."""
    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment,
        {"employee__age__gt": 25, "employer__name__contains": "Tech"},
    )
    assert "employee" in role_exprs
    assert "employer" in role_exprs
    assert len(role_exprs["employee"]) == 1
    assert len(role_exprs["employer"]) == 1


# ============================================================
# Role.multi tests
# ============================================================


def test_role_multi_attribute_on_any_player_type():
    """Test attribute valid if owned by ANY player type in Role.multi."""
    # 'name' exists on both Person and Bot
    _, _, role_exprs, _ = parse_role_lookup_filters(Interaction, {"actor__name": "Alice"})
    assert "actor" in role_exprs
    assert len(role_exprs["actor"]) == 1


def test_role_multi_attribute_on_one_player_only():
    """Test attribute valid even if only one player type has it."""
    # 'age' exists only on Person, not Bot - should still work
    _, _, role_exprs, _ = parse_role_lookup_filters(Interaction, {"actor__age__gt": 25})
    assert "actor" in role_exprs
    assert len(role_exprs["actor"]) == 1


# ============================================================
# Error handling tests
# ============================================================


def test_unknown_role_raises():
    """Test unknown role name raises ValueError."""
    with pytest.raises(ValueError, match="Unknown filter field 'unknown'"):
        parse_role_lookup_filters(Employment, {"unknown__age__gt": 30})


def test_unknown_attribute_raises():
    """Test unknown attribute on role player raises ValueError."""
    with pytest.raises(ValueError, match="do not have attribute 'unknown'"):
        parse_role_lookup_filters(Employment, {"employee__unknown": "value"})


def test_unknown_lookup_operator_raises():
    """Test unknown lookup operator raises ValueError."""
    with pytest.raises(ValueError, match="Unsupported lookup operator"):
        parse_role_lookup_filters(Employment, {"employee__name__bogus": "value"})


def test_role_lookup_without_attribute_raises():
    """Test role lookup without attribute specification raises error."""
    # This would be "employee__" which splits to ["employee", ""]
    with pytest.raises(ValueError):
        parse_role_lookup_filters(Employment, {"employee__": "value"})


def test_too_many_parts_raises():
    """Test too many __ parts raises error."""
    with pytest.raises(ValueError, match="too many parts"):
        parse_role_lookup_filters(Employment, {"employee__age__gt__extra": 30})


# ============================================================
# _build_lookup_expression tests
# ============================================================


def test_build_expression_wraps_raw_value():
    """Test raw values are wrapped in attribute type."""
    expr = build_lookup_expression(Age, "gt", 30)
    assert isinstance(expr, ComparisonExpr)
    assert expr.value.value == 30


def test_build_expression_preserves_wrapped_value():
    """Test already-wrapped values are preserved."""
    expr = build_lookup_expression(Age, "gt", Age(30))
    assert isinstance(expr, ComparisonExpr)
    assert expr.value.value == 30


# ============================================================
# Relation attribute lookup tests
# ============================================================


def test_parse_relation_attribute_lookup_gt():
    """Test relation attribute lookup creates expression in attr_exprs."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"salary__gt": 85000}
    )
    assert attr_filters == {}
    assert role_filters == {}
    assert role_exprs == {}
    assert len(attr_exprs) == 1
    assert isinstance(attr_exprs[0], ComparisonExpr)
    assert attr_exprs[0].operator == ">"


def test_parse_relation_attribute_lookup_contains():
    """Test relation string attribute lookup creates expression."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"position__contains": "Engineer"}
    )
    assert attr_filters == {}
    assert len(attr_exprs) == 1
    assert isinstance(attr_exprs[0], StringExpr)
    assert attr_exprs[0].operation == "contains"


def test_parse_mixed_with_relation_attribute_lookup():
    """Test parsing mix of role lookups and relation attribute lookups."""
    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment,
        {
            "employee__age__gt": 25,
            "salary__gt": 85000,
        },
    )
    assert attr_filters == {}
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 1
    assert len(attr_exprs) == 1
    assert isinstance(attr_exprs[0], ComparisonExpr)


# ============================================================
# iid__in lookup tests
# ============================================================


def test_parse_relation_iid_in():
    """Test iid__in lookup on relation itself creates OR of IidExpr."""
    from type_bridge.expressions import IidExpr

    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment, {"iid__in": ["0x1a2b3c4d", "0x5e6f7a8b"]}
    )
    assert attr_filters == {}
    assert role_filters == {}
    assert role_exprs == {}
    assert len(attr_exprs) == 1
    expr = attr_exprs[0]
    assert isinstance(expr, BooleanExpr)
    assert expr.operation == "or"
    assert len(expr.operands) == 2
    assert all(isinstance(e, IidExpr) for e in expr.operands)


def test_parse_relation_iid_in_single_value():
    """Test iid__in with single value returns just IidExpr."""
    from type_bridge.expressions import IidExpr

    _, _, _, attr_exprs = parse_role_lookup_filters(Employment, {"iid__in": ["0x1a2b3c4d"]})
    assert len(attr_exprs) == 1
    assert isinstance(attr_exprs[0], IidExpr)


def test_parse_relation_iid_in_requires_iterable():
    """Test iid__in requires iterable."""
    with pytest.raises(ValueError, match="requires an iterable"):
        parse_role_lookup_filters(Employment, {"iid__in": "0x1a2b3c4d"})


def test_parse_relation_iid_in_requires_non_empty():
    """Test iid__in requires non-empty iterable."""
    with pytest.raises(ValueError, match="non-empty"):
        parse_role_lookup_filters(Employment, {"iid__in": []})


def test_parse_role_iid_in():
    """Test role__iid__in lookup on role player creates RolePlayerExpr with IidExpr."""
    from type_bridge.expressions import IidExpr

    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employee__iid__in": ["0x1a2b3c4d", "0x5e6f7a8b"]}
    )
    assert "employee" in role_exprs
    assert len(role_exprs["employee"]) == 1
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert role_expr.role_name == "employee"
    assert isinstance(role_expr.inner_expr, BooleanExpr)
    assert role_expr.inner_expr.operation == "or"
    assert len(role_expr.inner_expr.operands) == 2
    assert all(isinstance(e, IidExpr) for e in role_expr.inner_expr.operands)


def test_parse_role_iid_in_single_value():
    """Test role__iid__in with single value returns IidExpr directly."""
    from type_bridge.expressions import IidExpr

    _, _, role_exprs, _ = parse_role_lookup_filters(
        Employment, {"employee__iid__in": ["0x1a2b3c4d"]}
    )
    role_expr = role_exprs["employee"][0]
    assert isinstance(role_expr, RolePlayerExpr)
    assert isinstance(role_expr.inner_expr, IidExpr)


def test_parse_role_iid_in_requires_iterable():
    """Test role__iid__in requires iterable."""
    with pytest.raises(ValueError, match="requires an iterable"):
        parse_role_lookup_filters(Employment, {"employee__iid__in": "0x1a2b3c4d"})


def test_parse_role_iid_in_requires_non_empty():
    """Test role__iid__in requires non-empty iterable."""
    with pytest.raises(ValueError, match="non-empty"):
        parse_role_lookup_filters(Employment, {"employee__iid__in": []})


def test_parse_role_iid_in_with_other_filters():
    """Test role__iid__in combined with other filters."""
    from type_bridge.expressions import IidExpr

    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        Employment,
        {
            "employee__iid__in": ["0x1a2b3c4d"],
            "employer__name__contains": "Tech",
            "salary__gt": 50000,
        },
    )
    assert attr_filters == {}
    assert "employee" in role_exprs
    assert "employer" in role_exprs
    # Employee has IID filter
    employee_expr = role_exprs["employee"][0]
    assert isinstance(employee_expr.inner_expr, IidExpr)
    # Employer has string filter
    employer_expr = role_exprs["employer"][0]
    assert isinstance(employer_expr.inner_expr, StringExpr)
    # Salary has comparison
    assert len(attr_exprs) == 1
    assert isinstance(attr_exprs[0], ComparisonExpr)
