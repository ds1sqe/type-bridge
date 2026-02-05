"""Negative type-check scenarios for multi-player roles."""

from typing import Any, cast

import pytest
from pydantic import ValidationError

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags


def test_multi_role_rejects_invalid_player_type():
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

    with pytest.raises(ValidationError):
        Trace(origin=cast(Any, Report(name=Name("Report"))))
