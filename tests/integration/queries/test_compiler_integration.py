"""Integration tests for QueryCompiler against a real TypeDB instance.

Tests that compiled AST nodes produce valid TypeQL that executes correctly.
This ensures the compiler generates syntactically correct queries for all
supported AST node types.
"""

import pytest

from type_bridge import Database, Entity, Relation, TypeFlags
from type_bridge.attribute import AttributeFlags, Integer, String
from type_bridge.attribute.flags import Card, Flag, Key
from type_bridge.models.role import Role
from type_bridge.query.ast import (
    DeleteClause,
    DeleteThingStatement,
    EntityPattern,
    FetchAttribute,
    FetchClause,
    FetchFunction,
    FetchNestedWildcard,
    FetchVariable,
    FunctionCallValue,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    RawPattern,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    ValueComparisonPattern,
)
from type_bridge.query.compiler import QueryCompiler

# ============================================================================
# Test Schema
# ============================================================================


class PersonName(String):
    flags = AttributeFlags(name="person-name")


class PersonAge(Integer):
    flags = AttributeFlags(name="person-age")


class PersonEmail(String):
    flags = AttributeFlags(name="person-email")


class Person(Entity):
    flags = TypeFlags(name="test_person")
    name: PersonName = Flag(Key)
    age: PersonAge | None = None
    emails: list[PersonEmail] = Flag(Card(min=0))  # Multi-value, unbounded max


class FriendshipSince(Integer):
    flags = AttributeFlags(name="friendship-since")


class Friendship(Relation):
    flags = TypeFlags(name="test_friendship")
    friend: Role[Person] = Flag(Card(2, 2))
    since: FriendshipSince | None = None


# Schema for testing
TEST_SCHEMA = """
define
attribute person-name, value string;
attribute person-age, value integer;
attribute person-email, value string;
attribute friendship-since, value integer;

entity test_person,
    owns person-name @key,
    owns person-age,
    owns person-email;

relation test_friendship,
    relates friend @card(2..2),
    owns friendship-since;

test_person plays test_friendship:friend;

fun count-test-persons() -> integer:
    match $p isa test_person;
    return count($p);

fun list-test-person-names() -> { person-name }:
    match $p isa test_person, has person-name $n;
    return { $n };
"""


# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def db(docker_typedb):
    """Provide database connection."""
    from tests.integration.conftest import TEST_DB_ADDRESS, TEST_DB_HTTP_PORT

    db_name = "test_compiler_integration"

    database = Database(
        address=TEST_DB_ADDRESS,
        database=db_name,
        http_port=TEST_DB_HTTP_PORT,
    )
    database.connect()
    if database.database_exists():
        database.delete_database()
    database.create_database()
    try:
        yield database
    finally:
        if database.database_exists():
            database.delete_database()
        database.close()


@pytest.fixture
def setup_schema(db):
    """Set up schema and insert test data."""
    with db.transaction("schema") as tx:
        tx.execute(TEST_SCHEMA)
        tx.commit()

    # Insert test data using raw TypeQL to avoid ORM complexity
    with db.transaction("write") as tx:
        tx.execute("""
            insert
            $alice isa test_person, has person-name "Alice", has person-age 30;
            $bob isa test_person, has person-name "Bob", has person-age 25;
            $charlie isa test_person, has person-name "Charlie", has person-age 35;
        """)
        tx.commit()

    # Create friendship using raw TypeQL
    with db.transaction("write") as tx:
        tx.execute("""
            match
            $alice isa test_person, has person-name "Alice";
            $bob isa test_person, has person-name "Bob";
            insert
            (friend: $alice, friend: $bob) isa test_friendship, has friendship-since 2020;
        """)
        tx.commit()

    yield db


@pytest.fixture
def compiler():
    return QueryCompiler()


# ============================================================================
# Tests: Match Clause Patterns
# ============================================================================


