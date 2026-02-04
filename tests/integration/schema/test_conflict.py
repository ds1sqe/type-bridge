"""Integration tests for schema conflict detection."""

import pytest
from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    SchemaConflictError,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(3)
def test_schema_conflict_detection(clean_db):
    """Test that schema conflict detection prevents data loss."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    # Create initial schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Modify schema - remove age attribute
    class PersonModified(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        # age attribute removed!

    # Try to sync modified schema - should detect conflict
    schema_manager2 = SchemaManager(clean_db)
    schema_manager2.register(PersonModified)

    with pytest.raises(SchemaConflictError) as exc_info:
        schema_manager2.sync_schema()

    # Verify error message mentions removed attribute
    error_msg = str(exc_info.value)
    assert "Age" in error_msg or "age" in error_msg.lower()
