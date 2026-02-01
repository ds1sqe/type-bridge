"""Tests for Bookstore query patterns.

Tests abstract attribute inheritance, ternary relations, relations with owned
attributes, value constraints, range constraints, and unique constraints.
Based on TypeDB Bookstore example schema.
"""

import pytest

from type_bridge import (
    AttributeFlags,
    Database,
    DateTime,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
    Unique,
)

# =============================================================================
# Test: Abstract Attribute Inheritance
# =============================================================================


@pytest.mark.integration
class TestAbstractAttributeInheritance:
    """Test abstract attribute types with concrete subtypes."""

    @pytest.fixture
    def schema_with_abstract_attrs(self, clean_db: Database):
        """Set up schema with abstract attribute inheritance.

        Models ISBN hierarchy: isbn (abstract) -> isbn-13, isbn-10
        """

        # Abstract attribute (Python base class, not in TypeDB directly)
        # In TypeBridge, we use separate attribute classes with different names
        class Isbn13(String):
            """ISBN-13 identifier."""

            flags = AttributeFlags(name="isbn_13_book")

        class Isbn10(String):
            """ISBN-10 identifier."""

            flags = AttributeFlags(name="isbn_10_book")

        class BookTitle(String):
            pass

        class PageCount(Integer):
            pass

        # Abstract book with both ISBN types
        class Book(Entity):
            flags = TypeFlags(name="book_abstract", abstract=True)
            # Can have 0-2 isbn values (multiple formats)
            isbn_13: Isbn13 = Flag(Key)
            isbn_10: Isbn10 | None = Flag(Unique)  # Unique but optional
            title: BookTitle
            page_count: PageCount | None = None

        class Hardback(Book):
            flags = TypeFlags(name="hardback_book")

        class Paperback(Book):
            flags = TypeFlags(name="paperback_book")

        class Ebook(Book):
            flags = TypeFlags(name="ebook_book")

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Book, Hardback, Paperback, Ebook)
        schema_manager.sync_schema(force=True)

        return clean_db, Book, Hardback, Paperback, Ebook, Isbn13, Isbn10, BookTitle, PageCount

    def test_insert_book_with_both_isbn_types(self, schema_with_abstract_attrs):
        """Insert book with both ISBN-13 and ISBN-10."""
        db, Book, Hardback, Paperback, Ebook, Isbn13, Isbn10, BookTitle, PageCount = (
            schema_with_abstract_attrs
        )

        book = Hardback(
            isbn_13=Isbn13("978-0-13-468599-1"),
            isbn_10=Isbn10("0-13-468599-1"),
            title=BookTitle("The Pragmatic Programmer"),
            page_count=PageCount(352),
        )
        Hardback.manager(db).insert(book)

        result = Hardback.manager(db).get(isbn_13="978-0-13-468599-1")
        assert len(result) == 1
        assert str(result[0].isbn_10) == "0-13-468599-1"
        assert str(result[0].title) == "The Pragmatic Programmer"

    def test_insert_book_without_optional_isbn10(self, schema_with_abstract_attrs):
        """Insert book with only ISBN-13 (ISBN-10 is optional)."""
        db, Book, Hardback, Paperback, Ebook, Isbn13, Isbn10, BookTitle, PageCount = (
            schema_with_abstract_attrs
        )

        book = Ebook(
            isbn_13=Isbn13("978-1-234-56789-0"),
            title=BookTitle("Digital Only Book"),
        )
        Ebook.manager(db).insert(book)

        result = Ebook.manager(db).get(isbn_13="978-1-234-56789-0")
        assert len(result) == 1
        assert result[0].isbn_10 is None

    def test_query_different_book_formats(self, schema_with_abstract_attrs):
        """Query books by format (subtype)."""
        db, Book, Hardback, Paperback, Ebook, Isbn13, Isbn10, BookTitle, PageCount = (
            schema_with_abstract_attrs
        )

        # Insert different formats
        Hardback.manager(db).insert(
            Hardback(
                isbn_13=Isbn13("978-1-111-11111-1"),
                title=BookTitle("Hardback Edition"),
                page_count=PageCount(500),
            )
        )
        Paperback.manager(db).insert(
            Paperback(
                isbn_13=Isbn13("978-2-222-22222-2"),
                title=BookTitle("Paperback Edition"),
                page_count=PageCount(450),
            )
        )
        Ebook.manager(db).insert(
            Ebook(
                isbn_13=Isbn13("978-3-333-33333-3"),
                title=BookTitle("Digital Edition"),
            )
        )

        # Query each type
        hardbacks = Hardback.manager(db).all()
        paperbacks = Paperback.manager(db).all()
        ebooks = Ebook.manager(db).all()

        assert len(hardbacks) == 1
        assert len(paperbacks) == 1
        assert len(ebooks) == 1


