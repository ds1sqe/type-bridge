"""Integration tests for schema generation with all attribute types."""

import pytest

from type_bridge import (
    Boolean,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(8)
def test_schema_with_all_attribute_types(clean_db):
    """Test schema generation for all 9 attribute types."""

    # Define all attribute types
    class Name(String):
        pass

    class Age(Integer):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class BirthDate(Date):
        pass

    class CreatedAt(DateTime):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Balance(Decimal):
        pass

    class SessionDuration(Duration):
        pass

    # Create entity with all types
    class CompleteRecord(Entity):
        flags = TypeFlags(name="complete_record")
        name: Name = Flag(Key)
        age: Age | None
        is_active: IsActive | None
        score: Score | None
        birth_date: BirthDate | None
        created_at: CreatedAt | None
        updated_at: UpdatedAt | None
        balance: Balance | None
        session_duration: SessionDuration | None

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(CompleteRecord)
    schema_manager.sync_schema(force=True)

    # Verify schema was created
    schema_info = schema_manager.collect_schema_info()

    # Verify entity exists
    entity_names = {e.get_type_name() for e in schema_info.entities}
    assert "complete_record" in entity_names
    record_entity = [e for e in schema_info.entities if e.get_type_name() == "complete_record"][0]
    owned_attrs = record_entity.get_owned_attributes()

    # Verify all attribute types are registered
    attr_names = {a.get_attribute_name() for a in schema_info.attribute_classes}
    assert "Name" in attr_names
    assert "Age" in attr_names
    assert "IsActive" in attr_names
    assert "Score" in attr_names
    assert "BirthDate" in attr_names
    assert "CreatedAt" in attr_names
    assert "UpdatedAt" in attr_names
    assert "Balance" in attr_names
    assert "SessionDuration" in attr_names

    # Verify entity owns all attributes (field names)
    assert "name" in owned_attrs
    assert "age" in owned_attrs
    assert "is_active" in owned_attrs
    assert "score" in owned_attrs
    assert "birth_date" in owned_attrs
    assert "created_at" in owned_attrs
    assert "updated_at" in owned_attrs
    assert "balance" in owned_attrs
    assert "session_duration" in owned_attrs

    # Verify name is key
    assert owned_attrs["name"].flags.is_key is True

    # Verify optional attributes have correct cardinality (0..1)
    assert owned_attrs["age"].flags.card_min == 0
    assert owned_attrs["age"].flags.card_max == 1
    assert owned_attrs["is_active"].flags.card_min == 0
    assert owned_attrs["is_active"].flags.card_max == 1
    assert owned_attrs["score"].flags.card_min == 0
    assert owned_attrs["score"].flags.card_max == 1
