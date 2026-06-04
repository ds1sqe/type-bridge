"""Unit tests for update query generation to ensure multi-value diffs are guarded."""

from typing import Any, cast

from type_bridge import (
    Card,
    Database,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.crud import TypeDBManager
from type_bridge.models.base import TypeDBType
from type_bridge.typedb_driver import TransactionType


class _RecordingTypeDBManager[T: TypeDBType](TypeDBManager[T]):
    """TypeDBManager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type[T]):
        # Use a mock connection - the manager won't actually execute queries
        super().__init__(cast(Database, object()), model_class)
        self.queries: list[str] = []

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        self.queries.append(query)
        return []


def test_entity_update_multi_value_uses_guards():
    """Updating multi-value attributes should guard against deleting kept values."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    person = Person(name=Name("Alice"), tags=[Tag("keep"), Tag("drop")])
    mgr = _RecordingTypeDBManager(Person)

    mgr.update(person)

    query = mgr.queries[-1]
    attr_name = Tag.get_attribute_name()
    # Unified ModelManager uses $x and simple attribute variable names
    attr_var = f"${attr_name}"
    expected_try = (
        "try {\n"
        f"  $x has {attr_name} {attr_var};\n"
        f'  not {{ {attr_var} == "keep"; }};\n'
        f'  not {{ {attr_var} == "drop"; }};\n'
        "};"
    )
    assert expected_try in query


def test_relation_update_multi_value_uses_guards():
    """Relation updates should also guard multi-value deletions."""

    class Note(String):
        pass

    class Doc(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user")
        doc: Doc = Flag(Key)

    class Attachment(Relation):
        flags = TypeFlags(name="attachment")
        owner: Role[User] = Role("owner", User)
        notes: list[Note] = Flag(Card(min=0))

    attachment = Attachment(owner=User(doc=Doc("ref")), notes=[Note("keep"), Note("old")])
    # Set IID to simulate a fetched relation (relations require IID for identification)
    object.__setattr__(attachment, "_iid", "0xtest123")
    mgr = _RecordingTypeDBManager(Attachment)

    mgr.update(attachment)

    query = mgr.queries[-1]
    attr_name = Note.get_attribute_name()
    # Unified ModelManager uses $x and simple attribute variable names
    attr_var = f"${attr_name}"
    expected_try = (
        "try {\n"
        f"  $x has {attr_name} {attr_var};\n"
        f'  not {{ {attr_var} == "keep"; }};\n'
        f'  not {{ {attr_var} == "old"; }};\n'
        "};"
    )
    assert expected_try in query


def test_entity_update_uses_iid_when_available():
    """update() should use IID-based matching when _iid is set.

    Regression test: update() used to only use @key attributes for matching,
    failing with KeyAttributeError for entities without @key even if _iid was set.
    """

    class ItemTitle(String):
        pass

    class ItemValue(String):
        pass

    # Entity WITHOUT @key attributes
    class Item(Entity):
        flags = TypeFlags(name="item")
        title: ItemTitle
        value: ItemValue | None = None

    item = Item(title=ItemTitle("test"), value=ItemValue("old"))
    # Simulate a fetched entity with _iid set
    object.__setattr__(item, "_iid", "0x1234567890abcdef")

    mgr = _RecordingTypeDBManager(Item)
    mgr.update(item)

    query = mgr.queries[-1]
    # Should use IID matching in the main match clause
    assert "iid 0x1234567890abcdef" in query
    # The main match clause should use IID, not key attributes
    # (attribute bindings in try blocks for replace logic are OK)
    assert "$x isa item, iid 0x1234567890abcdef" in query


def test_entity_update_falls_back_to_key_when_no_iid():
    """update() should use @key attributes when _iid is not available."""

    class Name(String):
        pass

    class Status(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        status: Status | None = None

    person = Person(name=Name("Alice"), status=Status("active"))
    # No _iid set - should fall back to key attributes

    mgr = _RecordingTypeDBManager(Person)
    mgr.update(person)

    query = mgr.queries[-1]
    # Should use key attribute matching
    assert 'has Name "Alice"' in query
    # Should NOT have iid
    assert "iid 0x" not in query


def test_entity_update_without_iid_or_key_raises():
    """update() should raise ValueError when entity has no _iid and no @key."""
    import pytest

    class ItemTitle(String):
        pass

    # Entity WITHOUT @key attributes
    class Item(Entity):
        flags = TypeFlags(name="item2")
        title: ItemTitle

    item = Item(title=ItemTitle("test"))
    # No _iid set, no @key attributes

    mgr = _RecordingTypeDBManager(Item)

    with pytest.raises(ValueError, match="cannot be identified"):
        mgr.update(item)


# ============================================================================
# Delete IID-based matching tests
# ============================================================================


def test_entity_delete_uses_iid_when_available():
    """delete() should use IID-based matching when _iid is set.

    Regression test: delete() should prefer IID matching for efficiency.
    """

    class ItemTitle(String):
        pass

    # Entity WITHOUT @key attributes
    class Item(Entity):
        flags = TypeFlags(name="item3")
        title: ItemTitle

    item = Item(title=ItemTitle("test"))
    # Simulate a fetched entity with _iid set
    object.__setattr__(item, "_iid", "0x1234567890abcdef")

    mgr = _RecordingTypeDBManager(Item)
    mgr.delete(item)

    query = mgr.queries[-1]
    # Should use IID matching
    assert "iid 0x1234567890abcdef" in query
    # Should NOT have attribute matching
    assert "has ItemTitle" not in query


def test_entity_delete_falls_back_to_key_when_no_iid():
    """delete() should use @key attributes when _iid is not available."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person3")
        name: Name = Flag(Key)

    person = Person(name=Name("Alice"))
    # No _iid set - should fall back to key attributes

    # Mock filter().count() to return 1 (entity exists)
    class _MockTypeDBManager(_RecordingTypeDBManager[Person]):
        def filter(self, *_expressions: Any, **_filters: Any) -> Any:
            class _MockQuery:
                def count(self) -> int:
                    return 1

            return _MockQuery()

    mgr = _MockTypeDBManager(Person)
    mgr.delete(person)

    query = mgr.queries[-1]
    # Should use key attribute matching
    assert 'has Name "Alice"' in query
    # Should NOT have iid
    assert "iid 0x" not in query


def test_relation_delete_uses_iid_when_available():
    """TypeDBManager.delete() for relations should use IID-based matching when _iid is set."""

    class Doc(String):
        pass

    class User(Entity):
        flags = TypeFlags(name="user2")
        doc: Doc = Flag(Key)

    class Link(Relation):
        flags = TypeFlags(name="link")
        source: Role[User] = Role("source", User)
        target: Role[User] = Role("target", User)

    link = Link(source=User(doc=Doc("a")), target=User(doc=Doc("b")))
    # Simulate a fetched relation with _iid set
    object.__setattr__(link, "_iid", "0xabcdef1234567890")

    mgr = _RecordingTypeDBManager(Link)
    mgr.delete(link)

    query = mgr.queries[-1]
    # Should use IID matching
    assert "iid 0xabcdef1234567890" in query
    # Should NOT have role player matching
    assert "source:" not in query
    assert "target:" not in query
