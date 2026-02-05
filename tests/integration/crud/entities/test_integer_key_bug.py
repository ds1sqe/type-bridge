"""Regression test for Integer key bug.

This test ensures that entities with Integer-type key attributes can be
inserted and queried correctly. Previously, Integer keys would insert
successfully but fail to query due to incorrect TypeQL value formatting.

Bug: Integer attribute values were being quoted as strings in TypeQL queries,
causing type mismatch errors when querying by Integer key.

Fix: Extract .value from Attribute instances before formatting in _format_value().
"""

import pytest

from type_bridge import Database, Entity, Integer, String, TypeFlags
from type_bridge.attribute.flags import Flag, Key


class EntityId(Integer):
    """Integer ID attribute for testing."""

    pass


class EntityKey(String):
    """String key attribute for comparison."""

    pass


class Description(String):
    """Description attribute."""

    pass


class IntegerKeyEntity(Entity):
    """Entity with Integer key (regression test for bug)."""

    flags = TypeFlags(name="integer_key_entity")
    id: EntityId = Flag(Key)
    description: Description


class StringKeyEntity(Entity):
    """Entity with String key (control - should always work)."""

    flags = TypeFlags(name="string_key_entity")
    key: EntityKey = Flag(Key)
    description: Description


@pytest.mark.integration
@pytest.mark.order(301)
def test_integer_key_insert_and_query(clean_db: Database):
    """Test that entities with Integer keys can be inserted and queried.

    This is a regression test for the bug where Integer-keyed entities would
    insert successfully but fail to query by their key value.
    """
    from type_bridge import SchemaManager

    # Register schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(IntegerKeyEntity)
    schema_manager.sync_schema(force=True)

    # Insert entity with Integer key
    entity = IntegerKeyEntity(id=EntityId(12345), description=Description("Test entity"))

    manager = IntegerKeyEntity.manager(clean_db)
    manager.insert(entity)

    # Query by Integer key - this failed before the fix
    results = manager.get(id=EntityId(12345))

    assert len(results) == 1, "Should find exactly one entity by Integer key"
    assert results[0].id.value == 12345
    assert results[0].description.value == "Test entity"


@pytest.mark.integration
@pytest.mark.order(302)
def test_integer_key_vs_string_key_comparison(clean_db: Database):
    """Compare Integer key behavior with String key behavior (both should work)."""
    from type_bridge import SchemaManager

    # Register schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(IntegerKeyEntity, StringKeyEntity)
    schema_manager.sync_schema(force=True)

    int_manager = IntegerKeyEntity.manager(clean_db)
    str_manager = StringKeyEntity.manager(clean_db)

    # Insert entities
    int_entity = IntegerKeyEntity(id=EntityId(99999), description=Description("Integer key test"))
    str_entity = StringKeyEntity(
        key=EntityKey("KEY-99999"), description=Description("String key test")
    )

    int_manager.insert(int_entity)
    str_manager.insert(str_entity)

    # Query both by their keys
    int_results = int_manager.get(id=EntityId(99999))
    str_results = str_manager.get(key=EntityKey("KEY-99999"))

    # Both should succeed
    assert len(int_results) == 1, "Integer key query should work"
    assert len(str_results) == 1, "String key query should work"

    assert int_results[0].id.value == 99999
    assert str_results[0].key.value == "KEY-99999"


@pytest.mark.integration
@pytest.mark.order(303)
def test_integer_key_with_different_values(clean_db: Database):
    """Test querying with different Integer key values."""
    from type_bridge import SchemaManager

    # Register schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(IntegerKeyEntity)
    schema_manager.sync_schema(force=True)

    manager = IntegerKeyEntity.manager(clean_db)

    # Insert multiple entities with different Integer keys
    entities = [
        IntegerKeyEntity(id=EntityId(1), description=Description("First")),
        IntegerKeyEntity(id=EntityId(100), description=Description("Hundred")),
        IntegerKeyEntity(id=EntityId(9999), description=Description("Large")),
        IntegerKeyEntity(id=EntityId(-42), description=Description("Negative")),
    ]

    for entity in entities:
        manager.insert(entity)

    # Query each by its specific Integer key
    result_1 = manager.get(id=EntityId(1))
    result_100 = manager.get(id=EntityId(100))
    result_9999 = manager.get(id=EntityId(9999))
    result_neg = manager.get(id=EntityId(-42))

    assert len(result_1) == 1 and result_1[0].description.value == "First"
    assert len(result_100) == 1 and result_100[0].description.value == "Hundred"
    assert len(result_9999) == 1 and result_9999[0].description.value == "Large"
    assert len(result_neg) == 1 and result_neg[0].description.value == "Negative"


@pytest.mark.integration
@pytest.mark.order(304)
def test_integer_key_filter_chainable_query(clean_db: Database):
    """Test that Integer key works with chainable query API."""
    from type_bridge import SchemaManager

    # Register schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(IntegerKeyEntity)
    schema_manager.sync_schema(force=True)

    manager = IntegerKeyEntity.manager(clean_db)

    entity = IntegerKeyEntity(id=EntityId(777), description=Description("Lucky"))
    manager.insert(entity)

    # Use chainable query API with filter
    results = manager.filter(id=EntityId(777)).execute()

    assert len(results) == 1
    assert results[0].id.value == 777
    assert results[0].description.value == "Lucky"


@pytest.mark.integration
@pytest.mark.order(305)
def test_integer_key_all_and_count(clean_db: Database):
    """Test that Integer key entities can be retrieved with all() and counted."""
    from type_bridge import SchemaManager

    # Register schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(IntegerKeyEntity)
    schema_manager.sync_schema(force=True)

    manager = IntegerKeyEntity.manager(clean_db)

    # Insert entities
    entities = [
        IntegerKeyEntity(id=EntityId(555), description=Description("Count test 1")),
        IntegerKeyEntity(id=EntityId(666), description=Description("Count test 2")),
    ]

    for entity in entities:
        manager.insert(entity)

    # Verify all() includes the entities
    all_entities = manager.all()
    assert len(all_entities) == 2

    # Verify we can find specific entities by key
    specific_1 = manager.get(id=EntityId(555))
    specific_2 = manager.get(id=EntityId(666))

    assert len(specific_1) == 1
    assert len(specific_2) == 1