class TestMatchPatterns:
    """Tests for various match patterns compiled and executed against TypeDB."""

    @pytest.mark.integration
    def test_entity_pattern_basic(self, setup_schema, compiler) -> None:
        """Basic entity pattern matches entities."""
        db = setup_schema
        pattern = EntityPattern(variable="$p", type_name="test_person")
        match = MatchClause(patterns=[pattern])
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 3

    @pytest.mark.integration
    def test_entity_pattern_with_has_constraint(self, setup_schema, compiler) -> None:
        """Entity pattern with has constraint filters correctly."""
        db = setup_schema
        pattern = EntityPattern(
            variable="$p",
            type_name="test_person",
            constraints=[
                HasConstraint(attr_name="person-name", value=LiteralValue("Alice", "string"))
            ],
        )
        match = MatchClause(patterns=[pattern])
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["name"] == "Alice"

    @pytest.mark.integration
    def test_entity_pattern_with_has_variable(self, setup_schema, compiler) -> None:
        """Entity pattern with has constraint using variable."""
        db = setup_schema
        # Match person and bind name to variable
        patterns = [
            EntityPattern(variable="$p", type_name="test_person"),
            HasPattern(thing_var="$p", attr_type="person-name", attr_var="$n"),
        ]
        match = MatchClause(patterns=patterns)
        fetch = FetchClause(items=[FetchVariable(key="name", var="$n")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 3
        names = {r["name"] for r in results}
        assert names == {"Alice", "Bob", "Charlie"}

    @pytest.mark.integration
    def test_relation_pattern_with_role_players(self, setup_schema, compiler) -> None:
        """Relation pattern matches relations with role players."""
        db = setup_schema
        pattern = RelationPattern(
            variable="$f",
            type_name="test_friendship",
            role_players=[
                RolePlayer(role="friend", player_var="$p1"),
                RolePlayer(role="friend", player_var="$p2"),
            ],
        )
        match = MatchClause(patterns=[pattern])
        fetch = FetchClause(items=[FetchFunction(key="friendship_iid", func_name="iid", var="$f")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        # One friendship, but 2 results due to role player permutations
        assert len(results) >= 1
        assert results[0]["friendship_iid"].startswith("0x")

    @pytest.mark.integration
    def test_value_comparison_pattern(self, setup_schema, compiler) -> None:
        """Value comparison pattern filters by value."""
        db = setup_schema
        patterns = [
            EntityPattern(variable="$p", type_name="test_person"),
            HasPattern(thing_var="$p", attr_type="person-age", attr_var="$age"),
            ValueComparisonPattern(var="$age", operator=">=", value=LiteralValue(30, "long")),
        ]
        match = MatchClause(patterns=patterns)
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        # Alice (30) and Charlie (35)
        assert len(results) == 2
        names = {r["name"] for r in results}
        assert names == {"Alice", "Charlie"}

    @pytest.mark.integration
    def test_value_comparison_with_variable(self, setup_schema, compiler) -> None:
        """Value comparison with variable reference."""
        db = setup_schema
        # Find persons older than 25
        patterns = [
            EntityPattern(variable="$p", type_name="test_person"),
            HasPattern(thing_var="$p", attr_type="person-age", attr_var="$age"),
            ValueComparisonPattern(var="$age", operator=">", value="25"),  # String value
        ]
        match = MatchClause(patterns=patterns)
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 2

    @pytest.mark.integration
    def test_not_pattern(self, setup_schema, compiler) -> None:
        """Not pattern excludes matching entities."""
        db = setup_schema
        # Find persons who are NOT named Alice
        patterns = [
            EntityPattern(variable="$p", type_name="test_person"),
            NotPattern(
                patterns=[
                    HasPattern(thing_var="$p", attr_type="person-name", attr_var="$n"),
                    ValueComparisonPattern(
                        var="$n", operator="==", value=LiteralValue("Alice", "string")
                    ),
                ]
            ),
        ]
        match = MatchClause(patterns=patterns)
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 2
        names = {r["name"] for r in results}
        assert "Alice" not in names

    @pytest.mark.integration
    def test_or_pattern(self, setup_schema, compiler) -> None:
        """Or pattern matches either alternative."""
        db = setup_schema
        # Find persons named Alice OR Bob
        patterns = [
            EntityPattern(variable="$p", type_name="test_person"),
            HasPattern(thing_var="$p", attr_type="person-name", attr_var="$n"),
            OrPattern(
                alternatives=[
                    [
                        ValueComparisonPattern(
                            var="$n", operator="==", value=LiteralValue("Alice", "string")
                        )
                    ],
                    [
                        ValueComparisonPattern(
                            var="$n", operator="==", value=LiteralValue("Bob", "string")
                        )
                    ],
                ]
            ),
        ]
        match = MatchClause(patterns=patterns)
        fetch = FetchClause(items=[FetchVariable(key="name", var="$n")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 2
        names = {r["name"] for r in results}
        assert names == {"Alice", "Bob"}

    @pytest.mark.integration
    def test_raw_pattern(self, setup_schema, compiler) -> None:
        """Raw pattern passes through literal TypeQL."""
        db = setup_schema
        match = MatchClause(
            patterns=[RawPattern(content='$p isa test_person, has person-name "Alice"')]
        )
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["name"] == "Alice"


# ============================================================================
# Tests: Insert, Update, Delete
# ============================================================================


class TestMutationStatements:
    """Tests for insert, update, delete statements."""

    @pytest.mark.integration
    def test_insert_entity(self, setup_schema, compiler) -> None:
        """Insert statement creates entity."""
        db = setup_schema
        insert = InsertClause(
            statements=[
                IsaStatement(variable="$p", type_name="test_person"),
                HasStatement(
                    subject_var="$p",
                    attr_name="person-name",
                    value=LiteralValue("David", "string"),
                ),
                HasStatement(
                    subject_var="$p",
                    attr_name="person-age",
                    value=LiteralValue(40, "long"),
                ),
            ]
        )

        query = compiler.compile(insert)

        with db.transaction("write") as tx:
            tx.execute(query)
            tx.commit()

        # Verify insert
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    'match $p isa test_person, has person-name "David"; fetch { "age": $p.person-age };'
                )
            )

        assert len(results) == 1
        assert results[0]["age"] == 40

    @pytest.mark.integration
    def test_insert_relation_with_variable(self, setup_schema, compiler) -> None:
        """Insert relation statement with variable.

        Tests RelationStatement with include_variable=True.
        Note: TypeDB 3.x requires the variable before role players in insert.
        """
        db = setup_schema

        # First get Alice and Charlie IIDs
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    """match $p isa test_person, has person-name $n;
                    { $n == "Alice"; } or { $n == "Charlie"; };
                    fetch { "iid": iid($p), "name": $n };"""
                )
            )

        alice_iid = next(r["iid"] for r in results if r["name"] == "Alice")
        charlie_iid = next(r["iid"] for r in results if r["name"] == "Charlie")

        # Build insert with match - must include type in pattern for TypeDB type inference
        # Using EntityPattern with IidConstraint ensures TypeDB knows $a and $c are persons
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$a",
                    type_name="test_person",
                    constraints=[IidConstraint(iid=alice_iid)],
                ),
                EntityPattern(
                    variable="$c",
                    type_name="test_person",
                    constraints=[IidConstraint(iid=charlie_iid)],
                ),
            ]
        )
        # Test just the relation statement without inline attributes
        insert = InsertClause(
            statements=[
                RelationStatement(
                    variable="$f",
                    type_name="test_friendship",
                    role_players=[
                        RolePlayer(role="friend", player_var="$a"),
                        RolePlayer(role="friend", player_var="$c"),
                    ],
                    include_variable=True,
                ),
            ]
        )

        query = compiler.compile(match) + "\n" + compiler.compile(insert)

        with db.transaction("write") as tx:
            tx.execute(query)
            tx.commit()

        # Verify - count friendships (should be 2 now: Alice-Bob and Alice-Charlie)
        with db.transaction() as tx:
            results = list(tx.execute('match $f isa test_friendship; fetch { "iid": iid($f) };'))

        assert len(results) == 2

    @pytest.mark.integration
    def test_insert_relation_without_variable(self, setup_schema, compiler) -> None:
        """Insert relation statement without variable (inline attributes).

        Tests RelationStatement with include_variable=False and inline attributes.
        """
        db = setup_schema

        # Get Bob and Charlie IIDs (use different pair to avoid duplicate)
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    """match $p isa test_person, has person-name $n;
                    { $n == "Bob"; } or { $n == "Charlie"; };
                    fetch { "iid": iid($p), "name": $n };"""
                )
            )

        bob_iid = next(r["iid"] for r in results if r["name"] == "Bob")
        charlie_iid = next(r["iid"] for r in results if r["name"] == "Charlie")

        # Build insert without variable - include_variable=False means no $f prefix
        # Must use EntityPattern with IidConstraint for TypeDB type inference
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$b",
                    type_name="test_person",
                    constraints=[IidConstraint(iid=bob_iid)],
                ),
                EntityPattern(
                    variable="$c",
                    type_name="test_person",
                    constraints=[IidConstraint(iid=charlie_iid)],
                ),
            ]
        )
        insert = InsertClause(
            statements=[
                RelationStatement(
                    variable="$f",  # Not used when include_variable=False
                    type_name="test_friendship",
                    role_players=[
                        RolePlayer(role="friend", player_var="$b"),
                        RolePlayer(role="friend", player_var="$c"),
                    ],
                    include_variable=False,
                    attributes=[
                        HasStatement(
                            subject_var="$f",
                            attr_name="friendship-since",
                            value=LiteralValue(2022, "long"),
                        )
                    ],
                ),
            ]
        )

        query = compiler.compile(match) + "\n" + compiler.compile(insert)

        with db.transaction("write") as tx:
            tx.execute(query)
            tx.commit()

        # Verify - friendship with 2022 should exist
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    'match $f isa test_friendship, has friendship-since 2022; fetch { "iid": iid($f) };'
                )
            )

        assert len(results) == 1

    @pytest.mark.integration
    def test_delete_entity(self, setup_schema, compiler) -> None:
        """Delete statement removes entity."""
        db = setup_schema

        # Match Charlie and delete
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Charlie", "string")
                        )
                    ],
                )
            ]
        )
        delete = DeleteClause(statements=[DeleteThingStatement(variable="$p")])

        query = compiler.compile(match) + "\n" + compiler.compile(delete)

        with db.transaction("write") as tx:
            tx.execute(query)
            tx.commit()

        # Verify deletion
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    'match $p isa test_person, has person-name "Charlie"; fetch { "name": $p.person-name };'
                )
            )

        assert len(results) == 0


