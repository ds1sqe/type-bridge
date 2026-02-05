"""Complex query tests based on TypeDB social network schema.

Tests derived from: https://github.com/typedb/typedb-examples/tree/master/use-cases/social-network

These tests cover:
- Deep inheritance hierarchies (content → page → profile → person)
- Self-referential symmetric relations (friendship with @card(2))
- Geographic location traversal
- Cross-entity filtering via relations
- Polymorphic role players
- Multi-value attributes with filtering
- Chained relation traversal
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
    Unique,
)

# =============================================================================
# Shared Attribute Types
# =============================================================================


class PersonName(String):
    """Name attribute for persons."""

    pass


class CompanyName(String):
    """Name attribute for companies."""

    pass


class CityName(String):
    """Name attribute for cities."""

    pass


class StateName(String):
    """Name attribute for states."""

    pass


class CountryName(String):
    """Name attribute for countries."""

    pass


class Email(String):
    """Email attribute."""

    pass


class JobTitle(String):
    """Job title attribute."""

    pass


class Salary(Integer):
    """Salary attribute."""

    pass


class Tag(String):
    """Tag for multi-value attribute testing."""

    pass


class PostContent(String):
    """Post content."""

    pass


# Distinct types for inheritance hierarchy tests
class ContentId(String):
    """Content ID for hierarchy tests."""

    pass


class PageName(String):
    """Page name for hierarchy tests."""

    pass


class ProfileEmail(String):
    """Profile email for hierarchy tests."""

    pass


class ProfileId(String):
    """Profile ID for polymorphic tests."""

    pass


class DisplayName(String):
    """Display name for polymorphic tests."""

    pass


class PostId(String):
    """Post ID for polymorphic tests."""

    pass


# =============================================================================
# Test Class: Deep Inheritance Hierarchy
# =============================================================================


@pytest.mark.integration
class TestDeepInheritanceHierarchy:
    """Test querying across deep inheritance hierarchies.

    Schema pattern: content (abstract) → page (abstract) → profile (abstract) → person
    """

    @pytest.fixture
    def schema_with_deep_hierarchy(self, clean_db: Database):
        """Set up schema with 4-level inheritance."""

        # Level 1: Abstract content
        class Content(Entity):
            flags = TypeFlags(name="content_deep", abstract=True)
            content_id: ContentId = Flag(Key)

        # Level 2: Abstract page (extends content)
        class Page(Content):
            flags = TypeFlags(name="page_deep", abstract=True)
            page_name: PageName

        # Level 3: Abstract profile (extends page)
        class Profile(Page):
            flags = TypeFlags(name="profile_deep", abstract=True)
            profile_email: ProfileEmail

        # Level 4: Concrete person (extends profile)
        class Person(Profile):
            flags = TypeFlags(name="person_deep")
            salary: Salary | None = None

        # Level 4: Concrete organization (extends profile)
        class Organization(Profile):
            flags = TypeFlags(name="org_deep")
            employee_count: Salary | None = None  # Reusing Salary type for int

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Content, Page, Profile, Person, Organization)
        schema_manager.sync_schema(force=True)

        return clean_db, Content, Page, Profile, Person, Organization

    def test_query_concrete_returns_all_inherited_attrs(self, schema_with_deep_hierarchy):
        """Querying concrete type returns all inherited attributes."""
        db, _, _, _, Person, _ = schema_with_deep_hierarchy

        # Insert person with all attributes from hierarchy
        person = Person(
            content_id=ContentId("p1"),
            page_name=PageName("Alice Page"),
            profile_email=ProfileEmail("alice@example.com"),
            salary=Salary(100000),
        )
        Person.manager(db).insert(person)

        # Query and verify all attributes
        results = Person.manager(db).all()
        assert len(results) == 1
        fetched = results[0]
        assert str(fetched.content_id) == "p1"
        assert str(fetched.page_name) == "Alice Page"
        assert str(fetched.profile_email) == "alice@example.com"
        assert int(fetched.salary) == 100000

    def test_filter_on_inherited_attribute(self, schema_with_deep_hierarchy):
        """Filter using attribute defined in parent class."""
        db, _, _, _, Person, _ = schema_with_deep_hierarchy

        # Insert multiple persons
        Person.manager(db).insert(
            Person(
                content_id=ContentId("p1"),
                page_name=PageName("Alice"),
                profile_email=ProfileEmail("alice@example.com"),
            )
        )
        Person.manager(db).insert(
            Person(
                content_id=ContentId("p2"),
                page_name=PageName("Bob"),
                profile_email=ProfileEmail("bob@example.com"),
            )
        )

        # Filter by inherited attribute (profile_email from Profile)
        results = Person.manager(db).filter(profile_email="alice@example.com").execute()
        assert len(results) == 1
        assert str(results[0].content_id) == "p1"

    def test_sibling_types_isolated(self, schema_with_deep_hierarchy):
        """Sibling types at same level don't interfere."""
        db, _, _, _, Person, Organization = schema_with_deep_hierarchy

        # Insert person and organization
        Person.manager(db).insert(
            Person(
                content_id=ContentId("p1"),
                page_name=PageName("Alice"),
                profile_email=ProfileEmail("alice@example.com"),
            )
        )
        Organization.manager(db).insert(
            Organization(
                content_id=ContentId("o1"),
                page_name=PageName("TechCorp"),
                profile_email=ProfileEmail("contact@techcorp.com"),
            )
        )

        # Query each type
        persons = Person.manager(db).all()
        orgs = Organization.manager(db).all()

        assert len(persons) == 1
        assert len(orgs) == 1
        assert str(persons[0].content_id) == "p1"
        assert str(orgs[0].content_id) == "o1"


