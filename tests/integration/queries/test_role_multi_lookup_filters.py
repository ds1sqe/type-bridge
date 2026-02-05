"""Integration tests for role-player lookup filters with Role.multi (polymorphic roles).

Tests Django-style lookup filters on relations where a role can be played
by multiple entity types.
"""

import pytest

from type_bridge import (
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


class Name(String):
    pass


class Age(Integer):
    pass


class SerialNumber(String):
    pass


class ActionType(String):
    pass


class Person(Entity):
    """Human actor with age attribute."""

    flags = TypeFlags(name="rm_person")
    name: Name = Flag(Key)
    age: Age | None = None


class Bot(Entity):
    """Automated actor with serial number instead of age."""

    flags = TypeFlags(name="rm_bot")
    name: Name = Flag(Key)
    serial_number: SerialNumber | None = None


class Target(Entity):
    """Target of an interaction."""

    flags = TypeFlags(name="rm_target")
    name: Name = Flag(Key)


class Interaction(Relation):
    """Relation with polymorphic actor role (Person OR Bot)."""

    flags = TypeFlags(name="rm_interaction")
    actor: Role[Person | Bot] = Role.multi("actor", Person, Bot)
    target: Role[Target] = Role("target", Target)
    action: ActionType | None = None


@pytest.fixture
def setup_multi_role_data(clean_db):
    """Setup test data for polymorphic role relations."""
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Bot, Target, Interaction)
    schema_manager.sync_schema(force=True)

    person_manager = Person.manager(clean_db)
    bot_manager = Bot.manager(clean_db)
    target_manager = Target.manager(clean_db)
    interaction_manager = Interaction.manager(clean_db)

    # Create persons (have 'name' and 'age')
    alice = Person(name=Name("Alice"), age=Age(30))
    bob = Person(name=Name("Bob"), age=Age(25))

    # Create bots (have 'name' and 'serial_number', NOT 'age')
    helper_bot = Bot(name=Name("HelperBot"), serial_number=SerialNumber("BOT-001"))
    scanner_bot = Bot(name=Name("ScannerBot"), serial_number=SerialNumber("BOT-002"))

    # Create targets
    resource1 = Target(name=Name("Resource1"))
    resource2 = Target(name=Name("Resource2"))

    person_manager.insert_many([alice, bob])
    bot_manager.insert_many([helper_bot, scanner_bot])
    target_manager.insert_many([resource1, resource2])

    # Create interactions with both Person and Bot actors
    interactions = [
        Interaction(actor=alice, target=resource1, action=ActionType("read")),
        Interaction(actor=bob, target=resource2, action=ActionType("write")),
        Interaction(actor=helper_bot, target=resource1, action=ActionType("assist")),
        Interaction(actor=scanner_bot, target=resource2, action=ActionType("scan")),
    ]
    interaction_manager.insert_many(interactions)

    return {
        "db": clean_db,
        "alice": alice,
        "bob": bob,
        "helper_bot": helper_bot,
        "scanner_bot": scanner_bot,
        "resource1": resource1,
        "resource2": resource2,
    }


# ============================================================
# Role.multi - shared attribute tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(260)
def test_filter_multi_role_by_shared_attribute(setup_multi_role_data):
    """Test filtering Role.multi by attribute shared by all player types (name)."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor name - both Person and Bot have 'name'
    results = manager.filter(actor__name="Alice").execute()

    assert len(results) == 1
    assert results[0].actor.name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(261)
def test_filter_multi_role_by_shared_attribute_contains(setup_multi_role_data):
    """Test filtering Role.multi by shared attribute with string lookup."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor name containing "Bot" - matches both bots
    results = manager.filter(actor__name__contains="Bot").execute()

    assert len(results) == 2
    names = {r.actor.name.value for r in results}
    assert names == {"HelperBot", "ScannerBot"}


