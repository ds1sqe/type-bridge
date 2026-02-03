"""Integration tests for relations with abstract role types.

Tests that relations can use abstract entity types in role definitions
and successfully insert/query with concrete entity instances.
"""

import pytest

from type_bridge import Database, Entity, Integer, Relation, SchemaManager, String, TypeFlags
from type_bridge.attribute.flags import Flag, Key
from type_bridge.models.role import Role


# Abstract base entity with key attribute
class TokenText(String):
    pass


class TokenNote(String):
    pass


class Token(Entity):
    """Abstract base class for all logical tokens."""

    flags = TypeFlags(name="token", abstract=True)
    text: TokenText = Flag(Key)
    note: TokenNote | None = None  # Optional attribute for testing updates


# Concrete token types
class Symptom(Token):
    """Observed behavior or symptom."""

    flags = TypeFlags(name="symptom")


class Problem(Token):
    """Identified problem."""

    flags = TypeFlags(name="problem")


class Hypothesis(Token):
    """Proposed hypothesis."""

    flags = TypeFlags(name="hypothesis")


# Another entity for relations
class IssueKey(String):
    pass


class Issue(Entity):
    """Simple issue entity."""

    flags = TypeFlags(name="issue")
    key: IssueKey = Flag(Key)


# Relation using abstract type in role definition
class TokenOrigin(Relation):
    """Token was extracted from an issue."""

    flags = TypeFlags(name="token_origin")
    token: Role[Token] = Role("token", Token)  # Abstract type in role!
    issue: Role[Issue] = Role("issue", Issue)


# Additional attributes for update tests
class Confidence(Integer):
    pass


class TokenOriginWithConfidence(Relation):
    """Token origin with confidence score."""

    flags = TypeFlags(name="token_origin_conf")
    token: Role[Token] = Role("token", Token)
    issue: Role[Issue] = Role("issue", Issue)
    confidence: Confidence | None = None


@pytest.mark.integration
@pytest.mark.order(60)
def test_insert_relation_with_abstract_role_type(clean_db: Database):
    """Test inserting relations with abstract entity types in role definitions."""
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Token, Symptom, Problem, Hypothesis, Issue, TokenOrigin)
    schema_manager.sync_schema()

    # Create and insert entities
    issue = Issue(key=IssueKey("TEST-1"))
    symptom = Symptom(text=TokenText("fever"))
    problem = Problem(text=TokenText("infection"))

    with clean_db:
        issue.insert(clean_db)
        symptom.insert(clean_db)
        problem.insert(clean_db)

    # Insert relations using different concrete token types
    origin1 = TokenOrigin(token=symptom, issue=issue)
    origin2 = TokenOrigin(token=problem, issue=issue)

    with clean_db:
        origin1.insert(clean_db)
        origin2.insert(clean_db)

    # Verify relations were inserted by querying them back
    manager = TokenOrigin.manager(clean_db)
    all_origins = manager.all()

    assert len(all_origins) == 2
    # Check that we got both token types
    token_texts = {origin.token.text.value for origin in all_origins}
    assert token_texts == {"fever", "infection"}

    # Enhanced: Retrieve each relation individually by role player
    symptom_origins = manager.get(token=symptom)
    assert len(symptom_origins) == 1
    assert symptom_origins[0].token.text.value == "fever"
    assert symptom_origins[0].issue.key.value == "TEST-1"

    problem_origins = manager.get(token=problem)
    assert len(problem_origins) == 1
    assert problem_origins[0].token.text.value == "infection"
    assert problem_origins[0].issue.key.value == "TEST-1"

    # Enhanced: Verify filtering by issue
    issue_origins = manager.get(issue=issue)
    assert len(issue_origins) == 2


@pytest.mark.integration
@pytest.mark.order(61)
def test_query_relation_by_abstract_role_player(clean_db: Database):
    """Test querying relations by role players that inherit from abstract types."""
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Token, Symptom, Problem, Issue, TokenOrigin)
    schema_manager.sync_schema()

    # Create test data
    issue1 = Issue(key=IssueKey("ISSUE-1"))
    issue2 = Issue(key=IssueKey("ISSUE-2"))
    symptom = Symptom(text=TokenText("headache"))
    problem = Problem(text=TokenText("dehydration"))

    with clean_db:
        issue1.insert(clean_db)
        issue2.insert(clean_db)
        symptom.insert(clean_db)
        problem.insert(clean_db)

    # Create relations
    origin1 = TokenOrigin(token=symptom, issue=issue1)
    origin2 = TokenOrigin(token=problem, issue=issue2)

    manager = TokenOrigin.manager(clean_db)
    with clean_db:
        manager.insert_many([origin1, origin2])

    # Query by role player (entity with inherited key attribute)
    origins_for_symptom = manager.get(token=symptom)
    assert len(origins_for_symptom) == 1
    assert origins_for_symptom[0].token.text.value == "headache"
    assert origins_for_symptom[0].issue.key.value == "ISSUE-1"

    origins_for_problem = manager.get(token=problem)
    assert len(origins_for_problem) == 1
    assert origins_for_problem[0].token.text.value == "dehydration"
    assert origins_for_problem[0].issue.key.value == "ISSUE-2"

    # Enhanced: Test manager.all() returns all relations
    all_origins = manager.all()
    assert len(all_origins) == 2
    all_texts = {origin.token.text.value for origin in all_origins}
    assert all_texts == {"headache", "dehydration"}

    # Enhanced: Query by issue role player
    issue1_origins = manager.get(issue=issue1)
    assert len(issue1_origins) == 1
    assert issue1_origins[0].token.text.value == "headache"


