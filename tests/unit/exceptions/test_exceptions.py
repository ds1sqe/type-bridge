"""Unit tests for exception classes."""

from types import SimpleNamespace

import pytest

from type_bridge.crud.exceptions import (
    EntityNotFoundError,
    KeyAttributeError,
    NotFoundError,
    NotUniqueError,
    RelationNotFoundError,
)
from type_bridge.expressions.boolean import BooleanExpr
from type_bridge.migration.exceptions import SchemaConflictError, SchemaValidationError
from type_bridge.validation import ReservedWordError, ValidationError


def _schema_diff() -> SimpleNamespace:
    return SimpleNamespace(
        removed_entities=set(),
        removed_relations=set(),
        removed_attributes=set(),
        modified_attributes=set(),
        modified_entities=set(),
        modified_relations=set(),
        added_entities=set(),
    )


class TestCrudExceptions:
    """Tests for CRUD exception classes."""

    def test_not_found_error_inherits_from_lookup_error(self):
        """NotFoundError should inherit from LookupError."""
        assert issubclass(NotFoundError, LookupError)

    def test_not_found_error_message(self):
        """NotFoundError should accept a message."""
        error = NotFoundError("Item not found")
        assert str(error) == "Item not found"

    def test_entity_not_found_error_inherits_from_not_found_error(self):
        """EntityNotFoundError should inherit from NotFoundError."""
        assert issubclass(EntityNotFoundError, NotFoundError)
        assert issubclass(EntityNotFoundError, LookupError)

    def test_entity_not_found_error_message(self):
        """EntityNotFoundError should accept a message."""
        error = EntityNotFoundError("Person with name=Alice not found")
        assert "Alice" in str(error)

    def test_entity_not_found_error_can_be_caught_as_not_found(self):
        """EntityNotFoundError should be catchable as NotFoundError."""
        with pytest.raises(NotFoundError):
            raise EntityNotFoundError("Entity missing")

    def test_relation_not_found_error_inherits_from_not_found_error(self):
        """RelationNotFoundError should inherit from NotFoundError."""
        assert issubclass(RelationNotFoundError, NotFoundError)
        assert issubclass(RelationNotFoundError, LookupError)

    def test_relation_not_found_error_message(self):
        """RelationNotFoundError should accept a message."""
        error = RelationNotFoundError("Employment relation not found")
        assert "Employment" in str(error)

    def test_relation_not_found_error_can_be_caught_as_not_found(self):
        """RelationNotFoundError should be catchable as NotFoundError."""
        with pytest.raises(NotFoundError):
            raise RelationNotFoundError("Relation missing")

    def test_not_unique_error_inherits_from_value_error(self):
        """NotUniqueError should inherit from ValueError."""
        assert issubclass(NotUniqueError, ValueError)

    def test_not_unique_error_message(self):
        """NotUniqueError should accept a message."""
        error = NotUniqueError("Multiple matches found")
        assert "Multiple" in str(error)


class TestKeyAttributeError:
    """Tests for KeyAttributeError exception class."""

    def test_key_attribute_error_inherits_from_value_error(self):
        """KeyAttributeError should inherit from ValueError."""
        assert issubclass(KeyAttributeError, ValueError)

    def test_key_attribute_error_none_key_message(self):
        """KeyAttributeError should build message for None key."""
        error = KeyAttributeError(
            entity_type="Person",
            operation="update",
            field_name="name",
        )
        assert "Cannot update Person" in str(error)
        assert "key attribute 'name' is None" in str(error)
        assert "before calling update()" in str(error)

    def test_key_attribute_error_no_keys_message(self):
        """KeyAttributeError should build message for no @key attributes."""
        error = KeyAttributeError(
            entity_type="Person",
            operation="update",
            all_fields=["title", "description", "priority"],
        )
        assert "Cannot update Person" in str(error)
        assert "no @key attributes found" in str(error)
        assert "['title', 'description', 'priority']" in str(error)
        assert "Hint: Add Flag(Key)" in str(error)

    def test_key_attribute_error_delete_operation(self):
        """KeyAttributeError should work for delete operation."""
        error = KeyAttributeError(
            entity_type="User",
            operation="delete",
            field_name="id",
        )
        assert "Cannot delete User" in str(error)
        assert "before calling delete()" in str(error)

    def test_key_attribute_error_stores_attributes(self):
        """KeyAttributeError should store entity_type, operation, field_name."""
        error = KeyAttributeError(
            entity_type="Person",
            operation="update",
            field_name="name",
        )
        assert error.entity_type == "Person"
        assert error.operation == "update"
        assert error.field_name == "name"
        assert error.all_fields is None

    def test_key_attribute_error_stores_all_fields(self):
        """KeyAttributeError should store all_fields when no @key defined."""
        error = KeyAttributeError(
            entity_type="Task",
            operation="update",
            all_fields=["title", "status"],
        )
        assert error.entity_type == "Task"
        assert error.operation == "update"
        assert error.field_name is None
        assert error.all_fields == ["title", "status"]

    def test_key_attribute_error_catchable_as_value_error(self):
        """KeyAttributeError should be catchable as ValueError."""
        with pytest.raises(ValueError):
            raise KeyAttributeError(
                entity_type="Person",
                operation="update",
                field_name="name",
            )


