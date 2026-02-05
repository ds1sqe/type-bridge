"""Integration tests for Duration attribute CRUD operations."""

from datetime import timedelta

import pytest

from type_bridge import Duration, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(55)
def test_duration_insert(clean_db):
    """Test inserting entity with Duration attribute."""

    class SessionName(String):
        pass

    class SessionDuration(Duration):
        pass

    class Session(Entity):
        flags = TypeFlags(name="session_dur")
        name: SessionName = Flag(Key)
        duration: SessionDuration

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Session)
    schema_manager.sync_schema(force=True)

    manager = Session.manager(clean_db)

    # Insert session with Duration
    session = Session(
        name=SessionName("Training"), duration=SessionDuration(timedelta(hours=2, minutes=30))
    )
    manager.insert(session)

    # Verify insertion
    results = manager.get(name="Training")
    assert len(results) == 1
    assert results[0].duration.value == timedelta(hours=2, minutes=30)


@pytest.mark.integration
@pytest.mark.order(56)
def test_duration_fetch(clean_db):
    """Test fetching entity by Duration attribute."""

    class SessionName(String):
        pass

    class SessionDuration(Duration):
        pass

    class Session(Entity):
        flags = TypeFlags(name="session_dur_fetch")
        name: SessionName = Flag(Key)
        duration: SessionDuration

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Session)
    schema_manager.sync_schema(force=True)

    manager = Session.manager(clean_db)

    # Insert sessions
    sessions = [
        Session(name=SessionName("ShortMeet"), duration=SessionDuration(timedelta(minutes=30))),
        Session(name=SessionName("LongMeet"), duration=SessionDuration(timedelta(hours=3))),
    ]
    manager.insert_many(sessions)

    # Fetch by Duration value
    results = manager.get(duration=timedelta(minutes=30))
    assert len(results) == 1
    assert results[0].name.value == "ShortMeet"


@pytest.mark.integration
@pytest.mark.order(57)
def test_duration_update(clean_db):
    """Test updating Duration attribute."""

    class SessionName(String):
        pass

    class SessionDuration(Duration):
        pass

    class Session(Entity):
        flags = TypeFlags(name="session_dur_update")
        name: SessionName = Flag(Key)
        duration: SessionDuration

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Session)
    schema_manager.sync_schema(force=True)

    manager = Session.manager(clean_db)

    # Insert session
    session = Session(name=SessionName("Workshop"), duration=SessionDuration(timedelta(hours=1)))
    manager.insert(session)

    # Fetch and update
    results = manager.get(name="Workshop")
    session_fetched = results[0]
    session_fetched.duration = SessionDuration(timedelta(hours=1, minutes=30))
    manager.update(session_fetched)

    # Verify update
    updated = manager.get(name="Workshop")
    assert updated[0].duration.value == timedelta(hours=1, minutes=30)


@pytest.mark.integration
@pytest.mark.order(58)
def test_duration_delete(clean_db):
    """Test deleting entity with Duration attribute."""

    class SessionName(String):
        pass

    class SessionDuration(Duration):
        pass

    class Session(Entity):
        flags = TypeFlags(name="session_dur_delete")
        name: SessionName = Flag(Key)
        duration: SessionDuration

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Session)
    schema_manager.sync_schema(force=True)

    manager = Session.manager(clean_db)

    # Insert session
    session = Session(name=SessionName("Standup"), duration=SessionDuration(timedelta(minutes=15)))
    manager.insert(session)

    # Delete by Duration attribute using filter
    deleted_count = manager.filter(duration=timedelta(minutes=15)).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Standup")
    assert len(results) == 0
