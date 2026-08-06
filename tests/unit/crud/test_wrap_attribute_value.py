"""Tests for unified wrap_attribute_value function."""

import pytest

from tests.utils.handwritten import Boolean, Card, Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.crud.types import wrap_attribute_value


class Name(String):
    pass


class Age(Integer):
    pass


class Tag(String):
    pass


class Enabled(Boolean):
    pass


class SampleEntity(Entity):
    flags = TypeFlags(name="test_entity")
    name: Name = Flag(Key)
    age: Age | None = None
    enabled: Enabled | None = None
    tags: list[Tag] = Flag(Card(min=0))


class TestWrapSingleValue:
    """Tests for wrapping single-value attributes."""

    def test_wrap_raw_string(self) -> None:
        """Test wrapping a raw string value."""
        attr_info = SampleEntity.get_all_attributes()["name"]
        result = wrap_attribute_value("Alice", attr_info)
        assert isinstance(result, Name)
        assert result.value == "Alice"

    def test_wrap_raw_integer(self) -> None:
        """Test wrapping a raw integer value."""
        attr_info = SampleEntity.get_all_attributes()["age"]
        result = wrap_attribute_value(30, attr_info)
        assert isinstance(result, Age)
        assert result.value == 30

    def test_already_wrapped(self) -> None:
        """Test that already-wrapped values pass through."""
        attr_info = SampleEntity.get_all_attributes()["name"]
        name = Name("Alice")
        result = wrap_attribute_value(name, attr_info)
        assert result is name

    def test_none_value(self) -> None:
        """Test that None returns None."""
        attr_info = SampleEntity.get_all_attributes()["age"]
        result = wrap_attribute_value(None, attr_info)
        assert result is None

    def test_wrap_boolean_false(self) -> None:
        """Test wrapping a raw False value."""
        attr_info = SampleEntity.get_all_attributes()["enabled"]
        result = wrap_attribute_value(False, attr_info)
        assert isinstance(result, Enabled)
        assert result.value is False

    def test_wrap_string_false_rejected_for_boolean(self) -> None:
        """String false must not hydrate as a truthy Boolean wrapper."""
        attr_info = SampleEntity.get_all_attributes()["enabled"]
        with pytest.raises(TypeError, match="Enabled expects bool"):
            wrap_attribute_value("False", attr_info)


class TestWrapMultiValue:
    """Tests for wrapping multi-value attributes."""

    def test_wrap_list(self) -> None:
        """Test wrapping a list of values."""
        attr_info = SampleEntity.get_all_attributes()["tags"]
        result = wrap_attribute_value(["a", "b", "c"], attr_info)
        assert isinstance(result, list)
        assert len(result) == 3
        assert all(isinstance(t, Tag) for t in result)
        assert [t.value for t in result] == ["a", "b", "c"]

    def test_wrap_single_as_list(self) -> None:
        """Test that single values are converted to list for multi-value attrs."""
        attr_info = SampleEntity.get_all_attributes()["tags"]
        result = wrap_attribute_value("single", attr_info)
        assert isinstance(result, list)
        assert len(result) == 1
        assert result[0].value == "single"

    def test_empty_list(self) -> None:
        """Test that empty list returns empty list."""
        attr_info = SampleEntity.get_all_attributes()["tags"]
        result = wrap_attribute_value([], attr_info)
        assert result == []

    def test_list_with_none_values(self) -> None:
        """Test that None values in list are filtered out."""
        attr_info = SampleEntity.get_all_attributes()["tags"]
        result = wrap_attribute_value(["a", None, "b"], attr_info)
        assert len(result) == 2
        assert [t.value for t in result] == ["a", "b"]


class TestPydanticValidation:
    """Tests for _pydantic_validate integration."""

    def test_with_pydantic_validate(self) -> None:
        """Test that _pydantic_validate is used when available."""
        attr_info = SampleEntity.get_all_attributes()["name"]
        result = wrap_attribute_value("test", attr_info, use_pydantic_validate=True)
        assert isinstance(result, Name)
        assert result.value == "test"

    def test_without_pydantic_validate(self) -> None:
        """Test wrapping without _pydantic_validate (uses constructor)."""
        attr_info = SampleEntity.get_all_attributes()["name"]
        result = wrap_attribute_value("test", attr_info, use_pydantic_validate=False)
        assert isinstance(result, Name)
        assert result.value == "test"