# =============================================================================
# Test: Ternary Relations (3+ roles)
# =============================================================================


@pytest.mark.integration
class TestTernaryRelations:
    """Test relations with three or more roles."""

    @pytest.fixture
    def schema_with_ternary_relation(self, clean_db: Database):
        """Set up schema with ternary publishing relation.

        publishing: publisher + published (book) + publication (year/edition)
        """

        class CompanyName(String):
            pass

        class BookIsbn(String):
            pass

        class BookTitle(String):
            pass

        class PublicationYear(Integer):
            pass

        # Entities
        class Publisher(Entity):
            flags = TypeFlags(name="publisher_ternary")
            name: CompanyName = Flag(Key)

        class TernaryBook(Entity):
            flags = TypeFlags(name="book_ternary")
            isbn: BookIsbn = Flag(Key)
            title: BookTitle

        class Publication(Entity):
            flags = TypeFlags(name="publication_ternary")
            year: PublicationYear = Flag(Key)

        # Ternary relation - connects all three
        class Publishing(Relation):
            flags = TypeFlags(name="publishing_ternary")
            publisher: Role[Publisher] = Role("publisher", Publisher)
            published: Role[TernaryBook] = Role("published", TernaryBook)
            publication: Role[Publication] = Role("publication", Publication)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Publisher, TernaryBook, Publication, Publishing)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Publisher,
            TernaryBook,
            Publication,
            Publishing,
            CompanyName,
            BookIsbn,
            BookTitle,
            PublicationYear,
        )

    def test_insert_ternary_relation(self, schema_with_ternary_relation):
        """Insert a ternary relation connecting three entities."""
        (
            db,
            Publisher,
            TernaryBook,
            Publication,
            Publishing,
            CompanyName,
            BookIsbn,
            BookTitle,
            PublicationYear,
        ) = schema_with_ternary_relation

        # Create entities
        publisher = Publisher(name=CompanyName("O'Reilly Media"))
        book = TernaryBook(isbn=BookIsbn("978-1-492-03400-1"), title=BookTitle("Fluent Python"))
        pub_info = Publication(year=PublicationYear(2022))

        Publisher.manager(db).insert(publisher)
        TernaryBook.manager(db).insert(book)
        Publication.manager(db).insert(pub_info)

        # Fetch with IIDs
        pub_fetched = Publisher.manager(db).get(name="O'Reilly Media")[0]
        book_fetched = TernaryBook.manager(db).get(isbn="978-1-492-03400-1")[0]
        year_fetched = Publication.manager(db).get(year=2022)[0]

        # Create ternary relation
        publishing = Publishing(
            publisher=pub_fetched,
            published=book_fetched,
            publication=year_fetched,
        )
        Publishing.manager(db).insert(publishing)

        # Query and verify
        relations = Publishing.manager(db).all()
        assert len(relations) == 1
        assert str(relations[0].publisher.name) == "O'Reilly Media"
        assert str(relations[0].published.title) == "Fluent Python"
        assert int(relations[0].publication.year) == 2022

    def test_query_ternary_by_one_role(self, schema_with_ternary_relation):
        """Query ternary relations filtering by one role player."""
        (
            db,
            Publisher,
            TernaryBook,
            Publication,
            Publishing,
            CompanyName,
            BookIsbn,
            BookTitle,
            PublicationYear,
        ) = schema_with_ternary_relation

        # Create two publishers with books
        for pub_name, isbn, title, year in [
            ("Penguin", "978-0-14-028329-7", "1984", 1949),
            ("Penguin", "978-0-14-118776-1", "Animal Farm", 1945),
            ("HarperCollins", "978-0-06-093546-7", "To Kill a Mockingbird", 1960),
        ]:
            pub = Publisher(name=CompanyName(pub_name))
            book = TernaryBook(isbn=BookIsbn(isbn), title=BookTitle(title))
            pub_info = Publication(year=PublicationYear(year))

            # Insert if not exists (by key)
            existing = Publisher.manager(db).get(name=pub_name)
            if not existing:
                Publisher.manager(db).insert(pub)

            TernaryBook.manager(db).insert(book)
            Publication.manager(db).insert(pub_info)

        # Create publishing relations
        for pub_name, isbn, year in [
            ("Penguin", "978-0-14-028329-7", 1949),
            ("Penguin", "978-0-14-118776-1", 1945),
            ("HarperCollins", "978-0-06-093546-7", 1960),
        ]:
            pub = Publisher.manager(db).get(name=pub_name)[0]
            book = TernaryBook.manager(db).get(isbn=isbn)[0]
            pub_info = Publication.manager(db).get(year=year)[0]

            Publishing.manager(db).insert(
                Publishing(publisher=pub, published=book, publication=pub_info)
            )

        # Query all publishing relations
        all_relations = Publishing.manager(db).all()
        assert len(all_relations) == 3