# =============================================================================
# Test Class: Self-Referential Symmetric Relations
# =============================================================================


@pytest.mark.integration
class TestSelfReferentialRelations:
    """Test self-referential relations like friendship.

    Pattern: friendship relates friend @card(2) - both roles same type
    """

    @pytest.fixture
    def schema_with_friendship(self, clean_db: Database):
        """Set up schema with self-referential friendship."""

        class Person(Entity):
            flags = TypeFlags(name="person_friend")
            name: PersonName = Flag(Key)
            email: Email | None = None

        class Friendship(Relation):
            flags = TypeFlags(name="friendship_self")
            # Both roles are Person - self-referential symmetric
            friend1: Role[Person] = Role("friend1", Person)
            friend2: Role[Person] = Role("friend2", Person)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Friendship)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Friendship

    def test_insert_self_referential_relation(self, schema_with_friendship):
        """Insert friendship between two persons."""
        db, Person, Friendship = schema_with_friendship

        alice = Person(name=PersonName("Alice"))
        bob = Person(name=PersonName("Bob"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)

        # Fetch with IIDs for relation insert
        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]

        friendship = Friendship(friend1=alice, friend2=bob)
        Friendship.manager(db).insert(friendship)

        friendships = Friendship.manager(db).all()
        assert len(friendships) == 1

    def test_query_friends_of_person(self, schema_with_friendship):
        """Query to find all friends of a specific person."""
        db, Person, Friendship = schema_with_friendship

        # Create 4 people
        for name in ["Alice", "Bob", "Carol", "Dave"]:
            Person.manager(db).insert(Person(name=PersonName(name)))

        # Fetch with IIDs
        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]
        carol = Person.manager(db).get(name="Carol")[0]
        dave = Person.manager(db).get(name="Dave")[0]

        # Alice is friends with Bob and Carol
        Friendship.manager(db).insert(Friendship(friend1=alice, friend2=bob))
        Friendship.manager(db).insert(Friendship(friend1=alice, friend2=carol))

        # Bob is also friends with Dave
        Friendship.manager(db).insert(Friendship(friend1=bob, friend2=dave))

        # Query friendships involving Alice (as friend1)
        alice_friendships = Friendship.manager(db).filter(friend1=alice).execute()
        assert len(alice_friendships) == 2

    def test_bidirectional_friendship_query(self, schema_with_friendship):
        """Friendships can be queried from either side."""
        db, Person, Friendship = schema_with_friendship

        Person.manager(db).insert(Person(name=PersonName("Alice")))
        Person.manager(db).insert(Person(name=PersonName("Bob")))

        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]

        # Insert friendship (alice as friend1, bob as friend2)
        Friendship.manager(db).insert(Friendship(friend1=alice, friend2=bob))

        # Query from friend2 side
        friendships_via_bob = Friendship.manager(db).filter(friend2=bob).execute()
        assert len(friendships_via_bob) == 1
        assert str(friendships_via_bob[0].friend1.name) == "Alice"


