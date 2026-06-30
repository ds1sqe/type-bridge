"""Test Boolean attribute type."""

import pytest

from type_bridge import Boolean, Card, Entity, Flag, Key, String, TypeFlags


def test_boolean_creation():
    """Test creating Boolean attribute."""

    class IsActive(Boolean):
        pass

    active = IsActive(True)
    assert active.value is True
    assert isinstance(active, Boolean)

    inactive = IsActive(False)
    assert inactive.value is False


@pytest.mark.parametrize("raw", ["False", "false", "0", "1", "", 0, 1, None])
def test_boolean_rejects_non_bool_values(raw):
    """Boolean wrappers must not coerce non-bool values by truthiness."""

    class IsActive(Boolean):
        pass

    with pytest.raises(TypeError, match="IsActive expects bool"):
        IsActive(raw)


def test_boolean_pydantic_validate_rejects_string_false():
    """Pydantic validation must not turn string false into True."""

    class IsActive(Boolean):
        pass

    with pytest.raises(TypeError, match="IsActive expects bool"):
        IsActive._pydantic_validate("False")


def test_boolean_pydantic_serialize_rejects_string_false():
    """Raw string serialization must not turn string false into True."""

    class IsActive(Boolean):
        pass

    with pytest.raises(TypeError, match="IsActive expects bool"):
        IsActive._pydantic_serialize("False")


def test_boolean_pydantic_hooks_preserve_real_bool_values():
    """Real bool values remain valid through validation and serialization."""

    class IsActive(Boolean):
        pass

    inactive = IsActive._pydantic_validate(False)
    assert inactive.value is False
    assert bool(inactive) is False
    assert IsActive._pydantic_serialize(inactive) is False
    assert IsActive._pydantic_serialize(False) is False


def test_boolean_value_type():
    """Test that Boolean has correct value_type for TypeDB."""

    class IsActive(Boolean):
        pass

    assert IsActive.value_type == "boolean"


def test_boolean_in_entity():
    """Test using Boolean in an entity."""

    class Name(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user")
        is_active: IsActive
        name: Name = Flag(Key)

    user = User(name=Name("Alice"), is_active=IsActive(True))
    assert user.is_active.value is True


def test_boolean_insert_query():
    """Test insert query generation with Boolean attributes."""

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user")
        is_active: IsActive

    # Test with True value
    user_active = User(is_active=IsActive(True))
    query_true = user_active.to_insert_query()

    assert "$e isa user" in query_true
    assert "has IsActive true" in query_true
    assert "has IsActive false" not in query_true
    # Booleans should NOT be quoted
    assert '"true"' not in query_true

    # Test with False value
    user_inactive = User(is_active=IsActive(False))
    query_false = user_inactive.to_insert_query()

    assert "$e isa user" in query_false
    assert "has IsActive false" in query_false
    assert "has IsActive true" not in query_false
    # Booleans should NOT be quoted
    assert '"false"' not in query_false


def test_boolean_optional_attribute():
    """Test Boolean as optional attribute."""

    class Name(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        name: Name = Flag(Key)
        is_active: IsActive | None

    # Test with None
    user = User(name=Name("Alice"), is_active=None)
    query = user.to_insert_query()
    assert 'has Name "Alice"' in query
    assert "has IsActive" not in query

    # Test with value
    user2 = User(name=Name("Bob"), is_active=IsActive(True))
    query2 = user2.to_insert_query()
    assert "has IsActive true" in query2


def test_multi_value_boolean_insert_query():
    """Test insert query generation with multi-value Boolean attributes."""

    class FeatureFlag(Boolean):
        pass

    class Config(Entity):
        flags = TypeFlags(name="config")
        feature_flags: list[FeatureFlag] = Flag(Card(min=1))

    # Create entity with multiple boolean values
    config = Config(feature_flags=[FeatureFlag(True), FeatureFlag(False), FeatureFlag(True)])
    query = config.to_insert_query()

    assert "$e isa config" in query
    # Should have multiple boolean values
    assert query.count("has FeatureFlag true") == 2
    assert query.count("has FeatureFlag false") == 1


def test_boolean_comparison():
    """Test Boolean attribute comparison."""

    class IsActive(Boolean):
        pass

    # Same value
    b1 = IsActive(True)
    b2 = IsActive(True)
    assert b1 == b2

    # Different values
    b3 = IsActive(False)
    assert b1 != b3


def test_boolean_string_representation():
    """Test string representation of Boolean attributes."""

    class IsActive(Boolean):
        pass

    active = IsActive(True)

    # Test __repr__
    assert "IsActive" in repr(active)
    assert "True" in repr(active)
