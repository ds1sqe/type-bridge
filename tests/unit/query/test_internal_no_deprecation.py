"""Internal legacy-node rendering must not emit the public deprecation warning."""

from __future__ import annotations

import warnings

import pytest

from tests.utils.handwritten import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.crud.patterns import build_entity_match_pattern, build_relation_match_pattern
from type_bridge.crud.types import build_metadata_fetch
from type_bridge.expressions import FunctionQuery, ReturnType
from type_bridge.query.ast import MatchClause, RawPattern
from type_bridge.query.compiler import QueryCompiler


class Name(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)


class Friendship(Relation):
    flags = TypeFlags(name="friendship")
    friend: Role[Person] = Role("friend", Person)


def test_internal_rendering_paths_do_not_warn() -> None:
    alice = Person(name=Name("alice"))
    friendship = Friendship(friend=alice)
    function = FunctionQuery(name="get-ids", return_type=ReturnType(["id"], is_stream=True))
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        alice.to_insert_query()
        alice.get_match_clause_info()
        friendship.get_match_clause_info()
        function.to_match_let()
        function.to_fetch()
        build_entity_match_pattern(Person, filters={"name": "alice"})
        build_relation_match_pattern(Friendship)
        build_metadata_fetch("$e")


def test_public_legacy_compiler_methods_still_warn() -> None:
    compiler = QueryCompiler()
    clause = MatchClause(patterns=[RawPattern(content="$x isa person;")])
    with pytest.warns(DeprecationWarning, match="deprecated since 2.3.0"):
        compiler.compile(clause)
    with pytest.warns(DeprecationWarning, match="deprecated since 2.3.0"):
        compiler.compile_clause_body(clause)
    with pytest.warns(DeprecationWarning, match="deprecated since 2.3.0"):
        compiler.compile_batch([clause])
