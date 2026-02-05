"""Integration tests for DateTimeTZ attribute CRUD operations."""

from datetime import UTC, datetime

import pytest

from type_bridge import DateTimeTZ, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(47)
def test_datetimetz_insert(clean_db):
    """Test inserting entity with DateTimeTZ attribute."""

    class EventName(String):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_tz")
        name: EventName = Flag(Key)
        updated_at: UpdatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event with DateTimeTZ
    event = Event(
        name=EventName("GlobalMeet"),
        updated_at=UpdatedAt(datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)),
    )
    manager.insert(event)

    # Verify insertion
    results = manager.get(name="GlobalMeet")
    assert len(results) == 1
    assert results[0].updated_at.value == datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)


@pytest.mark.integration
@pytest.mark.order(48)
def test_datetimetz_fetch(clean_db):
    """Test fetching entity by DateTimeTZ attribute."""

    class EventName(String):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_tz_fetch")
        name: EventName = Flag(Key)
        updated_at: UpdatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert events
    events = [
        Event(
            name=EventName("MorningSync"),
            updated_at=UpdatedAt(datetime(2024, 2, 1, 9, 0, 0, tzinfo=UTC)),
        ),
        Event(
            name=EventName("EveningReview"),
            updated_at=UpdatedAt(datetime(2024, 3, 1, 18, 0, 0, tzinfo=UTC)),
        ),
    ]
    manager.insert_many(events)

    # Fetch by DateTimeTZ value
    results = manager.get(updated_at=datetime(2024, 2, 1, 9, 0, 0, tzinfo=UTC))
    assert len(results) == 1
    assert results[0].name.value == "MorningSync"


@pytest.mark.integration
@pytest.mark.order(49)
def test_datetimetz_update(clean_db):
    """Test updating DateTimeTZ attribute."""

    class EventName(String):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_tz_update")
        name: EventName = Flag(Key)
        updated_at: UpdatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event
    event = Event(
        name=EventName("Standup"),
        updated_at=UpdatedAt(datetime(2024, 4, 1, 10, 0, 0, tzinfo=UTC)),
    )
    manager.insert(event)

    # Fetch and update
    results = manager.get(name="Standup")
    event_fetched = results[0]
    event_fetched.updated_at = UpdatedAt(datetime(2024, 4, 1, 10, 15, 0, tzinfo=UTC))
    manager.update(event_fetched)

    # Verify update
    updated = manager.get(name="Standup")
    assert updated[0].updated_at.value == datetime(2024, 4, 1, 10, 15, 0, tzinfo=UTC)


@pytest.mark.integration
@pytest.mark.order(50)
def test_datetimetz_delete(clean_db):
    """Test deleting entity with DateTimeTZ attribute."""

    class EventName(String):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_tz_delete")
        name: EventName = Flag(Key)
        updated_at: UpdatedAt

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event
    event = Event(
        name=EventName("Retrospective"),
        updated_at=UpdatedAt(datetime(2024, 5, 1, 15, 0, 0, tzinfo=UTC)),
    )
    manager.insert(event)

    # Delete by DateTimeTZ attribute using filter
    deleted_count = manager.filter(updated_at=datetime(2024, 5, 1, 15, 0, 0, tzinfo=UTC)).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Retrospective")
    assert len(results) == 0
