"""Integration tests that generate code and verify it imports correctly.

These tests exercise the full code generation pipeline:
1. Parse a TQL schema
2. Generate Python modules
3. Import the generated modules
4. Verify the classes are properly created
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from type_bridge.generator import generate_models

if TYPE_CHECKING:
    from types import ModuleType

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _import_generated_package(package_path: Path) -> dict[str, ModuleType]:
    """Import generated package modules dynamically.

    Returns dict with 'attributes', 'entities', 'relations' modules.
    """
    # Add package parent to path temporarily
    parent = str(package_path.parent)
    package_name = package_path.name

    if parent not in sys.path:
        sys.path.insert(0, parent)

    try:
        # Import the package and its submodules
        modules = {}

        # Import __init__
        spec = importlib.util.spec_from_file_location(package_name, package_path / "__init__.py")
        if spec and spec.loader:
            pkg = importlib.util.module_from_spec(spec)
            sys.modules[package_name] = pkg
            spec.loader.exec_module(pkg)

        # Import submodules
        for name in ["attributes", "entities", "relations"]:
            mod_path = package_path / f"{name}.py"
            mod_name = f"{package_name}.{name}"
            spec = importlib.util.spec_from_file_location(mod_name, mod_path)
            if spec and spec.loader:
                mod = importlib.util.module_from_spec(spec)
                sys.modules[mod_name] = mod
                spec.loader.exec_module(mod)
                modules[name] = mod

        return modules

    finally:
        # Clean up sys.modules to avoid test pollution
        keys_to_remove = [k for k in sys.modules if k.startswith(package_name)]
        for key in keys_to_remove:
            del sys.modules[key]
        if parent in sys.path:
            sys.path.remove(parent)


class TestBookstoreSchema:
    """Integration tests for the bookstore schema."""

    SCHEMA_PATH = FIXTURES_DIR / "bookstore.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the bookstore package."""
        output = tmp_path / "bookstore"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    def test_generates_all_modules(self, tmp_path: Path) -> None:
        """All expected files are generated."""
        output = tmp_path / "bookstore"
        generate_models(self.SCHEMA_PATH, output)

        assert (output / "__init__.py").exists()
        assert (output / "attributes.py").exists()
        assert (output / "entities.py").exists()
        assert (output / "relations.py").exists()
        assert (output / "schema.tql").exists()

    def test_imports_without_error(self, generated_package: dict[str, ModuleType]) -> None:
        """Generated modules import without error."""
        assert "attributes" in generated_package
        assert "entities" in generated_package
        assert "relations" in generated_package

    def test_entity_classes_exist(self, generated_package: dict[str, ModuleType]) -> None:
        """Expected entity classes are generated."""
        entities = generated_package["entities"]

        # Check key entities exist
        assert hasattr(entities, "Book")
        assert hasattr(entities, "Hardback")
        assert hasattr(entities, "Paperback")
        assert hasattr(entities, "Ebook")
        assert hasattr(entities, "User")
        assert hasattr(entities, "Order")
        assert hasattr(entities, "Contributor")

    def test_entity_inheritance(self, generated_package: dict[str, ModuleType]) -> None:
        """Entity inheritance is correctly set up."""
        entities = generated_package["entities"]

        # Hardback, Paperback, Ebook should inherit from Book
        assert issubclass(entities.Hardback, entities.Book)
        assert issubclass(entities.Paperback, entities.Book)
        assert issubclass(entities.Ebook, entities.Book)

        # Publisher, Courier should inherit from Company
        assert hasattr(entities, "Company")
        assert issubclass(entities.Publisher, entities.Company)
        assert issubclass(entities.Courier, entities.Company)

    def test_attribute_classes_exist(self, generated_package: dict[str, ModuleType]) -> None:
        """Expected attribute classes are generated."""
        attributes = generated_package["attributes"]

        # Check key attributes exist
        assert hasattr(attributes, "Isbn")
        assert hasattr(attributes, "Isbn13")
        assert hasattr(attributes, "Isbn10")
        assert hasattr(attributes, "Title")
        assert hasattr(attributes, "Name")
        assert hasattr(attributes, "Price")

    def test_attribute_inheritance(self, generated_package: dict[str, ModuleType]) -> None:
        """Attribute inheritance is correctly set up."""
        attributes = generated_package["attributes"]

        # isbn-13 and isbn-10 should inherit from isbn
        assert issubclass(attributes.Isbn13, attributes.Isbn)
        assert issubclass(attributes.Isbn10, attributes.Isbn)

    def test_relation_classes_exist(self, generated_package: dict[str, ModuleType]) -> None:
        """Expected relation classes are generated."""
        relations = generated_package["relations"]

        # Check key relations exist
        assert hasattr(relations, "Contribution")
        assert hasattr(relations, "Authoring")
        assert hasattr(relations, "Publishing")
        assert hasattr(relations, "OrderLine")

    def test_relation_inheritance(self, generated_package: dict[str, ModuleType]) -> None:
        """Relation inheritance is correctly set up."""
        relations = generated_package["relations"]

        # Authoring, Editing, Illustrating should inherit from Contribution
        assert issubclass(relations.Authoring, relations.Contribution)
        assert issubclass(relations.Editing, relations.Contribution)
        assert issubclass(relations.Illustrating, relations.Contribution)