# =============================================================================
# Test Class: Geographic Location Hierarchy
# =============================================================================


@pytest.mark.integration
class TestGeographicHierarchy:
    """Test geographic location hierarchy and traversal.

    Pattern: Country → State → City with location relations
    """

    @pytest.fixture
    def schema_with_geography(self, clean_db: Database):
        """Set up geographic hierarchy schema."""

        class Country(Entity):
            flags = TypeFlags(name="country_geo")
            name: CountryName = Flag(Key)

        class State(Entity):
            flags = TypeFlags(name="state_geo")
            name: StateName = Flag(Key)

        class City(Entity):
            flags = TypeFlags(name="city_geo")
            name: CityName = Flag(Key)

        class Person(Entity):
            flags = TypeFlags(name="person_geo")
            name: PersonName = Flag(Key)

        # Country contains State
        class CountryContainsState(Relation):
            flags = TypeFlags(name="country_state_geo")
            country: Role[Country] = Role("country", Country)
            state: Role[State] = Role("state", State)

        # State contains City
        class StateContainsCity(Relation):
            flags = TypeFlags(name="state_city_geo")
            state: Role[State] = Role("state", State)
            city: Role[City] = Role("city", City)

        # Person lives in City
        class LivesIn(Relation):
            flags = TypeFlags(name="lives_in_geo")
            person: Role[Person] = Role("resident", Person)
            city: Role[City] = Role("location", City)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(
            Country,
            State,
            City,
            Person,
            CountryContainsState,
            StateContainsCity,
            LivesIn,
        )
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Country,
            State,
            City,
            Person,
            CountryContainsState,
            StateContainsCity,
            LivesIn,
        )

    def test_hierarchical_insert_and_query(self, schema_with_geography):
        """Insert and query geographic hierarchy."""
        (
            db,
            Country,
            State,
            City,
            Person,
            CountryContainsState,
            StateContainsCity,
            LivesIn,
        ) = schema_with_geography

        # Insert geography
        Country.manager(db).insert(Country(name=CountryName("USA")))
        State.manager(db).insert(State(name=StateName("California")))
        City.manager(db).insert(City(name=CityName("San Francisco")))

        # Fetch with IIDs
        usa = Country.manager(db).get(name="USA")[0]
        ca = State.manager(db).get(name="California")[0]
        sf = City.manager(db).get(name="San Francisco")[0]

        # Create relations
        CountryContainsState.manager(db).insert(CountryContainsState(country=usa, state=ca))
        StateContainsCity.manager(db).insert(StateContainsCity(state=ca, city=sf))

        # Query states in USA
        states = CountryContainsState.manager(db).filter(country=usa).execute()
        assert len(states) == 1
        assert str(states[0].state.name) == "California"

    def test_cross_hierarchy_query(self, schema_with_geography):
        """Query people living in a specific state via city."""
        (
            db,
            Country,
            State,
            City,
            Person,
            CountryContainsState,
            StateContainsCity,
            LivesIn,
        ) = schema_with_geography

        # Setup geography
        State.manager(db).insert(State(name=StateName("California")))
        City.manager(db).insert(City(name=CityName("San Francisco")))
        City.manager(db).insert(City(name=CityName("Los Angeles")))

        ca = State.manager(db).get(name="California")[0]
        sf = City.manager(db).get(name="San Francisco")[0]
        la = City.manager(db).get(name="Los Angeles")[0]

        StateContainsCity.manager(db).insert(StateContainsCity(state=ca, city=sf))
        StateContainsCity.manager(db).insert(StateContainsCity(state=ca, city=la))

        # Add people
        Person.manager(db).insert(Person(name=PersonName("Alice")))
        Person.manager(db).insert(Person(name=PersonName("Bob")))

        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]

        LivesIn.manager(db).insert(LivesIn(person=alice, city=sf))
        LivesIn.manager(db).insert(LivesIn(person=bob, city=la))

        # Query people in SF
        sf_residents = LivesIn.manager(db).filter(city=sf).execute()
        assert len(sf_residents) == 1
        assert str(sf_residents[0].person.name) == "Alice"


