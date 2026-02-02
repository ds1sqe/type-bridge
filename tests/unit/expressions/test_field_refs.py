"""Unit tests for field reference system."""

from type_bridge import Entity, TypeFlags
from type_bridge.attribute import Double, Integer, String
from type_bridge.attribute.flags import Flag, Key
from type_bridge.expressions import AggregateExpr, ComparisonExpr, StringExpr
from type_bridge.fields import NumericFieldRef, StringFieldRef


# Test attribute types
class Name(String):
    pass


class Age(Integer):
    pass


class Score(Double):
    pass


# Test entity
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age
    score: Score


class TestFieldReferences:
    """Test field reference creation and type safety."""

    def test_class_level_access_returns_field_ref(self):
        """Class-level access should return FieldRef."""
        # String field returns StringFieldRef
        assert isinstance(Person.c.name, StringFieldRef)
        assert Person.c.name.field_name == "name"
        assert Person.c.name.attr_type is Name

        # Numeric fields return NumericFieldRef
        assert isinstance(Person.c.age, NumericFieldRef)
        assert Person.c.age.field_name == "age"
        assert Person.c.age.attr_type is Age

        assert isinstance(Person.c.score, NumericFieldRef)
        assert Person.c.score.field_name == "score"
        assert Person.c.score.attr_type is Score

    def test_instance_level_access_returns_value(self):
        """Instance-level access should return attribute value."""
        person = Person(
            name=Name("Alice"),
            age=Age(30),
            score=Score(95.5),
        )

        # Instance access returns attribute values
        assert isinstance(person.name, Name)
        assert person.name.value == "Alice"

        assert isinstance(person.age, Age)
        assert person.age.value == 30

        assert isinstance(person.score, Score)
        assert person.score.value == 95.5

    def test_comparison_methods_return_typed_expressions(self):
        """Comparison methods should return ComparisonExpr with correct types."""
        # Greater than
        expr_gt = Person.c.age.gt(Age(30))
        assert isinstance(expr_gt, ComparisonExpr)
        assert expr_gt.attr_type is Age
        assert expr_gt.operator == ">"
        assert expr_gt.value.value == 30

        # Less than
        expr_lt = Person.c.score.lt(Score(90.0))
        assert isinstance(expr_lt, ComparisonExpr)
        assert expr_lt.operator == "<"

        # Greater than or equal
        expr_gte = Person.c.age.gte(Age(18))
        assert isinstance(expr_gte, ComparisonExpr)
        assert expr_gte.operator == ">="

        # Less than or equal
        expr_lte = Person.c.age.lte(Age(65))
        assert isinstance(expr_lte, ComparisonExpr)
        assert expr_lte.operator == "<="

        # Equal
        expr_eq = Person.c.age.eq(Age(30))
        assert isinstance(expr_eq, ComparisonExpr)
        assert expr_eq.operator == "=="

        # Not equal
        expr_neq = Person.c.age.neq(Age(30))
        assert isinstance(expr_neq, ComparisonExpr)
        assert expr_neq.operator == "!="

    def test_string_methods_return_string_expressions(self):
        """String field methods should return StringExpr."""
        # Contains
        expr_contains = Person.c.name.contains(Name("Alice"))
        assert isinstance(expr_contains, StringExpr)
        assert expr_contains.attr_type is Name
        assert expr_contains.operation == "contains"
        assert expr_contains.pattern.value == "Alice"

        # Like (regex)
        expr_like = Person.c.name.like(Name("A.*"))
        assert isinstance(expr_like, StringExpr)
        assert expr_like.operation == "like"

        # Regex (alias for like)
        expr_regex = Person.c.name.regex(Name("^A"))
        assert isinstance(expr_regex, StringExpr)
        assert expr_regex.operation == "regex"

    def test_numeric_aggregation_methods(self):
        """Numeric field aggregation methods should return AggregateExpr."""
        # Sum
        expr_sum = Person.c.age.sum()
        assert isinstance(expr_sum, AggregateExpr)
        assert expr_sum.attr_type is Age
        assert expr_sum.function == "sum"

        # Average (internally uses 'mean' to match TypeDB 3.x)
        expr_avg = Person.c.score.avg()
        assert isinstance(expr_avg, AggregateExpr)
        assert expr_avg.function == "mean"

        # Max
        expr_max = Person.c.age.max()
        assert isinstance(expr_max, AggregateExpr)
        assert expr_max.function == "max"

        # Min
        expr_min = Person.c.score.min()
        assert isinstance(expr_min, AggregateExpr)
        assert expr_min.function == "min"

        # Median
        expr_median = Person.c.age.median()
        assert isinstance(expr_median, AggregateExpr)
        assert expr_median.function == "median"

        # Standard deviation
        expr_std = Person.c.score.std()
        assert isinstance(expr_std, AggregateExpr)
        assert expr_std.function == "std"

    def test_string_field_does_not_have_numeric_methods(self):
        """String fields should not have numeric aggregation methods."""
        # String fields don't have sum, avg, etc.
        assert not hasattr(Person.c.name, "sum")
        assert not hasattr(Person.c.name, "avg")

    def test_numeric_field_does_not_have_string_methods(self):
        """Numeric fields should not have string-specific methods."""
        # Numeric fields don't have contains, like, regex
        # (they have the base FieldRef type, not StringFieldRef)
        # Note: They're NumericFieldRef, which doesn't add these methods
        assert not hasattr(Person.c.age, "contains")
        assert not hasattr(Person.c.age, "like")
        assert not hasattr(Person.c.age, "regex")


