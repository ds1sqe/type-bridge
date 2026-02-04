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

        Polymorphic queries now fetch ALL attributes including subtype-specific ones
        using TypeDB's nested wildcard syntax: "attributes": { $e.* }
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

        # Verify all attributes are populated including subtype-specific ones
        for artifact in artifacts:
            assert artifact._iid is not None
            assert artifact.name is not None  # name is on Artifact base type

            # Subtype-specific attributes are now populated directly from polymorphic query
            if isinstance(artifact, UserStory):
                assert artifact.name.value == "Login Feature"
                assert artifact.priority is not None
                assert artifact.priority.value == 1
            elif isinstance(artifact, DesignAspect):
                assert artifact.name.value == "Security"
                assert artifact.category is not None
                assert artifact.category.value == "NFR"

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

    def test_get_by_iid_on_supertype_fetches_subtype_attributes(self):
        """Test that get_by_iid on supertype manager fetches subtype-specific attributes.

        Regression test: Previously, querying via base type manager didn't fetch
        subtype-specific attributes because the fetch clause explicitly listed
        attributes from the base type only. The fix uses TypeDB's wildcard syntax
        with nested braces ("attributes": { $e.* }) which fetches ALL attributes
        the entity actually has, including subtype-specific ones.
        """
        # Insert a UserStory with priority (subtype-specific attribute)
        story = UserStory(name=ArtifactName("Polymorphic IID Test"), priority=Priority(42))
        UserStory.manager(self.db).insert(story)

        # Get IID via concrete type manager
        fetched_story = UserStory.manager(self.db).get(name="Polymorphic IID Test")[0]
        iid = fetched_story._iid
        assert iid is not None

        # Fetch via SUPERTYPE manager using IID - this is the key test
        artifact = Artifact.manager(self.db).get_by_iid(iid)
        assert artifact is not None
        assert type(artifact).__name__ == "UserStory"
        assert isinstance(artifact, UserStory)  # Runtime type check

        # The bug: subtype attributes weren't being fetched
        # After fix: subtype attributes SHOULD be populated
        # Cast to concrete type for type checker (already verified via isinstance)
        user_story = artifact  # type: UserStory
        assert user_story.priority is not None, "Subtype attribute 'priority' should be populated"
        assert user_story.priority.value == 42

    def test_get_on_supertype_fetches_subtype_attributes(self):
        """Test that get() on supertype manager fetches subtype-specific attributes.

        Regression test for polymorphic attribute fetching.
        """
        # Insert a DesignAspect with category (subtype-specific attribute)
        aspect = DesignAspect(name=ArtifactName("Polymorphic Get Test"), category=Category("NFR"))
        DesignAspect.manager(self.db).insert(aspect)

        # Get via SUPERTYPE manager - this is the key test
        artifacts = Artifact.manager(self.db).get(name="Polymorphic Get Test")
        assert len(artifacts) == 1
        artifact = artifacts[0]
        assert type(artifact).__name__ == "DesignAspect"
        assert isinstance(artifact, DesignAspect)  # Runtime type check

        # The bug: subtype attributes weren't being fetched
        # After fix: subtype attributes SHOULD be populated
        # Cast to concrete type for type checker (already verified via isinstance)
        design_aspect = artifact  # type: DesignAspect
        assert design_aspect.category is not None, (
            "Subtype attribute 'category' should be populated"
        )
        assert design_aspect.category.value == "NFR"

    def test_filter_on_supertype_fetches_subtype_attributes(self):
        """Test that filter().execute() on supertype fetches subtype-specific attributes.

        Regression test for polymorphic attribute fetching.
        """
        # Insert multiple entities of different subtypes
        story = UserStory(name=ArtifactName("Story for Filter"), priority=Priority(99))
        UserStory.manager(self.db).insert(story)

        aspect = DesignAspect(name=ArtifactName("Aspect for Filter"), category=Category("Security"))
        DesignAspect.manager(self.db).insert(aspect)

        # Filter on supertype
        artifacts = (
            Artifact.manager(self.db).filter(name=ArtifactName("Story for Filter")).execute()
        )
        assert len(artifacts) == 1
        artifact = artifacts[0]
        assert type(artifact).__name__ == "UserStory"
        assert isinstance(artifact, UserStory)

        # Subtype attributes should be populated
        user_story = artifact  # type: UserStory
        assert user_story.priority is not None, "Subtype attribute 'priority' should be populated"
        assert user_story.priority.value == 99

    def test_all_on_supertype_fetches_subtype_attributes(self):
        """Test that all() on supertype manager fetches subtype-specific attributes.

        Regression test for polymorphic attribute fetching.
        """
        # Insert entities of different subtypes with their specific attributes
        story = UserStory(name=ArtifactName("All Test Story"), priority=Priority(77))
        UserStory.manager(self.db).insert(story)

        aspect = DesignAspect(name=ArtifactName("All Test Aspect"), category=Category("Usability"))
        DesignAspect.manager(self.db).insert(aspect)

        # Query all via supertype
        artifacts = Artifact.manager(self.db).all()
        assert len(artifacts) == 2

        # Find each entity and verify subtype attributes are populated
        story_found = False
        aspect_found = False

        for artifact in artifacts:
            if isinstance(artifact, UserStory):
                story_found = True
                assert artifact.priority is not None, "UserStory.priority should be populated"
                assert artifact.priority.value == 77
            elif isinstance(artifact, DesignAspect):
                aspect_found = True
                assert artifact.category is not None, "DesignAspect.category should be populated"
                assert artifact.category.value == "Usability"

        assert story_found, "UserStory not found in polymorphic query results"
        assert aspect_found, "DesignAspect not found in polymorphic query results"

    def test_mixed_subtypes_with_optional_attributes(self):
        """Test polymorphic queries with entities that have None subtype attributes.

        Some entities may have their subtype-specific attribute set to None.
        The polymorphic query should handle this correctly.
        """
        # Insert story WITH priority
        story_with = UserStory(name=ArtifactName("Story With Priority"), priority=Priority(5))
        UserStory.manager(self.db).insert(story_with)

        # Insert story WITHOUT priority (None)
        story_without = UserStory(name=ArtifactName("Story Without Priority"), priority=None)
        UserStory.manager(self.db).insert(story_without)

        # Query via supertype
        artifacts = Artifact.manager(self.db).all()
        assert len(artifacts) == 2

        for artifact in artifacts:
            assert isinstance(artifact, UserStory)
            if artifact.name.value == "Story With Priority":
                assert artifact.priority is not None
                assert artifact.priority.value == 5
            elif artifact.name.value == "Story Without Priority":
                # This should be None, not missing
                assert artifact.priority is None