# =============================================================================
# Test Class: Polymorphic Role Players
# =============================================================================


@pytest.mark.integration
class TestPolymorphicRolePlayers:
    """Test relations where roles accept abstract types."""

    @pytest.fixture
    def schema_with_polymorphic_roles(self, clean_db: Database):
        """Set up schema with polymorphic role players."""

        # Abstract profile type
        class Profile(Entity):
            flags = TypeFlags(name="profile_poly", abstract=True)
            profile_id: ProfileId = Flag(Key)
            display_name: DisplayName

        # Concrete person
        class Person(Profile):
            flags = TypeFlags(name="person_poly")
            person_email: Email | None = None

        # Concrete organization
        class Organization(Profile):
            flags = TypeFlags(name="org_poly")
            org_website: Email | None = None  # Reusing Email type

        # Post authored by any profile
        class Post(Entity):
            flags = TypeFlags(name="post_poly")
            post_id: PostId = Flag(Key)
            content: PostContent

        # Authoring relation - author can be person or org
        class Authoring(Relation):
            flags = TypeFlags(name="authoring_poly")
            author: Role[Profile] = Role("author", Profile)
            post: Role[Post] = Role("post", Post)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Profile, Person, Organization, Post, Authoring)
        schema_manager.sync_schema(force=True)

        return clean_db, Profile, Person, Organization, Post, Authoring

    def test_insert_relation_with_person_author(self, schema_with_polymorphic_roles):
        """Insert authoring relation with Person as author.

        Verifies that polymorphic role players are resolved to their
        concrete type, not the abstract declared type.
        """
        db, Profile, Person, _, Post, Authoring = schema_with_polymorphic_roles

        Person.manager(db).insert(
            Person(
                profile_id=ProfileId("alice"),
                display_name=DisplayName("Alice Smith"),
                person_email=Email("alice@example.com"),
            )
        )
        Post.manager(db).insert(Post(post_id=PostId("post1"), content=PostContent("Hello world")))

        alice = Person.manager(db).get(profile_id="alice")[0]
        post = Post.manager(db).get(post_id="post1")[0]

        Authoring.manager(db).insert(Authoring(author=alice, post=post))

        authorings = Authoring.manager(db).all()
        assert len(authorings) == 1

        # CRITICAL: Role player should be resolved to concrete type (Person),
        # not the abstract declared type (Profile)
        author = authorings[0].author
        assert isinstance(author, Person), f"Expected Person, got {type(author).__name__}"
        assert type(author) is not Profile  # Should NOT be the abstract type

        # Inherited attributes from Profile are accessible
        assert str(author.profile_id) == "alice"
        assert str(author.display_name) == "Alice Smith"

        # Person-specific attributes should also be accessible
        assert str(author.person_email) == "alice@example.com"

    def test_insert_relation_with_org_author(self, schema_with_polymorphic_roles):
        """Insert authoring relation with Organization as author.

        Verifies that Organization (concrete subtype) is returned,
        not the abstract Profile type.
        """
        db, Profile, _, Organization, Post, Authoring = schema_with_polymorphic_roles

        Organization.manager(db).insert(
            Organization(
                profile_id=ProfileId("techcorp"),
                display_name=DisplayName("TechCorp Inc"),
                org_website=Email("https://techcorp.com"),
            )
        )
        Post.manager(db).insert(
            Post(post_id=PostId("announcement"), content=PostContent("Big news!"))
        )

        org = Organization.manager(db).get(profile_id="techcorp")[0]
        post = Post.manager(db).get(post_id="announcement")[0]

        Authoring.manager(db).insert(Authoring(author=org, post=post))

        authorings = Authoring.manager(db).all()
        assert len(authorings) == 1

        # CRITICAL: Should be Organization, not Profile
        author = authorings[0].author
        assert isinstance(author, Organization), (
            f"Expected Organization, got {type(author).__name__}"
        )
        assert type(author) is not Profile

        # Inherited attributes from Profile
        assert str(author.profile_id) == "techcorp"
        assert str(author.display_name) == "TechCorp Inc"

        # Organization-specific attribute
        assert str(author.org_website) == "https://techcorp.com"

    def test_mixed_authors_resolved_to_concrete_types(self, schema_with_polymorphic_roles):
        """Mixed author types are resolved to their concrete types.

        When querying relations with polymorphic role players, each
        role player should be resolved to its actual concrete type.
        """
        db, _, Person, Organization, Post, Authoring = schema_with_polymorphic_roles

        # Insert person and org with their specific attributes
        Person.manager(db).insert(
            Person(
                profile_id=ProfileId("alice"),
                display_name=DisplayName("Alice"),
                person_email=Email("alice@example.com"),
            )
        )
        Organization.manager(db).insert(
            Organization(
                profile_id=ProfileId("techcorp"),
                display_name=DisplayName("TechCorp"),
                org_website=Email("https://techcorp.com"),
            )
        )

        # Insert posts
        Post.manager(db).insert(Post(post_id=PostId("post1"), content=PostContent("Personal post")))
        Post.manager(db).insert(Post(post_id=PostId("post2"), content=PostContent("Company post")))

        alice = Person.manager(db).get(profile_id="alice")[0]
        org = Organization.manager(db).get(profile_id="techcorp")[0]
        post1 = Post.manager(db).get(post_id="post1")[0]
        post2 = Post.manager(db).get(post_id="post2")[0]

        Authoring.manager(db).insert(Authoring(author=alice, post=post1))
        Authoring.manager(db).insert(Authoring(author=org, post=post2))

        # Query all authorings
        authorings = Authoring.manager(db).all()
        assert len(authorings) == 2

        # CRITICAL: Each author should be its concrete type
        author_types = {type(a.author).__name__ for a in authorings}
        assert author_types == {"Person", "Organization"}

        # Find each authoring and verify concrete type attributes
        for authoring in authorings:
            if str(authoring.author.profile_id) == "alice":
                assert isinstance(authoring.author, Person)
                assert str(authoring.author.person_email) == "alice@example.com"
            else:
                assert isinstance(authoring.author, Organization)
                assert str(authoring.author.org_website) == "https://techcorp.com"

        # Authors have distinct IIDs
        author_iids = {a.author._iid for a in authorings}
        assert len(author_iids) == 2