# ============================================================================
# Tests: Fetch Items
# ============================================================================


class TestFetchItems:
    """Tests for various fetch item types."""

    @pytest.mark.integration
    def test_fetch_attribute(self, setup_schema, compiler) -> None:
        """FetchAttribute retrieves single attribute."""
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Alice", "string")
                        )
                    ],
                )
            ]
        )
        fetch = FetchClause(
            items=[
                FetchAttribute(key="name", var="$p", attr_name="person-name"),
                FetchAttribute(key="age", var="$p", attr_name="person-age"),
            ]
        )

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["name"] == "Alice"
        assert results[0]["age"] == 30

    @pytest.mark.integration
    def test_fetch_variable(self, setup_schema, compiler) -> None:
        """FetchVariable retrieves variable directly."""
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(variable="$p", type_name="test_person"),
                HasPattern(thing_var="$p", attr_type="person-name", attr_var="$n"),
            ]
        )
        fetch = FetchClause(items=[FetchVariable(key="person_name", var="$n")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 3
        names = {r["person_name"] for r in results}
        assert names == {"Alice", "Bob", "Charlie"}

    @pytest.mark.integration
    def test_fetch_function(self, setup_schema, compiler) -> None:
        """FetchFunction retrieves function result."""
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Alice", "string")
                        )
                    ],
                )
            ]
        )
        fetch = FetchClause(items=[FetchFunction(key="iid", func_name="iid", var="$p")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["iid"].startswith("0x")

    @pytest.mark.integration
    def test_fetch_nested_wildcard(self, setup_schema, compiler) -> None:
        """FetchNestedWildcard retrieves all attributes in a nested object.

        TypeDB requires wildcard to be wrapped in braces: "key": { $var.* }
        The standalone syntax "key": $var.* causes a parse error.
        """
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Alice", "string")
                        )
                    ],
                )
            ]
        )
        fetch = FetchClause(items=[FetchNestedWildcard(key="all", var="$p")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        # Nested wildcard returns all owned attributes under the "all" key
        assert "all" in results[0]
        attrs = results[0]["all"]
        assert "person-name" in attrs
        assert attrs["person-name"] == "Alice"

    @pytest.mark.integration
    def test_fetch_raw_string(self, setup_schema, compiler) -> None:
        """Raw string fetch item passes through."""
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Alice", "string")
                        )
                    ],
                )
            ]
        )
        # Raw string item
        fetch = FetchClause(items=['"raw_name": $p.person-name'])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["raw_name"] == "Alice"