@pytest.mark.integration
@pytest.mark.order(62)
def test_insert_many_with_abstract_role_types(clean_db: Database):
    """Test bulk inserting relations with abstract role types."""
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Token, Symptom, Problem, Hypothesis, Issue, TokenOrigin)
    schema_manager.sync_schema()

    # Create entities
    issue = Issue(key=IssueKey("BULK-1"))
    symptom = Symptom(text=TokenText("cough"))
    problem = Problem(text=TokenText("virus"))
    hypothesis = Hypothesis(text=TokenText("seasonal-flu"))

    with clean_db:
        issue.insert(clean_db)
        symptom.insert(clean_db)
        problem.insert(clean_db)
        hypothesis.insert(clean_db)

    # Bulk insert relations with different concrete token types
    origins = [
        TokenOrigin(token=symptom, issue=issue),
        TokenOrigin(token=problem, issue=issue),
        TokenOrigin(token=hypothesis, issue=issue),
    ]

    manager = TokenOrigin.manager(clean_db)
    with clean_db:
        manager.insert_many(origins)

    # Verify all were inserted
    all_origins = manager.all()
    assert len(all_origins) == 3

    # Verify all three concrete token types are present
    token_texts = {origin.token.text.value for origin in all_origins}
    assert token_texts == {"cough", "virus", "seasonal-flu"}

    # Enhanced: Retrieve each relation individually by specific token
    symptom_origins = manager.get(token=symptom)
    assert len(symptom_origins) == 1
    assert symptom_origins[0].token.text.value == "cough"

    problem_origins = manager.get(token=problem)
    assert len(problem_origins) == 1
    assert problem_origins[0].token.text.value == "virus"

    hypothesis_origins = manager.get(token=hypothesis)
    assert len(hypothesis_origins) == 1
    assert hypothesis_origins[0].token.text.value == "seasonal-flu"

    # Enhanced: Verify all have the same issue
    for origin in all_origins:
        assert origin.issue.key.value == "BULK-1"


@pytest.mark.integration
@pytest.mark.order(63)
def test_update_entity_with_inherited_attributes(clean_db: Database):
    """Test updating entities that inherit optional attributes from abstract parents."""
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Token, Symptom, Problem, Issue, TokenOrigin)
    schema_manager.sync_schema()

    # Insert entity with inherited key attribute
    symptom = Symptom(text=TokenText("fever"))
    with clean_db:
        symptom.insert(clean_db)

    # Retrieve and verify initial state
    symptom_manager = Symptom.manager(clean_db)
    retrieved = symptom_manager.get(text="fever")
    assert len(retrieved) == 1
    assert retrieved[0].text.value == "fever"
    assert retrieved[0].note is None

    # Update the inherited optional attribute
    symptom_to_update = retrieved[0]
    symptom_to_update.note = TokenNote("patient-reported")

    with clean_db:
        symptom_manager.update(symptom_to_update)

    # Verify update persisted
    updated = symptom_manager.get(text="fever")
    assert len(updated) == 1
    assert updated[0].text.value == "fever"  # Key unchanged
    assert updated[0].note is not None
    assert updated[0].note.value == "patient-reported"  # Note updated

    # Update note again
    symptom_to_update = updated[0]
    symptom_to_update.note = TokenNote("doctor-confirmed")

    with clean_db:
        symptom_manager.update(symptom_to_update)

    # Verify second update persisted
    final = symptom_manager.get(text="fever")
    assert len(final) == 1
    assert final[0].note is not None
    assert final[0].note.value == "doctor-confirmed"


@pytest.mark.integration
@pytest.mark.order(64)
def test_get_relations_comprehensive_filtering(clean_db: Database):
    """Test comprehensive filtering of relations with abstract role types."""
    # Setup schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Token, Symptom, Problem, Hypothesis, Issue, TokenOriginWithConfidence)
    schema_manager.sync_schema()

    # Create test data
    issue1 = Issue(key=IssueKey("COMP-1"))
    issue2 = Issue(key=IssueKey("COMP-2"))
    symptom1 = Symptom(text=TokenText("fever"))
    symptom2 = Symptom(text=TokenText("cough"))
    problem = Problem(text=TokenText("infection"))

    with clean_db:
        issue1.insert(clean_db)
        issue2.insert(clean_db)
        symptom1.insert(clean_db)
        symptom2.insert(clean_db)
        problem.insert(clean_db)

    # Insert relations with attributes
    origin1 = TokenOriginWithConfidence(token=symptom1, issue=issue1, confidence=Confidence(95))
    origin2 = TokenOriginWithConfidence(token=symptom2, issue=issue1, confidence=Confidence(80))
    origin3 = TokenOriginWithConfidence(token=problem, issue=issue2, confidence=Confidence(90))

    manager = TokenOriginWithConfidence.manager(clean_db)
    with clean_db:
        manager.insert_many([origin1, origin2, origin3])

    # Test: Get all relations
    all_origins = manager.all()
    assert len(all_origins) == 3

    # Test: Filter by specific role player (token)
    symptom1_origins = manager.get(token=symptom1)
    assert len(symptom1_origins) == 1
    assert symptom1_origins[0].token.text.value == "fever"
    assert symptom1_origins[0].confidence is not None
    assert symptom1_origins[0].confidence.value == 95

    # Test: Filter by issue role player
    issue1_origins = manager.get(issue=issue1)
    assert len(issue1_origins) == 2
    token_texts = {origin.token.text.value for origin in issue1_origins}
    assert token_texts == {"fever", "cough"}

    # Test: Verify each relation has correct attributes
    for origin in all_origins:
        assert origin.confidence is not None
        assert isinstance(origin.confidence.value, int)
        assert 80 <= origin.confidence.value <= 95