# =============================================================================
# Test: Relations with Owned Attributes
# =============================================================================


@pytest.mark.integration
class TestRelationsWithAttributes:
    """Test relations that own their own attributes."""

    @pytest.fixture
    def schema_with_relation_attrs(self, clean_db: Database):
        """Set up schema where relations own attributes.

        promotion-inclusion: relates promotion and item, owns discount
        order-line: relates order and item, owns quantity
        """

        class PromoCode(String):
            pass

        class PromoName(String):
            pass

        class ProductName(String):
            pass

        class ProductPrice(Double):
            pass

        class Discount(Double):
            """Discount percentage for promotion."""

            pass

        class Quantity(Integer):
            """Quantity ordered."""

            pass

        class OrderId(String):
            pass

        # Entities
        class Promotion(Entity):
            flags = TypeFlags(name="promotion_rel_attr")
            code: PromoCode = Flag(Key)
            name: PromoName

        class Product(Entity):
            flags = TypeFlags(name="product_rel_attr")
            name: ProductName = Flag(Key)
            price: ProductPrice

        class Order(Entity):
            flags = TypeFlags(name="order_rel_attr")
            order_id: OrderId = Flag(Key)

        # Relation with owned attribute (discount)
        class PromotionInclusion(Relation):
            flags = TypeFlags(name="promotion_inclusion_rel_attr")
            promotion: Role[Promotion] = Role("promotion", Promotion)
            item: Role[Product] = Role("item", Product)
            discount: Discount  # Relation owns this attribute!

        # Relation with owned attribute (quantity)
        class OrderLine(Relation):
            flags = TypeFlags(name="order_line_rel_attr")
            order: Role[Order] = Role("order", Order)
            item: Role[Product] = Role("item", Product)
            quantity: Quantity  # Relation owns this attribute!

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Promotion, Product, Order, PromotionInclusion, OrderLine)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Promotion,
            Product,
            Order,
            PromotionInclusion,
            OrderLine,
            PromoCode,
            PromoName,
            ProductName,
            ProductPrice,
            Discount,
            Quantity,
            OrderId,
        )

    def test_insert_relation_with_attribute(self, schema_with_relation_attrs):
        """Insert relation that owns its own attribute."""
        (
            db,
            Promotion,
            Product,
            Order,
            PromotionInclusion,
            OrderLine,
            PromoCode,
            PromoName,
            ProductName,
            ProductPrice,
            Discount,
            Quantity,
            OrderId,
        ) = schema_with_relation_attrs

        # Create entities
        promo = Promotion(code=PromoCode("SUMMER20"), name=PromoName("Summer Sale"))
        product = Product(name=ProductName("TypeDB Book"), price=ProductPrice(49.99))

        Promotion.manager(db).insert(promo)
        Product.manager(db).insert(product)

        # Fetch with IIDs
        promo_fetched = Promotion.manager(db).get(code="SUMMER20")[0]
        product_fetched = Product.manager(db).get(name="TypeDB Book")[0]

        # Create relation with its own attribute
        inclusion = PromotionInclusion(
            promotion=promo_fetched,
            item=product_fetched,
            discount=Discount(0.20),  # 20% off
        )
        PromotionInclusion.manager(db).insert(inclusion)

        # Query and verify
        inclusions = PromotionInclusion.manager(db).all()
        assert len(inclusions) == 1
        assert float(inclusions[0].discount) == 0.20

    def test_filter_relation_by_owned_attribute(self, schema_with_relation_attrs):
        """Filter relations by their owned attributes."""
        (
            db,
            Promotion,
            Product,
            Order,
            PromotionInclusion,
            OrderLine,
            PromoCode,
            PromoName,
            ProductName,
            ProductPrice,
            Discount,
            Quantity,
            OrderId,
        ) = schema_with_relation_attrs

        # Create order and products
        order = Order(order_id=OrderId("ORD-001"))
        product1 = Product(name=ProductName("Widget A"), price=ProductPrice(10.0))
        product2 = Product(name=ProductName("Widget B"), price=ProductPrice(25.0))

        Order.manager(db).insert(order)
        Product.manager(db).insert(product1)
        Product.manager(db).insert(product2)

        # Fetch
        order_f = Order.manager(db).get(order_id="ORD-001")[0]
        p1_f = Product.manager(db).get(name="Widget A")[0]
        p2_f = Product.manager(db).get(name="Widget B")[0]

        # Create order lines with different quantities
        OrderLine.manager(db).insert(OrderLine(order=order_f, item=p1_f, quantity=Quantity(5)))
        OrderLine.manager(db).insert(OrderLine(order=order_f, item=p2_f, quantity=Quantity(2)))

        # Query all order lines
        lines = OrderLine.manager(db).all()
        assert len(lines) == 2

        # Check quantities
        quantities = sorted([int(line.quantity) for line in lines])
        assert quantities == [2, 5]


