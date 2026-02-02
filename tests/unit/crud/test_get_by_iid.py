"""Unit tests for get_by_iid method validation."""

from unittest.mock import MagicMock, patch

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


class Name(String):
    pass


class Age(Integer):
    pass


class PersonModel(Entity):
    flags = TypeFlags(name="test_person")
    name: Name = Flag(Key)
    age: Age | None = None


class TestGetByIidValidation:
    """Tests for get_by_iid method parameter validation."""

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_raises_on_empty_iid(self, mock_executor):
        """Test raises ValueError for empty IID."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid("")

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_raises_on_invalid_iid_format(self, mock_executor):
        """Test raises ValueError for IID not starting with 0x."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid("1e00000000000000000000")

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_raises_on_none_iid(self, mock_executor):
        """Test raises ValueError for None IID."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid(None)  # type: ignore

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_accepts_valid_iid_format(self, mock_executor):
        """Test accepts valid IID format."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        mock_executor_instance = MagicMock()
        mock_executor_instance.execute.return_value = []
        mock_executor.return_value = mock_executor_instance

        manager = TypeDBManager(mock_connection, PersonModel)

        # Should not raise, just return None since no results
        result = manager.get_by_iid("0x1e00000000000000000000")
        assert result is None

    def test_iid_persists_after_attribute_modification(self):
        """Verify _iid survives attribute changes on entity.

        Regression test: Pydantic's validate_assignment=True can reset private
        attributes like _iid when modifying model fields. The PrivateAttr in
        TypeDBType should preserve _iid during attribute assignment.
        """
        entity = PersonModel(name=Name("test"), age=Age(25))
        # Use direct assignment for PrivateAttr
        entity._iid = "0x123"

        # Modify an attribute - this used to clear _iid
        entity.name = Name("modified")

        # _iid should still be there
        assert entity._iid == "0x123"

    def test_iid_persists_after_multiple_attribute_modifications(self):
        """Verify _iid survives multiple consecutive attribute changes."""
        entity = PersonModel(name=Name("test"), age=Age(25))
        # Use direct assignment for PrivateAttr
        entity._iid = "0x456789"

        # Multiple modifications
        entity.name = Name("first change")
        entity.age = Age(30)
        entity.name = Name("second change")

        # _iid should still be there
        assert entity._iid == "0x456789"