class TestSocialMediaSchema:
    """Integration tests for the social media schema."""

    SCHEMA_PATH = FIXTURES_DIR / "social_media.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the social media package."""
        output = tmp_path / "social_media"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    def test_imports_without_error(self, generated_package: dict[str, ModuleType]) -> None:
        """Generated modules import without error."""
        assert "attributes" in generated_package
        assert "entities" in generated_package
        assert "relations" in generated_package

    def test_entity_inheritance_chain(self, generated_package: dict[str, ModuleType]) -> None:
        """Deep entity inheritance chains work correctly."""
        entities = generated_package["entities"]

        # content -> page -> profile
        assert hasattr(entities, "Content")
        assert hasattr(entities, "Page")
        assert hasattr(entities, "Profile")

        assert issubclass(entities.Page, entities.Content)
        assert issubclass(entities.Profile, entities.Page)

        # content -> post -> text-post, image-post
        assert hasattr(entities, "Post")
        assert hasattr(entities, "TextPost")
        assert hasattr(entities, "ImagePost")

        assert issubclass(entities.Post, entities.Content)
        assert issubclass(entities.TextPost, entities.Post)
        assert issubclass(entities.ImagePost, entities.Post)

    def test_relation_inheritance_chain(self, generated_package: dict[str, ModuleType]) -> None:
        """Deep relation inheritance chains work correctly."""
        relations = generated_package["relations"]

        # interaction -> content-engagement -> posting, commenting, reaction
        assert hasattr(relations, "Interaction")
        assert hasattr(relations, "ContentEngagement")
        assert hasattr(relations, "Posting")
        assert hasattr(relations, "Commenting")
        assert hasattr(relations, "Reaction")

        assert issubclass(relations.ContentEngagement, relations.Interaction)
        assert issubclass(relations.Posting, relations.ContentEngagement)
        assert issubclass(relations.Commenting, relations.ContentEngagement)
        assert issubclass(relations.Reaction, relations.ContentEngagement)

    def test_attribute_inheritance_chain(self, generated_package: dict[str, ModuleType]) -> None:
        """Deep attribute inheritance chains work correctly."""
        attributes = generated_package["attributes"]

        # id -> post-id, profile-id, group-id, comment-id
        assert hasattr(attributes, "Id")
        assert hasattr(attributes, "PostId")
        assert hasattr(attributes, "ProfileId")
        assert hasattr(attributes, "GroupId")
        assert hasattr(attributes, "CommentId")

        assert issubclass(attributes.PostId, attributes.Id)
        assert issubclass(attributes.ProfileId, attributes.Id)
        assert issubclass(attributes.GroupId, attributes.Id)
        assert issubclass(attributes.CommentId, attributes.Id)

        # payload -> text-payload -> bio, comment-text, post-text
        assert hasattr(attributes, "Payload")
        assert hasattr(attributes, "TextPayload")
        assert hasattr(attributes, "Bio")

        assert issubclass(attributes.TextPayload, attributes.Payload)
        assert issubclass(attributes.Bio, attributes.TextPayload)

    def test_values_constraint(self, generated_package: dict[str, ModuleType]) -> None:
        """@values constraint generates allowed_values."""
        attributes = generated_package["attributes"]

        assert hasattr(attributes, "Emoji")
        emoji_cls = attributes.Emoji
        assert hasattr(emoji_cls, "allowed_values")
        assert "like" in emoji_cls.allowed_values
        assert "love" in emoji_cls.allowed_values

    def test_regex_constraint(self, generated_package: dict[str, ModuleType]) -> None:
        """@regex constraint generates regex attribute."""
        attributes = generated_package["attributes"]

        assert hasattr(attributes, "PostImage")
        post_image_cls = attributes.PostImage
        assert hasattr(post_image_cls, "regex")
        assert ".png" in post_image_cls.regex