# =============================================================================
# Test: Multiple Entity Plays Same Role in Different Relations
# =============================================================================


@pytest.mark.integration
class TestEntityMultipleRelations:
    """Test entities that play roles in multiple different relations."""

    @pytest.fixture
    def schema_with_contributor_roles(self, clean_db: Database):
        """Set up schema where contributor plays different roles.

        contributor can be: author, editor, illustrator (in different relations)
        """

        class PersonName(String):
            pass

        class WorkTitle(String):
            pass

        class WorkIsbn(String):
            pass

        class Contributor(Entity):
            flags = TypeFlags(name="contributor_multi")
            name: PersonName = Flag(Key)

        class Work(Entity):
            flags = TypeFlags(name="work_multi")
            isbn: WorkIsbn = Flag(Key)
            title: WorkTitle

        # Base contribution relation
        class Contribution(Relation):
            flags = TypeFlags(name="contribution_multi", abstract=True)
            contributor: Role[Contributor] = Role("contributor", Contributor)
            work: Role[Work] = Role("work", Work)

        # Specialized contributions
        class Authoring(Relation):
            flags = TypeFlags(name="authoring_multi")
            author: Role[Contributor] = Role("author", Contributor)
            work: Role[Work] = Role("work", Work)

        class Editing(Relation):
            flags = TypeFlags(name="editing_multi")
            editor: Role[Contributor] = Role("editor", Contributor)
            work: Role[Work] = Role("work", Work)

        class Illustrating(Relation):
            flags = TypeFlags(name="illustrating_multi")
            illustrator: Role[Contributor] = Role("illustrator", Contributor)
            work: Role[Work] = Role("work", Work)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Contributor, Work, Authoring, Editing, Illustrating)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Contributor,
            Work,
            Authoring,
            Editing,
            Illustrating,
            PersonName,
            WorkTitle,
            WorkIsbn,
        )

    def test_same_person_different_roles(self, schema_with_contributor_roles):
        """Same person can author one book and edit another."""
        (
            db,
            Contributor,
            Work,
            Authoring,
            Editing,
            Illustrating,
            PersonName,
            WorkTitle,
            WorkIsbn,
        ) = schema_with_contributor_roles

        # Create contributor and works
        alice = Contributor(name=PersonName("Alice Writer"))
        book1 = Work(isbn=WorkIsbn("111-1-111-11111-1"), title=WorkTitle("Alice's Novel"))
        book2 = Work(isbn=WorkIsbn("222-2-222-22222-2"), title=WorkTitle("Edited Anthology"))

        Contributor.manager(db).insert(alice)
        Work.manager(db).insert(book1)
        Work.manager(db).insert(book2)

        # Fetch
        alice_f = Contributor.manager(db).get(name="Alice Writer")[0]
        book1_f = Work.manager(db).get(isbn="111-1-111-11111-1")[0]
        book2_f = Work.manager(db).get(isbn="222-2-222-22222-2")[0]

        # Alice authors book1
        Authoring.manager(db).insert(Authoring(author=alice_f, work=book1_f))

        # Alice edits book2
        Editing.manager(db).insert(Editing(editor=alice_f, work=book2_f))

        # Verify
        authorings = Authoring.manager(db).all()
        editings = Editing.manager(db).all()

        assert len(authorings) == 1
        assert len(editings) == 1
        assert str(authorings[0].author.name) == "Alice Writer"
        assert str(editings[0].editor.name) == "Alice Writer"

    def test_same_work_multiple_contributors(self, schema_with_contributor_roles):
        """Same work can have author, editor, and illustrator."""
        (
            db,
            Contributor,
            Work,
            Authoring,
            Editing,
            Illustrating,
            PersonName,
            WorkTitle,
            WorkIsbn,
        ) = schema_with_contributor_roles

        # Create contributors
        for name in ["Author Person", "Editor Person", "Artist Person"]:
            Contributor.manager(db).insert(Contributor(name=PersonName(name)))

        # Create work
        book = Work(isbn=WorkIsbn("333-3-333-33333-3"), title=WorkTitle("Collaborative Work"))
        Work.manager(db).insert(book)

        # Fetch all
        author = Contributor.manager(db).get(name="Author Person")[0]
        editor = Contributor.manager(db).get(name="Editor Person")[0]
        artist = Contributor.manager(db).get(name="Artist Person")[0]
        book_f = Work.manager(db).get(isbn="333-3-333-33333-3")[0]

        # Create all contribution types for same work
        Authoring.manager(db).insert(Authoring(author=author, work=book_f))
        Editing.manager(db).insert(Editing(editor=editor, work=book_f))
        Illustrating.manager(db).insert(Illustrating(illustrator=artist, work=book_f))

        # Verify each relation type
        assert len(Authoring.manager(db).all()) == 1
        assert len(Editing.manager(db).all()) == 1
        assert len(Illustrating.manager(db).all()) == 1


