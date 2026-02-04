"""Tests for Entity.to_dict() and Entity.from_dict()."""

import pytest
from type_bridge import (
    AttributeFlags,
    Card,
    Entity,
    Flag,
    Integer,
    String,
    TypeNameCase,
)


def test_to_dict_unwraps_values_and_supports_aliases():
    """Ensure to_dict unwraps Attribute values and can emit alias names."""

    class DisplayId(String):
        flags = AttributeFlags(name="display-id")

    class Title(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Epic(Entity):
        title: Title
        display_id: DisplayId
        summary: Title | None = None

    epic = Epic(title=Title("Bridge"), display_id=DisplayId("E-123"))

    assert epic.to_dict() == {
        "title": "Bridge",
        "display_id": "E-123",
        "summary": None,
    }

    assert epic.to_dict(by_alias=True) == {
        "title": "Bridge",
        "display-id": "E-123",
        "summary": None,
    }


def test_to_dict_honors_include_exclude_and_exclude_unset():
    """Filter fields using include/exclude and drop unset values when requested."""

    class Tagline(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class ExternalId(String):
        flags = AttributeFlags(name="external-id")

    class Project(Entity):
        external_id: ExternalId
        tagline: Tagline | None = None

    project = Project(external_id=ExternalId("123"))

    assert project.to_dict(include={"external_id"}) == {"external_id": "123"}

    assert project.to_dict(exclude={"external_id"}) == {"tagline": None}

    assert project.to_dict(exclude_unset=True) == {"external_id": "123"}


def test_from_dict_maps_fields_wraps_values_and_handles_unknowns():
    """from_dict should map external names, wrap Attribute types, and handle unknowns."""

    class DisplayId(String):
        flags = AttributeFlags(name="display-id")

    class Name(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Tag(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Epic(Entity):
        name: Name
        display_id: DisplayId
        tag: Tag | None = None

    data = {"name": "New Epic", "display-id": "D-9", "tag": "important"}

    epic = Epic.from_dict(data, field_mapping={"display-id": "display_id"})
    assert isinstance(epic.name, Name)
    assert epic.name.value == "New Epic"
    assert isinstance(epic.display_id, DisplayId)
    assert epic.display_id.value == "D-9"
    assert isinstance(epic.tag, Tag)
    assert epic.tag.value == "important"

    with pytest.raises(ValueError):
        Epic.from_dict({"name": "Test", "display_id": "ID", "extra": "x"})

    relaxed = Epic.from_dict({"name": "Test", "display_id": "ID", "extra": "x"}, strict=False)
    assert relaxed.name.value == "Test"
    assert relaxed.display_id.value == "ID"


def test_from_dict_skips_none_preserves_empty_and_handles_lists():
    """Skip None values, preserve empty strings, and wrap list items with Attribute types.

    Empty strings are valid values that should be preserved (not treated as None).
    Only actual None values are skipped.
    """

    class Tag(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Title(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Note(Entity):
        title: Title
        tags: list[Tag] = Flag(Card(min=1))
        summary: Title | None = None

    # Empty strings should be preserved (not filtered)
    data = {"title": "Note", "tags": ["one", "two", ""], "summary": ""}

    note = Note.from_dict(data, strict=False)

    # Empty string in list is preserved
    assert [tag.value for tag in note.tags] == ["one", "two", ""]
    # Empty string for optional field becomes Title(""), not None
    assert note.summary is not None
    assert note.summary.value == ""

    # Verify that actual None values ARE skipped
    data_with_none = {"title": "Note", "tags": ["one"], "summary": None}
    note2 = Note.from_dict(data_with_none, strict=False)
    assert note2.summary is None


def test_to_dict_from_dict_round_trip_with_alias_names():
    """Round-trip through by_alias dicts should preserve data."""

    class DisplayId(String):
        flags = AttributeFlags(name="display-id")

    class Title(String):
        flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)

    class Score(Integer):
        pass

    class Epic(Entity):
        title: Title
        display_id: DisplayId
        score: Score

    original = Epic(title=Title("Bridge"), display_id=DisplayId("E-1"), score=Score(5))

    data = original.to_dict(by_alias=True)
    restored = Epic.from_dict(data, field_mapping={"display-id": "display_id"})

    assert restored.to_dict() == original.to_dict()
    assert isinstance(restored.display_id, DisplayId)
