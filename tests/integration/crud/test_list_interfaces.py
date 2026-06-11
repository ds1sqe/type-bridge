"""Integration tests for list-interface schema support.

Scope: schema-side only.  TypeDB rejects instance-level list operations with
REP256 ("List types are not yet implemented"), so these tests prove the two
things the engine CAN do today, and pin the boundary it cannot:

1. ``test_list_schema_sync_accepted`` — models declaring ``Flag(Ordered,
   Distinct)`` owns and ``Role(..., ordered=True, distinct=True)`` sync to a
   live TypeDB, and a second sync is a no-op.
2. ``test_list_instance_writes_unimplemented`` — a raw TypeQL list insert is
   rejected by the engine.  When this test FAILS on a newer TypeDB, the
   engine has implemented list instances: schedule the deferred runtime
   list-semantics work and replace this pin with real round-trip tests.
"""

import pytest

from type_bridge import (
    AttributeFlags,
    Database,
    Distinct,
    Entity,
    Flag,
    Key,
    Ordered,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
class TestListInterfaces:
    """Live-DB schema sync for ordered/distinct owns and roles."""

    @pytest.fixture
    def list_schema(self, clean_db: Database):
        """Define list-bearing models, sync schema, return context."""

        class ListId(String):
            flags = AttributeFlags(name="li_id")

        class ListTag(String):
            flags = AttributeFlags(name="li_tag")

        class Book(Entity):
            flags = TypeFlags(name="li_book")
            id: ListId = Flag(Key)
            tags: list[ListTag] = Flag(Ordered, Distinct)

        class Reviewer(Entity):
            flags = TypeFlags(name="li_reviewer")
            id: ListId = Flag(Key)

        class Rating(Relation):
            flags = TypeFlags(name="li_rating")
            rated: Role[Book] = Role("rated", Book)
            reviewer: Role[Reviewer] = Role("reviewer", Reviewer, ordered=True, distinct=True)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Book, Reviewer, Rating)
        schema_manager.sync_schema(force=True)

        return clean_db, Book, Reviewer, Rating

    def test_list_schema_sync_accepted(self, list_schema):
        """List-bearing schema syncs, and a re-sync is idempotent."""
        db, Book, Reviewer, Rating = list_schema

        schema = db.get_schema()
        assert "li_book" in schema
        assert "li_rating" in schema

        # Re-sync the same models: define is idempotent, must not raise.
        schema_manager = SchemaManager(db)
        schema_manager.register(Book, Reviewer, Rating)
        schema_manager.sync_schema(force=True)

    def test_list_instance_writes_unimplemented(self, list_schema):
        """The engine still rejects list-instance writes (REP256 boundary pin)."""
        db, _, _, _ = list_schema

        raw_insert = 'insert $b isa li_book, has li_id "x", has li_tag[] ["a", "b"];'
        with pytest.raises(Exception) as exc_info:
            db.execute_query(raw_insert, transaction_type="write")

        error_text = str(exc_info.value).lower()
        assert "not yet implemented" in error_text or "rep256" in error_text, (
            f"Expected the engine's list-unimplemented rejection; got: {exc_info.value!r}. "
            f"If the engine now ACCEPTS list inserts, implement runtime list semantics "
            f"and replace this pin with real round-trip tests."
        )
