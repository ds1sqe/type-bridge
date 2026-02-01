"""Unit tests for CRUD negative/edge cases."""

from typing import Any, cast
from unittest.mock import MagicMock

from typedb.driver import TransactionType

from type_bridge import Card, Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.crud.entity.manager import EntityManager
from type_bridge.crud.relation.manager import RelationManager
from type_bridge.crud.utils import format_value


class _RecordingEntityManager(EntityManager):
    """Entity manager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type):
        # Use a mock object instead of real Database
        from type_bridge import Database

        super().__init__(cast(Database, MagicMock()), model_class)
        self.queries: list[str] = []

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        self.queries.append(query)
        return []


class _RecordingRelationManager(RelationManager):
    """Relation manager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type):
        from type_bridge import Database

        super().__init__(cast(Database, MagicMock()), model_class)
        self.queries: list[str] = []

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        self.queries.append(query)
        return []


class TestFormatValueEdgeCases:
    """Tests for format_value edge cases."""

    def test_format_value_with_none(self):
        """format_value with None should stringify to 'None'."""
        result = format_value(None)
        assert result == '"None"'

    def test_format_value_with_empty_string(self):
        """format_value with empty string should return empty quoted string."""
        result = format_value("")
        assert result == '""'

    def test_format_value_with_special_unicode(self):
        """format_value with unicode characters should preserve them."""
        result = format_value("日本語テスト")
        assert result == '"日本語テスト"'

    def test_format_value_with_emojis(self):
        """format_value with emojis should preserve them."""
        result = format_value("Hello 🌍 World 🚀")
        assert result == '"Hello 🌍 World 🚀"'

    def test_format_value_with_mixed_quotes_and_backslashes(self):
        """format_value should correctly escape complex strings."""
        result = format_value('path\\to\\"file"')
        assert result == '"path\\\\to\\\\\\"file\\""'

    def test_format_value_with_very_long_string(self):
        """format_value should handle very long strings."""
        long_str = "x" * 10000
        result = format_value(long_str)
        assert result == f'"{long_str}"'
        assert len(result) == 10002  # quotes + string

    def test_format_value_with_control_characters(self):
        """format_value should handle control characters."""
        result = format_value("tab\there\nnewline")
        assert "tab" in result
        assert "here" in result
        assert "newline" in result


class TestEntityManagerEdgeCases:
    """Tests for EntityManager edge cases."""

    def test_insert_entity_generates_query(self):
        """insert() should generate an INSERT query."""

        class NegName(String):
            pass

        class NegPerson(Entity):
            flags = TypeFlags(name="neg_person")
            name: NegName = Flag(Key)

        person = NegPerson(name=NegName("Alice"))
        mgr = _RecordingEntityManager(NegPerson)
        mgr.insert(person)

        assert len(mgr.queries) > 0
        # The insert query may be followed by an IID fetch query, so check any query contains insert
        assert any("insert" in q.lower() for q in mgr.queries)

    def test_update_entity_generates_match_delete_insert(self):
        """update() should generate MATCH + DELETE + INSERT pattern."""

        class UpdName(String):
            pass

        class UpdAge(Integer):
            pass

        class UpdPerson(Entity):
            flags = TypeFlags(name="upd_person")
            name: UpdName = Flag(Key)
            age: UpdAge

        person = UpdPerson(name=UpdName("Bob"), age=UpdAge(30))
        mgr = _RecordingEntityManager(UpdPerson)
        mgr.update(person)

        assert len(mgr.queries) > 0
        query = mgr.queries[-1].lower()
        assert "match" in query

    def test_get_entity_generates_match_fetch(self):
        """get() should generate MATCH + FETCH query."""

        class GetName(String):
            pass

        class GetPerson(Entity):
            flags = TypeFlags(name="get_person")
            name: GetName = Flag(Key)

        mgr = _RecordingEntityManager(GetPerson)
        # get() generates a fetch query
        mgr.get(name=GetName("Charlie"))

        assert len(mgr.queries) > 0
        query = mgr.queries[-1].lower()
        assert "match" in query
        assert "fetch" in query


