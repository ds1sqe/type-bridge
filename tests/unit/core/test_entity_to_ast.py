"""Unit tests for Entity.to_ast method."""

from type_bridge import Boolean, Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.query.ast import HasStatement, InsertClause, IsaStatement, LiteralValue


class Name(String):
    pass


class Age(Integer):
    pass


class Active(Boolean):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None
    active: Active | None = None


def test_to_ast_basic():
    """Test basic AST generation for an entity."""
    person = Person(name=Name("Alice"), age=Age(30))
    result = person.to_ast("$p")

    assert isinstance(result, InsertClause)
    assert len(result.statements) == 3  # isa + 2 has

    # First statement should be isa
    isa_stmt = result.statements[0]
    assert isinstance(isa_stmt, IsaStatement)
    assert isa_stmt.variable == "$p"
    assert isa_stmt.type_name == "person"


def test_to_ast_string_attribute():
    """Test AST generation correctly types string attributes."""
    person = Person(name=Name("Alice"))
    result = person.to_ast("$p")

    # Find the name has statement (attribute name uses class name)
    has_stmts = [s for s in result.statements if isinstance(s, HasStatement)]
    name_stmt = next(s for s in has_stmts if s.attr_name == "Name")

    assert isinstance(name_stmt.value, LiteralValue)
    assert name_stmt.value.value == "Alice"
    assert name_stmt.value.value_type == "string"


def test_to_ast_integer_attribute():
    """Test AST generation correctly types integer attributes."""
    person = Person(name=Name("Alice"), age=Age(30))
    result = person.to_ast("$p")

    has_stmts = [s for s in result.statements if isinstance(s, HasStatement)]
    age_stmt = next(s for s in has_stmts if s.attr_name == "Age")

    assert isinstance(age_stmt.value, LiteralValue)
    assert age_stmt.value.value == 30
    assert age_stmt.value.value_type == "long"


def test_to_ast_boolean_attribute():
    """Test AST generation correctly types boolean attributes."""
    person = Person(name=Name("Alice"), active=Active(True))
    result = person.to_ast("$p")

    has_stmts = [s for s in result.statements if isinstance(s, HasStatement)]
    active_stmt = next(s for s in has_stmts if s.attr_name == "Active")

    assert isinstance(active_stmt.value, LiteralValue)
    assert active_stmt.value.value is True
    assert active_stmt.value.value_type == "boolean"


def test_to_ast_type_refinement_bool_from_raw():
    """Test that bool values are refined to 'boolean' type even without attribute type hint."""
    # This tests the type refinement logic where raw bool values
    # get correctly typed as "boolean" in the AST
    person = Person(name=Name("Alice"), active=Active(False))
    result = person.to_ast("$p")

    has_stmts = [s for s in result.statements if isinstance(s, HasStatement)]
    active_stmt = next(s for s in has_stmts if s.attr_name == "Active")

    assert active_stmt.value.value_type == "boolean"
    assert active_stmt.value.value is False
