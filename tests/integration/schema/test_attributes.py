"""Integration tests for schema attribute constraints."""

import pytest

from type_bridge import Entity, Flag, Key, SchemaManager, String, TypeFlags, Unique


@pytest.mark.integration
@pytest.mark.order(5)
def test_schema_with_unique_attributes(clean_db):
    """Test schema creation with unique attributes."""

    class Email(String):
        pass

    class Username(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user")
        email: Email = Flag(Key)
        username: Username = Flag(Unique)

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()
    user_entity = [e for e in schema_info.entities if e.get_type_name() == "user"][0]
    owned_attrs = user_entity.get_owned_attributes()

    # Check key and unique flags
    assert owned_attrs["email"].flags.is_key is True
    assert owned_attrs["username"].flags.is_unique is True