# =============================================================================
# Test: Hierarchical Entity Types (Place hierarchy)
# =============================================================================


@pytest.mark.integration
class TestHierarchicalEntities:
    """Test hierarchical entity type patterns (city -> state -> country)."""

    @pytest.fixture
    def schema_with_place_hierarchy(self, clean_db: Database):
        """Set up schema with place hierarchy."""

        class PlaceName(String):
            pass

        # Abstract place with concrete subtypes
        class Place(Entity):
            flags = TypeFlags(name="place_hier", abstract=True)
            name: PlaceName = Flag(Key)

        class City(Place):
            flags = TypeFlags(name="city_hier")

        class State(Place):
            flags = TypeFlags(name="state_hier")

        class Country(Place):
            flags = TypeFlags(name="country_hier")

        # Locating relation (place located in another place)
        class Locating(Relation):
            flags = TypeFlags(name="locating_hier")
            located: Role[Place] = Role("located", Place)
            location: Role[Place] = Role("location", Place)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Place, City, State, Country, Locating)
        schema_manager.sync_schema(force=True)

        return clean_db, Place, City, State, Country, Locating, PlaceName

    def test_build_place_hierarchy(self, schema_with_place_hierarchy):
        """Build hierarchical location chain: city -> state -> country."""
        db, Place, City, State, Country, Locating, PlaceName = schema_with_place_hierarchy

        # Create places
        city = City(name=PlaceName("San Francisco"))
        state = State(name=PlaceName("California"))
        country = Country(name=PlaceName("United States"))

        City.manager(db).insert(city)
        State.manager(db).insert(state)
        Country.manager(db).insert(country)

        # Fetch
        city_f = City.manager(db).get(name="San Francisco")[0]
        state_f = State.manager(db).get(name="California")[0]
        country_f = Country.manager(db).get(name="United States")[0]

        # Create hierarchy
        Locating.manager(db).insert(Locating(located=city_f, location=state_f))
        Locating.manager(db).insert(Locating(located=state_f, location=country_f))

        # Verify
        locations = Locating.manager(db).all()
        assert len(locations) == 2

    def test_query_polymorphic_place_role(self, schema_with_place_hierarchy):
        """Query relations where role player is polymorphic (any Place subtype)."""
        db, Place, City, State, Country, Locating, PlaceName = schema_with_place_hierarchy

        # Create multiple cities in same state
        for city_name in ["Los Angeles", "San Diego", "Sacramento"]:
            City.manager(db).insert(City(name=PlaceName(city_name)))

        state = State(name=PlaceName("California"))
        State.manager(db).insert(state)

        state_f = State.manager(db).get(name="California")[0]

        # Link all cities to state
        for city_name in ["Los Angeles", "San Diego", "Sacramento"]:
            city = City.manager(db).get(name=city_name)[0]
            Locating.manager(db).insert(Locating(located=city, location=state_f))

        # Query all locations
        locations = Locating.manager(db).all()
        assert len(locations) == 3

        # All located entities should be City instances (polymorphic resolution)
        for loc in locations:
            assert isinstance(loc.located, City)


# =============================================================================
# Test: Advanced Aggregations on Relations
# =============================================================================


