"""Unit tests for Relation.to_ast method."""

from type_bridge import (
    Boolean,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.query.ast import InsertClause, LiteralValue, RelationStatement


class Name(String):
    pass


class Salary(Integer):
    pass


class FullTime(Boolean):
    pass


class Employee(Entity):
    flags = TypeFlags(name="employee")
    name: Name = Flag(Key)


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Employee] = Role("employee", Employee)
    employer: Role[Company] = Role("employer", Company)
    salary: Salary | None = None
    full_time: FullTime | None = None


def test_relation_to_ast_basic():
    """Test basic AST generation for a relation."""
    emp = Employee(name=Name("Alice"))
    company = Company(name=Name("TechCorp"))
    employment = Employment(employee=emp, employer=company, salary=Salary(50000))

    result = employment.to_ast("$r")

    assert isinstance(result, InsertClause)
    assert len(result.statements) == 1

    rel_stmt = result.statements[0]
    assert isinstance(rel_stmt, RelationStatement)
    assert rel_stmt.type_name == "employment"
    assert len(rel_stmt.role_players) == 2


def test_relation_to_ast_integer_attribute():
    """Test AST generation correctly types integer attributes."""
    emp = Employee(name=Name("Alice"))
    company = Company(name=Name("TechCorp"))
    employment = Employment(employee=emp, employer=company, salary=Salary(75000))

    result = employment.to_ast("$r")
    rel_stmt = result.statements[0]
    assert isinstance(rel_stmt, RelationStatement)

    # Find salary attribute (uses class name)
    salary_attr = next(a for a in rel_stmt.attributes if a.attr_name == "Salary")

    assert isinstance(salary_attr.value, LiteralValue)
    assert salary_attr.value.value == 75000
    assert salary_attr.value.value_type == "long"


def test_relation_to_ast_boolean_attribute():
    """Test AST generation correctly types boolean attributes."""
    emp = Employee(name=Name("Alice"))
    company = Company(name=Name("TechCorp"))
    employment = Employment(employee=emp, employer=company, full_time=FullTime(True))

    result = employment.to_ast("$r")
    rel_stmt = result.statements[0]
    assert isinstance(rel_stmt, RelationStatement)

    # Find full_time attribute (uses class name)
    ft_attr = next(a for a in rel_stmt.attributes if a.attr_name == "FullTime")

    assert isinstance(ft_attr.value, LiteralValue)
    assert ft_attr.value.value is True
    assert ft_attr.value.value_type == "boolean"


def test_relation_to_ast_type_refinement():
    """Test that type refinement works for relation attributes."""
    emp = Employee(name=Name("Alice"))
    company = Company(name=Name("TechCorp"))
    employment = Employment(employee=emp, employer=company, full_time=FullTime(False))

    result = employment.to_ast("$r")
    rel_stmt = result.statements[0]
    assert isinstance(rel_stmt, RelationStatement)

    ft_attr = next(a for a in rel_stmt.attributes if a.attr_name == "FullTime")

    # Verify boolean type refinement works
    assert isinstance(ft_attr.value, LiteralValue)
    assert ft_attr.value.value_type == "boolean"
    assert ft_attr.value.value is False
