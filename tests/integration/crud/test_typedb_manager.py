"""Integration tests for the unified TypeDBManager.

These tests validate that TypeDBManager works correctly with a real TypeDB instance
before switching it to be the default manager.
"""

import pytest

from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge.crud import TypeDBManager

# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture(scope="function")
def db_with_extended_schema(clean_db):
    """Provide a database with extended schema for TypeDBManager tests.

    Includes additional types beyond the basic db_with_extended_schema fixture:
    - Tag attribute for multi-value testing
    - Friendship relation for relation tests
    """

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Tag(String):
        pass

    class Since(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None = None
        tags: list[Tag] = Flag(Card(min=0))

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")
        friend1: Role[Person] = Role("friend1", Person)
        friend2: Role[Person] = Role("friend2", Person)
        since: Since | None = None

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Friendship)
    schema_manager.sync_schema(force=True)

    yield clean_db


# ============================================================================
# Entity Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(500)  # Run after basic tests
def test_typedb_manager_entity_insert(db_with_extended_schema):
    """Test TypeDBManager can insert entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None = None

    manager = TypeDBManager(db_with_extended_schema, Person)

    # Insert entity
    alice = Person(name=Name("Alice"), age=Age(30))
    result = manager.insert(alice)

    assert result is alice


@pytest.mark.integration
@pytest.mark.order(501)
def test_typedb_manager_entity_get(db_with_extended_schema):
    """Test TypeDBManager can fetch entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None = None

    manager = TypeDBManager(db_with_extended_schema, Person)

    # Insert some entities
    manager.insert(Person(name=Name("Bob"), age=Age(25)))
    manager.insert(Person(name=Name("Charlie"), age=Age(35)))

    # Fetch with filter
    results = manager.get(name="Bob")
    assert len(results) >= 1
    assert any(p.name.value == "Bob" for p in results)


@pytest.mark.integration
@pytest.mark.order(502)
def test_typedb_manager_entity_update(db_with_extended_schema):
    """Test TypeDBManager can update entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None = None

    manager = TypeDBManager(db_with_extended_schema, Person)

    # Insert entity
    diana = Person(name=Name("Diana"), age=Age(28))
    manager.insert(diana)

    # Fetch to get IID
    results = manager.get(name="Diana")
    assert len(results) >= 1
    fetched = results[0]

    # Update
    fetched.age = Age(29)
    manager.update(fetched)

    # Verify update
    updated = manager.get(name="Diana")
    assert len(updated) >= 1
    assert updated[0].age is not None
    assert updated[0].age.value == 29


@pytest.mark.integration
@pytest.mark.order(503)
def test_typedb_manager_entity_delete(db_with_extended_schema):
    """Test TypeDBManager can delete entities."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    manager = TypeDBManager(db_with_extended_schema, Person)

    # Insert entity
    eve = Person(name=Name("Eve"))
    manager.insert(eve)

    # Verify insertion
    results = manager.get(name="Eve")
    assert len(results) >= 1

    # Fetch to get IID and delete
    fetched = results[0]
    manager.delete(fetched)

    # Verify deletion
    results_after = manager.get(name="Eve")
    assert len(results_after) == 0


# ============================================================================
# Multi-Value Attribute Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(510)
def test_typedb_manager_multi_value_update(db_with_extended_schema):
    """Test TypeDBManager handles multi-value attribute updates with guards."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    manager = TypeDBManager(db_with_extended_schema, Person)

    # Insert entity with tags
    frank = Person(name=Name("Frank"), tags=[Tag("developer"), Tag("python")])
    manager.insert(frank)

    # Fetch to get IID
    results = manager.get(name="Frank")
    assert len(results) >= 1
    fetched = results[0]

    # Update tags - keep python, add java, remove developer
    fetched.tags = [Tag("python"), Tag("java")]
    manager.update(fetched)

    # Verify update
    updated = manager.get(name="Frank")
    assert len(updated) >= 1
    tag_values = {t.value for t in updated[0].tags}
    assert "python" in tag_values
    assert "java" in tag_values
    assert "developer" not in tag_values


# ============================================================================
# Relation Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(520)
def test_typedb_manager_relation_insert(db_with_extended_schema):
    """Test TypeDBManager can insert relations."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Since(String):
        pass

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")
        friend1: Role[Person] = Role("friend1", Person)
        friend2: Role[Person] = Role("friend2", Person)
        since: Since | None = None

    # Insert persons first
    person_mgr = TypeDBManager(db_with_extended_schema, Person)
    greg = Person(name=Name("Greg"))
    helen = Person(name=Name("Helen"))
    person_mgr.insert(greg)
    person_mgr.insert(helen)

    # Fetch to get entities with IIDs
    greg_fetched = person_mgr.get(name="Greg")[0]
    helen_fetched = person_mgr.get(name="Helen")[0]

    # Insert relation
    relation_mgr = TypeDBManager(db_with_extended_schema, Friendship)
    friendship = Friendship(
        friend1=greg_fetched,
        friend2=helen_fetched,
        since=Since("2024"),
    )
    result = relation_mgr.insert(friendship)

    assert result is friendship


@pytest.mark.integration
@pytest.mark.order(521)
def test_typedb_manager_relation_delete_with_iid(db_with_extended_schema):
    """Test TypeDBManager can delete relations using IID."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")
        friend1: Role[Person] = Role("friend1", Person)
        friend2: Role[Person] = Role("friend2", Person)

    # Insert persons
    person_mgr = TypeDBManager(db_with_extended_schema, Person)
    ivan = Person(name=Name("Ivan"))
    julia = Person(name=Name("Julia"))
    person_mgr.insert(ivan)
    person_mgr.insert(julia)

    # Fetch persons
    ivan_fetched = person_mgr.get(name="Ivan")[0]
    julia_fetched = person_mgr.get(name="Julia")[0]

    # Insert relation
    relation_mgr = TypeDBManager(db_with_extended_schema, Friendship)
    friendship = Friendship(friend1=ivan_fetched, friend2=julia_fetched)
    relation_mgr.insert(friendship)

    # Fetch relation to get IID
    friendships = relation_mgr.all()
    assert len(friendships) >= 1

    # Find our friendship (latest one)
    our_friendship = [f for f in friendships if hasattr(f, "_iid") and f._iid][0]

    # Delete using TypeDBManager
    relation_mgr.delete(our_friendship)