@pytest.mark.integration
class TestRelationAggregations:
    """Test aggregations on relations - count, group_by with role players."""

    @pytest.fixture
    def schema_with_sales_data(self, clean_db: Database):
        """Set up schema with books, authors, and sales data for aggregations."""

        class BookTitle(String):
            pass

        class AuthorName(String):
            pass

        class Price(Double):
            pass

        class Quantity(Integer):
            pass

        class SaleDate(DateTime):
            pass

        class Book(Entity):
            flags = TypeFlags(name="book_agg")
            title: BookTitle = Flag(Key)
            price: Price

        class Author(Entity):
            flags = TypeFlags(name="author_agg")
            name: AuthorName = Flag(Key)

        class Authorship(Relation):
            flags = TypeFlags(name="authorship_agg")
            book: Role[Book] = Role("book", Book)
            author: Role[Author] = Role("author", Author)

        class Sale(Relation):
            flags = TypeFlags(name="sale_agg")
            book: Role[Book] = Role("book", Book)
            quantity: Quantity
            sale_date: SaleDate

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Book, Author, Authorship, Sale)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Book,
            Author,
            Authorship,
            Sale,
            BookTitle,
            AuthorName,
            Price,
            Quantity,
            SaleDate,
        )

    def test_count_relations(self, schema_with_sales_data):
        """Count total number of relations."""
        (
            db,
            Book,
            Author,
            Authorship,
            Sale,
            BookTitle,
            AuthorName,
            Price,
            Quantity,
            SaleDate,
        ) = schema_with_sales_data

        # Create authors
        for name in ["Alice Writer", "Bob Author", "Carol Scribe"]:
            Author.manager(db).insert(Author(name=AuthorName(name)))

        # Create books
        for title, price in [
            ("Python Basics", 29.99),
            ("Advanced Python", 49.99),
            ("Data Science", 39.99),
        ]:
            Book.manager(db).insert(Book(title=BookTitle(title), price=Price(price)))

        alice = Author.manager(db).get(name="Alice Writer")[0]
        bob = Author.manager(db).get(name="Bob Author")[0]

        python_basics = Book.manager(db).get(title="Python Basics")[0]
        advanced_python = Book.manager(db).get(title="Advanced Python")[0]
        data_science = Book.manager(db).get(title="Data Science")[0]

        # Create authorships (Alice wrote 2 books, Bob wrote 1)
        Authorship.manager(db).insert(Authorship(book=python_basics, author=alice))
        Authorship.manager(db).insert(Authorship(book=advanced_python, author=alice))
        Authorship.manager(db).insert(Authorship(book=data_science, author=bob))

        # Count all authorships
        count = Authorship.manager(db).filter().count()
        assert count == 3

    def test_count_relations_with_filter(self, schema_with_sales_data):
        """Count relations filtered by role player."""
        (
            db,
            Book,
            Author,
            Authorship,
            Sale,
            BookTitle,
            AuthorName,
            Price,
            Quantity,
            SaleDate,
        ) = schema_with_sales_data

        # Create test data
        Author.manager(db).insert(Author(name=AuthorName("Prolific Writer")))
        Author.manager(db).insert(Author(name=AuthorName("Occasional Author")))

        prolific = Author.manager(db).get(name="Prolific Writer")[0]
        occasional = Author.manager(db).get(name="Occasional Author")[0]

        # Create books and authorships
        for i in range(5):
            book = Book(title=BookTitle(f"Book {i}"), price=Price(10.0 + i))
            Book.manager(db).insert(book)
            book_f = Book.manager(db).get(title=f"Book {i}")[0]
            Authorship.manager(db).insert(Authorship(book=book_f, author=prolific))

        Book.manager(db).insert(Book(title=BookTitle("Single Book"), price=Price(15.0)))
        single = Book.manager(db).get(title="Single Book")[0]
        Authorship.manager(db).insert(Authorship(book=single, author=occasional))

        # Count authorships filtered by author
        prolific_count = Authorship.manager(db).filter(author=prolific).count()
        occasional_count = Authorship.manager(db).filter(author=occasional).count()

        assert prolific_count == 5
        assert occasional_count == 1

    def test_relation_query_with_limit_offset(self, schema_with_sales_data):
        """Test pagination on relation queries."""
        (
            db,
            Book,
            Author,
            Authorship,
            Sale,
            BookTitle,
            AuthorName,
            Price,
            Quantity,
            SaleDate,
        ) = schema_with_sales_data

        # Create author and books
        Author.manager(db).insert(Author(name=AuthorName("Test Author")))
        author = Author.manager(db).get(name="Test Author")[0]

        for i in range(10):
            Book.manager(db).insert(Book(title=BookTitle(f"Book {i:02d}"), price=Price(10.0 + i)))
            book = Book.manager(db).get(title=f"Book {i:02d}")[0]
            Authorship.manager(db).insert(Authorship(book=book, author=author))

        # Test pagination
        page1 = Authorship.manager(db).filter().limit(3).execute()
        assert len(page1) == 3

        page2 = Authorship.manager(db).filter().offset(3).limit(3).execute()
        assert len(page2) == 3

        # Verify different results (by checking book titles don't overlap)
        page1_titles = {a.book.title.value for a in page1}
        page2_titles = {a.book.title.value for a in page2}
        assert page1_titles.isdisjoint(page2_titles)


