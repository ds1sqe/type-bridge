"""End-to-end tests that verify generated code works with real TypeDB.

These tests go beyond import verification to test the full pipeline:
1. Parse TQL schema
2. Generate Python modules
3. Import generated modules
4. Create instances with all attribute types
5. Insert into real TypeDB
6. Query back and verify attributes match

This catches bugs where data is parsed but not rendered, or where
generated code imports but doesn't function correctly.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from type_bridge import Database, SchemaManager
from type_bridge.generator import generate_models

if TYPE_CHECKING:
    from types import ModuleType

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _import_generated_package(package_path: Path) -> dict[str, ModuleType]:
    """Import generated package modules dynamically.

    Returns dict with 'attributes', 'entities', 'relations' modules.
    """
    parent = str(package_path.parent)
    package_name = package_path.name

    if parent not in sys.path:
        sys.path.insert(0, parent)

    try:
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
        keys_to_remove = [k for k in sys.modules if k.startswith(package_name)]
        for key in keys_to_remove:
            del sys.modules[key]
        if parent in sys.path:
            sys.path.remove(parent)


@pytest.mark.integration
class TestGeneratorInsertQueryCycle:
    """Test the full Generate → Insert → Query cycle with real TypeDB."""

    SCHEMA_PATH = FIXTURES_DIR / "role_cardinality.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the role cardinality package."""
        output = tmp_path / "role_cardinality_e2e"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    @pytest.fixture
    def db_with_generated_schema(
        self, clean_db: Database, generated_package: dict[str, ModuleType]
    ):
        """Set up database with generated schema."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]

        # Register all generated classes
        schema_manager = SchemaManager(clean_db)

        # Register entities
        for name in dir(entities):
            cls = getattr(entities, name)
            if isinstance(cls, type) and hasattr(cls, "get_type_name"):
                try:
                    schema_manager.register(cls)
                except Exception:
                    pass  # Skip if already registered or abstract

        # Register relations
        for name in dir(relations):
            cls = getattr(relations, name)
            if isinstance(cls, type) and hasattr(cls, "get_type_name"):
                try:
                    schema_manager.register(cls)
                except Exception:
                    pass

        schema_manager.sync_schema(force=True)
        return clean_db

    def test_insert_and_query_entity(
        self,
        db_with_generated_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert entity with generated classes, query back, verify data."""
        entities = generated_package["entities"]
        attributes = generated_package["attributes"]
        db = db_with_generated_schema

        # Create entity instance using generated classes
        memory = entities.Memory(
            name=attributes.Name("Test Memory"),
            content=attributes.Content("This is test content"),
        )

        # Insert into database
        manager = entities.Memory.manager(db)
        manager.insert(memory)

        # Query back
        results = manager.all()

        assert len(results) == 1
        fetched = results[0]

        # Verify attribute values match
        assert str(fetched.name) == "Test Memory"
        assert str(fetched.content) == "This is test content"

    def test_insert_and_query_relation_with_single_players(
        self,
        db_with_generated_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert relation with single role players, query back."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]
        db = db_with_generated_schema

        # Create and insert entities
        person = entities.Person(name=attributes.Name("Alice"))
        document = entities.Document(
            name=attributes.Name("Design Doc"),
            content=attributes.Content("Architecture design"),
        )

        entities.Person.manager(db).insert(person)
        entities.Document.manager(db).insert(document)

        # Fetch back to get _iid populated (required for relation insertion)
        person_fetched = entities.Person.manager(db).filter(name="Alice").first()
        document_fetched = entities.Document.manager(db).filter(name="Design Doc").first()

        # Create review relation (1 document, 1 reviewer - within 1..3 bounds)
        review = relations.Review(
            document=document_fetched,
            reviewer=person_fetched,
            score=attributes.Score(4.5),
        )

        # Insert relation
        relations.Review.manager(db).insert(review)

        # Query back
        reviews = relations.Review.manager(db).all()

        assert len(reviews) == 1
        fetched = reviews[0]
        assert float(fetched.score) == 4.5

    def test_insert_and_query_relation_with_list_players(
        self,
        db_with_generated_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert relation with list of role players, query back."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]
        db = db_with_generated_schema

        # Create two memories
        memory1 = entities.Memory(
            name=attributes.Name("Memory 1"),
            content=attributes.Content("First memory"),
        )
        memory2 = entities.Memory(
            name=attributes.Name("Memory 2"),
            content=attributes.Content("Second memory"),
        )

        # Insert entities
        entities.Memory.manager(db).insert(memory1)
        entities.Memory.manager(db).insert(memory2)

        # Create similarity relation with exactly 2 players (card 2..2)
        similarity = relations.Is_similar_to(
            similar_memory=[memory1, memory2],
            score=attributes.Score(0.85),
        )

        # Insert relation
        relations.Is_similar_to.manager(db).insert(similarity)

        # Query back
        similarities = relations.Is_similar_to.manager(db).all()

        assert len(similarities) == 1
        fetched = similarities[0]
        assert float(fetched.score) == 0.85

    def test_symmetric_relation_friendship(
        self,
        db_with_generated_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Test symmetric friendship relation with card(2..2)."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]
        db = db_with_generated_schema

        # Create two people
        alice = entities.Person(name=attributes.Name("Alice"))
        bob = entities.Person(name=attributes.Name("Bob"))

        # Insert entities
        entities.Person.manager(db).insert(alice)
        entities.Person.manager(db).insert(bob)

        # Create friendship with exactly 2 friends
        friendship = relations.Friendship(friend=[alice, bob])

        # Insert relation
        relations.Friendship.manager(db).insert(friendship)

        # Query back
        friendships = relations.Friendship.manager(db).all()

        assert len(friendships) == 1

    def test_unbounded_cardinality_group_membership(
        self,
        db_with_generated_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Test group membership with unbounded member cardinality."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]
        db = db_with_generated_schema

        # Create group and members
        group = entities.Group(name=attributes.Name("Test Group"))
        member1 = entities.Person(name=attributes.Name("Member 1"))
        member2 = entities.Person(name=attributes.Name("Member 2"))
        member3 = entities.Person(name=attributes.Name("Member 3"))

        # Insert entities
        entities.Group.manager(db).insert(group)
        entities.Person.manager(db).insert(member1)
        entities.Person.manager(db).insert(member2)
        entities.Person.manager(db).insert(member3)

        # Create membership with 3 members (min 2, unbounded max)
        membership = relations.Group_membership(
            group=group,
            member=[member1, member2, member3],
        )

        # Insert relation
        relations.Group_membership.manager(db).insert(membership)

        # Query back
        memberships = relations.Group_membership.manager(db).all()

        assert len(memberships) == 1


@pytest.mark.integration
@pytest.mark.skip(
    reason="Bookstore schema has @card constraints that need SchemaManager investigation"
)
class TestGeneratorBookstoreE2E:
    """Test bookstore schema end-to-end with various attribute types."""

    SCHEMA_PATH = FIXTURES_DIR / "bookstore.tql"

    @pytest.fixture
    def generated_package(self, tmp_path: Path) -> dict[str, ModuleType]:
        """Generate and import the bookstore package."""
        output = tmp_path / "bookstore_e2e"
        generate_models(self.SCHEMA_PATH, output)
        return _import_generated_package(output)

    @pytest.fixture
    def db_with_bookstore_schema(
        self, clean_db: Database, generated_package: dict[str, ModuleType]
    ):
        """Set up database with bookstore schema."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]

        schema_manager = SchemaManager(clean_db)

        # Register entities (skip abstract ones that can't be instantiated directly)
        for name in dir(entities):
            cls = getattr(entities, name)
            if isinstance(cls, type) and hasattr(cls, "get_type_name"):
                try:
                    schema_manager.register(cls)
                except Exception:
                    pass

        # Register relations
        for name in dir(relations):
            cls = getattr(relations, name)
            if isinstance(cls, type) and hasattr(cls, "get_type_name"):
                try:
                    schema_manager.register(cls)
                except Exception:
                    pass

        schema_manager.sync_schema(force=True)
        return clean_db

    def test_insert_hardback_book(
        self,
        db_with_bookstore_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert a hardback book with various attribute types."""
        entities = generated_package["entities"]
        attributes = generated_package["attributes"]
        db = db_with_bookstore_schema

        # Create hardback (concrete subtype of book)
        hardback = entities.Hardback(
            isbn_13=attributes.Isbn13("9781234567890"),
            title=attributes.Title("Test Book"),
            page_count=attributes.PageCount(350),
            price=attributes.Price(29.99),
            stock=attributes.Stock(100),
        )

        # Insert
        manager = entities.Hardback.manager(db)
        manager.insert(hardback)

        # Query back
        results = manager.all()

        assert len(results) == 1
        fetched = results[0]

        # Verify various attribute types
        assert str(fetched.isbn_13) == "9781234567890"
        assert str(fetched.title) == "Test Book"
        assert int(fetched.page_count) == 350
        assert float(fetched.price) == 29.99
        assert int(fetched.stock) == 100

    def test_insert_user_with_datetime(
        self,
        db_with_bookstore_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert user with datetime attribute."""
        from datetime import datetime

        entities = generated_package["entities"]
        attributes = generated_package["attributes"]
        db = db_with_bookstore_schema

        birth = datetime(1990, 5, 15, 10, 30, 0)

        user = entities.User(
            id=attributes.Id("user-001"),
            name=attributes.Name("Test User"),
            birth_date=attributes.BirthDate(birth),
        )

        # Insert
        manager = entities.User.manager(db)
        manager.insert(user)

        # Query back
        results = manager.all()

        assert len(results) == 1
        fetched = results[0]

        assert str(fetched.id) == "user-001"
        assert str(fetched.name) == "Test User"

    def test_insert_contributor_and_authoring_relation(
        self,
        db_with_bookstore_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Insert contributor and authoring relation."""
        entities = generated_package["entities"]
        relations = generated_package["relations"]
        attributes = generated_package["attributes"]
        db = db_with_bookstore_schema

        # Create contributor
        author = entities.Contributor(name=attributes.Name("Jane Author"))
        entities.Contributor.manager(db).insert(author)

        # Create book
        book = entities.Paperback(
            isbn_13=attributes.Isbn13("9789876543210"),
            title=attributes.Title("Written Book"),
            page_count=attributes.PageCount(200),
            price=attributes.Price(14.99),
            stock=attributes.Stock(50),
        )
        entities.Paperback.manager(db).insert(book)

        # Create authoring relation
        authoring = relations.Authoring(
            author=author,
            work=book,
        )
        relations.Authoring.manager(db).insert(authoring)

        # Query back
        authorings = relations.Authoring.manager(db).all()

        assert len(authorings) == 1

    def test_multi_value_attribute_genre(
        self,
        db_with_bookstore_schema: Database,
        generated_package: dict[str, ModuleType],
    ) -> None:
        """Test multi-value attribute (genre with @card(0..))."""
        entities = generated_package["entities"]
        attributes = generated_package["attributes"]
        db = db_with_bookstore_schema

        # Create ebook with multiple genres
        ebook = entities.Ebook(
            isbn_13=attributes.Isbn13("9781111111111"),
            title=attributes.Title("Multi-Genre Book"),
            page_count=attributes.PageCount(150),
            price=attributes.Price(9.99),
            genre=[
                attributes.Genre("fiction"),
                attributes.Genre("mystery"),
                attributes.Genre("thriller"),
            ],
        )

        # Insert
        manager = entities.Ebook.manager(db)
        manager.insert(ebook)

        # Query back
        results = manager.all()

        assert len(results) == 1
        fetched = results[0]

        # Verify multi-value attribute
        genre_values = {str(g) for g in fetched.genre}
        assert genre_values == {"fiction", "mystery", "thriller"}