class TestTypeTheoreticSchema:
    """Integration tests for the type-theoretic schema.

    This schema uses relations in unconventional ways (relations that play roles
    in other relations), testing more advanced TypeDB patterns.
    """

    SCHEMA_PATH = FIXTURES_DIR / "type_theoretic.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the type-theoretic package."""
        output = tmp_path / "type_theoretic"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    def test_imports_without_error(self, generated_package: dict[str, ModuleType]) -> None:
        """Generated modules import without error."""
        assert "attributes" in generated_package
        assert "entities" in generated_package
        assert "relations" in generated_package

    def test_relation_as_player(self, generated_package: dict[str, ModuleType]) -> None:
        """Relations that play roles are generated correctly."""
        relations = generated_package["relations"]

        # publication is a relation that plays roles in other relations
        assert hasattr(relations, "Publication")

        # book, serial, music-score inherit from publication
        assert hasattr(relations, "Book")
        assert hasattr(relations, "Serial")
        assert hasattr(relations, "MusicScore")

        assert issubclass(relations.Book, relations.Publication)
        assert issubclass(relations.Serial, relations.Publication)
        assert issubclass(relations.MusicScore, relations.Publication)

    def test_deep_relation_hierarchy(self, generated_package: dict[str, ModuleType]) -> None:
        """Multi-level relation inheritance works."""
        relations = generated_package["relations"]

        # publication -> book -> hardback, paperback, ebook
        assert hasattr(relations, "Hardback")
        assert hasattr(relations, "Paperback")
        assert hasattr(relations, "Ebook")

        assert issubclass(relations.Hardback, relations.Book)
        assert issubclass(relations.Paperback, relations.Book)
        assert issubclass(relations.Ebook, relations.Book)

        # publication -> serial -> newspaper, magazine, journal
        assert hasattr(relations, "Newspaper")
        assert hasattr(relations, "Magazine")
        assert hasattr(relations, "Journal")

        assert issubclass(relations.Newspaper, relations.Serial)
        assert issubclass(relations.Magazine, relations.Serial)
        assert issubclass(relations.Journal, relations.Serial)

    def test_attribute_hierarchy(self, generated_package: dict[str, ModuleType]) -> None:
        """Attribute hierarchy for ISN types."""
        attributes = generated_package["attributes"]

        # isn -> isbn -> isbn-13, isbn-10
        #     -> issn
        #     -> ismn
        assert hasattr(attributes, "Isn")
        assert hasattr(attributes, "Isbn")
        assert hasattr(attributes, "Isbn13")
        assert hasattr(attributes, "Isbn10")
        assert hasattr(attributes, "Issn")
        assert hasattr(attributes, "Ismn")

        assert issubclass(attributes.Isbn, attributes.Isn)
        assert issubclass(attributes.Isbn13, attributes.Isbn)
        assert issubclass(attributes.Isbn10, attributes.Isbn)
        assert issubclass(attributes.Issn, attributes.Isn)
        assert issubclass(attributes.Ismn, attributes.Isn)