# =============================================================================
# Test: Ordering Relations by Role Player Attributes
# =============================================================================


@pytest.mark.integration
class TestRelationOrdering:
    """Test ordering relations by their own attributes and role player attributes."""

    @pytest.fixture
    def schema_with_ratings(self, clean_db: Database):
        """Schema with book ratings for ordering tests."""

        class BookTitle(String):
            pass

        class Rating(Double):
            pass

        class ReviewerName(String):
            pass

        class Book(Entity):
            flags = TypeFlags(name="book_ord")
            title: BookTitle = Flag(Key)

        class Reviewer(Entity):
            flags = TypeFlags(name="reviewer_ord")
            name: ReviewerName = Flag(Key)

        class Review(Relation):
            flags = TypeFlags(name="review_ord")
            book: Role[Book] = Role("book", Book)
            reviewer: Role[Reviewer] = Role("reviewer", Reviewer)
            rating: Rating

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Book, Reviewer, Review)
        schema_manager.sync_schema(force=True)

        return clean_db, Book, Reviewer, Review, BookTitle, ReviewerName, Rating

    def test_order_by_relation_attribute(self, schema_with_ratings):
        """Order relations by their owned attribute (rating)."""
        db, Book, Reviewer, Review, BookTitle, ReviewerName, Rating = schema_with_ratings

        # Create test data
        Book.manager(db).insert(Book(title=BookTitle("Test Book")))
        book = Book.manager(db).get(title="Test Book")[0]

        for name, rating in [
            ("Alice", 4.5),
            ("Bob", 3.0),
            ("Carol", 5.0),
            ("Dave", 2.5),
        ]:
            Reviewer.manager(db).insert(Reviewer(name=ReviewerName(name)))
            reviewer = Reviewer.manager(db).get(name=name)[0]
            Review.manager(db).insert(Review(book=book, reviewer=reviewer, rating=Rating(rating)))

        # Order by rating ascending
        reviews_asc = Review.manager(db).filter().order_by("rating").execute()
        ratings_asc = [r.rating.value for r in reviews_asc]
        assert ratings_asc == sorted(ratings_asc)

        # Order by rating descending
        reviews_desc = Review.manager(db).filter().order_by("-rating").execute()
        ratings_desc = [r.rating.value for r in reviews_desc]
        assert ratings_desc == sorted(ratings_desc, reverse=True)

    def test_order_by_role_player_attribute(self, schema_with_ratings):
        """Order relations by role player's attribute."""
        db, Book, Reviewer, Review, BookTitle, ReviewerName, Rating = schema_with_ratings

        # Create reviewer
        Reviewer.manager(db).insert(Reviewer(name=ReviewerName("Test Reviewer")))
        reviewer = Reviewer.manager(db).get(name="Test Reviewer")[0]

        # Create books with reviews
        for title, rating in [
            ("Zebra Book", 4.0),
            ("Apple Book", 3.5),
            ("Mango Book", 4.5),
        ]:
            Book.manager(db).insert(Book(title=BookTitle(title)))
            book = Book.manager(db).get(title=title)[0]
            Review.manager(db).insert(Review(book=book, reviewer=reviewer, rating=Rating(rating)))

        # Order by book title (role player attribute)
        reviews = Review.manager(db).filter().order_by("book__title").execute()
        titles = [r.book.title.value for r in reviews]
        assert titles == sorted(titles)

    def test_combined_ordering_and_limit(self, schema_with_ratings):
        """Combine ordering with pagination for top-N queries."""
        db, Book, Reviewer, Review, BookTitle, ReviewerName, Rating = schema_with_ratings

        # Create test data
        Book.manager(db).insert(Book(title=BookTitle("Popular Book")))
        book = Book.manager(db).get(title="Popular Book")[0]

        ratings_data = [4.5, 3.0, 5.0, 2.5, 4.0, 3.5, 4.8, 2.0]
        for i, rating in enumerate(ratings_data):
            Reviewer.manager(db).insert(Reviewer(name=ReviewerName(f"Reviewer {i}")))
            reviewer = Reviewer.manager(db).get(name=f"Reviewer {i}")[0]
            Review.manager(db).insert(Review(book=book, reviewer=reviewer, rating=Rating(rating)))

        # Get top 3 reviews by rating
        top_reviews = Review.manager(db).filter().order_by("-rating").limit(3).execute()
        assert len(top_reviews) == 3
        top_ratings = [r.rating.value for r in top_reviews]
        # Should be the 3 highest: 5.0, 4.8, 4.5
        assert top_ratings == [5.0, 4.8, 4.5]


