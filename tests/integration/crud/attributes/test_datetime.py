"""Integration tests for DateTime attribute CRUD operations."""

from datetime import datetime

import pytest

from type_bridge import DateTime, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(43)
def test_datetime_insert(clean_db):
    """Test inserting entity with DateTime attribute."""

    class EventName(String):
        pass

    class CreatedAt(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_dt")
        name: EventName = Flag(Key)
        created_at: CreatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event with DateTime
    event = Event(
        name=EventName("Launch"),
        created_at=CreatedAt(datetime(2024, 1, 15, 10, 30, 0)),
    )
    manager.insert(event)

    # Verify insertion
    results = manager.get(name="Launch")
    assert len(results) == 1
    assert results[0].created_at.value == datetime(2024, 1, 15, 10, 30, 0)


@pytest.mark.integration
@pytest.mark.order(44)
def test_datetime_fetch(clean_db):
    """Test fetching entity by DateTime attribute."""

    class EventName(String):
        pass

    class CreatedAt(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_dt_fetch")
        name: EventName = Flag(Key)
        created_at: CreatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert events
    events = [
        Event(
            name=EventName("Meeting"),
            created_at=CreatedAt(datetime(2024, 2, 1, 9, 0, 0)),
        ),
        Event(
            name=EventName("Conference"),
            created_at=CreatedAt(datetime(2024, 3, 1, 14, 0, 0)),
        ),
    ]
    manager.insert_many(events)

    # Fetch by DateTime value
    results = manager.get(created_at=datetime(2024, 2, 1, 9, 0, 0))
    assert len(results) == 1
    assert results[0].name.value == "Meeting"


@pytest.mark.integration
@pytest.mark.order(45)
def test_datetime_update(clean_db):
    """Test updating DateTime attribute."""

    class EventName(String):
        pass

    class CreatedAt(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_dt_update")
        name: EventName = Flag(Key)
        created_at: CreatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event
    event = Event(
        name=EventName("Workshop"),
        created_at=CreatedAt(datetime(2024, 4, 1, 10, 0, 0)),
    )
    manager.insert(event)

    # Fetch and update
    results = manager.get(name="Workshop")
    event_fetched = results[0]
    event_fetched.created_at = CreatedAt(datetime(2024, 4, 1, 11, 0, 0))
    manager.update(event_fetched)

    # Verify update
    updated = manager.get(name="Workshop")
    assert updated[0].created_at.value == datetime(2024, 4, 1, 11, 0, 0)


@pytest.mark.integration
@pytest.mark.order(46)
def test_datetime_delete(clean_db):
    """Test deleting entity with DateTime attribute."""

    class EventName(String):
        pass

    class CreatedAt(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_dt_delete")
        name: EventName = Flag(Key)
        created_at: CreatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event
    event = Event(
        name=EventName("Webinar"),
        created_at=CreatedAt(datetime(2024, 5, 1, 15, 0, 0)),
    )
    manager.insert(event)

    # Delete by DateTime attribute using filter
    deleted_count = manager.filter(created_at=datetime(2024, 5, 1, 15, 0, 0)).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Webinar")
    assert len(results) == 0
