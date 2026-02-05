"""Test range validation for Integer and Double attributes."""

from typing import ClassVar

import pytest

from type_bridge import Double, Entity, Integer, String, TypeFlags


class TestIntegerRangeValidation:
    """Test range validation for Integer attributes."""

    def test_integer_in_range(self):
        """Test that values within range are accepted."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        age = Age(30)
        assert age.value == 30

    def test_integer_at_min_boundary(self):
        """Test that value at minimum boundary is accepted."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        age = Age(0)
        assert age.value == 0

    def test_integer_at_max_boundary(self):
        """Test that value at maximum boundary is accepted."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        age = Age(150)
        assert age.value == 150

    def test_integer_below_min_raises_error(self):
        """Test that values below minimum raise ValueError."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        with pytest.raises(ValueError, match="below minimum"):
            Age(-1)

    def test_integer_above_max_raises_error(self):
        """Test that values above maximum raise ValueError."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        with pytest.raises(ValueError, match="above maximum"):
            Age(200)

    def test_integer_min_only(self):
        """Test range with only minimum constraint."""

        class Score(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", None)

        # Valid: above minimum
        score = Score(1000000)
        assert score.value == 1000000

        # Invalid: below minimum
        with pytest.raises(ValueError, match="below minimum"):
            Score(-5)

    def test_integer_max_only(self):
        """Test range with only maximum constraint."""

        class Priority(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = (None, "10")

        # Valid: below maximum
        priority = Priority(-100)
        assert priority.value == -100

        # Invalid: above maximum
        with pytest.raises(ValueError, match="above maximum"):
            Priority(15)

    def test_integer_in_entity_validation(self):
        """Test range validation when Integer is used in an Entity."""

        class Name(String):
            pass

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name
            age: Age

        # Valid age
        person = Person(name=Name("Alice"), age=Age(30))
        assert person.age.value == 30

        # Invalid age in entity creation
        with pytest.raises(ValueError, match="above maximum"):
            Person(name=Name("Bob"), age=Age(200))


class TestDoubleRangeValidation:
    """Test range validation for Double attributes."""

    def test_double_in_range(self):
        """Test that values within range are accepted."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-273.15", "1000.0")

        temp = Temperature(25.5)
        assert temp.value == 25.5

    def test_double_at_min_boundary(self):
        """Test that value at minimum boundary is accepted."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-273.15", "1000.0")

        temp = Temperature(-273.15)
        assert temp.value == -273.15

    def test_double_at_max_boundary(self):
        """Test that value at maximum boundary is accepted."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-273.15", "1000.0")

        temp = Temperature(1000.0)
        assert temp.value == 1000.0

    def test_double_below_min_raises_error(self):
        """Test that values below minimum raise ValueError."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-273.15", "1000.0")

        with pytest.raises(ValueError, match="below minimum"):
            Temperature(-300.0)

    def test_double_above_max_raises_error(self):
        """Test that values above maximum raise ValueError."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-273.15", "1000.0")

        with pytest.raises(ValueError, match="above maximum"):
            Temperature(2000.0)

    def test_double_min_only(self):
        """Test range with only minimum constraint."""

        class Price(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0.0", None)

        # Valid: above minimum
        price = Price(99.99)
        assert price.value == 99.99

        # Invalid: below minimum
        with pytest.raises(ValueError, match="below minimum"):
            Price(-0.01)

    def test_double_max_only(self):
        """Test range with only maximum constraint."""

        class Discount(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = (None, "1.0")

        # Valid: below maximum
        discount = Discount(0.5)
        assert discount.value == 0.5

        # Invalid: above maximum
        with pytest.raises(ValueError, match="above maximum"):
            Discount(1.5)

    def test_double_in_entity_validation(self):
        """Test range validation when Double is used in an Entity."""

        class Name(String):
            pass

        class Salary(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0.0", "10000000.0")

        class Employee(Entity):
            flags = TypeFlags(name="employee")
            name: Name
            salary: Salary

        # Valid salary
        employee = Employee(name=Name("Alice"), salary=Salary(75000.0))
        assert employee.salary.value == 75000.0

        # Invalid salary in entity creation
        with pytest.raises(ValueError, match="above maximum"):
            Employee(name=Name("Bob"), salary=Salary(50000000.0))


class TestNoRangeConstraint:
    """Test that attributes without range_constraint work normally."""

    def test_integer_without_range(self):
        """Test that Integer without range accepts any value."""

        class ItemCount(Integer):
            pass

        # Should work with any values
        assert ItemCount(0).value == 0
        assert ItemCount(-1000000).value == -1000000
        assert ItemCount(1000000).value == 1000000

    def test_double_without_range(self):
        """Test that Double without range accepts any value."""

        class Metric(Double):
            pass

        # Should work with any values
        assert Metric(0.0).value == 0.0
        assert Metric(-1e308).value == -1e308
        assert Metric(1e308).value == 1e308