# ============================================================================
# Tests: Reduce/Aggregations
# ============================================================================


class TestReduce:
    """Tests for reduce clause with aggregations."""

    @pytest.mark.integration
    def test_reduce_count(self, setup_schema, compiler) -> None:
        """Reduce with count aggregation."""
        db = setup_schema
        match = MatchClause(patterns=[EntityPattern(variable="$p", type_name="test_person")])
        reduce = ReduceClause(
            assignments=[
                ReduceAssignment(
                    variable="$count",
                    expression=FunctionCallValue(function="count", args=["$p"]),
                )
            ]
        )

        query = compiler.compile(match) + "\n" + compiler.compile(reduce)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        # TypeDB 3.x reduce returns {'varname': {'value': N}}
        result = results[0]
        assert result["count"]["value"] == 3, f"Unexpected result: {result}"

    @pytest.mark.integration
    def test_reduce_with_string_expression(self, setup_schema, compiler) -> None:
        """Reduce with raw string expression."""
        db = setup_schema
        match = MatchClause(patterns=[EntityPattern(variable="$p", type_name="test_person")])
        reduce = ReduceClause(
            assignments=[ReduceAssignment(variable="$total", expression="count($p)")]
        )

        query = compiler.compile(match) + "\n" + compiler.compile(reduce)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        # TypeDB 3.x reduce returns {'varname': {'value': N}}
        result = results[0]
        assert result["total"]["value"] == 3, f"Unexpected result: {result}"