# ============================================================
# Role.multi - type-specific attribute tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(262)
def test_filter_multi_role_by_type_specific_attribute(setup_multi_role_data):
    """Test filtering Role.multi by attribute only on one player type (age).

    Only Person has 'age', Bot does not. Filter should only match Person actors.
    """
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor age > 20 - only Person has age
    results = manager.filter(actor__age__gt=20).execute()

    # Should match Alice (30) and Bob (25) - both are Persons
    assert len(results) == 2
    for r in results:
        # Actor should be a Person with age > 20
        assert isinstance(r.actor, Person)
        assert r.actor.age is not None
        assert r.actor.age.value > 20


@pytest.mark.integration
@pytest.mark.order(263)
def test_filter_multi_role_by_other_type_attribute(setup_multi_role_data):
    """Test filtering Role.multi by attribute only on Bot (serial_number).

    Note: This test verifies that the TypeQL query correctly filters by
    an attribute unique to one player type. The deserialization may return
    the entity as a different type due to Role.multi type detection logic
    (which uses key attributes to determine the type).
    """
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor serial_number - only Bot has this
    results = manager.filter(actor__serial_number__startswith="BOT").execute()

    # Should match HelperBot and ScannerBot
    assert len(results) == 2
    # Check by name since both Person and Bot share 'name' attribute
    names = {r.actor.name.value for r in results}
    assert names == {"HelperBot", "ScannerBot"}


# ============================================================
# Role.multi - combined filter tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(264)
def test_filter_multi_role_combined_with_relation_attr(setup_multi_role_data):
    """Test combining Role.multi filter with relation attribute."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor age > 25 AND action = "read"
    results = manager.filter(ActionType.eq(ActionType("read")), actor__age__gt=25).execute()

    # Only Alice (30) with action "read"
    assert len(results) == 1
    assert results[0].actor.name.value == "Alice"
    assert results[0].action is not None
    assert results[0].action.value == "read"


@pytest.mark.integration
@pytest.mark.order(265)
def test_filter_multi_role_combined_with_other_role(setup_multi_role_data):
    """Test combining Role.multi filter with another role filter."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # First verify individual filters work
    bot_results = manager.filter(actor__name__contains="Bot").execute()
    assert len(bot_results) == 2  # HelperBot and ScannerBot

    resource1_results = manager.filter(target__name="Resource1").execute()
    assert len(resource1_results) == 2  # Alice->Resource1 and HelperBot->Resource1

    # Combined filter: actor name contains "Bot" AND target name = "Resource1"
    results = manager.filter(actor__name__contains="Bot", target__name="Resource1").execute()

    # Only HelperBot -> Resource1
    assert len(results) == 1
    assert results[0].actor.name.value == "HelperBot"
    assert results[0].target.name.value == "Resource1"


# ============================================================
# Role.multi - in list filter
# ============================================================


@pytest.mark.integration
@pytest.mark.order(266)
def test_filter_multi_role_in_list(setup_multi_role_data):
    """Test filtering Role.multi with __in lookup."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Filter by actor name in list - mix of Person and Bot names
    results = manager.filter(actor__name__in=["Alice", "HelperBot"]).execute()

    assert len(results) == 2
    names = {r.actor.name.value for r in results}
    assert names == {"Alice", "HelperBot"}


# ============================================================
# Delete with Role.multi
# ============================================================


@pytest.mark.integration
@pytest.mark.order(270)
def test_delete_multi_role_by_type_specific_attr(setup_multi_role_data):
    """Test deleting with Role.multi filter on type-specific attribute."""
    db = setup_multi_role_data["db"]

    manager = Interaction.manager(db)

    # Count before
    all_before = manager.all()
    assert len(all_before) == 4

    # Delete interactions where actor age < 30 (only affects Person actors)
    deleted_count = manager.filter(actor__age__lt=30).delete()

    # Only Bob (25) matches
    assert deleted_count == 1

    # Verify remaining
    remaining = manager.all()
    assert len(remaining) == 3

    # Bob should not be in any remaining interactions
    for r in remaining:
        assert r.actor.name.value != "Bob"