class TestSchemaExceptions:
    """Tests for schema exception classes."""

    def test_schema_validation_error_inherits_from_exception(self):
        """SchemaValidationError should inherit from Exception."""
        assert issubclass(SchemaValidationError, Exception)

    def test_schema_validation_error_message(self):
        """SchemaValidationError should accept a message."""
        error = SchemaValidationError("Invalid schema definition")
        assert str(error) == "Invalid schema definition"

    def test_schema_conflict_error_stores_diff(self):
        """SchemaConflictError should store the diff."""
        diff = _schema_diff()
        error = SchemaConflictError(diff)
        assert error.diff is diff

    def test_schema_conflict_error_custom_message(self):
        """SchemaConflictError should accept custom message."""
        diff = _schema_diff()
        error = SchemaConflictError(diff, message="Custom conflict message")
        assert str(error) == "Custom conflict message"

    def test_schema_conflict_error_default_message(self):
        """SchemaConflictError should generate default message."""
        diff = _schema_diff()
        error = SchemaConflictError(diff)
        assert "Schema conflict detected" in str(error)

    def test_has_breaking_changes_with_empty_diff(self):
        """has_breaking_changes should return False for empty diff."""
        diff = _schema_diff()
        error = SchemaConflictError(diff)
        assert error.has_breaking_changes() is False

    def test_has_breaking_changes_with_removed_entities(self):
        """has_breaking_changes should return True when entities removed."""
        diff = _schema_diff()
        diff.removed_entities.add("removed-entity")
        error = SchemaConflictError(diff)
        assert error.has_breaking_changes() is True

    def test_has_breaking_changes_with_removed_relations(self):
        """has_breaking_changes should return True when relations removed."""
        diff = _schema_diff()
        diff.removed_relations.add("removed-relation")
        error = SchemaConflictError(diff)
        assert error.has_breaking_changes() is True

    def test_has_breaking_changes_with_removed_attributes(self):
        """has_breaking_changes should return True when attributes removed."""
        diff = _schema_diff()
        diff.removed_attributes.add("removed-attribute")
        error = SchemaConflictError(diff)
        assert error.has_breaking_changes() is True

    def test_no_breaking_changes_when_only_additions(self):
        """has_breaking_changes should return False when only additions."""
        diff = _schema_diff()
        diff.added_entities.add("new-entity")
        error = SchemaConflictError(diff)
        # Only additions, no removals or modifications
        assert error.has_breaking_changes() is False


class TestValidationExceptions:
    """Tests for validation exception classes."""

    def test_validation_error_inherits_from_value_error(self):
        """ValidationError should inherit from ValueError."""
        assert issubclass(ValidationError, ValueError)

    def test_validation_error_message(self):
        """ValidationError should accept a message."""
        error = ValidationError("Invalid input")
        assert str(error) == "Invalid input"

    def test_reserved_word_error_inherits_from_validation_error(self):
        """ReservedWordError should inherit from ValidationError."""
        assert issubclass(ReservedWordError, ValidationError)
        assert issubclass(ReservedWordError, ValueError)

    def test_reserved_word_error_message(self):
        """ReservedWordError should build message from word and context."""
        error = ReservedWordError("match", "entity")
        assert "match" in str(error)
        assert "entity" in str(error)
        assert "reserved word" in str(error)

    def test_reserved_word_error_with_suggestion(self):
        """ReservedWordError should include suggestion when provided."""
        error = ReservedWordError("entity", "entity", suggestion="my_entity")
        assert "my_entity" in str(error)

    def test_reserved_word_error_stores_attributes(self):
        """ReservedWordError should store word, context, and suggestion."""
        error = ReservedWordError("count", "attribute", suggestion="total")
        assert error.word == "count"
        assert error.context == "attribute"
        assert error.suggestion == "total"