# =============================================================================
# Test Class: Multi-Value Attributes
# =============================================================================


@pytest.mark.integration
class TestMultiValueAttributes:
    """Test multi-value attributes like tags."""

    @pytest.fixture
    def schema_with_tags(self, clean_db: Database):
        """Set up schema with multi-value tag attribute."""

        class Post(Entity):
            flags = TypeFlags(name="post_tags")
            post_id: PersonName = Flag(Key)
            content: PostContent
            tags: list[Tag] = Flag(Card(0, 10))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Post)
        schema_manager.sync_schema(force=True)

        return clean_db, Post

    def test_insert_with_multiple_tags(self, schema_with_tags):
        """Insert entity with multiple tag values."""
        db, Post = schema_with_tags

        post = Post(
            post_id=PersonName("p1"),
            content=PostContent("My first post"),
            tags=[Tag("python"), Tag("typedb"), Tag("orm")],
        )
        Post.manager(db).insert(post)

        results = Post.manager(db).all()
        assert len(results) == 1
        assert len(results[0].tags) == 3
        tag_values = {str(t) for t in results[0].tags}
        assert tag_values == {"python", "typedb", "orm"}

    def test_filter_by_tag_value(self, schema_with_tags):
        """Filter entities by specific tag value."""
        db, Post = schema_with_tags

        Post.manager(db).insert(
            Post(
                post_id=PersonName("p1"),
                content=PostContent("Python post"),
                tags=[Tag("python"), Tag("programming")],
            )
        )
        Post.manager(db).insert(
            Post(
                post_id=PersonName("p2"),
                content=PostContent("TypeDB post"),
                tags=[Tag("typedb"), Tag("database")],
            )
        )
        Post.manager(db).insert(
            Post(
                post_id=PersonName("p3"),
                content=PostContent("Both post"),
                tags=[Tag("python"), Tag("typedb")],
            )
        )

        # Filter by python tag
        python_posts = Post.manager(db).filter(tags=Tag("python")).execute()
        assert len(python_posts) == 2
        post_ids = {str(p.post_id) for p in python_posts}
        assert post_ids == {"p1", "p3"}

    def test_update_tags_list(self, schema_with_tags):
        """Update multi-value attribute list."""
        db, Post = schema_with_tags

        Post.manager(db).insert(
            Post(
                post_id=PersonName("p1"),
                content=PostContent("Original"),
                tags=[Tag("old"), Tag("tags")],
            )
        )

        # Fetch, update tags, save
        post = Post.manager(db).get(post_id="p1")[0]
        post.tags = [Tag("new"), Tag("updated"), Tag("tags")]
        Post.manager(db).update(post)

        # Verify
        updated = Post.manager(db).get(post_id="p1")[0]
        assert len(updated.tags) == 3
        tag_values = {str(t) for t in updated.tags}
        assert tag_values == {"new", "updated", "tags"}