# ============================================================================
# Tests: Let Assignments
# ============================================================================


class TestLetAssignment:
    """Tests for match let clause with function calls."""

    @pytest.mark.integration
    def test_let_single_variable_non_stream(self, setup_schema, compiler) -> None:
        """Let assignment with single variable, non-stream function."""
        db = setup_schema
        let_clause = MatchLetClause(
            assignments=[
                LetAssignment(
                    variables=["$count"],
                    expression=FunctionCallValue(function="count-test-persons", args=[]),
                    is_stream=False,
                )
            ]
        )
        fetch = FetchClause(items=[FetchVariable(key="count", var="$count")])

        query = compiler.compile(let_clause) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["count"] == 3

    @pytest.mark.integration
    def test_let_single_variable_stream(self, setup_schema, compiler) -> None:
        """Let assignment with single variable, stream function."""
        db = setup_schema
        let_clause = MatchLetClause(
            assignments=[
                LetAssignment(
                    variables=["$name"],
                    expression=FunctionCallValue(function="list-test-person-names", args=[]),
                    is_stream=True,
                )
            ]
        )
        fetch = FetchClause(items=[FetchVariable(key="name", var="$name")])

        query = compiler.compile(let_clause) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 3
        names = {r["name"] for r in results}
        assert names == {"Alice", "Bob", "Charlie"}

    @pytest.mark.integration
    def test_let_with_string_expression(self, setup_schema, compiler) -> None:
        """Let assignment with raw string expression."""
        db = setup_schema
        let_clause = MatchLetClause(
            assignments=[
                LetAssignment(
                    variables=["$count"],
                    expression="count-test-persons()",  # String, not AST
                    is_stream=False,
                )
            ]
        )
        fetch = FetchClause(items=[FetchVariable(key="count", var="$count")])

        query = compiler.compile(let_clause) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["count"] == 3


# ============================================================================
# Tests: IID Pattern and Constraint
# ============================================================================


class TestIidOperations:
    """Tests for IID-based operations."""

    @pytest.mark.integration
    def test_iid_pattern(self, setup_schema, compiler) -> None:
        """IidPattern matches by IID.

        Note: IidPattern alone doesn't provide type info, so we need to
        combine with an isa constraint or use iid() in fetch.
        """
        db = setup_schema

        # First get Alice's IID
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    'match $p isa! test_person, has person-name "Alice"; fetch { "iid": iid($p) };'
                )
            )
        assert len(results) == 1, f"Expected 1 result for Alice, got {len(results)}"
        alice_iid = results[0]["iid"]
        assert alice_iid.startswith("0x"), f"Invalid IID format: {alice_iid}"

        # Match by IID - need to also constrain type for fetch to work
        match = MatchClause(
            patterns=[
                IidPattern(variable="$p", iid=alice_iid),
                EntityPattern(variable="$p", type_name="test_person"),
            ]
        )
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["name"] == "Alice"

    @pytest.mark.integration
    def test_iid_constraint(self, setup_schema, compiler) -> None:
        """IidConstraint filters by IID."""
        db = setup_schema

        # First get Bob's IID
        with db.transaction() as tx:
            results = list(
                tx.execute(
                    'match $p isa test_person, has person-name "Bob"; fetch { "iid": iid($p) };'
                )
            )
        bob_iid = results[0]["iid"]

        # Match with IID constraint
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[IidConstraint(iid=bob_iid)],
                )
            ]
        )
        fetch = FetchClause(items=[FetchAttribute(key="name", var="$p", attr_name="person-name")])

        query = compiler.compile(match) + "\n" + compiler.compile(fetch)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["name"] == "Bob"


# ============================================================================
# Tests: Compile Batch
# ============================================================================


class TestCompileBatch:
    """Tests for compile_batch method."""

    @pytest.mark.integration
    def test_compile_batch_multiple_clauses(self, setup_schema, compiler) -> None:
        """compile_batch combines multiple clauses."""
        db = setup_schema
        match = MatchClause(
            patterns=[
                EntityPattern(
                    variable="$p",
                    type_name="test_person",
                    constraints=[
                        HasConstraint(
                            attr_name="person-name", value=LiteralValue("Alice", "string")
                        )
                    ],
                )
            ]
        )
        fetch = FetchClause(items=[FetchAttribute(key="age", var="$p", attr_name="person-age")])

        query = compiler.compile_batch([match, fetch])

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        assert results[0]["age"] == 30
