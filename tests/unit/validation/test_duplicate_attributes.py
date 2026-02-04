"""Tests for duplicate attribute type validation.

TypeDB best practice: Use distinct attribute types for each semantic field,
even when they share the same underlying value type.
"""

from unittest.mock import Mock

import pytest
from type_bridge import Entity, SchemaManager, SchemaValidationError
from type_bridge.attribute.datetime import DateTime
from type_bridge.attribute.flags import Flag, Key
from type_bridge.attribute.string import String
from type_bridge.session import Database


class TestDuplicateAttributeValidation:
    """Test validation for duplicate attribute types in entities/relations."""

    def test_duplicate_datetime_attributes_raises_error(self):
        """Test that using same DateTime attribute type for multiple fields raises error."""

        class TimeStamp(DateTime):
            pass

        class IssueKey(String):
            pass

        class Issue(Entity):
            key: IssueKey = Flag(Key)
            created: TimeStamp
            modified: TimeStamp

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Issue)

        # Should raise SchemaValidationError
        with pytest.raises(SchemaValidationError) as exc_info:
            schema_manager.generate_schema()

        error_msg = str(exc_info.value)
        assert "Issue" in error_msg
        assert "TimeStamp used in fields: 'created', 'modified'" in error_msg
        assert "TypeDB best practice" in error_msg
        assert "distinct attribute types" in error_msg

    def test_duplicate_string_attributes_raises_error(self):
        """Test that using same String attribute type for multiple fields raises error."""

        class Name(String):
            pass

        class Person(Entity):
            first_name: Name
            last_name: Name
            middle_name: Name

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Person)

        with pytest.raises(SchemaValidationError) as exc_info:
            schema_manager.generate_schema()

        error_msg = str(exc_info.value)
        assert "Person" in error_msg
        assert "Name used in fields:" in error_msg
        assert "first_name" in error_msg
        assert "last_name" in error_msg
        assert "middle_name" in error_msg

    def test_distinct_attribute_types_pass_validation(self):
        """Test that using distinct attribute types passes validation."""

        class CreatedStamp(DateTime):
            pass

        class ModifiedStamp(DateTime):
            pass

        class IssueKey(String):
            pass

        class Issue(Entity):
            key: IssueKey = Flag(Key)
            created: CreatedStamp
            modified: ModifiedStamp

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Issue)

        # Should not raise error
        schema = schema_manager.generate_schema()
        assert "CreatedStamp" in schema
        assert "ModifiedStamp" in schema
        assert "IssueKey" in schema

    def test_error_message_contains_example_solution(self):
        """Test that error message contains a helpful example solution."""

        class Status(String):
            pass

        class Task(Entity):
            current_status: Status
            previous_status: Status

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Task)

        with pytest.raises(SchemaValidationError) as exc_info:
            schema_manager.generate_schema()

        error_msg = str(exc_info.value)

        # Check for solution example
        assert "Example:" in error_msg
        assert "# Instead of:" in error_msg
        assert "# Use:" in error_msg
        assert "✓ Distinct types" in error_msg
        assert "❌ Reusing same type" in error_msg

    def test_multiple_entities_with_duplicates(self):
        """Test that each entity is validated independently."""

        class TimeStamp(DateTime):
            pass

        class IssueKey(String):
            pass

        class Issue(Entity):
            key: IssueKey = Flag(Key)
            created: TimeStamp
            modified: TimeStamp

        class Task(Entity):
            name: IssueKey  # Using IssueKey is OK here (different entity)

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Issue, Task)

        # Should raise error for Issue (not Task)
        with pytest.raises(SchemaValidationError) as exc_info:
            schema_manager.generate_schema()

        error_msg = str(exc_info.value)
        assert "Issue" in error_msg
        assert "TimeStamp" in error_msg

    def test_validation_explains_typedb_limitation(self):
        """Test that error message explains TypeDB's limitation clearly."""

        class Timestamp(DateTime):
            pass

        class Event(Entity):
            start: Timestamp
            end: Timestamp

        # Mock database for unit test
        db = Mock(spec=Database)
        db.database_exists.return_value = False
        schema_manager = SchemaManager(db)
        schema_manager.register(Event)

        with pytest.raises(SchemaValidationError) as exc_info:
            schema_manager.generate_schema()

        error_msg = str(exc_info.value)

        # Check explanation
        assert "TypeDB does not store field names" in error_msg
        assert "only stores attribute types" in error_msg
        assert "Why this happens:" in error_msg