# =============================================================================
# Test Class: Chained Relations (Employment Chain)
# =============================================================================


@pytest.mark.integration
class TestChainedRelations:
    """Test chained relation traversal.

    Pattern: Person → Employment → Company → Location
    """

    @pytest.fixture
    def schema_with_employment_chain(self, clean_db: Database):
        """Set up employment chain schema."""

        class Person(Entity):
            flags = TypeFlags(name="person_emp")
            name: PersonName = Flag(Key)

        class Company(Entity):
            flags = TypeFlags(name="company_emp")
            name: CompanyName = Flag(Key)

        class City(Entity):
            flags = TypeFlags(name="city_emp")
            name: CityName = Flag(Key)

        class Employment(Relation):
            flags = TypeFlags(name="employment_chain")
            employee: Role[Person] = Role("employee", Person)
            employer: Role[Company] = Role("employer", Company)
            job_title: JobTitle
            salary: Salary | None = None

        class CompanyLocation(Relation):
            flags = TypeFlags(name="company_location")
            company: Role[Company] = Role("company", Company)
            city: Role[City] = Role("headquarters", City)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Company, City, Employment, CompanyLocation)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Company, City, Employment, CompanyLocation

    def test_employment_with_salary(self, schema_with_employment_chain):
        """Create employment with salary attribute."""
        db, Person, Company, City, Employment, CompanyLocation = schema_with_employment_chain

        Person.manager(db).insert(Person(name=PersonName("Alice")))
        Company.manager(db).insert(Company(name=CompanyName("TechCorp")))

        alice = Person.manager(db).get(name="Alice")[0]
        techcorp = Company.manager(db).get(name="TechCorp")[0]

        employment = Employment(
            employee=alice,
            employer=techcorp,
            job_title=JobTitle("Engineer"),
            salary=Salary(120000),
        )
        Employment.manager(db).insert(employment)

        # Query and verify
        employments = Employment.manager(db).all()
        assert len(employments) == 1
        assert str(employments[0].job_title) == "Engineer"
        assert int(employments[0].salary) == 120000

    def test_find_employees_by_company(self, schema_with_employment_chain):
        """Find all employees of a specific company."""
        db, Person, Company, City, Employment, CompanyLocation = schema_with_employment_chain

        # Create companies
        Company.manager(db).insert(Company(name=CompanyName("TechCorp")))
        Company.manager(db).insert(Company(name=CompanyName("StartupInc")))

        # Create people
        for name in ["Alice", "Bob", "Carol"]:
            Person.manager(db).insert(Person(name=PersonName(name)))

        techcorp = Company.manager(db).get(name="TechCorp")[0]
        startup = Company.manager(db).get(name="StartupInc")[0]
        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]
        carol = Person.manager(db).get(name="Carol")[0]

        # Alice and Bob work at TechCorp, Carol at Startup
        Employment.manager(db).insert(
            Employment(employee=alice, employer=techcorp, job_title=JobTitle("Engineer"))
        )
        Employment.manager(db).insert(
            Employment(employee=bob, employer=techcorp, job_title=JobTitle("Designer"))
        )
        Employment.manager(db).insert(
            Employment(employee=carol, employer=startup, job_title=JobTitle("CEO"))
        )

        # Find TechCorp employees
        techcorp_emp = Employment.manager(db).filter(employer=techcorp).execute()
        assert len(techcorp_emp) == 2
        emp_names = {str(e.employee.name) for e in techcorp_emp}
        assert emp_names == {"Alice", "Bob"}

    def test_company_location_chain(self, schema_with_employment_chain):
        """Query through company location chain."""
        db, Person, Company, City, Employment, CompanyLocation = schema_with_employment_chain

        # Setup
        City.manager(db).insert(City(name=CityName("San Francisco")))
        Company.manager(db).insert(Company(name=CompanyName("TechCorp")))

        sf = City.manager(db).get(name="San Francisco")[0]
        techcorp = Company.manager(db).get(name="TechCorp")[0]

        CompanyLocation.manager(db).insert(CompanyLocation(company=techcorp, city=sf))

        # Query companies in SF
        sf_companies = CompanyLocation.manager(db).filter(city=sf).execute()
        assert len(sf_companies) == 1
        assert str(sf_companies[0].company.name) == "TechCorp"


