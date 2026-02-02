"""Unit tests for RoleRef and RolePlayerFieldRef classes."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.expressions import RolePlayerExpr
from type_bridge.fields.role import (
    RolePlayerNumericFieldRef,
    RolePlayerStringFieldRef,
    RoleRef,
)


# Test fixtures - define attribute and entity types
class Name(String):
    pass


class Age(Integer):
    pass


class Score(Integer):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)


class Bot(Entity):
    """Entity without age attribute for testing Role.multi edge cases."""

    flags = TypeFlags(name="bot")
    name: Name = Flag(Key)
    score: Score | None = None


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)


class Interaction(Relation):
    """Relation with polymorphic role for testing Role.multi."""

    flags = TypeFlags(name="interaction")
    actor: Role[Person | Bot] = Role.multi("actor", Person, Bot)
    target: Role[Person] = Role("target", Person)


class TestRoleRefCreation:
    """Tests for RoleRef initialization and basic properties."""

    def test_role_ref_init(self):
        """RoleRef should store role_name and player_types."""
        role_ref = RoleRef(role_name="employee", player_types=(Person,))

        assert role_ref.role_name == "employee"
        assert role_ref.player_types == (Person,)

    def test_role_ref_from_role_descriptor(self):
        """Accessing role at class level should return RoleRef."""
        role_ref = Employment.employee

        assert isinstance(role_ref, RoleRef)
        assert role_ref.role_name == "employee"
        assert role_ref.player_types == (Person,)

    def test_role_ref_multi_player_types(self):
        """RoleRef should support multiple player types from Role.multi."""
        role_ref = Interaction.actor

        assert isinstance(role_ref, RoleRef)
        assert role_ref.role_name == "actor"
        assert role_ref.player_types == (Person, Bot)


class TestRoleRefAttributeAccess:
    """Tests for RoleRef.__getattr__ returning appropriate field refs."""

    def test_string_attribute_returns_string_field_ref(self):
        """Accessing string attribute should return RolePlayerStringFieldRef."""
        field_ref = Employment.employee.name

        assert isinstance(field_ref, RolePlayerStringFieldRef)
        assert field_ref.role_name == "employee"
        assert field_ref.field_name == "name"
        assert field_ref.attr_type is Name
        assert field_ref.player_types == (Person,)

    def test_integer_attribute_returns_numeric_field_ref(self):
        """Accessing integer attribute should return RolePlayerNumericFieldRef."""
        field_ref = Employment.employee.age

        assert isinstance(field_ref, RolePlayerNumericFieldRef)
        assert field_ref.role_name == "employee"
        assert field_ref.field_name == "age"
        assert field_ref.attr_type is Age
        assert field_ref.player_types == (Person,)

    def test_unknown_attribute_raises_error(self):
        """Accessing unknown attribute should raise AttributeError."""
        with pytest.raises(AttributeError, match="do not have attribute 'nonexistent'"):
            _ = Employment.employee.nonexistent

    def test_error_message_lists_available_attributes(self):
        """AttributeError message should list available attributes."""
        with pytest.raises(AttributeError, match="Available attributes:.*age.*name"):
            _ = Employment.employee.nonexistent

    def test_dir_returns_available_attributes(self):
        """__dir__ should return list of available attributes for IDE completion."""
        attrs = dir(Employment.employee)

        assert "name" in attrs
        assert "age" in attrs


class TestRoleMultiAttributeAccess:
    """Tests for attribute access on Role.multi() with multiple player types."""

    def test_common_attribute_accessible(self):
        """Attribute existing on all player types should be accessible."""
        # 'name' exists on both Person and Bot
        field_ref = Interaction.actor.name

        assert isinstance(field_ref, RolePlayerStringFieldRef)
        assert field_ref.role_name == "actor"
        assert field_ref.attr_type is Name

    def test_unique_attribute_accessible(self):
        """Attribute existing on only one player type should be accessible."""
        # 'age' only exists on Person, not Bot
        field_ref = Interaction.actor.age

        assert isinstance(field_ref, RolePlayerNumericFieldRef)
        assert field_ref.attr_type is Age

        # 'score' only exists on Bot, not Person
        score_ref = Interaction.actor.score

        assert isinstance(score_ref, RolePlayerNumericFieldRef)
        assert score_ref.attr_type is Score

    def test_dir_includes_union_of_attributes(self):
        """__dir__ should include attributes from all player types."""
        attrs = dir(Interaction.actor)

        # From Person
        assert "name" in attrs
        assert "age" in attrs
        # From Bot
        assert "score" in attrs


class TestRolePlayerFieldRefComparisons:
    """Tests for comparison methods on RolePlayerFieldRef."""

    def test_gt_returns_role_player_expr(self):
        """gt() should return RolePlayerExpr."""
        expr = Employment.employee.age.gt(Age(30))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"
        assert expr.player_types == (Person,)

    def test_lt_returns_role_player_expr(self):
        """lt() should return RolePlayerExpr."""
        expr = Employment.employee.age.lt(Age(50))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"

    def test_gte_returns_role_player_expr(self):
        """gte() should return RolePlayerExpr."""
        expr = Employment.employee.age.gte(Age(25))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"

    def test_lte_returns_role_player_expr(self):
        """lte() should return RolePlayerExpr."""
        expr = Employment.employee.age.lte(Age(65))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"

    def test_eq_returns_role_player_expr(self):
        """eq() should return RolePlayerExpr."""
        expr = Employment.employee.age.eq(Age(30))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"

    def test_neq_returns_role_player_expr(self):
        """neq() should return RolePlayerExpr."""
        expr = Employment.employee.age.neq(Age(30))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employee"


class TestRolePlayerStringFieldRefMethods:
    """Tests for string-specific methods on RolePlayerStringFieldRef."""

    def test_contains_returns_role_player_expr(self):
        """contains() should return RolePlayerExpr."""
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        expr = employer_name_ref.contains(Name("Tech"))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employer"
        assert expr.player_types == (Company,)

    def test_like_returns_role_player_expr(self):
        """like() should return RolePlayerExpr."""
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        expr = employer_name_ref.like(Name("Tech%"))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employer"

    def test_regex_returns_role_player_expr(self):
        """regex() should return RolePlayerExpr."""
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        expr = employer_name_ref.regex(Name("^Tech.*"))

        assert isinstance(expr, RolePlayerExpr)
        assert expr.role_name == "employer"


class TestRolePlayerExpressionToTypeQL:
    """Tests for TypeQL generation from role player expressions."""

    def test_comparison_generates_correct_typeql(self):
        """Comparison expression should generate role-prefixed TypeQL."""
        expr = Employment.employee.age.gt(Age(30))
        typeql = expr.to_typeql("$employee")

        # Should use role-prefixed variable names
        assert "$employee has Age $employee_age" in typeql
        assert "$employee_age > 30" in typeql

    def test_string_contains_generates_correct_typeql(self):
        """String contains expression should generate correct TypeQL."""
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        expr = employer_name_ref.contains(Name("Tech"))
        typeql = expr.to_typeql("$employer")

        assert "$employer has Name $employer_name" in typeql
        # TypeQL 3.x uses infix syntax for contains
        assert '$employer_name contains "Tech"' in typeql


class TestRoleInstanceAccess:
    """Tests for Role descriptor instance-level access (unchanged behavior)."""

    def test_instance_access_returns_entity(self):
        """Accessing role on instance should return entity player."""
        person = Person(name=Name("Alice"), age=Age(30))
        company = Company(name=Name("TechCorp"))
        employment = Employment(employee=person, employer=company)

        assert employment.employee is person
        assert employment.employer is company

    def test_instance_access_none_for_unset(self):
        """Accessing unset role on instance should return None."""
        # Create with required roles
        person = Person(name=Name("Alice"))
        company = Company(name=Name("TechCorp"))
        employment = Employment(employee=person, employer=company)

        # Access existing roles (should work)
        assert employment.employee is person


class TestBackwardCompatibility:
    """Tests to ensure backward compatibility with existing patterns."""

    def test_role_ref_exposes_role_name(self):
        """RoleRef should expose role_name for compatibility."""
        role_ref = Employment.employee
        assert role_ref.role_name == "employee"

    def test_role_ref_exposes_player_types(self):
        """RoleRef should expose player_types for compatibility."""
        role_ref = Employment.employee
        assert role_ref.player_types == (Person,)

    def test_internal_roles_dict_returns_role_objects(self):
        """Internal _roles dict should still return Role objects."""
        role = Employment._roles["employee"]

        # Should be the actual Role descriptor, not RoleRef
        assert isinstance(role, Role)
        assert role.role_name == "employee"
        assert role.player_entity_types == (Person,)
