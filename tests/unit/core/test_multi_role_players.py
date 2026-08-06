"""Tests for roles playable by multiple entity types."""

import pytest

from tests.utils.handwritten import Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge._rust_runtime import generate_define_block
from type_bridge.migration._lower import _schema_info_for_models


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

    pytest.importorskip("type_bridge_core")
    typeql = generate_define_block(_schema_info_for_models([Document, Email, Trace]))

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
