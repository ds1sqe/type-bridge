"""Integration tests for entity delete operations."""

import pytest

from type_bridge import (
    Entity,
    EntityNotFoundError,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(19)
def test_delete_entity_instance(db_with_schema):
    """Test deleting entity by instance using @key attributes."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert entity
    jack = Person(name=Name("Jack"), age=Age(30))
    manager.insert(jack)

    # Verify insertion
    results = manager.get(name="Jack")
    assert len(results) == 1

    # Delete using instance
    deleted = manager.delete(jack)

    # Verify returns instance
    assert deleted is jack

    # Verify deletion
    results_after = manager.get(name="Jack")
    assert len(results_after) == 0


@pytest.mark.integration
@pytest.mark.order(20)
def test_delete_entity_returns_instance(db_with_schema):
    """Test that delete returns the entity instance, not a count."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    manager.insert(alice)

    deleted = manager.delete(alice)

    # Should return entity, not int
    assert isinstance(deleted, Person)
    assert deleted.name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(21)
def test_delete_entity_with_none_key_raises(db_with_schema):
    """Test that deleting entity with None key value raises ValueError."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    # Create valid entity then set key to None to simulate corrupt state
    person = Person(name=Name("Test"))
    person.__dict__["name"] = None  # type: ignore[index]

    with pytest.raises(ValueError, match="Cannot identify Person: key attribute 'name' is None"):
        manager.delete(person)


@pytest.mark.integration
@pytest.mark.order(22)
def test_delete_many_entities(db_with_schema):
    """Test batch deletion of multiple entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert multiple entities
    alice = Person(name=Name("Alice"), age=Age(25))
    bob = Person(name=Name("Bob"), age=Age(30))
    charlie = Person(name=Name("Charlie"), age=Age(35))

    manager.insert_many([alice, bob, charlie])

    # Verify insertion
    assert len(manager.all()) == 3

    # Delete multiple
    deleted = manager.delete_many([alice, bob])

    # Returns list of deleted entities
    assert len(deleted) == 2
    assert alice in deleted
    assert bob in deleted

    # Verify only charlie remains
    remaining = manager.all()
    assert len(remaining) == 1
    assert remaining[0].name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(23)
def test_delete_many_empty_list(db_with_schema):
    """Test that delete_many with empty list returns empty list."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    deleted = manager.delete_many([])
    assert deleted == []


@pytest.mark.integration
@pytest.mark.order(24)
def test_entity_delete_instance_method(db_with_schema):
    """Test entity.delete(connection) instance method."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    manager.insert(alice)

    # Verify insertion
    assert len(manager.all()) == 1

    # Delete using instance method
    result = alice.delete(db_with_schema)

    # Returns self for chaining
    assert result is alice

    # Verify deletion
    assert len(manager.all()) == 0


@pytest.mark.integration
@pytest.mark.order(25)
def test_filter_based_delete_still_works(db_with_schema):
    """Test that filter-based deletion via manager.filter(...).delete() still works."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert entities
    alice = Person(name=Name("Alice"), age=Age(25))
    bob = Person(name=Name("Bob"), age=Age(65))
    charlie = Person(name=Name("Charlie"), age=Age(70))

    manager.insert_many([alice, bob, charlie])

    # Delete using filter (old-style deletion via query)
    count = manager.filter(Age.gt(Age(60))).delete()

    assert count == 2

    # Only alice remains
    remaining = manager.all()
    assert len(remaining) == 1
    assert remaining[0].name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(26)
def test_delete_entity_without_key_single_match(db_with_schema):
    """Test deleting entity without @key when exactly 1 match exists."""

    class CounterValue(Integer):
        pass

    class Counter(Entity):
        flags = TypeFlags(name="test_counter_single")
        counter_value: CounterValue

    # Register schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Counter)
    schema_manager.sync_schema(force=True)

    manager = Counter.manager(db_with_schema)

    # Insert single entity
    counter = Counter(counter_value=CounterValue(42))
    manager.insert(counter)

    # Should be able to delete since exactly 1 match
    deleted = manager.delete(counter)
    assert deleted is counter

    # Verify deletion
    assert len(manager.all()) == 0


@pytest.mark.integration
@pytest.mark.order(27)
def test_delete_entity_without_key_and_no_iid_raises(db_with_schema):
    """Test that deleting entity without @key and no IID raises ValueError.

    The unified ModelManager requires either IID or @key attributes to identify
    the entity for deletion.

    Note: When entities are inserted, IID is populated via attribute matching if
    there's exactly one match. When counter1 is inserted, it's unique, so it gets
    an IID. When counter2 is inserted, there are now 2 matches, so counter2 does
    NOT get an IID. Therefore counter2 cannot be deleted.
    """

    class CounterValue(Integer):
        pass

    class Counter(Entity):
        flags = TypeFlags(name="test_counter_multi")
        counter_value: CounterValue

    # Register schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Counter)
    schema_manager.sync_schema(force=True)

    manager = Counter.manager(db_with_schema)

    # Insert two entities with same value
    counter1 = Counter(counter_value=CounterValue(42))
    counter2 = Counter(counter_value=CounterValue(42))
    manager.insert_many([counter1, counter2])

    # counter1 has IID (was unique when inserted), counter2 does NOT (2 matches when inserted)
    assert counter1._iid is not None, "counter1 should have IID (was unique at insert time)"
    assert counter2._iid is None, "counter2 should NOT have IID (multiple matches at insert time)"

    # Deleting counter2 should raise ValueError since it has no @key and no IID
    with pytest.raises(ValueError, match="no _iid set and no @key attributes defined"):
        manager.delete(counter2)


@pytest.mark.integration
@pytest.mark.order(28)
def test_delete_entity_without_key_and_no_iid_raises_even_if_not_in_db(db_with_schema):
    """Test that deleting entity without @key and no IID raises ValueError.

    Even if entity doesn't exist in DB, we still need IID or @key to identify it.
    """

    class CounterValue(Integer):
        pass

    class Counter(Entity):
        flags = TypeFlags(name="test_counter_no_match")
        counter_value: CounterValue

    # Register schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Counter)
    schema_manager.sync_schema(force=True)

    manager = Counter.manager(db_with_schema)

    # Create entity but don't insert it (not in DB)
    counter = Counter(counter_value=CounterValue(999))

    # Should raise ValueError since no @key and no IID
    with pytest.raises(ValueError, match="no _iid set and no @key attributes defined"):
        manager.delete(counter)


@pytest.mark.integration
@pytest.mark.order(29)
def test_delete_nonexistent_entity_with_key_is_idempotent(db_with_schema):
    """Test that deleting entity with @key that doesn't exist is idempotent (no error).

    The unified ModelManager.delete() doesn't validate existence before deletion.
    Delete is idempotent - if you try to delete something that doesn't exist,
    the query succeeds but deletes nothing.
    """

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    # Create entity but don't insert it (not in DB)
    nonexistent = Person(name=Name("NonExistent"))

    # Act - Delete should succeed (idempotent, no error)
    result = manager.delete(nonexistent)

    # Assert - Returns the instance (same as when actually deleted)
    assert result is nonexistent


@pytest.mark.integration
@pytest.mark.order(30)
def test_delete_many_nonexistent_entity_is_idempotent(db_with_schema):
    """Test that delete_many is idempotent and ignores nonexistent entities by default."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    # Insert one entity
    alice = Person(name=Name("Alice"))
    manager.insert(alice)

    # Create another entity but don't insert it
    nonexistent = Person(name=Name("NonExistent"))

    # Should NOT raise EntityNotFoundError (idempotent batch delete by default)
    # The operation should succeed, deleting alice and ignoring nonexistent
    deleted = manager.delete_many([alice, nonexistent])

    # Verify return value (only actually-deleted entities)
    assert len(deleted) == 1
    assert alice in deleted
    assert nonexistent not in deleted

    # Verify Alice is gone
    results = manager.get(name="Alice")
    assert len(results) == 0


@pytest.mark.integration
@pytest.mark.order(31)
def test_delete_many_strict_mode_raises_for_missing(db_with_schema):
    """Test that delete_many with strict=True raises EntityNotFoundError for missing entities."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    # Insert one entity
    alice = Person(name=Name("Alice"))
    manager.insert(alice)

    # Create another entity but don't insert it
    nonexistent = Person(name=Name("NonExistent"))

    # Should raise EntityNotFoundError in strict mode
    with pytest.raises(EntityNotFoundError, match="entity\\(ies\\) not found"):
        manager.delete_many([alice, nonexistent], strict=True)

    # Verify Alice is still there (transaction was rolled back due to error)
    results = manager.get(name="Alice")
    assert len(results) == 1


@pytest.mark.integration
@pytest.mark.order(32)
def test_delete_many_strict_mode_succeeds_when_all_exist(db_with_schema):
    """Test that delete_many with strict=True succeeds when all entities exist."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = Person.manager(db_with_schema)

    # Insert entities
    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    manager.insert_many([alice, bob])

    # Should succeed in strict mode since all entities exist
    deleted = manager.delete_many([alice, bob], strict=True)

    # Verify return value
    assert len(deleted) == 2
    assert alice in deleted
    assert bob in deleted

    # Verify both are gone
    assert len(manager.all()) == 0
