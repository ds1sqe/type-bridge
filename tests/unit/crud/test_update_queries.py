"""Unit tests for update query generation to ensure multi-value diffs are guarded."""

from typing import Any, cast

from typedb.driver import TransactionType

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
from type_bridge.crud.entity.manager import EntityManager
from type_bridge.crud.relation.manager import RelationManager


class _RecordingEntityManager(EntityManager):
    """Entity manager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type):
        super().__init__(cast(Database, object()), model_class)
        self.queries: list[str] = []

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        self.queries.append(query)
        return []


class _RecordingRelationManager(RelationManager):
    """Relation manager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type):
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
    mgr = _RecordingEntityManager(Person)

    mgr.update(person)

    query = mgr.queries[-1]
    attr_name = Tag.get_attribute_name()
    # Variable name includes entity var suffix for batch query compatibility
    attr_var = f"${attr_name}_e"
    expected_try = (
        "try {\n"
        f"  $e has {attr_name} {attr_var};\n"
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
    mgr = _RecordingRelationManager(Attachment)

    mgr.update(attachment)

    query = mgr.queries[-1]
    attr_name = Note.get_attribute_name()
    attr_var = f"${attr_name}"
    expected_try = (
        "try {\n"
        f"  $r has {attr_name} {attr_var};\n"
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

    mgr = _RecordingEntityManager(Item)
    mgr.update(item)

    query = mgr.queries[-1]
    # Should use IID matching, not key attributes
    assert "iid 0x1234567890abcdef" in query
    # Should NOT try to match by non-existent key attributes
    assert "has ItemTitle" not in query.split("match")[1].split("update")[0]


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

    mgr = _RecordingEntityManager(Person)
    mgr.update(person)

    query = mgr.queries[-1]
    # Should use key attribute matching
    assert 'has Name "Alice"' in query
    # Should NOT have iid
    assert "iid 0x" not in query


def test_entity_update_without_iid_or_key_raises():
    """update() should raise KeyAttributeError when entity has no _iid and no @key."""
    import pytest

    from type_bridge.crud.exceptions import KeyAttributeError

    class ItemTitle(String):
        pass

    # Entity WITHOUT @key attributes
    class Item(Entity):
        flags = TypeFlags(name="item2")
        title: ItemTitle

    item = Item(title=ItemTitle("test"))
    # No _iid set, no @key attributes

    mgr = _RecordingEntityManager(Item)

    with pytest.raises(KeyAttributeError):
        mgr.update(item)
