"""Tests for roles playable by multiple entity types."""

from typing import cast

import pytest

from type_bridge import Database, Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge.migration.info import SchemaInfo


def test_role_allows_multiple_player_types_validation():
    class Name(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document")
        name: Name = Flag(Key)

    class Email(Entity):
        flags = TypeFlags(name="email")
        name: Name = Flag(Key)

    class Report(Entity):
        flags = TypeFlags(name="report")
        name: Name = Flag(Key)

    class Trace(Relation):
        flags = TypeFlags(name="trace")
        origin: Role[Document | Email] = Role.multi("origin", Document, Email)

    doc = Document(name=Name("Doc"))
    mail = Email(name=Name("Mail"))

    trace_with_doc = Trace(origin=doc)
    trace_with_email = Trace(origin=mail)

    assert trace_with_doc.origin is doc
    assert trace_with_email.origin is mail


def test_schema_emits_multiple_plays_entries():
    class Name(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document")
        name: Name = Flag(Key)

    class Email(Entity):
        flags = TypeFlags(name="email")
        name: Name = Flag(Key)

    class Trace(Relation):
        flags = TypeFlags(name="trace")
        origin: Role[Document | Email] = Role.multi("origin", Document, Email)

    schema = SchemaInfo()
    schema.attribute_classes = {Name}
    schema.entities = [Document, Email]
    schema.relations = [Trace]

    typeql = schema.to_typeql()

    assert "document plays trace:origin;" in typeql
    assert "email plays trace:origin;" in typeql


def test_role_multi_requires_two_player_types():
    class Name(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document")
        name: Name = Flag(Key)

    with pytest.raises(ValueError):
        Role.multi("origin", Document)


def test_delete_filters_use_actual_role_player_type_for_multi_roles(
    monkeypatch: pytest.MonkeyPatch,
):
    monkeypatch.setenv("TYPE_BRIDGE_BACKEND", "python")

    class PersonId(String):
        pass

    class BotId(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        person_id: PersonId = Flag(Key)

    class Bot(Entity):
        flags = TypeFlags(name="bot")
        bot_id: BotId = Flag(Key)

    class Interaction(Relation):
        flags = TypeFlags(name="interaction")
        actor: Role[Person | Bot] = Role.multi("actor", Person, Bot)

    class _FakeTx:
        def __init__(self, recorder):
            self.recorder = recorder

        def execute(self, query: str):
            self.recorder.append(query)
            return []

        def commit(self):
            return None

    class _FakeDB:
        def __init__(self, recorder):
            self.recorder = recorder

        def transaction(self, *_args, **_kwargs):
            class _TxCtx:
                def __init__(self, recorder):
                    self.recorder = recorder

                def __enter__(self):
                    return _FakeTx(self.recorder)

                def __exit__(self, exc_type, exc, tb):
                    return False

            return _TxCtx(self.recorder)

    recorder: list[str] = []
    fake_db = _FakeDB(recorder)

    bot = Bot(bot_id=BotId("b-1"))

    Interaction.manager(cast(Database, fake_db)).filter(actor=bot).delete()

    assert recorder, "Expected delete to execute a query"
    built_query = recorder[0]
    assert "bot_id" in built_query or "BotId" in built_query
