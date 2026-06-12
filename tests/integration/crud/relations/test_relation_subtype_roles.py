"""Integration tests for relation subtype role inheritance and specialization.

Verifies the effective-role-set descriptor contract end-to-end:
- Schema sync registers contribution and authoring correctly.
- Insert via the Authoring manager fills both the plain-inherited role ('work')
  and the specialized role ('author').
- Fetch + hydration returns both role players correctly.
"""

import pytest

from type_bridge import (
    Database,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
class TestRelationSubtypeRoles:
    """Live-DB CRUD through a relation-subtype manager exercising inherited and own roles."""

    @pytest.fixture
    def subtype_schema(self, clean_db: Database):
        """Define contribution/authoring models, sync schema, return context."""

        class Title(String):
            pass

        class ContribAuthorName(String):
            pass

        class Contributor(Entity):
            flags = TypeFlags(name="rt_contributor")
            name: ContribAuthorName = Flag(Key)

        class Work(Entity):
            flags = TypeFlags(name="rt_work")
            title: Title = Flag(Key)

        class Author(Entity):
            flags = TypeFlags(name="rt_author")
            name: ContribAuthorName = Flag(Key)

        class Contribution(Relation):
            flags = TypeFlags(name="rt_contribution")
            contributor: Role[Contributor] = Role("contributor", Contributor)
            work: Role[Work] = Role("work", Work)

        class Authoring(Contribution):
            flags = TypeFlags(name="rt_authoring")
            author: Role[Author] = Role("author", Author, overrides="contributor")

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Contributor, Work, Author, Contribution, Authoring)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Contributor,
            Work,
            Author,
            Contribution,
            Authoring,
            Title,
            ContribAuthorName,
        )

    def test_authoring_descriptor_effective_roles(self, subtype_schema):
        """Authoring descriptor lists [work, author] — the canonical effective set."""
        _, _, _, _, _, Authoring, _, _ = subtype_schema
        from type_bridge._rust_runtime import descriptor_for_model

        d = descriptor_for_model(Authoring)
        role_names = [r["role_name"] for r in d["roles"]]
        assert role_names == ["work", "author"], f"unexpected roles: {role_names}"
        assert "contributor" not in role_names, "'contributor' must be excluded (overridden)"

    def test_insert_and_fetch_authoring_with_both_roles(self, subtype_schema):
        """Insert an Authoring instance that fills both the inherited 'work' role
        and the specializing 'author' role, then verify hydration returns both players.
        """
        db, _, Work, Author, _, Authoring, Title, ContribAuthorName = subtype_schema

        the_work = Work(title=Title("Sonnet 18"))
        the_author = Author(name=ContribAuthorName("Shakespeare"))

        Work.manager(db).insert(the_work)
        Author.manager(db).insert(the_author)

        authoring = Authoring(work=the_work, author=the_author)
        Authoring.manager(db).insert(authoring)

        results = Authoring.manager(db).all()
        assert len(results) == 1, f"expected 1 authoring, got {len(results)}"

        fetched = results[0]
        # Plain-inherited role player must hydrate.
        assert fetched.work is not None, "plain-inherited 'work' role player must hydrate"
        assert fetched.work.title.value == "Sonnet 18"
        # Specializing role player must hydrate.
        assert fetched.author is not None, "specializing 'author' role player must hydrate"
        assert fetched.author.name.value == "Shakespeare"

    def test_contribution_manager_unaffected(self, subtype_schema):
        """The Contribution manager still operates on its own type; no regression."""
        db, Contributor, Work, _, Contribution, _, Title, ContribAuthorName = subtype_schema

        contrib_person = Contributor(name=ContribAuthorName("Anonymous"))
        contrib_work = Work(title=Title("Untitled"))

        Contributor.manager(db).insert(contrib_person)
        Work.manager(db).insert(contrib_work)

        c = Contribution(contributor=contrib_person, work=contrib_work)
        Contribution.manager(db).insert(c)

        results = Contribution.manager(db).all()
        assert len(results) >= 1


@pytest.mark.integration
class TestAbstractParentRole:
    """Live-DB CRUD exercising an abstract parent role and its specializations.

    The 'participant' role on BaseEvent is abstract; ConcreteEvent overrides it
    with 'actor'.  A plain child relation (PlainEvent) plain-inherits the abstract
    role — the engine accepts players on a plain-inherited abstract role on a
    subtype relation.
    """

    @pytest.fixture
    def abstract_role_schema(self, clean_db: Database):
        """Define base/concrete/plain event models, sync schema, return context."""

        class EventId(String):
            pass

        class ActorName(String):
            pass

        class EventActor(Entity):
            flags = TypeFlags(name="ar_actor")
            name: ActorName = Flag(Key)

        class BaseEvent(Relation):
            flags = TypeFlags(name="ar_base_event")
            participant: Role[EventActor] = Role("participant", EventActor, abstract=True)

        class ConcreteEvent(BaseEvent):
            """Child that overrides the abstract parent role with a specialization."""

            flags = TypeFlags(name="ar_concrete_event")
            actor: Role[EventActor] = Role("actor", EventActor, overrides="participant")

        class PlainEvent(BaseEvent):
            """Child that plain-inherits the abstract parent role (engine accepts this)."""

            flags = TypeFlags(name="ar_plain_event")

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(EventActor, BaseEvent, ConcreteEvent, PlainEvent)
        schema_manager.sync_schema(force=True)

        return clean_db, EventActor, BaseEvent, ConcreteEvent, PlainEvent, ActorName

    def test_sync_schema_succeeds(self, abstract_role_schema):
        """Schema sync with an abstract parent role must complete without error."""
        # If we reach this point, sync_schema() raised no exception.
        _, _, _, _, _, _ = abstract_role_schema

    def test_insert_through_specializing_role_works(self, abstract_role_schema):
        """Insert via ConcreteEvent (overrides participant with actor) and fetch back."""
        db, EventActor, _, ConcreteEvent, _, ActorName = abstract_role_schema

        actor = EventActor(name=ActorName("alice"))
        EventActor.manager(db).insert(actor)

        event = ConcreteEvent(actor=actor)
        ConcreteEvent.manager(db).insert(event)

        results = ConcreteEvent.manager(db).all()
        assert len(results) == 1
        assert results[0].actor is not None
        assert results[0].actor.name.value == "alice"

    def test_insert_through_inherited_abstract_role_works(self, abstract_role_schema):
        """Insert via PlainEvent's plain-inherited abstract role.

        The engine evidence confirms: a concrete subtype that plain-inherits an
        abstract role CAN have players supplied for it — only direct play at the
        DECLARING relation's own scope is rejected by the engine.
        """
        db, EventActor, _, _, PlainEvent, ActorName = abstract_role_schema

        actor = EventActor(name=ActorName("bob"))
        EventActor.manager(db).insert(actor)

        event = PlainEvent(participant=actor)
        PlainEvent.manager(db).insert(event)

        results = PlainEvent.manager(db).all()
        assert len(results) == 1
        assert results[0].participant is not None
        assert results[0].participant.name.value == "bob"
