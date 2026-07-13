"""Tests for TypeDB 3.12+ @doc/@meta annotation declaration and lowering."""

from type_bridge import (
    AttributeFlags,
    Doc,
    Entity,
    Flag,
    Key,
    Meta,
    Relation,
    String,
    TypeFlags,
)
from type_bridge.migration.info import SchemaInfo
from type_bridge.models.role import Role
from type_bridge.typeql.annotations import (
    escape_annotation_string,
    format_doc_meta_annotations,
)


class PersonName(String):
    flags = AttributeFlags(name="person_name", doc="a personal name", meta={"pii": "true"})


class Nick(String):
    pass


class DocPerson(Entity):
    flags = TypeFlags(
        name="doc_person",
        doc="an individual client",
        meta={"icon": "silhouette.png"},
    )
    name: PersonName = Flag(Key, Doc("full legal name"), Meta("column", "name"))
    nick: Nick | None = None


class DocFriendship(Relation):
    flags = TypeFlags(name="doc_friendship", doc="a mutual bond", meta={"color": "blue"})
    friend: Role[DocPerson] = Role(
        "friend",
        DocPerson,
        doc="one side of the bond",
        meta={"endpoint": "true"},
    )


class TestMarkers:
    def test_flag_captures_doc_and_meta(self) -> None:
        flags = Flag(Key, Doc("d"), Meta("k1", "v1"), Meta("k2", "v2"))
        assert flags.doc == "d"
        assert flags.meta == {"k1": "v1", "k2": "v2"}

    def test_flag_annotations_order_constraints_first(self) -> None:
        flags = Flag(Key, Doc("d"), Meta("k", "v"))
        assert flags.to_typeql_annotations() == ["@key", '@doc("d")', '@meta("k", "v")']

    def test_type_flags_capture(self) -> None:
        flags = TypeFlags(doc="d", meta={"k": "v"})
        assert flags.doc == "d"
        assert flags.meta == {"k": "v"}

    def test_type_flags_default_meta_is_independent(self) -> None:
        one = TypeFlags()
        two = TypeFlags()
        one.meta["k"] = "v"
        assert two.meta == {}


class TestFormatting:
    def test_escape_matches_typedb_export(self) -> None:
        assert (
            escape_annotation_string('line1\nline2 with "quotes" and back\\slash')
            == '"line1\\nline2 with \\"quotes\\" and back\\\\slash"'
        )

    def test_meta_keys_sorted(self) -> None:
        assert format_doc_meta_annotations(None, {"b": "2", "a": "1"}) == [
            '@meta("a", "1")',
            '@meta("b", "2")',
        ]


class TestLegacyLowering:
    def test_attribute_definition_carries_doc_meta(self) -> None:
        schema = PersonName.to_schema_definition()
        assert schema == (
            'attribute person_name @doc("a personal name") @meta("pii", "true"), value string;'
        )

    def test_entity_definition_carries_doc_meta(self) -> None:
        schema = DocPerson.to_schema_definition()
        assert schema is not None
        assert schema.startswith(
            'entity doc_person @doc("an individual client") @meta("icon", "silhouette.png")'
        )
        assert '    owns person_name @key @doc("full legal name") @meta("column", "name")' in schema

    def test_relation_definition_carries_doc_meta(self) -> None:
        schema = DocFriendship.to_schema_definition()
        assert schema is not None
        assert schema.startswith(
            'relation doc_friendship @doc("a mutual bond") @meta("color", "blue")'
        )
        assert '    relates friend @doc("one side of the bond") @meta("endpoint", "true")' in schema


class TestRustLowering:
    """The Rust generate_define_block path (authoritative lowering)."""

    def _schema_info(self) -> SchemaInfo:
        info = SchemaInfo()
        info.entities.append(DocPerson)
        info.relations.append(DocFriendship)
        return info

    def test_define_block_carries_type_level_annotations(self) -> None:
        typeql = self._schema_info().to_typeql()
        assert (
            'entity doc_person @doc("an individual client") @meta("icon", "silhouette.png"),'
            in typeql
        )
        assert 'relation doc_friendship @doc("a mutual bond") @meta("color", "blue"),' in typeql
        assert (
            'attribute person_name @doc("a personal name") @meta("pii", "true"), value string;'
            in typeql
        )

    def test_define_block_carries_capability_annotations(self) -> None:
        typeql = self._schema_info().to_typeql()
        assert '    owns person_name @key @doc("full legal name") @meta("column", "name")' in typeql
        assert '    relates friend @doc("one side of the bond") @meta("endpoint", "true")' in typeql

    def test_define_block_round_trips_through_introspection_parser(self) -> None:
        """Emitted define text parses back into an identical annotated schema."""
        import json

        import type_bridge_core

        typeql = self._schema_info().to_typeql()
        reparsed = json.loads(type_bridge_core.TypeSchema.from_typeql(typeql).to_json())
        entity = reparsed["entities"]["doc_person"]
        assert entity["doc"] == "an individual client"
        assert entity["meta"] == {"icon": "silhouette.png"}
        owns = entity["owns"][0]
        assert owns["doc"] == "full legal name"
        assert owns["meta"] == {"column": "name"}
        role = reparsed["relations"]["doc_friendship"]["roles"][0]
        assert role["doc"] == "one side of the bond"
        assert role["meta"] == {"endpoint": "true"}