# =============================================================================
# Test: Complex Filter Expressions on Relations
# =============================================================================


@pytest.mark.integration
class TestComplexRelationFilters:
    """Test complex filter expressions on relations."""

    @pytest.fixture
    def schema_with_inventory(self, clean_db: Database):
        """Schema with inventory data for complex filtering."""

        class ProductName(String):
            pass

        class WarehouseName(String):
            pass

        class StockLevel(Integer):
            pass

        class LastRestocked(DateTime):
            pass

        class Product(Entity):
            flags = TypeFlags(name="product_inv")
            name: ProductName = Flag(Key)

        class Warehouse(Entity):
            flags = TypeFlags(name="warehouse_inv")
            name: WarehouseName = Flag(Key)

        class Inventory(Relation):
            flags = TypeFlags(name="inventory_inv")
            product: Role[Product] = Role("product", Product)
            warehouse: Role[Warehouse] = Role("warehouse", Warehouse)
            stock_level: StockLevel
            last_restocked: LastRestocked | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Product, Warehouse, Inventory)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Product,
            Warehouse,
            Inventory,
            ProductName,
            WarehouseName,
            StockLevel,
            LastRestocked,
        )

    def test_filter_by_relation_attribute_range(self, schema_with_inventory):
        """Filter relations by attribute value range."""
        (
            db,
            Product,
            Warehouse,
            Inventory,
            ProductName,
            WarehouseName,
            StockLevel,
            LastRestocked,
        ) = schema_with_inventory

        # Create test data
        Product.manager(db).insert(Product(name=ProductName("Widget")))
        product = Product.manager(db).get(name="Widget")[0]

        for wh_name, stock in [
            ("Warehouse A", 100),
            ("Warehouse B", 50),
            ("Warehouse C", 200),
            ("Warehouse D", 25),
        ]:
            Warehouse.manager(db).insert(Warehouse(name=WarehouseName(wh_name)))
            warehouse = Warehouse.manager(db).get(name=wh_name)[0]
            Inventory.manager(db).insert(
                Inventory(product=product, warehouse=warehouse, stock_level=StockLevel(stock))
            )

        # Filter by stock level > 50
        high_stock = Inventory.manager(db).filter(StockLevel.gt(StockLevel(50))).execute()
        assert len(high_stock) == 2
        assert all(inv.stock_level.value > 50 for inv in high_stock)

        # Filter by stock level <= 50
        low_stock = Inventory.manager(db).filter(StockLevel.lte(StockLevel(50))).execute()
        assert len(low_stock) == 2
        assert all(inv.stock_level.value <= 50 for inv in low_stock)

    def test_filter_by_role_player_and_attribute(self, schema_with_inventory):
        """Combine role player filter with attribute filter."""
        (
            db,
            Product,
            Warehouse,
            Inventory,
            ProductName,
            WarehouseName,
            StockLevel,
            LastRestocked,
        ) = schema_with_inventory

        # Create products and warehouses
        for name in ["Product A", "Product B"]:
            Product.manager(db).insert(Product(name=ProductName(name)))

        for name in ["Main Warehouse", "Backup Warehouse"]:
            Warehouse.manager(db).insert(Warehouse(name=WarehouseName(name)))

        main_wh = Warehouse.manager(db).get(name="Main Warehouse")[0]
        backup_wh = Warehouse.manager(db).get(name="Backup Warehouse")[0]
        product_a = Product.manager(db).get(name="Product A")[0]
        product_b = Product.manager(db).get(name="Product B")[0]

        # Create inventory entries
        Inventory.manager(db).insert(
            Inventory(product=product_a, warehouse=main_wh, stock_level=StockLevel(100))
        )
        Inventory.manager(db).insert(
            Inventory(product=product_a, warehouse=backup_wh, stock_level=StockLevel(20))
        )
        Inventory.manager(db).insert(
            Inventory(product=product_b, warehouse=main_wh, stock_level=StockLevel(75))
        )
        Inventory.manager(db).insert(
            Inventory(product=product_b, warehouse=backup_wh, stock_level=StockLevel(30))
        )

        # Filter: Product A with stock > 50
        result = (
            Inventory.manager(db)
            .filter(product=product_a)
            .filter(StockLevel.gt(StockLevel(50)))
            .execute()
        )

        assert len(result) == 1
        assert result[0].warehouse.name.value == "Main Warehouse"
        assert result[0].stock_level.value == 100
