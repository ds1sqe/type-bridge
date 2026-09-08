"""Unit tests for polymorphic role player type resolution.

These tests verify that when fetching relations with polymorphic role players
(where a role can accept multiple entity types via union types), each role
player is resolved to its actual concrete type, not just the first type in
the union.
"""

from tests.utils.handwritten import Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge.crud.utils import (
    build_role_player_fetch_items,
    extract_role_players_from_results,
    resolve_entity_class_from_label,
)


# Test entity classes
class Name(String):
    pass


class PersonName(String):
    pass


class EventTitle(String):
    pass


class TaskTitle(String):
    pass


class MemoryContent(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: PersonName = Flag(Key)


class Event(Entity):
    flags = TypeFlags(name="event")
    title: EventTitle = Flag(Key)


class Task(Entity):
    flags = TypeFlags(name="task")
    title: TaskTitle = Flag(Key)


class Memory(Entity):
    flags = TypeFlags(name="memory")
    content: MemoryContent = Flag(Key)


# Relation with polymorphic role player
class About(Relation):
    flags = TypeFlags(name="about")
    memory: Role[Memory] = Role("memory", Memory)
    subject: Role[Person | Event | Task] = Role("subject", Person, Event, Task)


class TestResolveEntityClassFromLabel:
    """Tests for resolve_entity_class_from_label utility function."""

    def test_resolves_person_type_label(self):
        """Should resolve 'person' label to Person class."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label("person", allowed)
        assert result is Person

    def test_resolves_event_type_label(self):
        """Should resolve 'event' label to Event class."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label("event", allowed)
        assert result is Event

    def test_resolves_task_type_label(self):
        """Should resolve 'task' label to Task class."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label("task", allowed)
        assert result is Task

    def test_falls_back_to_first_when_no_label(self):
        """Should return first allowed class when type_label is None."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label(None, allowed)
        assert result is Person

    def test_falls_back_to_first_when_empty_label(self):
        """Should return first allowed class when type_label is empty string."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label("", allowed)
        assert result is Person

    def test_falls_back_to_first_when_unknown_label(self):
        """Should return first allowed class when type_label doesn't match any class."""
        allowed = (Person, Event, Task)
        result = resolve_entity_class_from_label("unknown_type", allowed)
        assert result is Person

    def test_resolves_subclass_type_label(self):
        """Should resolve subclass type labels correctly."""

        class Employee(Person):
            flags = TypeFlags(name="employee")

        allowed = (Person, Event, Task)
        # Employee is a subclass of Person, should be found via __subclasses__
        result = resolve_entity_class_from_label("employee", allowed)
        assert result is Employee

    def test_single_allowed_class(self):
        """Should work with single allowed class."""
        allowed = (Person,)
        result = resolve_entity_class_from_label("person", allowed)
        assert result is Person


class TestBuildRolePlayerFetchItems:
    """Tests for build_role_player_fetch_items utility function."""

    def test_builds_iid_fetch_item(self):
        """Should include IID fetch item for each role."""
        role_info = {"subject": ("$subject", (Person, Event, Task))}
        items = build_role_player_fetch_items(role_info)
        assert '"subject_iid": iid($subject)' in items

    def test_builds_type_fetch_item(self):
        """Should include type label fetch item for each role."""
        role_info = {"subject": ("$subject", (Person, Event, Task))}
        items = build_role_player_fetch_items(role_info)
        assert '"subject_type": label($subject_type)' in items

    def test_builds_attributes_fetch_item(self):
        """Should include attributes fetch item for each role."""
        role_info = {"subject": ("$subject", (Person, Event, Task))}
        items = build_role_player_fetch_items(role_info)
        # The attributes fetch should contain the nested structure
        assert any("$subject.*" in item for item in items)

    def test_builds_all_three_items_per_role(self):
        """Should build exactly 3 fetch items per role (iid, type, attributes)."""
        role_info = {"subject": ("$subject", (Person, Event, Task))}
        items = build_role_player_fetch_items(role_info)
        assert len(items) == 3

    def test_builds_items_for_multiple_roles(self):
        """Should build fetch items for all roles."""
        role_info = {
            "memory": ("$memory", (Memory,)),
            "subject": ("$subject", (Person, Event, Task)),
        }
        items = build_role_player_fetch_items(role_info)
        # 3 items per role × 2 roles = 6 items
        assert len(items) == 6
        # Check memory role items
        assert '"memory_iid": iid($memory)' in items
        assert '"memory_type": label($memory_type)' in items
        # Check subject role items
        assert '"subject_iid": iid($subject)' in items
        assert '"subject_type": label($subject_type)' in items


class TestPolymorphicRolePlayerResolutionIntegration:
    """Integration tests for the complete polymorphic resolution flow.

    These tests verify that the fetch query structure and result processing
    would correctly resolve polymorphic role players in a real query scenario.
    """

    def test_fetch_items_enable_type_resolution(self):
        """Verify fetch items contain necessary info for type resolution."""
        # Build role info like RelationQuery does
        role_info = {}
        for role_name, role in About._roles.items():
            role_var = f"${role_name}"
            role_info[role_name] = (role_var, role.player_entity_types)

        fetch_items = build_role_player_fetch_items(role_info)

        # Should have items for both roles
        assert '"memory_type": label($memory_type)' in fetch_items
        assert '"subject_type": label($subject_type)' in fetch_items

    def test_resolution_with_simulated_query_result(self):
        """Simulate query result processing with different entity types."""
        allowed_entity_classes = (Person, Event, Task)

        # Simulate result where subject is a Person
        person_result = {"subject_type": "person"}
        resolved = resolve_entity_class_from_label(
            person_result.get("subject_type"), allowed_entity_classes
        )
        assert resolved is Person

        # Simulate result where subject is an Event
        event_result = {"subject_type": "event"}
        resolved = resolve_entity_class_from_label(
            event_result.get("subject_type"), allowed_entity_classes
        )
        assert resolved is Event

        # Simulate result where subject is a Task
        task_result = {"subject_type": "task"}
        resolved = resolve_entity_class_from_label(
            task_result.get("subject_type"), allowed_entity_classes
        )
        assert resolved is Task

    def test_union_order_does_not_affect_resolution(self):
        """Type resolution should work regardless of union type order.

        Regression test: Previously, resolution fell back to the first type
        in the union regardless of the actual entity type.
        """
        # First in union is Person
        allowed = (Person, Event, Task)

        # Even though Event is second in the union, it should resolve correctly
        resolved = resolve_entity_class_from_label("event", allowed)
        assert resolved is Event
        assert resolved is not Person

        # Task is third but should still resolve correctly
        resolved = resolve_entity_class_from_label("task", allowed)
        assert resolved is Task
        assert resolved is not Person

    def test_relation_role_player_hydrates_from_iid_only_placeholder(self):
        """Relation role players may be fetched by IID without their own roles."""
        role_info = {"mentioned": ("$mentioned", (About,))}
        result_group = [
            {
                "mentioned": {},
                "mentioned_iid": "0xabc",
                "mentioned_type": "about",
            }
        ]

        role_players = extract_role_players_from_results(result_group, role_info, set())

        mentioned = role_players["mentioned"]
        assert isinstance(mentioned, About)
        assert getattr(mentioned, "_iid") == "0xabc"

    def test_relation_role_player_deduplicates_by_iid_not_empty_attributes(self):
        """Multiple attribute-less relation players must not collapse together."""
        role_info = {"mentioned": ("$mentioned", (About,))}
        result_group = [
            {
                "mentioned": {},
                "mentioned_iid": "0xabc",
                "mentioned_type": "about",
            },
            {
                "mentioned": {},
                "mentioned_iid": "0xdef",
                "mentioned_type": "about",
            },
        ]

        role_players = extract_role_players_from_results(result_group, role_info, {"mentioned"})

        mentioned = role_players["mentioned"]
        assert [getattr(player, "_iid") for player in mentioned] == ["0xabc", "0xdef"]
