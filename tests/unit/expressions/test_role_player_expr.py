"""Unit tests for RolePlayerExpr type-safe role-player expression wrapper."""

from tests.utils.handwritten import Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.expressions import RolePlayerExpr


# Test models
class Name(String):
    pass


class Age(Integer):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)


# ============================================================
# Basic RolePlayerExpr tests
# ============================================================


def test_role_player_expr_init():
    """Test RolePlayerExpr initialization stores role name and inner expression."""
    inner_expr = Age.gt(Age(30))
    role_expr = RolePlayerExpr(
        role_name="employee",
        inner_expr=inner_expr,
        player_types=(Person,),
    )
    assert role_expr.role_name == "employee"
    assert role_expr.inner_expr is inner_expr
    assert role_expr.player_types == (Person,)


def test_role_player_expr_to_typeql_delegates_to_inner():
    """Test to_typeql delegates to inner expression with role variable."""
    inner_expr = Age.gt(Age(30))
    role_expr = RolePlayerExpr(
        role_name="employee",
        inner_expr=inner_expr,
        player_types=(Person,),
    )
    # The inner expression generates TypeQL with the role variable
    result = role_expr.to_typeql("$employee")
    # Should use unique variable name $employee__age (double underscore separator)
    assert "$employee has Age $employee__age" in result
    assert "$employee__age > 30" in result


def test_role_player_expr_get_attribute_types():
    """Test get_attribute_types delegates to inner expression."""
    inner_expr = Age.gt(Age(30))
    role_expr = RolePlayerExpr(
        role_name="employee",
        inner_expr=inner_expr,
        player_types=(Person,),
    )
    attr_types = role_expr.get_attribute_types()
    assert Age in attr_types


def test_role_player_expr_repr():
    """Test __repr__ returns useful debug information."""
    inner_expr = Age.gt(Age(30))
    role_expr = RolePlayerExpr(
        role_name="employee",
        inner_expr=inner_expr,
        player_types=(Person,),
    )
    repr_str = repr(role_expr)
    assert "RolePlayerExpr" in repr_str
    assert "employee" in repr_str
    assert "ComparisonExpr" in repr_str


# ============================================================
# TypeDB 3.x variable scoping tests
# ============================================================


def test_role_player_expr_unique_variable_naming():
    """Test that different role variables produce unique attribute variables.

    This is critical for TypeDB 3.x where using the same variable twice
    creates an implicit equality constraint.
    """
    inner_expr = Name.eq(Name("Alice"))

    # Create expressions for different roles
    actor_expr = RolePlayerExpr(
        role_name="actor",
        inner_expr=inner_expr,
        player_types=(Person,),
    )
    target_expr = RolePlayerExpr(
        role_name="target",
        inner_expr=inner_expr,
        player_types=(Person,),
    )

    actor_typeql = actor_expr.to_typeql("$actor")
    target_typeql = target_expr.to_typeql("$target")

    # Should use $actor__name and $target__name (double underscore), not both $name
    assert "$actor__name" in actor_typeql
    assert "$target__name" in target_typeql
    assert "$name" not in actor_typeql  # Should NOT use generic $name
    assert "$name" not in target_typeql


def test_role_player_expr_with_string_expr():
    """Test RolePlayerExpr works with StringExpr inner expressions."""
    inner_expr = Name.contains(Name("Tech"))
    role_expr = RolePlayerExpr(
        role_name="employer",
        inner_expr=inner_expr,
        player_types=(Company,),
    )
    result = role_expr.to_typeql("$employer")
    # Attribute type name is "Name" (class name), double underscore separator
    assert "$employer has Name $employer__name" in result
    assert "contains" in result


# ============================================================
# Role.multi (polymorphic roles) tests
# ============================================================


def test_role_player_expr_multi_player_types():
    """Test RolePlayerExpr stores multiple player types for Role.multi."""
    inner_expr = Name.eq(Name("Bot1"))
    role_expr = RolePlayerExpr(
        role_name="actor",
        inner_expr=inner_expr,
        player_types=(Person, Company),  # Simulating Role.multi
    )
    assert role_expr.player_types == (Person, Company)
    # to_typeql still works the same way (attr type name is "Name", double underscore)
    result = role_expr.to_typeql("$actor")
    assert "$actor has Name $actor__name" in result
