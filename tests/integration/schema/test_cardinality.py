"""Integration tests for schema cardinality constraints."""

import pytest

from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(4)
def test_schema_with_cardinality(clean_db):
    """Test schema creation with various cardinality constraints."""

    class Tag(String):
        pass

    class Score(Integer):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        email: Email = Flag(Key)
        tags: list[Tag] = Flag(Card(min=1))  # At least 1 tag
        scores: list[Score] = Flag(Card(max=5))  # At most 5 scores

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()
    person_entity = [e for e in schema_info.entities if e.get_type_name() == "person"][0]
    owned_attrs = person_entity.get_owned_attributes()

    # Check attribute ownership
    assert "email" in owned_attrs
    assert "tags" in owned_attrs
    assert "scores" in owned_attrs

    # Check cardinality flags
    assert owned_attrs["tags"].flags.is_key is False
    assert owned_attrs["tags"].flags.card_min == 1
    assert owned_attrs["tags"].flags.card_max is None  # unbounded
