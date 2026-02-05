"""Integration tests for String attribute CRUD operations."""

import pytest

from type_bridge import Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(31)
def test_string_insert(clean_db):
    """Test inserting entity with String attribute."""

    class Username(String):
        pass

    class Bio(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_str")
        username: Username = Flag(Key)
        bio: Bio

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user with String
    user = User(username=Username("alice"), bio=Bio("Python developer"))
    manager.insert(user)

    # Verify insertion
    results = manager.get(username="alice")
    assert len(results) == 1
    assert results[0].bio.value == "Python developer"


@pytest.mark.integration
@pytest.mark.order(32)
def test_string_fetch(clean_db):
    """Test fetching entity by String attribute."""

    class Username(String):
        pass

    class Bio(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_str_fetch")
        username: Username = Flag(Key)
        bio: Bio

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert users
    users = [
        User(username=Username("bob"), bio=Bio("Java developer")),
        User(username=Username("charlie"), bio=Bio("Python developer")),
    ]
    manager.insert_many(users)

    # Fetch by String value
    python_devs = manager.get(bio="Python developer")
    assert len(python_devs) == 1
    assert python_devs[0].username.value == "charlie"


@pytest.mark.integration
@pytest.mark.order(33)
def test_string_update(clean_db):
    """Test updating String attribute."""

    class Username(String):
        pass

    class Bio(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_str_update")
        username: Username = Flag(Key)
        bio: Bio

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user
    user = User(username=Username("diana"), bio=Bio("Junior developer"))
    manager.insert(user)

    # Fetch and update
    results = manager.get(username="diana")
    user_fetched = results[0]
    user_fetched.bio = Bio("Senior developer")
    manager.update(user_fetched)

    # Verify update
    updated = manager.get(username="diana")
    assert updated[0].bio.value == "Senior developer"


@pytest.mark.integration
@pytest.mark.order(34)
def test_string_delete(clean_db):
    """Test deleting entity with String attribute."""

    class Username(String):
        pass

    class Bio(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_str_delete")
        username: Username = Flag(Key)
        bio: Bio

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert user
    user = User(username=Username("eve"), bio=Bio("TypeDB expert"))
    manager.insert(user)

    # Delete by String attribute using filter
    deleted_count = manager.filter(bio="TypeDB expert").delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(username="eve")
    assert len(results) == 0


@pytest.mark.integration
@pytest.mark.order(35)
def test_string_special_characters_escaping(clean_db):
    """Test inserting, fetching, and updating strings with quotes and backslashes."""

    class Name(String):
        pass

    class Description(String):
        pass

    class Message(Entity):
        flags = TypeFlags(name="message_escaping")
        name: Name = Flag(Key)
        description: Description | None

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Message)
    schema_manager.sync_schema(force=True)

    manager = Message.manager(clean_db)

    # Test 1: Insert with double quotes
    msg1 = Message(name=Name("greeting"), description=Description('He said "hello" to me'))
    manager.insert(msg1)

    # Verify quotes are preserved
    results = manager.get(name="greeting")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == 'He said "hello" to me'

    # Test 2: Insert with backslashes
    msg2 = Message(name=Name("path"), description=Description("C:\\Users\\Documents"))
    manager.insert(msg2)

    # Verify backslashes are preserved
    results = manager.get(name="path")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == "C:\\Users\\Documents"

    # Test 3: Insert with both quotes and backslashes
    msg3 = Message(
        name=Name("complex"), description=Description(r'Path: "C:\Program Files\App" is valid')
    )
    manager.insert(msg3)

    # Verify complex string is preserved
    results = manager.get(name="complex")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == r'Path: "C:\Program Files\App" is valid'

    # Test 4: Update with special characters
    msg_to_update = manager.get(name="greeting")[0]
    msg_to_update.description = Description('She replied "goodbye" with a smile\\')
    manager.update(msg_to_update)

    # Verify update worked
    updated = manager.get(name="greeting")
    assert len(updated) == 1
    assert isinstance(updated[0].description, Description)
    assert updated[0].description.value == 'She replied "goodbye" with a smile\\'

    # Test 5: Multiple quotes and various special patterns
    msg4 = Message(
        name=Name("multiple_quotes"),
        description=Description('Multiple "quotes" in "different" places'),
    )
    manager.insert(msg4)

    results = manager.get(name="multiple_quotes")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == 'Multiple "quotes" in "different" places'

    # Test 6: Single quotes (should not be escaped in TypeQL double-quoted strings)
    msg5 = Message(
        name=Name("single_quotes"), description=Description("It's a 'test' with single quotes")
    )
    manager.insert(msg5)

    results = manager.get(name="single_quotes")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == "It's a 'test' with single quotes"

    # Test 7: Empty string (edge case)
    msg6 = Message(name=Name("empty"), description=Description(""))
    manager.insert(msg6)

    results = manager.get(name="empty")
    assert len(results) == 1
    assert isinstance(results[0].description, Description)
    assert results[0].description.value == ""

    # Test 8: Verify all messages were inserted
    # Note: We have 6 distinct messages because Test 4 updated Test 1's message
    all_messages = manager.all()
    assert len(all_messages) == 6
