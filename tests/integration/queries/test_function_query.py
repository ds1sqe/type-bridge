"""
Integration tests for FunctionQuery against a real TypeDB instance.

Tests that FunctionQuery generates valid TypeQL that executes correctly.
"""

import pytest

from type_bridge import Database, Entity, TypeFlags
from type_bridge.attribute import AttributeFlags, Integer, String
from type_bridge.attribute.flags import Flag, Key
from type_bridge.expressions import FunctionQuery, ReturnType

# ============================================================================
# Test Schema
# ============================================================================


class ArtifactId(String):
    flags = AttributeFlags(name="artifact-id")


class ArtifactName(String):
    flags = AttributeFlags(name="artifact-name")


class ArtifactScore(Integer):
    flags = AttributeFlags(name="artifact-score")


class Artifact(Entity):
    flags = TypeFlags(name="test_artifact")
    artifact_id: ArtifactId = Flag(Key)
    name: ArtifactName
    score: ArtifactScore | None = None


# Schema with function definition
SCHEMA_WITH_FUNCTION = """
define
attribute artifact-id, value string;
attribute artifact-name, value string;
attribute artifact-score, value integer;

entity test_artifact,
    owns artifact-id @key,
    owns artifact-name,
    owns artifact-score;

fun count-test-artifacts() -> integer:
    match $a isa test_artifact;
    return count($a);

fun list-test-artifact-ids() -> { artifact-id }:
    match $a isa test_artifact, has artifact-id $id;
    return { $id };

fun get-artifacts-with-score($min_score: integer) -> { artifact-id, artifact-score }:
    match
        $a isa test_artifact,
        has artifact-id $id,
        has artifact-score $score;
        $score >= $min_score;
    return { $id, $score };
"""


# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def db(docker_typedb):
    """Provide database connection."""
    from typedb.driver import DriverOptions

    from tests.integration.conftest import TEST_DB_ADDRESS
    from type_bridge import Credentials, TypeDB

    # Create database if needed
    driver = TypeDB.driver(
        address=TEST_DB_ADDRESS,
        credentials=Credentials(username="admin", password="password"),
        driver_options=DriverOptions(is_tls_enabled=False),
    )
    db_name = "test_function_query"
    if driver.databases.contains(db_name):
        driver.databases.get(db_name).delete()
    driver.databases.create(db_name)
    driver.close()

    database = Database(address=TEST_DB_ADDRESS, database=db_name)
    database.connect()
    yield database
    database.close()


@pytest.fixture
def setup_schema_with_functions(db):
    """Set up schema with functions and insert test data."""
    # Apply the schema with functions directly using SCHEMA transaction
    with db.transaction("schema") as tx:
        tx.execute(SCHEMA_WITH_FUNCTION)
        tx.commit()

    # Insert test data
    manager = Artifact.manager(db)
    test_artifacts = [
        Artifact(
            artifact_id=ArtifactId("art-001"),
            name=ArtifactName("First Artifact"),
            score=ArtifactScore(85),
        ),
        Artifact(
            artifact_id=ArtifactId("art-002"),
            name=ArtifactName("Second Artifact"),
            score=ArtifactScore(92),
        ),
        Artifact(
            artifact_id=ArtifactId("art-003"),
            name=ArtifactName("Third Artifact"),
            score=ArtifactScore(78),
        ),
        Artifact(
            artifact_id=ArtifactId("art-004"),
            name=ArtifactName("Fourth Artifact"),
            score=ArtifactScore(95),
        ),
        Artifact(
            artifact_id=ArtifactId("art-005"),
            name=ArtifactName("Fifth Artifact"),
            score=ArtifactScore(67),
        ),
    ]
    manager.insert_many(test_artifacts)

    yield db


# ============================================================================
# Tests: Simple Count Function
# ============================================================================


class TestCountFunction:
    """Tests for simple count function."""

    def test_count_function_query_generation(self) -> None:
        """FunctionQuery generates correct TypeQL for count function."""
        fn = FunctionQuery(
            name="count-test-artifacts",
            return_type=ReturnType(["integer"]),
        )

        query = fn.to_query()
        # AST-based output may have different formatting (newlines)
        assert "let $integer = count-test-artifacts();" in query
        assert '"integer": $integer' in query

    @pytest.mark.integration
    def test_count_function_executes(self, setup_schema_with_functions) -> None:
        """Count function executes and returns correct count."""
        db = setup_schema_with_functions

        fn = FunctionQuery(
            name="count-test-artifacts",
            return_type=ReturnType(["integer"]),
        )

        query = fn.to_query()

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 1
        # The result should contain the count
        result = results[0]
        assert "integer" in result
        assert result["integer"] == 5


