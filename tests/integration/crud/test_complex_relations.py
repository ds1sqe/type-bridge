"""Tests for complex relation patterns beyond simple 2-role cases.

Tests self-referential relations, 3+ role relations, polymorphic role players,
multi-value roles, and relation updates.
"""

import pytest

from type_bridge import (
    Card,
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
class TestSelfReferentialRelations:
    """Test self-referential relations where same entity type plays multiple roles."""

    @pytest.fixture
    def schema_with_self_ref(self, clean_db: Database):
        """Set up schema with self-referential relation."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_self_ref")
            name: Name = Flag(Key)

        # Self-referential: person manages person
        class Management(Relation):
            flags = TypeFlags(name="management_self_ref")
            supervisor: Role[Person] = Role("supervisor", Person)
            subordinate: Role[Person] = Role("subordinate", Person)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Management)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Management, Name

    def test_insert_self_referential_relation(self, schema_with_self_ref):
        """Insert self-referential relation where same type plays both roles."""
        db, Person, Management, Name = schema_with_self_ref

        # Create two people
        manager = Person(name=Name("Alice"))
        report = Person(name=Name("Bob"))

        Person.manager(db).insert(manager)
        Person.manager(db).insert(report)

        # Create management relation
        mgmt = Management(supervisor=manager, subordinate=report)
        Management.manager(db).insert(mgmt)

        # Query back
        relations = Management.manager(db).all()
        assert len(relations) == 1

    def test_query_self_referential_relation(self, schema_with_self_ref):
        """Query self-referential relation with filters."""
        db, Person, Management, Name = schema_with_self_ref

        # Create people
        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        carol = Person(name=Name("Carol"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        Person.manager(db).insert(carol)

        # Alice manages Bob and Carol
        Management.manager(db).insert(Management(supervisor=alice, subordinate=bob))
        Management.manager(db).insert(Management(supervisor=alice, subordinate=carol))

        # Query all management relations
        relations = Management.manager(db).all()
        assert len(relations) == 2

    def test_chain_of_self_referential_relations(self, schema_with_self_ref):
        """Test chain: A manages B, B manages C."""
        db, Person, Management, Name = schema_with_self_ref

        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        carol = Person(name=Name("Carol"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        Person.manager(db).insert(carol)

        # Chain: Alice -> Bob -> Carol
        Management.manager(db).insert(Management(supervisor=alice, subordinate=bob))
        Management.manager(db).insert(Management(supervisor=bob, subordinate=carol))

        relations = Management.manager(db).all()
        assert len(relations) == 2


@pytest.mark.integration
class TestThreeRoleRelations:
    """Test relations with 3 or more roles."""

    @pytest.fixture
    def schema_with_three_roles(self, clean_db: Database):
        """Set up schema with 3-role relation."""

        class Name(String):
            pass

        class Amount(Integer):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_three_role")
            name: Name = Flag(Key)

        class Product(Entity):
            flags = TypeFlags(name="product_three_role")
            name: Name = Flag(Key)

        class Location(Entity):
            flags = TypeFlags(name="location_three_role")
            name: Name = Flag(Key)

        # 3-role relation: person buys product at location
        class Purchase(Relation):
            flags = TypeFlags(name="purchase_three_role")
            buyer: Role[Person] = Role("buyer", Person)
            item: Role[Product] = Role("item", Product)
            store: Role[Location] = Role("store", Location)
            amount: Amount

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Product, Location, Purchase)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Product, Location, Purchase, Name, Amount

    def test_insert_three_role_relation(self, schema_with_three_roles):
        """Insert relation with 3 distinct roles."""
        db, Person, Product, Location, Purchase, Name, Amount = schema_with_three_roles

        # Create entities
        buyer = Person(name=Name("Alice"))
        product = Product(name=Name("Laptop"))
        store = Location(name=Name("TechStore"))

        Person.manager(db).insert(buyer)
        Product.manager(db).insert(product)
        Location.manager(db).insert(store)

        # Create purchase
        purchase = Purchase(buyer=buyer, item=product, store=store, amount=Amount(1))
        Purchase.manager(db).insert(purchase)

        # Query back
        purchases = Purchase.manager(db).all()
        assert len(purchases) == 1
        assert int(purchases[0].amount) == 1

    def test_multiple_three_role_relations(self, schema_with_three_roles):
        """Multiple 3-role relations with shared entities."""
        db, Person, Product, Location, Purchase, Name, Amount = schema_with_three_roles

        # Create entities
        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        laptop = Product(name=Name("Laptop"))
        phone = Product(name=Name("Phone"))
        store = Location(name=Name("TechStore"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        Product.manager(db).insert(laptop)
        Product.manager(db).insert(phone)
        Location.manager(db).insert(store)

        # Multiple purchases at same store
        Purchase.manager(db).insert(
            Purchase(buyer=alice, item=laptop, store=store, amount=Amount(1))
        )
        Purchase.manager(db).insert(Purchase(buyer=bob, item=phone, store=store, amount=Amount(2)))
        Purchase.manager(db).insert(
            Purchase(buyer=alice, item=phone, store=store, amount=Amount(1))
        )

        purchases = Purchase.manager(db).all()
        assert len(purchases) == 3


@pytest.mark.integration
class TestPolymorphicRolePlayers:
    """Test relations where role accepts abstract type with concrete subtypes."""

    @pytest.fixture
    def schema_with_polymorphic(self, clean_db: Database):
        """Set up schema with polymorphic role players."""

        class Name(String):
            pass

        class Content(String):
            pass

        # Abstract base
        class Document(Entity):
            flags = TypeFlags(name="document_poly", abstract=True)
            name: Name = Flag(Key)

        # Concrete subtypes
        class Report(Document):
            flags = TypeFlags(name="report_poly")
            content: Content

        class Memo(Document):
            flags = TypeFlags(name="memo_poly")
            content: Content

        class Person(Entity):
            flags = TypeFlags(name="person_poly")
            name: Name = Flag(Key)

        # Relation with polymorphic role player (Document can be Report or Memo)
        class Authorship(Relation):
            flags = TypeFlags(name="authorship_poly")
            author: Role[Person] = Role("author", Person)
            document: Role[Document] = Role("document", Document)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Document, Report, Memo, Person, Authorship)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Document, Report, Memo, Authorship, Name, Content

    def test_insert_with_concrete_subtype_report(self, schema_with_polymorphic):
        """Insert relation with Report as polymorphic role player."""
        db, Person, Document, Report, Memo, Authorship, Name, Content = schema_with_polymorphic

        author = Person(name=Name("Alice"))
        report = Report(name=Name("Q4 Report"), content=Content("Financial data"))

        Person.manager(db).insert(author)
        Report.manager(db).insert(report)

        authorship = Authorship(author=author, document=report)
        Authorship.manager(db).insert(authorship)

        relations = Authorship.manager(db).all()
        assert len(relations) == 1

    def test_insert_with_concrete_subtype_memo(self, schema_with_polymorphic):
        """Insert relation with Memo as polymorphic role player."""
        db, Person, Document, Report, Memo, Authorship, Name, Content = schema_with_polymorphic

        author = Person(name=Name("Bob"))
        memo = Memo(name=Name("Meeting Notes"), content=Content("Discussion points"))

        Person.manager(db).insert(author)
        Memo.manager(db).insert(memo)

        authorship = Authorship(author=author, document=memo)
        Authorship.manager(db).insert(authorship)

        relations = Authorship.manager(db).all()
        assert len(relations) == 1

    def test_mixed_subtypes_in_relations(self, schema_with_polymorphic):
        """Multiple relations with different concrete subtypes."""
        db, Person, Document, Report, Memo, Authorship, Name, Content = schema_with_polymorphic

        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        report = Report(name=Name("Annual Report"), content=Content("Annual summary"))
        memo = Memo(name=Name("Quick Note"), content=Content("Reminder"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        Report.manager(db).insert(report)
        Memo.manager(db).insert(memo)

        # Alice authors Report, Bob authors Memo
        Authorship.manager(db).insert(Authorship(author=alice, document=report))
        Authorship.manager(db).insert(Authorship(author=bob, document=memo))

        relations = Authorship.manager(db).all()
        assert len(relations) == 2


@pytest.mark.integration
class TestMultiValueRoles:
    """Test relations with roles that allow multiple players."""

    @pytest.fixture
    def schema_with_multi_role(self, clean_db: Database):
        """Set up schema with multi-value role."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_multi_role")
            name: Name = Flag(Key)

        class Project(Entity):
            flags = TypeFlags(name="project_multi_role")
            name: Name = Flag(Key)

        # Relation with multi-value role: multiple members per team
        class Team(Relation):
            flags = TypeFlags(name="team_multi_role")
            project: Role[Project] = Role("project", Project)
            member: Role[Person] = Role("member", Person, cardinality=Card(2))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Project, Team)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Project, Team, Name

    def test_insert_with_list_of_role_players(self, schema_with_multi_role):
        """Insert relation with list of players for multi-value role."""
        db, Person, Project, Team, Name = schema_with_multi_role

        project = Project(name=Name("Alpha"))
        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))

        Project.manager(db).insert(project)
        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)

        # Create team with multiple members
        team = Team(project=project, member=[alice, bob])
        Team.manager(db).insert(team)

        teams = Team.manager(db).all()
        assert len(teams) == 1

    def test_insert_with_three_players(self, schema_with_multi_role):
        """Insert relation with 3+ players for unbounded role."""
        db, Person, Project, Team, Name = schema_with_multi_role

        project = Project(name=Name("Beta"))
        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        carol = Person(name=Name("Carol"))

        Project.manager(db).insert(project)
        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        Person.manager(db).insert(carol)

        team = Team(project=project, member=[alice, bob, carol])
        Team.manager(db).insert(team)

        teams = Team.manager(db).all()
        assert len(teams) == 1