class TestExpressionChaining:
    """Test expression composition with boolean operators."""

    def test_and_chaining(self):
        """Test AND composition of expressions."""
        expr1 = Person.c.age.gt(Age(18))
        expr2 = Person.c.age.lt(Age(65))
        combined = expr1.and_(expr2)

        from type_bridge.expressions import BooleanExpr

        assert isinstance(combined, BooleanExpr)
        assert combined.operation == "and"
        assert len(combined.operands) == 2
        assert expr1 in combined.operands
        assert expr2 in combined.operands

    def test_or_chaining(self):
        """Test OR composition of expressions."""
        expr1 = Person.c.age.lt(Age(20))
        expr2 = Person.c.age.gt(Age(40))
        combined = expr1.or_(expr2)

        from type_bridge.expressions import BooleanExpr

        assert isinstance(combined, BooleanExpr)
        assert combined.operation == "or"
        assert len(combined.operands) == 2

    def test_not_negation(self):
        """Test NOT negation of expressions."""
        expr = Person.c.age.eq(Age(30))
        negated = expr.not_()

        from type_bridge.expressions import BooleanExpr

        assert isinstance(negated, BooleanExpr)
        assert negated.operation == "not"
        assert len(negated.operands) == 1
        assert negated.operands[0] is expr

    def test_complex_boolean_composition(self):
        """Test complex boolean expressions."""
        # (age > 18 AND age < 65) OR score > 90
        expr1 = Person.c.age.gt(Age(18)).and_(Person.c.age.lt(Age(65)))
        expr2 = Person.c.score.gt(Score(90.0))
        complex_expr = expr1.or_(expr2)

        from type_bridge.expressions import BooleanExpr

        assert isinstance(complex_expr, BooleanExpr)
        assert complex_expr.operation == "or"
        # First operand should be the AND expression
        assert isinstance(complex_expr.operands[0], BooleanExpr)
        assert complex_expr.operands[0].operation == "and"


class TestExpressionToTypeQL:
    """Tests for TypeQL generation from expressions."""

    def test_comparison_to_typeql(self):
        """Test comparison expression TypeQL generation."""
        expr = Person.c.age.gt(Age(30))
        pattern = expr.to_typeql("$p")

        # Variable names include entity prefix to avoid collisions
        assert "$p has Age $p_age" in pattern
        assert "$p_age > 30" in pattern

    def test_string_contains_to_typeql(self):
        """Test string contains TypeQL generation."""
        expr = Person.c.name.contains(Name("Alice"))
        pattern = expr.to_typeql("$p")

        # Variable names include entity prefix to avoid collisions
        assert "$p has Name $p_name" in pattern
        assert '$p_name contains "Alice"' in pattern

    def test_string_like_to_typeql(self):
        """Test string like (regex) TypeQL generation."""
        expr = Person.c.name.like(Name("A.*"))
        pattern = expr.to_typeql("$p")

        # Variable names include entity prefix to avoid collisions
        assert "$p has Name $p_name" in pattern
        assert '$p_name like "A.*"' in pattern

    def test_boolean_and_to_typeql(self):
        """Test AND expression TypeQL generation."""
        expr = Person.c.age.gt(Age(18)).and_(Person.c.age.lt(Age(65)))
        pattern = expr.to_typeql("$p")

        # AND just concatenates patterns; variable names include entity prefix
        assert "$p has Age $p_age" in pattern
        assert "$p_age > 18" in pattern
        assert "$p_age < 65" in pattern

    def test_boolean_or_to_typeql(self):
        """Test OR expression TypeQL generation."""
        expr = Person.c.age.lt(Age(20)).or_(Person.c.age.gt(Age(40)))
        pattern = expr.to_typeql("$p")

        # OR creates disjunction blocks
        # Variable names include entity prefix
        assert "{" in pattern and "}" in pattern
        assert " or " in pattern
        assert "$p_age < 20" in pattern
        assert "$p_age > 40" in pattern

    def test_boolean_not_to_typeql(self):
        """Test NOT expression TypeQL generation."""
        expr = Person.c.age.eq(Age(30)).not_()
        pattern = expr.to_typeql("$p")

        # NOT creates negation block; variable names include entity prefix
        assert "not {" in pattern
        assert "}" in pattern
        assert "$p_age == 30" in pattern

    def test_aggregate_to_typeql(self):
        """Test aggregate expression TypeQL generation."""
        expr = Person.c.age.avg()
        pattern = expr.to_typeql("$p")

        # TypeDB 3.x uses 'mean' instead of 'avg'
        assert "mean($p_age)" in pattern

        # Count doesn't need attr_type
        from type_bridge.expressions import AggregateExpr

        count_expr = AggregateExpr(attr_type=None, function="count")
        count_pattern = count_expr.to_typeql("$p")
        assert "count($p)" in count_pattern


class TestFieldDescriptorValidation:
    """Test field descriptor behavior and edge cases."""

    def test_none_value_handling(self):
        """Test handling of None values in optional fields."""

        # Create person with explicit None for age (optional field)
        class OptionalAgePerson(Entity):
            flags = TypeFlags(name="person2")
            name: Name = Flag(Key)
            age: Age | None

        # Explicitly pass None for optional field
        person = OptionalAgePerson(name=Name("Bob"), age=None)
        # When None is explicitly passed, it should be None
        assert person.age is None

    def test_field_assignment_after_creation(self):
        """Test field assignment after entity creation."""
        person = Person(name=Name("Alice"), age=Age(25), score=Score(85.0))

        # Update field value
        person.age = Age(26)
        assert person.age.value == 26

        person.score = Score(90.0)
        assert person.score.value == 90.0
