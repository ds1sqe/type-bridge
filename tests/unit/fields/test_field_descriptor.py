"""Unit tests for FieldDescriptor and FieldRef classes."""

from type_bridge import Entity, TypeFlags
from type_bridge.attribute import Boolean, DateTime, Double, Integer, String
from type_bridge.attribute.decimal import Decimal
from type_bridge.attribute.flags import Flag, Key
from type_bridge.fields import (
    FieldDescriptor,
    FieldRef,
    NumericFieldRef,
    StringFieldRef,
)


# Test attribute types
class Name(String):
    pass


class Age(Integer):
    pass


class Score(Double):
    pass


class Price(Decimal):
    pass


class IsActive(Boolean):
    pass


class CreatedAt(DateTime):
    pass


class TestFieldDescriptorInit:
    """Tests for FieldDescriptor initialization."""

    def test_descriptor_stores_field_name(self):
        """Descriptor should store the field name."""
        descriptor = FieldDescriptor("name", Name)
        assert descriptor.field_name == "name"

    def test_descriptor_stores_attr_type(self):
        """Descriptor should store the attribute type."""
        descriptor = FieldDescriptor("age", Age)
        assert descriptor.attr_type is Age


class TestFieldRefTypeSelection:
    """Tests for FieldDescriptor._make_field_ref type selection."""

    def test_string_attr_returns_string_field_ref(self):
        """String attribute should return StringFieldRef."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_str")
            name: Name = Flag(Key)

        ref = TestEntity.name
        assert isinstance(ref, StringFieldRef)
        assert ref.field_name == "name"
        assert ref.attr_type is Name

    def test_integer_attr_returns_numeric_field_ref(self):
        """Integer attribute should return NumericFieldRef."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_int")
            name: Name = Flag(Key)
            age: Age

        ref = TestEntity.age
        assert isinstance(ref, NumericFieldRef)
        assert ref.field_name == "age"
        assert ref.attr_type is Age

    def test_double_attr_returns_numeric_field_ref(self):
        """Double attribute should return NumericFieldRef."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_dbl")
            name: Name = Flag(Key)
            score: Score

        ref = TestEntity.score
        assert isinstance(ref, NumericFieldRef)
        assert ref.attr_type is Score

    def test_decimal_attr_returns_numeric_field_ref(self):
        """Decimal attribute should return NumericFieldRef."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_dec")
            name: Name = Flag(Key)
            price: Price

        ref = TestEntity.price
        assert isinstance(ref, NumericFieldRef)
        assert ref.attr_type is Price

    def test_boolean_attr_returns_base_field_ref(self):
        """Boolean attribute should return base FieldRef (not specialized)."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_bool")
            name: Name = Flag(Key)
            is_active: IsActive

        ref = TestEntity.is_active
        # Boolean returns base FieldRef, not StringFieldRef or NumericFieldRef
        assert isinstance(ref, FieldRef)
        assert not isinstance(ref, StringFieldRef)
        assert not isinstance(ref, NumericFieldRef)
        assert ref.attr_type is IsActive

    def test_datetime_attr_returns_base_field_ref(self):
        """DateTime attribute should return base FieldRef."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_entity_dt")
            name: Name = Flag(Key)
            created_at: CreatedAt

        ref = TestEntity.created_at
        assert isinstance(ref, FieldRef)
        assert not isinstance(ref, StringFieldRef)
        assert not isinstance(ref, NumericFieldRef)
        assert ref.attr_type is CreatedAt


class TestFieldRefEntityType:
    """Tests for FieldRef entity_type tracking."""

    def test_field_ref_stores_entity_type(self):
        """FieldRef should store the owning entity type (or metaclass)."""

        class Person(Entity):
            flags = TypeFlags(name="person_ref")
            name: Name = Flag(Key)

        ref = Person.name
        # entity_type stores the class or metaclass depending on implementation
        assert isinstance(ref, FieldRef)
        assert ref.entity_type is not None

    def test_own_field_refs_work(self):
        """Field references should work on entity's own fields."""

        class Employee(Entity):
            flags = TypeFlags(name="employee_ref_own")
            name: Name = Flag(Key)
            age: Age

        # Check own fields
        name_ref = Employee.name
        assert isinstance(name_ref, StringFieldRef)
        assert name_ref.field_name == "name"

        age_ref = Employee.age
        assert isinstance(age_ref, NumericFieldRef)
        assert age_ref.field_name == "age"


