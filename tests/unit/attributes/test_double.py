"""Test Double attribute type."""

from tests.utils.handwritten import Card, Double, Entity, Flag, Key, String, TypeFlags


def test_double_creation():
    """Test creating Double attribute."""

    class Score(Double):
        pass

    score = Score(95.5)
    assert score.value == 95.5
    assert isinstance(score, Double)


def test_double_value_type():
    """Test that Double has correct value_type for TypeDB."""

    class Score(Double):
        pass

    assert Score.value_type == "double"


def test_double_in_entity():
    """Test using Double in an entity."""

    class Name(String):
        pass

    class Score(Double):
        pass

    class Student(Entity):
        flags = TypeFlags(name="student")
        name: Name = Flag(Key)
        score: Score

    student = Student(name=Name("Alice"), score=Score(95.5))
    assert student.score.value == 95.5


def test_double_insert_query():
    """Test insert query generation with Double attributes."""

    class Score(Double):
        pass

    class TestResult(Entity):
        flags = TypeFlags(name="test-result")
        score: Score

    # Test with float value
    result = TestResult(score=Score(95.5))
    query = result.to_insert_query()

    assert "$e isa test-result" in query
    assert "has Score 95.5" in query
    # Doubles should NOT be quoted
    assert '"95.5"' not in query


def test_double_edge_cases():
    """Test insert query generation with Double edge cases."""

    class Reading(Double):
        pass

    class Measurement(Entity):
        flags = TypeFlags(name="measurement")
        value: Reading

    # Test with zero
    measurement_zero = Measurement(value=Reading(0.0))
    query_zero = measurement_zero.to_insert_query()
    assert "has Reading 0.0" in query_zero

    # Test with negative float
    measurement_negative = Measurement(value=Reading(-3.14))
    query_negative = measurement_negative.to_insert_query()
    assert "has Reading -3.14" in query_negative

    # Test with scientific notation value
    measurement_small = Measurement(value=Reading(0.000001))
    query_small = measurement_small.to_insert_query()
    # Python will format this in scientific notation or decimal
    assert "has Reading" in query_small


def test_double_optional_attribute():
    """Test Double as optional attribute."""

    class Name(String):
        pass

    class Score(Double):
        pass

    class Student(Entity):
        flags = TypeFlags(name="student")
        name: Name = Flag(Key)
        score: Score | None

    # Test with None
    student = Student(name=Name("Alice"), score=None)
    query = student.to_insert_query()
    assert 'has Name "Alice"' in query
    assert "has Score" not in query

    # Test with value
    student2 = Student(name=Name("Bob"), score=Score(87.5))
    query2 = student2.to_insert_query()
    assert "has Score 87.5" in query2


def test_multi_value_double_insert_query():
    """Test insert query generation with multi-value Double attributes."""

    class Score(Double):
        pass

    class Student(Entity):
        flags = TypeFlags(name="student")
        scores: list[Score] = Flag(Card(min=1))

    # Create entity with multiple scores
    student = Student(scores=[Score(95.5), Score(87.3), Score(92.0)])
    query = student.to_insert_query()

    assert "$e isa student" in query
    assert "has Score 95.5" in query
    assert "has Score 87.3" in query
    assert "has Score 92.0" in query


def test_double_comparison():
    """Test Double attribute comparison."""

    class Score(Double):
        pass

    # Same value
    s1 = Score(95.5)
    s2 = Score(95.5)
    assert s1 == s2

    # Different values
    s3 = Score(87.3)
    assert s1 != s3


def test_double_string_representation():
    """Test string representation of Double attributes."""

    class Score(Double):
        pass

    score = Score(95.5)

    # Test __repr__
    assert "Score" in repr(score)
    assert "95.5" in repr(score)


def test_double_precision():
    """Test Double preserves floating-point precision."""

    class Measurement(Double):
        pass

    # Test with high precision value
    value = Measurement(3.141592653589793)
    assert value.value == 3.141592653589793


def test_double_large_numbers():
    """Test Double with large numbers."""

    class Amount(Double):
        pass

    # Test with large number
    large = Amount(1e15)
    assert large.value == 1e15

    # Test with very small number
    small = Amount(1e-15)
    assert small.value == 1e-15
