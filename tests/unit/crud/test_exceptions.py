"""Unit tests for CRUD exceptions."""

from type_bridge.crud.exceptions import (
    EntityNotFoundError,
    HydrationError,
    KeyAttributeError,
    NotFoundError,
    NotUniqueError,
    RelationNotFoundError,
)


class TestHydrationError:
    """Tests for HydrationError exception."""

    def test_hydration_error_attributes(self):
        """HydrationError stores model_type, raw_data, and cause."""
        cause = ValueError("invalid value")
        raw_data = {"name": "test", "age": "not_a_number"}

        error = HydrationError(
            model_type="Person",
            raw_data=raw_data,
            cause=cause,
        )

        assert error.model_type == "Person"
        assert error.raw_data == raw_data
        assert error.cause is cause

    def test_hydration_error_message(self):
        """HydrationError message includes context."""
        cause = ValueError("invalid value")
        raw_data = {"name": "test"}

        error = HydrationError(
            model_type="Person",
            raw_data=raw_data,
            cause=cause,
        )

        message = str(error)
        assert "Person" in message
        assert "invalid value" in message
        assert "name" in message  # raw_data included

    def test_hydration_error_is_runtime_error(self):
        """HydrationError inherits from RuntimeError."""
        error = HydrationError(
            model_type="Person",
            raw_data={},
            cause=ValueError("test"),
        )
        assert isinstance(error, RuntimeError)

    def test_hydration_error_can_be_raised_and_caught(self):
        """HydrationError can be raised and caught properly."""
        cause = ValueError("original error")

        try:
            raise HydrationError(
                model_type="Person",
                raw_data={"id": 1},
                cause=cause,
            ) from cause
        except HydrationError as e:
            assert e.model_type == "Person"
            assert e.__cause__ is cause


class TestExceptionHierarchy:
    """Tests for exception class hierarchy."""

    def test_not_found_errors_inherit_from_lookup_error(self):
        """NotFoundError and subclasses inherit from LookupError."""
        assert issubclass(NotFoundError, LookupError)
        assert issubclass(EntityNotFoundError, NotFoundError)
        assert issubclass(RelationNotFoundError, NotFoundError)

    def test_not_unique_error_is_value_error(self):
        """NotUniqueError inherits from ValueError."""
        assert issubclass(NotUniqueError, ValueError)

    def test_key_attribute_error_is_value_error(self):
        """KeyAttributeError inherits from ValueError."""
        assert issubclass(KeyAttributeError, ValueError)


class TestKeyAttributeError:
    """Tests for KeyAttributeError exception."""

    def test_key_none_message(self):
        """KeyAttributeError for None key includes field name."""
        error = KeyAttributeError(
            entity_type="Person",
            operation="update",
            field_name="id",
        )
        message = str(error)
        assert "Person" in message
        assert "update" in message
        assert "id" in message
        assert "None" in message

    def test_no_key_message(self):
        """KeyAttributeError for no keys lists all fields."""
        error = KeyAttributeError(
            entity_type="Person",
            operation="delete",
            all_fields=["name", "age", "email"],
        )
        message = str(error)
        assert "Person" in message
        assert "delete" in message
        assert "name" in message
        assert "@key" in message
