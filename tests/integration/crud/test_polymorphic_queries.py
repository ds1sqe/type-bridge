"""Integration tests for polymorphic entity queries.

Tests for issue #65: Polymorphic entity instantiation.
When querying a supertype, entities should be instantiated as their actual
concrete subtype class.
"""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


# Define attribute types
class ArtifactName(String):
    pass


class Priority(Integer):
    pass


class Category(String):
    pass


# Define entity hierarchy
class Artifact(Entity):
    """Abstract base artifact type."""

    flags = TypeFlags(name="artifact", abstract=True)
    name: ArtifactName = Flag(Key)


class UserStory(Artifact):
    """Concrete user story subtype."""

    flags = TypeFlags(name="user_story")
    priority: Priority | None = None


class DesignAspect(Artifact):
    """Concrete design aspect subtype."""

    flags = TypeFlags(name="design_aspect")
    category: Category | None = None


@pytest.mark.integration
class TestPolymorphicEntityQueries:
    """Tests for polymorphic entity instantiation."""

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Artifact)
        schema_manager.register(UserStory)
        schema_manager.register(DesignAspect)
        schema_manager.sync_schema(force=True)

    def test_query_supertype_returns_concrete_subtypes(self):
        """Test that querying supertype returns entities with correct concrete types.

        Note: Polymorphic queries can only fetch attributes defined on the base type.
        Subtype-specific attributes (like priority on UserStory) are NOT populated
        because TypeDB type-checks that all fetched attributes exist on all matching
        types. To get full subtype attributes, use get_by_iid() with the returned IID.
        """
        # Insert entities of different subtypes
        story = UserStory(name=ArtifactName("Login Feature"), priority=Priority(1))
        UserStory.manager(self.db).insert(story)

        aspect = DesignAspect(name=ArtifactName("Security"), category=Category("NFR"))
        DesignAspect.manager(self.db).insert(aspect)

        # Query the supertype
        artifacts = Artifact.manager(self.db).all()
        assert len(artifacts) == 2

        # Verify each entity has the correct concrete type
        types_found = {type(a).__name__ for a in artifacts}
        assert types_found == {"UserStory", "DesignAspect"}

        # Verify common attributes (from base type) are populated
        for artifact in artifacts:
            assert artifact._iid is not None
            assert artifact.name is not None  # name is on Artifact base type

        # Subtype-specific attributes are NOT populated from polymorphic queries
        # (TypeDB limitation - can only fetch attributes common to all matching types)
        # To get full attributes, use get_by_iid() with the concrete manager:
        for artifact in artifacts:
            if isinstance(artifact, UserStory):
                # Use get_by_iid to fetch full entity with subtype attributes
                full_story = UserStory.manager(self.db).get_by_iid(artifact._iid)
                assert full_story is not None
                assert full_story.name.value == "Login Feature"
                assert full_story.priority is not None
                assert full_story.priority.value == 1
            elif isinstance(artifact, DesignAspect):
                full_aspect = DesignAspect.manager(self.db).get_by_iid(artifact._iid)
                assert full_aspect is not None
                assert full_aspect.name.value == "Security"
                assert full_aspect.category is not None
                assert full_aspect.category.value == "NFR"

    def test_query_concrete_type_returns_same_type(self):
        """Test that querying a concrete type returns that exact type."""
        story = UserStory(name=ArtifactName("Epic Feature"), priority=Priority(2))
        UserStory.manager(self.db).insert(story)

        # Query the concrete type directly
        stories = UserStory.manager(self.db).all()
        assert len(stories) == 1
        assert type(stories[0]).__name__ == "UserStory"
        assert stories[0]._iid is not None

    def test_filter_supertype_returns_concrete_subtypes(self):
        """Test that filter on supertype returns correct concrete types."""
        story = UserStory(name=ArtifactName("Search Feature"), priority=Priority(3))
        UserStory.manager(self.db).insert(story)

        aspect = DesignAspect(name=ArtifactName("Performance"), category=Category("NFR"))
        DesignAspect.manager(self.db).insert(aspect)

        # Filter by name on supertype
        artifacts = Artifact.manager(self.db).filter(name=ArtifactName("Search Feature")).execute()
        assert len(artifacts) == 1
        assert type(artifacts[0]).__name__ == "UserStory"
        assert artifacts[0]._iid is not None

    def test_get_by_iid_on_supertype_returns_concrete_type(self):
        """Test that get_by_iid on supertype manager returns concrete type."""
        story = UserStory(name=ArtifactName("Delete Feature"), priority=Priority(4))
        UserStory.manager(self.db).insert(story)

        # Get IID via concrete type manager
        fetched_story = UserStory.manager(self.db).get(name="Delete Feature")[0]
        iid = fetched_story._iid
        assert iid is not None

        # Fetch via supertype manager using IID
        artifact = Artifact.manager(self.db).get_by_iid(iid)
        assert artifact is not None
        assert type(artifact).__name__ == "UserStory"
        assert artifact.name.value == "Delete Feature"

    def test_polymorphic_query_with_multiple_same_type(self):
        """Test polymorphic query with multiple entities of same subtype."""
        story1 = UserStory(name=ArtifactName("Feature A"), priority=Priority(1))
        story2 = UserStory(name=ArtifactName("Feature B"), priority=Priority(2))
        UserStory.manager(self.db).insert(story1)
        UserStory.manager(self.db).insert(story2)

        # Query supertype
        artifacts = Artifact.manager(self.db).all()
        assert len(artifacts) == 2
        assert all(type(a).__name__ == "UserStory" for a in artifacts)
        assert all(a._iid is not None for a in artifacts)
