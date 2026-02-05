"""Integration tests for Boolean attribute CRUD operations."""

import pytest

from type_bridge import Boolean, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(23)
def test_boolean_insert(clean_db):
    """Test inserting entity with Boolean attribute."""

    class Email(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_bool")
        email: Email = Flag(Key)
        is_active: IsActive

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user with Boolean
    user = User(email=Email("alice@example.com"), is_active=IsActive(True))
    manager.insert(user)

    # Verify insertion
    results = manager.get(email="alice@example.com")
    assert len(results) == 1
    assert results[0].is_active.value is True


@pytest.mark.integration
@pytest.mark.order(24)
def test_boolean_fetch(clean_db):
    """Test fetching entity by Boolean attribute."""

    class Email(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_bool_fetch")
        email: Email = Flag(Key)
        is_active: IsActive

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert users
    users = [
        User(email=Email("active@example.com"), is_active=IsActive(True)),
        User(email=Email("inactive@example.com"), is_active=IsActive(False)),
    ]
    manager.insert_many(users)

    # Fetch by Boolean value
    active_users = manager.get(is_active=True)
    assert len(active_users) == 1
    assert active_users[0].email.value == "active@example.com"


@pytest.mark.integration
@pytest.mark.order(25)
def test_boolean_update(clean_db):
    """Test updating Boolean attribute."""

    class Email(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_bool_update")
        email: Email = Flag(Key)
        is_active: IsActive

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user
    user = User(email=Email("user@example.com"), is_active=IsActive(True))
    manager.insert(user)

    # Fetch and update
    results = manager.get(email="user@example.com")
    user_fetched = results[0]
    user_fetched.is_active = IsActive(False)
    manager.update(user_fetched)

    # Verify update
    updated = manager.get(email="user@example.com")
    assert updated[0].is_active.value is False


@pytest.mark.integration
@pytest.mark.order(26)
def test_boolean_delete(clean_db):
    """Test deleting entity with Boolean attribute."""

    class Email(String):
        pass

    class IsActive(Boolean):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_bool_delete")
        email: Email = Flag(Key)
        is_active: IsActive

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user
    user = User(email=Email("delete@example.com"), is_active=IsActive(True))
    manager.insert(user)

    # Delete by Boolean attribute using filter
    deleted_count = manager.filter(is_active=True).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(email="delete@example.com")
    assert len(results) == 0