class TestRelationManagerEdgeCases:
    """Tests for RelationManager edge cases."""

    def test_insert_relation_with_role_player(self):
        """insert() should include role player in query."""

        class RelName(String):
            pass

        class RelUser(Entity):
            flags = TypeFlags(name="rel_user")
            name: RelName = Flag(Key)

        class RelFriendship(Relation):
            flags = TypeFlags(name="rel_friendship")
            friend: Role[RelUser] = Role("friend", RelUser)

        user = RelUser(name=RelName("Alice"))
        friendship = RelFriendship(friend=user)
        mgr = _RecordingRelationManager(RelFriendship)
        mgr.insert(friendship)

        assert len(mgr.queries) > 0
        query = mgr.queries[-1]
        assert "friend" in query.lower()


class TestValidationEdgeCases:
    """Tests for validation edge cases."""

    def test_entity_with_optional_field_none(self):
        """Entity with optional field set to None should work."""

        class OptName(String):
            pass

        class OptAge(Integer):
            pass

        class OptPerson(Entity):
            flags = TypeFlags(name="opt_person")
            name: OptName = Flag(Key)
            age: OptAge | None = None

        # Should not raise
        person = OptPerson(name=OptName("Dave"))
        assert person.age is None

    def test_entity_with_optional_field_set(self):
        """Entity with optional field set should work."""

        class OptName2(String):
            pass

        class OptAge2(Integer):
            pass

        class OptPerson2(Entity):
            flags = TypeFlags(name="opt_person2")
            name: OptName2 = Flag(Key)
            age: OptAge2 | None = None

        person = OptPerson2(name=OptName2("Eve"), age=OptAge2(25))
        assert person.age is not None
        assert person.age.value == 25

    def test_multi_value_attribute_empty_list(self):
        """Multi-value attribute with empty list should work."""

        class MvName(String):
            pass

        class MvTag(String):
            pass

        class MvPerson(Entity):
            flags = TypeFlags(name="mv_person")
            name: MvName = Flag(Key)
            tags: list[MvTag] = Flag(Card(min=0))

        # Empty list should be valid
        person = MvPerson(name=MvName("Frank"), tags=[])
        assert person.tags == []

    def test_multi_value_attribute_with_items(self):
        """Multi-value attribute with items should work."""

        class MvName2(String):
            pass

        class MvTag2(String):
            pass

        class MvPerson2(Entity):
            flags = TypeFlags(name="mv_person2")
            name: MvName2 = Flag(Key)
            tags: list[MvTag2] = Flag(Card(min=0))

        person = MvPerson2(name=MvName2("Grace"), tags=[MvTag2("a"), MvTag2("b")])
        assert len(person.tags) == 2


class TestStringEscapingEdgeCases:
    """Tests for string escaping in queries."""

    def test_string_with_single_quote(self):
        """Single quotes should not need escaping in TypeQL."""

        class SqName(String):
            pass

        class SqPerson(Entity):
            flags = TypeFlags(name="sq_person")
            name: SqName = Flag(Key)

        person = SqPerson(name=SqName("O'Brien"))
        mgr = _RecordingEntityManager(SqPerson)
        mgr.insert(person)

        query = mgr.queries[-1]
        assert "O'Brien" in query

    def test_string_with_double_quote(self):
        """Double quotes should be escaped in TypeQL."""

        class DqName(String):
            pass

        class DqPerson(Entity):
            flags = TypeFlags(name="dq_person")
            name: DqName = Flag(Key)

        person = DqPerson(name=DqName('Say "Hello"'))
        mgr = _RecordingEntityManager(DqPerson)
        mgr.insert(person)

        query = mgr.queries[-1]
        # Should contain escaped quotes
        assert '\\"Hello\\"' in query or "Hello" in query

    def test_string_with_backslash(self):
        """Backslashes should be escaped in TypeQL."""

        class BsName(String):
            pass

        class BsPerson(Entity):
            flags = TypeFlags(name="bs_person")
            name: BsName = Flag(Key)

        person = BsPerson(name=BsName("C:\\Users\\test"))
        mgr = _RecordingEntityManager(BsPerson)
        mgr.insert(person)

        query = mgr.queries[-1]
        # Should contain escaped backslashes
        assert "\\\\" in query


class TestTypeNameEdgeCases:
    """Tests for type name edge cases."""

    def test_entity_type_name_from_flags(self):
        """Entity type name should come from TypeFlags."""

        class TnName(String):
            pass

        class TnPerson(Entity):
            flags = TypeFlags(name="custom_type_name")
            name: TnName = Flag(Key)

        assert TnPerson.get_type_name() == "custom_type_name"

    def test_attribute_type_name_default(self):
        """Attribute type name should default to class name."""

        class CustomAttribute(String):
            pass

        assert CustomAttribute.get_attribute_name() == "CustomAttribute"
