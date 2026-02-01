"""Unit tests for get_by_iid method validation."""

from unittest.mock import MagicMock, patch

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


class Name(String):
    pass


class Age(Integer):
    pass


class TestPerson(Entity):
    flags = TypeFlags(name="test_person")
    name: Name = Flag(Key)
    age: Age | None = None


class TestGetByIidValidation:
    """Tests for get_by_iid method parameter validation."""

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_raises_on_empty_iid(self, mock_executor):
        """Test raises ValueError for empty IID."""
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        manager = EntityManager(mock_connection, TestPerson)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid("")

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_raises_on_invalid_iid_format(self, mock_executor):
        """Test raises ValueError for IID not starting with 0x."""
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        manager = EntityManager(mock_connection, TestPerson)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid("1e00000000000000000000")

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_raises_on_none_iid(self, mock_executor):
        """Test raises ValueError for None IID."""
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        manager = EntityManager(mock_connection, TestPerson)

        with pytest.raises(ValueError, match="Invalid IID format"):
            manager.get_by_iid(None)  # type: ignore

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_accepts_valid_iid_format(self, mock_executor):
        """Test accepts valid IID format."""
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        mock_executor_instance = MagicMock()
        mock_executor_instance.execute.return_value = []
        mock_executor.return_value = mock_executor_instance

        manager = EntityManager(mock_connection, TestPerson)

        # Should not raise, just return None since no results
        result = manager.get_by_iid("0x1e00000000000000000000")
        assert result is None

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_preserves_iid_on_returned_entity(self, mock_executor):
        """Test that get_by_iid sets _iid on the returned entity.

        Regression test: The fetch query doesn't return _iid in results,
        so _iid must be set from the input parameter.
        """
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        mock_executor_instance = MagicMock()
        # First call: type lookup returns the type
        # Second call: attribute fetch returns entity data (without _iid!)
        mock_executor_instance.execute.side_effect = [
            [{"_type": "test_person"}],  # type lookup
            [{"Name": "Alice", "Age": 30}],  # attribute fetch - note: no _iid key
        ]
        mock_executor.return_value = mock_executor_instance

        manager = EntityManager(mock_connection, TestPerson)
        iid = "0x1e00000000000000000000"

        result = manager.get_by_iid(iid)

        # The _iid should be preserved from the input parameter
        assert result is not None
        assert getattr(result, "_iid", None) == iid

    @patch("type_bridge.crud.entity.manager.ConnectionExecutor")
    def test_preserves_iid_even_when_result_has_none(self, mock_executor):
        """Test that get_by_iid uses input iid even if result has _iid=None.

        Regression test: dict.get("_iid", default) returns None if the key
        exists with None value, not the default. We should always use the
        input iid parameter.
        """
        from type_bridge.crud import EntityManager

        mock_connection = MagicMock()
        mock_executor_instance = MagicMock()
        mock_executor_instance.execute.side_effect = [
            [{"_type": "test_person"}],
            [{"Name": "Alice", "Age": 30, "_iid": None}],  # _iid exists but is None
        ]
        mock_executor.return_value = mock_executor_instance

        manager = EntityManager(mock_connection, TestPerson)
        iid = "0x1e00000000000000000000"

        result = manager.get_by_iid(iid)

        # The _iid should be the input parameter, not None
        assert result is not None
        assert getattr(result, "_iid", None) == iid