# ============================================================================
# Tests: Stream Function
# ============================================================================


class TestStreamFunction:
    """Tests for stream function returning multiple rows."""

    def test_stream_function_query_generation(self) -> None:
        """FunctionQuery generates correct TypeQL for stream function."""
        fn = FunctionQuery(
            name="list-test-artifact-ids",
            return_type=ReturnType(["artifact-id"], is_stream=True),
        )

        query = fn.to_query()
        # AST-based output may have different formatting (newlines)
        assert "let $artifact_id in list-test-artifact-ids();" in query
        assert '"artifact_id": $artifact_id' in query

    def test_stream_function_with_limit(self) -> None:
        """Stream function with limit clause."""
        fn = FunctionQuery(
            name="list-test-artifact-ids",
            return_type=ReturnType(["artifact-id"], is_stream=True),
        )

        query = fn.to_query(limit=3)
        assert "limit 3;" in query

    @pytest.mark.integration
    def test_stream_function_executes(self, setup_schema_with_functions) -> None:
        """Stream function executes and returns multiple rows."""
        db = setup_schema_with_functions

        fn = FunctionQuery(
            name="list-test-artifact-ids",
            return_type=ReturnType(["artifact-id"], is_stream=True),
        )

        query = fn.to_query()

        with db.transaction() as tx:
            results = list(tx.execute(query))

        # Should return 5 artifact IDs
        assert len(results) == 5
        ids = {r["artifact_id"] for r in results}
        assert "art-001" in ids
        assert "art-005" in ids

    @pytest.mark.integration
    def test_stream_function_with_limit_executes(self, setup_schema_with_functions) -> None:
        """Stream function with limit returns correct number of results."""
        db = setup_schema_with_functions

        fn = FunctionQuery(
            name="list-test-artifact-ids",
            return_type=ReturnType(["artifact-id"], is_stream=True),
        )

        query = fn.to_query(limit=2)

        with db.transaction() as tx:
            results = list(tx.execute(query))

        assert len(results) == 2


# ============================================================================
# Tests: Parameterized Composite Function
# ============================================================================


class TestCompositeFunction:
    """Tests for parameterized function with composite return type."""

    def test_composite_function_query_generation(self) -> None:
        """FunctionQuery generates correct TypeQL for composite return.

        TypeQL 3.x uses comma-separated variables without parentheses:
        let $a, $b in func();  -- correct
        let ($a, $b) in func();  -- WRONG, causes syntax error
        """
        fn = FunctionQuery(
            name="get-artifacts-with-score",
            args=[("$min_score", 80)],
            return_type=ReturnType(["artifact-id", "artifact-score"], is_stream=True),
        )

        query = fn.to_query()
        assert "get-artifacts-with-score(80)" in query
        # Verify correct let syntax without parentheses
        assert "let $artifact_id, $artifact_score in get-artifacts-with-score(80);" in query
        # Explicitly verify no parentheses around variables
        assert "let ($artifact_id" not in query
        assert '"artifact_id": $artifact_id' in query
        assert '"artifact_score": $artifact_score' in query

    @pytest.mark.integration
    def test_composite_function_executes(self, setup_schema_with_functions) -> None:
        """Composite function executes and returns tuples."""
        db = setup_schema_with_functions

        fn = FunctionQuery(
            name="get-artifacts-with-score",
            args=[("$min_score", 80)],
            return_type=ReturnType(["artifact-id", "artifact-score"], is_stream=True),
        )

        query = fn.to_query()

        with db.transaction() as tx:
            results = list(tx.execute(query))

        # Should return artifacts with score >= 80 (art-001:85, art-002:92, art-004:95)
        assert len(results) == 3
        scores = {r["artifact_score"] for r in results}
        assert all(s >= 80 for s in scores)


# ============================================================================
# Tests: Query Modifiers
# ============================================================================


class TestQueryModifiers:
    """Tests for query modifiers (sort, offset, limit)."""

    def test_sort_query_generation(self) -> None:
        """Sort modifier in query."""
        fn = FunctionQuery(
            name="get-artifacts-with-score",
            args=[("$min_score", 0)],
            return_type=ReturnType(["artifact-id", "artifact-score"], is_stream=True),
        )

        query = fn.to_query(sort_var="artifact_score", sort_order="desc")
        assert "sort $artifact_score desc;" in query

    def test_pagination_query_generation(self) -> None:
        """Offset and limit in query."""
        fn = FunctionQuery(
            name="list-test-artifact-ids",
            return_type=ReturnType(["artifact-id"], is_stream=True),
        )

        query = fn.to_query(offset=2, limit=3)
        # Offset must come before limit
        assert query.index("offset 2;") < query.index("limit 3;")