class TestRoleCardinalitySchema:
    """Integration tests for role cardinality patterns.

    Tests that relations with role cardinality annotations (@card) are
    correctly generated and can be instantiated with multiple role players.
    """

    SCHEMA_PATH = FIXTURES_DIR / "role_cardinality.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the role cardinality package."""
        output = tmp_path / "role_cardinality"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    def test_imports_without_error(self, generated_package: dict[str, ModuleType]) -> None:
        """Generated modules import without error."""
        assert "attributes" in generated_package
        assert "entities" in generated_package
        assert "relations" in generated_package

    def test_relation_classes_exist(self, generated_package: dict[str, ModuleType]) -> None:
        """Expected relation classes with role cardinality are generated."""
        relations = generated_package["relations"]

        # Note: Generator uses underscores for multi-word names (e.g., is_similar_to -> Is_similar_to)
        assert hasattr(relations, "Is_similar_to")
        assert hasattr(relations, "Friendship")
        assert hasattr(relations, "Group_membership")
        assert hasattr(relations, "Review")

    def test_role_has_card_attribute(self, generated_package: dict[str, ModuleType]) -> None:
        """Roles with cardinality have Card in their definition."""
        relations = generated_package["relations"]

        # Check that Is_similar_to has a similar_memory role with cardinality
        is_similar_to = relations.Is_similar_to
        roles = is_similar_to.get_roles()
        assert "similar_memory" in roles

        # The role should have cardinality info
        similar_memory_role = roles["similar_memory"]
        assert similar_memory_role.cardinality is not None
        assert similar_memory_role.cardinality.min == 2
        assert similar_memory_role.cardinality.max == 2

    def test_friendship_role_cardinality(self, generated_package: dict[str, ModuleType]) -> None:
        """Friendship relation has @card(2..2) on friend role."""
        relations = generated_package["relations"]

        friendship = relations.Friendship
        roles = friendship.get_roles()
        assert "friend" in roles

        friend_role = roles["friend"]
        assert friend_role.cardinality is not None
        assert friend_role.cardinality.min == 2
        assert friend_role.cardinality.max == 2

    def test_unbounded_cardinality_role(self, generated_package: dict[str, ModuleType]) -> None:
        """Group_membership has @card(2..) on member role (unbounded)."""
        relations = generated_package["relations"]

        membership = relations.Group_membership
        roles = membership.get_roles()

        # group role has default cardinality (1..1) - not emitted, so None
        group_role = roles["group"]
        assert group_role.cardinality is None  # Default cardinality not emitted

        # member role has @card(2..) - unbounded
        member_role = roles["member"]
        assert member_role.cardinality is not None
        assert member_role.cardinality.min == 2
        assert member_role.cardinality.max is None  # Unbounded

    def test_bounded_cardinality_role(self, generated_package: dict[str, ModuleType]) -> None:
        """Review has @card(1..3) on reviewer role."""
        relations = generated_package["relations"]

        review = relations.Review
        roles = review.get_roles()

        # document role has @card(1..1) - default, not emitted
        document_role = roles["document"]
        assert document_role.cardinality is None  # Default cardinality not emitted

        # reviewer role has @card(1..3)
        reviewer_role = roles["reviewer"]
        assert reviewer_role.cardinality is not None
        assert reviewer_role.cardinality.min == 1
        assert reviewer_role.cardinality.max == 3

    def test_create_relation_with_list_players(
        self, generated_package: dict[str, ModuleType]
    ) -> None:
        """Create relation instance with list of role players."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]

        # Create two memory instances
        memory1 = entities.Memory(
            name=attributes.Name("Memory 1"), content=attributes.Content("First memory content")
        )
        memory2 = entities.Memory(
            name=attributes.Name("Memory 2"), content=attributes.Content("Second memory content")
        )

        # Create similarity relation with list of players
        similarity = relations.Is_similar_to(
            similar_memory=[memory1, memory2], score=attributes.Score(0.95)
        )

        assert similarity.similar_memory == [memory1, memory2]
        # Compare by value since attribute objects have value-based equality
        assert float(similarity.score) == 0.95

    def test_symmetric_friendship_with_list(self, generated_package: dict[str, ModuleType]) -> None:
        """Friendship relation with list of two friends."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]

        alice = entities.Person(name=attributes.Name("Alice"))
        bob = entities.Person(name=attributes.Name("Bob"))

        friendship = relations.Friendship(friend=[alice, bob])

        assert len(friendship.friend) == 2
        assert alice in friendship.friend
        assert bob in friendship.friend

    def test_unbounded_role_with_multiple_players(
        self, generated_package: dict[str, ModuleType]
    ) -> None:
        """Group membership with 3+ members."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]

        group = entities.Group(name=attributes.Name("Test Group"))
        member1 = entities.Person(name=attributes.Name("Member 1"))
        member2 = entities.Person(name=attributes.Name("Member 2"))
        member3 = entities.Person(name=attributes.Name("Member 3"))

        membership = relations.Group_membership(group=group, member=[member1, member2, member3])

        assert membership.group == group
        assert len(membership.member) == 3
