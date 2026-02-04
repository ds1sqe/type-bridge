import pytest
from type_bridge import Entity, Flag, Key, SchemaManager, String, TypeFlags


class Name(String):
    pass


class Email(String):
    pass


class Phone(String):
    pass


class Notes(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person_issue_47")
    name: Name = Flag(Key)
    email: Email | None = None
    phone: Phone | None = None
    notes: Notes | None = None


@pytest.mark.integration
def test_update_entity_with_multiple_none_optional_attributes(clean_db):
    """Regression test for Issue #47: update() fails silently when entity
    has multiple optional attributes where some are None.
    """
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # 1. Insert Person with only one optional attribute set (email)
    # phone and notes are None (and thus not in DB)
    person = Person(name=Name("Test User"), email=Email("old@example.com"), phone=None, notes=None)
    manager.insert(person)

    # 2. Fetch the entity
    # When fetched, email will be set, but phone and notes will be None
    fetched_person = manager.get(name="Test User")[0]

    assert fetched_person.email is not None
    assert fetched_person.email.value == "old@example.com"
    assert fetched_person.phone is None
    assert fetched_person.notes is None

    # 3. Modify the existing optional attribute
    fetched_person.email = Email("new@example.com")

    # 4. Update the entity
    # This should update the email, but due to the bug, the match clause
    # might include "has phone $phone" which fails because phone doesn't exist.
    manager.update(fetched_person)

    # 5. Verify the update
    updated_person = manager.get(name="Test User")[0]

    # The update should have succeeded
    assert updated_person.email is not None
    assert updated_person.email.value == "new@example.com", (
        f"Update failed silently! Expected 'new@example.com' but got '{updated_person.email.value}'"
    )
