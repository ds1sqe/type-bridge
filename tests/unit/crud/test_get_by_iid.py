"""Unit tests for get_by_iid method validation."""

from unittest.mock import MagicMock, patch

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
    def test_returns_none_on_empty_iid(self, mock_executor):
        """Test returns None for empty IID (graceful handling)."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        # Invalid IIDs return None instead of raising (not found)
        result = manager.get_by_iid("")
        assert result is None

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_returns_none_on_invalid_iid_format(self, mock_executor):
        """Test returns None for IID not starting with 0x (graceful handling)."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        # Invalid IIDs return None instead of raising (not found)
        result = manager.get_by_iid("1e00000000000000000000")
        assert result is None

    @patch("type_bridge.crud.typedb_manager.ConnectionExecutor")
    def test_returns_none_on_none_iid(self, mock_executor):
        """Test returns None for None IID (graceful handling)."""
        from type_bridge.crud import TypeDBManager

        mock_connection = MagicMock()
        manager = TypeDBManager(mock_connection, PersonModel)

        # Invalid IIDs return None instead of raising (not found)
        # Cast to str to test None handling at runtime
        from typing import cast

        result = manager.get_by_iid(cast(str, None))  # Intentionally testing None input
        assert result is None

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