# =============================================================================
# Test Class: Complex Filter Combinations
# =============================================================================


@pytest.mark.integration
class TestComplexFilterCombinations:
    """Test complex filter combinations across entities and relations."""

    @pytest.fixture
    def schema_for_complex_filters(self, clean_db: Database):
        """Set up schema for complex filter tests."""

        class Person(Entity):
            flags = TypeFlags(name="person_filter")
            name: PersonName = Flag(Key)
            email: Email = Flag(Unique)
            salary: Salary | None = None

        class Department(Entity):
            flags = TypeFlags(name="dept_filter")
            name: CompanyName = Flag(Key)

        class Works(Relation):
            flags = TypeFlags(name="works_filter")
            employee: Role[Person] = Role("employee", Person)
            department: Role[Department] = Role("department", Department)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Department, Works)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Department, Works

    def test_filter_by_multiple_entity_attributes(self, schema_for_complex_filters):
        """Filter entity by multiple attributes."""
        db, Person, Department, Works = schema_for_complex_filters

        Person.manager(db).insert(
            Person(
                name=PersonName("Alice"),
                email=Email("alice@example.com"),
                salary=Salary(100000),
            )
        )
        Person.manager(db).insert(
            Person(
                name=PersonName("Bob"),
                email=Email("bob@example.com"),
                salary=Salary(80000),
            )
        )

        # Filter by email
        results = Person.manager(db).filter(email="alice@example.com").execute()
        assert len(results) == 1
        assert str(results[0].name) == "Alice"

    def test_relation_filter_by_role_player_attributes(self, schema_for_complex_filters):
        """Filter relations by role player attributes."""
        db, Person, Department, Works = schema_for_complex_filters

        # Setup
        Person.manager(db).insert(
            Person(name=PersonName("Alice"), email=Email("alice@example.com"))
        )
        Person.manager(db).insert(Person(name=PersonName("Bob"), email=Email("bob@example.com")))
        Department.manager(db).insert(Department(name=CompanyName("Engineering")))
        Department.manager(db).insert(Department(name=CompanyName("Marketing")))

        alice = Person.manager(db).get(name="Alice")[0]
        bob = Person.manager(db).get(name="Bob")[0]
        eng = Department.manager(db).get(name="Engineering")[0]
        mkt = Department.manager(db).get(name="Marketing")[0]

        Works.manager(db).insert(Works(employee=alice, department=eng))
        Works.manager(db).insert(Works(employee=bob, department=mkt))

        # Filter by department
        eng_workers = Works.manager(db).filter(department=eng).execute()
        assert len(eng_workers) == 1
        assert str(eng_workers[0].employee.name) == "Alice"
