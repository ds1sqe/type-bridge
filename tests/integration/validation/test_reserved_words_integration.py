"""Integration tests for reserved word validation."""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge.validation import ReservedWordError, validate_type_name


@pytest.mark.integration
@pytest.mark.order(500)
class TestReservedWordValidation:
    """Tests for reserved word validation during schema sync."""

    def test_validate_type_name_rejects_reserved_word_entity(self):
        """validate_type_name should reject reserved words for entities."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("entity", "entity")

        assert exc_info.value.word == "entity"
        assert exc_info.value.context == "entity"

    def test_validate_type_name_rejects_reserved_word_match(self):
        """validate_type_name should reject 'match' keyword."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("match", "entity")

        assert exc_info.value.word == "match"

    def test_validate_type_name_rejects_reserved_word_attribute(self):
        """validate_type_name should reject 'attribute' keyword."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("attribute", "attribute")

        assert exc_info.value.word == "attribute"

    def test_validate_type_name_rejects_reserved_word_relation(self):
        """validate_type_name should reject 'relation' keyword."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("relation", "relation")

        assert exc_info.value.word == "relation"

    def test_validate_type_name_accepts_valid_name(self):
        """validate_type_name should accept valid names."""
        # Should not raise
        validate_type_name("person", "entity")
        validate_type_name("employment", "relation")
        validate_type_name("name", "attribute")
        validate_type_name("employee", "role")

    def test_validate_type_name_accepts_compound_name_with_reserved_word(self):
        """validate_type_name should accept compound names containing reserved words."""
        # These should not raise since they're not exact matches
        validate_type_name("entity_type", "entity")
        validate_type_name("match_result", "entity")
        validate_type_name("attribute_value", "attribute")


@pytest.mark.integration
@pytest.mark.order(501)
class TestValidNamesSyncToDatabase:
    """Tests for valid names syncing successfully to database."""

    def test_entity_with_valid_name_syncs(self, clean_db):
        """Entity with valid name should sync to database."""

        class ValidName(String):
            pass

        class ValidPerson(Entity):
            flags = TypeFlags(name="valid_person")
            name: ValidName = Flag(Key)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(ValidPerson)

        # Should not raise
        schema_manager.sync_schema(force=True)

        # Verify schema was created
        schema = clean_db.get_schema()
        assert "valid_person" in schema

    def test_compound_name_containing_reserved_word_syncs(self, clean_db):
        """Compound name containing reserved word should sync."""

        class MyEntityName(String):
            pass

        class MyEntityType(Entity):
            flags = TypeFlags(name="my_entity_type")
            name: MyEntityName = Flag(Key)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(MyEntityType)

        # Should not raise - "my_entity_type" is not a reserved word
        schema_manager.sync_schema(force=True)

        schema = clean_db.get_schema()
        assert "my_entity_type" in schema

    def test_relation_with_valid_name_syncs(self, clean_db):
        """Relation with valid name should sync to database."""

        class FriendName(String):
            pass

        class Friend(Entity):
            flags = TypeFlags(name="friend_entity")
            name: FriendName = Flag(Key)

        class FriendshipRelation(Relation):
            flags = TypeFlags(name="friendship_relation")
            person: Role[Friend] = Role("person", Friend)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Friend, FriendshipRelation)

        # Should not raise
        schema_manager.sync_schema(force=True)

        schema = clean_db.get_schema()
        assert "friendship_relation" in schema


@pytest.mark.integration
@pytest.mark.order(502)
class TestReservedWordErrorMessages:
    """Tests for reserved word error message generation."""

    def test_error_includes_word_and_context(self):
        """Error message should include the word and context."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("define", "entity")

        error_msg = str(exc_info.value)
        assert "define" in error_msg
        assert "entity" in error_msg
        assert "reserved word" in error_msg.lower()

    def test_error_includes_suggestions(self):
        """Error message should include alternative suggestions."""
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("count", "attribute")

        error_msg = str(exc_info.value)
        # Should have suggestions like "total", "quantity", etc.
        assert "count" in error_msg
        # Check that some form of suggestion is present
        assert "Suggestion" in error_msg or "suggestion" in error_msg.lower()

    def test_error_for_different_contexts(self):
        """Error messages should mention the specific context."""
        # Entity context
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("entity", "entity")
        assert "entity name" in str(exc_info.value).lower()

        # Attribute context
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("string", "attribute")
        assert "attribute name" in str(exc_info.value).lower()

        # Role context
        with pytest.raises(ReservedWordError) as exc_info:
            validate_type_name("has", "role")
        assert "role name" in str(exc_info.value).lower()


@pytest.mark.integration
@pytest.mark.order(503)
class TestCaseInsensitiveValidation:
    """Tests for case-insensitive reserved word validation."""

    def test_uppercase_reserved_word_rejected(self):
        """Uppercase reserved words should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name("ENTITY", "entity")

    def test_mixed_case_reserved_word_rejected(self):
        """Mixed case reserved words should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name("Entity", "entity")

        with pytest.raises(ReservedWordError):
            validate_type_name("MATCH", "entity")

    def test_lowercase_reserved_word_rejected(self):
        """Lowercase reserved words should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name("entity", "entity")
