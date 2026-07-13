"""SchemaManager's live-sync lowering must carry every role/owns marker.

``SchemaInfo.to_rust_schema_info`` hand-projects model descriptors into the
Rust ``SchemaInfo`` dict shape.  That projection once dropped ``overrides``,
``is_abstract``, ``ordered``, ``distinct``, and ``is_ordered`` — the synced
schema silently lost role specialization and abstract/list markers while the
descriptor-registry path carried them.  These tests pin the emitted define
block so the projection cannot drop a marker again.
"""

from __future__ import annotations

from type_bridge import (
    AttributeFlags,
    Card,
    Distinct,
    Entity,
    Flag,
    Key,
    Ordered,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge._rust_runtime import descriptor_for_model
from type_bridge.migration.info import SchemaInfo


class SyncMarkerId(String):
    flags = AttributeFlags(name="sm_id")


class SyncMarkerTag(String):
    flags = AttributeFlags(name="sm_tag")


class SyncMarkerBook(Entity):
    flags = TypeFlags(name="sm_book")
    id: SyncMarkerId = Flag(Key)
    tags: list[SyncMarkerTag] = Flag(Ordered, Distinct)


class SyncMarkerReviewer(Entity):
    flags = TypeFlags(name="sm_reviewer")
    id: SyncMarkerId = Flag(Key)


class SyncMarkerRating(Relation):
    flags = TypeFlags(name="sm_rating")
    rated: Role[SyncMarkerBook] = Role("rated", SyncMarkerBook, abstract=True)
    reviewer: Role[SyncMarkerReviewer] = Role(
        "reviewer",
        SyncMarkerReviewer,
        cardinality=Card(0, 3),
        ordered=True,
        distinct=True,
    )


class SyncMarkerCritique(SyncMarkerRating):
    flags = TypeFlags(name="sm_critique")
    target: Role[SyncMarkerBook] = Role("target", SyncMarkerBook, overrides="rated")


def _schema() -> SchemaInfo:
    schema = SchemaInfo()
    schema.entities = [SyncMarkerBook, SyncMarkerReviewer]
    schema.relations = [SyncMarkerRating, SyncMarkerCritique]
    return schema


def test_sync_define_carries_owns_list_markers() -> None:
    typeql = _schema().to_typeql()
    assert "owns sm_tag[] @distinct" in typeql
    # No implicit scalar card may leak onto the list attribute.
    assert "owns sm_tag[] @distinct @card" not in typeql


def test_sync_define_carries_role_markers() -> None:
    typeql = _schema().to_typeql()
    assert "relates rated @abstract" in typeql
    assert "relates reviewer[] @distinct @card(0..3)" in typeql
    assert "relates target as rated" in typeql


def test_direct_entity_definition_matches_sync_owns_markers() -> None:
    """The legacy direct renderer must not erase list ownership semantics."""
    direct = SyncMarkerBook.to_schema_definition()
    synced = _schema().to_typeql()

    assert direct is not None
    assert "owns sm_tag[] @distinct" in direct
    assert "owns sm_tag[] @distinct" in synced


def test_direct_relation_definition_matches_sync_role_markers() -> None:
    """Direct relation TypeQL carries every marker retained by SchemaInfo."""
    direct_parent = SyncMarkerRating.to_schema_definition()
    direct_child = SyncMarkerCritique.to_schema_definition()
    synced = _schema().to_typeql()

    assert direct_parent is not None
    assert direct_child is not None
    assert "relates rated @abstract" in direct_parent
    assert "relates reviewer[] @distinct @card(0..3)" in direct_parent
    assert "relates target as rated" in direct_child
    assert "relates reviewer[] @distinct @card(0..3)" in synced
    assert "relates target as rated" in synced


def test_bare_ordered_descriptor_is_optional_without_inventing_card() -> None:
    """Python matches bindgen/Node: bare ordered owns are optional, cardless lists."""
    descriptor = descriptor_for_model(SyncMarkerBook)
    tag = next(
        attribute
        for attribute in descriptor["owned_attributes"]
        if attribute["field_name"] == "tags"
    )

    assert tag == {
        "field_name": "tags",
        "attr_name": "sm_tag",
        "value_type": "string",
        "annotations": ["Distinct"],
        "is_optional": True,
        "is_ordered": True,
    }