class TestBooleanExprValidation:
    """Tests for BooleanExpr validation."""

    def test_not_with_zero_operands_raises(self):
        """NOT with zero operands should raise ValueError."""
        with pytest.raises(ValueError, match="NOT operation requires exactly 1 operand"):
            BooleanExpr("not", [])

    def test_not_with_two_operands_raises(self):
        """NOT with two operands should raise ValueError."""
        from tests.utils.handwritten import Entity, Flag, Integer, Key, TypeFlags

        class BoolTestAge(Integer):
            pass

        class BoolTestName(Integer):
            pass

        class BoolTestPerson(Entity):
            flags = TypeFlags(name="bool_test_person")
            name: BoolTestName = Flag(Key)
            age: BoolTestAge

        expr1 = BoolTestPerson.age.gt(BoolTestAge(18))
        expr2 = BoolTestPerson.age.lt(BoolTestAge(65))

        with pytest.raises(ValueError, match="NOT operation requires exactly 1 operand"):
            BooleanExpr("not", [expr1, expr2])

    def test_and_with_one_operand_raises(self):
        """AND with one operand should raise ValueError."""
        from tests.utils.handwritten import Entity, Flag, Integer, Key, TypeFlags

        class AndTestAge(Integer):
            pass

        class AndTestName(Integer):
            pass

        class AndTestPerson(Entity):
            flags = TypeFlags(name="and_test_person")
            name: AndTestName = Flag(Key)
            age: AndTestAge

        expr = AndTestPerson.age.gt(AndTestAge(18))

        with pytest.raises(ValueError, match="AND operation requires at least 2 operands"):
            BooleanExpr("and", [expr])

    def test_or_with_one_operand_raises(self):
        """OR with one operand should raise ValueError."""
        from tests.utils.handwritten import Entity, Flag, Integer, Key, TypeFlags

        class OrTestAge(Integer):
            pass

        class OrTestName(Integer):
            pass

        class OrTestPerson(Entity):
            flags = TypeFlags(name="or_test_person")
            name: OrTestName = Flag(Key)
            age: OrTestAge

        expr = OrTestPerson.age.gt(OrTestAge(18))

        with pytest.raises(ValueError, match="OR operation requires at least 2 operands"):
            BooleanExpr("or", [expr])

    def test_and_with_two_operands_succeeds(self):
        """AND with two operands should succeed."""
        from tests.utils.handwritten import Entity, Flag, Integer, Key, TypeFlags

        class AndOkAge(Integer):
            pass

        class AndOkName(Integer):
            pass

        class AndOkPerson(Entity):
            flags = TypeFlags(name="and_ok_person")
            name: AndOkName = Flag(Key)
            age: AndOkAge

        expr1 = AndOkPerson.age.gt(AndOkAge(18))
        expr2 = AndOkPerson.age.lt(AndOkAge(65))

        # Should not raise
        result = BooleanExpr("and", [expr1, expr2])
        assert result.operation == "and"
        assert len(result.operands) == 2

    def test_not_with_one_operand_succeeds(self):
        """NOT with one operand should succeed."""
        from tests.utils.handwritten import Entity, Flag, Integer, Key, TypeFlags

        class NotOkAge(Integer):
            pass

        class NotOkName(Integer):
            pass

        class NotOkPerson(Entity):
            flags = TypeFlags(name="not_ok_person")
            name: NotOkName = Flag(Key)
            age: NotOkAge

        expr = NotOkPerson.age.eq(NotOkAge(30))

        # Should not raise
        result = BooleanExpr("not", [expr])
        assert result.operation == "not"
        assert len(result.operands) == 1