@pytest.mark.integration
class TestRelationWithAttributes:
    """Test relations that own attributes."""

    @pytest.fixture
    def schema_with_relation_attrs(self, clean_db: Database):
        """Set up schema with relation attributes."""

        class Name(String):
            pass

        class Rating(Integer):
            pass

        class Comment(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_rel_attr")
            name: Name = Flag(Key)

        class Product(Entity):
            flags = TypeFlags(name="product_rel_attr")
            name: Name = Flag(Key)

        class Review(Relation):
            flags = TypeFlags(name="review_rel_attr")
            reviewer: Role[Person] = Role("reviewer", Person)
            product: Role[Product] = Role("product", Product)
            rating: Rating
            comment: Comment | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Product, Review)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Product, Review, Name, Rating, Comment

    def test_insert_relation_with_required_attr(self, schema_with_relation_attrs):
        """Insert relation with required attribute."""
        db, Person, Product, Review, Name, Rating, Comment = schema_with_relation_attrs

        reviewer = Person(name=Name("Alice"))
        product = Product(name=Name("Widget"))

        Person.manager(db).insert(reviewer)
        Product.manager(db).insert(product)

        review = Review(reviewer=reviewer, product=product, rating=Rating(5))
        Review.manager(db).insert(review)

        reviews = Review.manager(db).all()
        assert len(reviews) == 1
        assert int(reviews[0].rating) == 5

    def test_insert_relation_with_optional_attr(self, schema_with_relation_attrs):
        """Insert relation with optional attribute."""
        db, Person, Product, Review, Name, Rating, Comment = schema_with_relation_attrs

        reviewer = Person(name=Name("Bob"))
        product = Product(name=Name("Gadget"))

        Person.manager(db).insert(reviewer)
        Product.manager(db).insert(product)

        review = Review(
            reviewer=reviewer,
            product=product,
            rating=Rating(4),
            comment=Comment("Great product!"),
        )
        Review.manager(db).insert(review)

        reviews = Review.manager(db).all()
        assert len(reviews) == 1
        assert str(reviews[0].comment) == "Great product!"

    def test_update_relation_attribute(self, schema_with_relation_attrs):
        """Update attribute on existing relation."""
        db, Person, Product, Review, Name, Rating, Comment = schema_with_relation_attrs

        reviewer = Person(name=Name("Carol"))
        product = Product(name=Name("Thing"))

        Person.manager(db).insert(reviewer)
        Product.manager(db).insert(product)

        review = Review(reviewer=reviewer, product=product, rating=Rating(3))
        Review.manager(db).insert(review)

        # Update rating
        fetched = Review.manager(db).all()[0]
        fetched.rating = Rating(5)
        Review.manager(db).update(fetched)

        # Verify
        updated = Review.manager(db).all()[0]
        assert int(updated.rating) == 5