class TestFieldRefComparisons:
    """Tests for FieldRef comparison method delegation."""

    def test_lt_delegates_to_attr_type(self):
        """lt() should delegate to attribute type's lt method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_lt")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.lt(Age(30))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == "<"
        assert expr.attr_type is Age

    def test_gt_delegates_to_attr_type(self):
        """gt() should delegate to attribute type's gt method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_gt")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.gt(Age(18))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == ">"

    def test_lte_delegates_to_attr_type(self):
        """lte() should delegate to attribute type's lte method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_lte")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.lte(Age(65))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == "<="

    def test_gte_delegates_to_attr_type(self):
        """gte() should delegate to attribute type's gte method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_gte")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.gte(Age(18))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == ">="

    def test_eq_delegates_to_attr_type(self):
        """eq() should delegate to attribute type's eq method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_eq")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.eq(Age(30))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == "=="

    def test_neq_delegates_to_attr_type(self):
        """neq() should delegate to attribute type's neq method."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_neq")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import ComparisonExpr

        expr = TestEntity.age.neq(Age(0))
        assert isinstance(expr, ComparisonExpr)
        assert expr.operator == "!="


class TestStringFieldRefOperations:
    """Tests for StringFieldRef string-specific operations."""

    def test_contains_returns_string_expr(self):
        """contains() should return StringExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_contains")
            name: Name = Flag(Key)

        from type_bridge.expressions import StringExpr

        expr = TestEntity.name.contains(Name("test"))
        assert isinstance(expr, StringExpr)
        assert expr.operation == "contains"
        assert expr.attr_type is Name

    def test_like_returns_string_expr(self):
        """like() should return StringExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_like")
            name: Name = Flag(Key)

        from type_bridge.expressions import StringExpr

        expr = TestEntity.name.like(Name("A.*"))
        assert isinstance(expr, StringExpr)
        assert expr.operation == "like"

    def test_regex_returns_string_expr(self):
        """regex() should return StringExpr (alias for like)."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_regex")
            name: Name = Flag(Key)

        from type_bridge.expressions import StringExpr

        expr = TestEntity.name.regex(Name("^test"))
        assert isinstance(expr, StringExpr)
        assert expr.operation == "regex"


class TestNumericFieldRefOperations:
    """Tests for NumericFieldRef aggregation operations."""

    def test_sum_returns_aggregate_expr(self):
        """sum() should return AggregateExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_sum")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.age.sum()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "sum"
        assert expr.attr_type is Age
        assert expr.field_name == "age"

    def test_avg_returns_aggregate_expr_with_mean(self):
        """avg() should return AggregateExpr with 'mean' function."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_avg")
            name: Name = Flag(Key)
            score: Score

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.score.avg()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "mean"  # TypeDB uses 'mean' not 'avg'
        assert expr.attr_type is Score

    def test_max_returns_aggregate_expr(self):
        """max() should return AggregateExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_max")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.age.max()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "max"

    def test_min_returns_aggregate_expr(self):
        """min() should return AggregateExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_min")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.age.min()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "min"

    def test_median_returns_aggregate_expr(self):
        """median() should return AggregateExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_median")
            name: Name = Flag(Key)
            age: Age

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.age.median()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "median"

    def test_std_returns_aggregate_expr(self):
        """std() should return AggregateExpr."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_std")
            name: Name = Flag(Key)
            score: Score

        from type_bridge.expressions import AggregateExpr

        expr = TestEntity.score.std()
        assert isinstance(expr, AggregateExpr)
        assert expr.function == "std"


class TestFieldDescriptorInstanceAccess:
    """Tests for FieldDescriptor instance-level access."""

    def test_instance_access_returns_attribute_value(self):
        """Instance-level access should return the attribute value."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_inst_access")
            name: Name = Flag(Key)

        entity = TestEntity(name=Name("Alice"))
        assert isinstance(entity.name, Name)
        assert entity.name.value == "Alice"

    def test_instance_access_returns_none_for_optional(self):
        """Instance-level access should return None for unset optional fields."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_inst_opt")
            name: Name = Flag(Key)
            age: Age | None = None

        entity = TestEntity(name=Name("Bob"))
        # Optional field should be None when not set
        assert entity.age is None

    def test_field_set_updates_value(self):
        """Setting a field should update the value."""

        class TestEntity(Entity):
            flags = TypeFlags(name="test_inst_set")
            name: Name = Flag(Key)
            age: Age

        entity = TestEntity(name=Name("Charlie"), age=Age(25))
        entity.age = Age(26)
        assert entity.age.value == 26


class TestFieldRefDirectInstantiation:
    """Tests for direct FieldRef instantiation (edge cases)."""

    def test_field_ref_init(self):
        """FieldRef can be instantiated directly."""
        ref = FieldRef(field_name="test", attr_type=Age, entity_type=None)
        assert ref.field_name == "test"
        assert ref.attr_type is Age
        assert ref.entity_type is None

    def test_string_field_ref_init(self):
        """StringFieldRef can be instantiated directly."""
        ref = StringFieldRef(field_name="name", attr_type=Name, entity_type=None)
        assert ref.field_name == "name"
        assert ref.attr_type is Name

    def test_numeric_field_ref_init(self):
        """NumericFieldRef can be instantiated directly."""
        ref = NumericFieldRef(field_name="age", attr_type=Age, entity_type=None)
        assert ref.field_name == "age"
        assert ref.attr_type is Age
