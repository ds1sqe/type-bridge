"""Tests for TypeQL annotation generation in schema definitions."""

from typing import ClassVar

from type_bridge import Double, Integer, String


class TestRangeAnnotationGeneration:
    """Test @range annotation generation in schema definitions."""

    def test_integer_range_both_bounds(self) -> None:
        """Test @range with both min and max bounds."""

        class Age(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

        schema = Age.to_schema_definition()
        assert schema == "attribute Age, value integer @range(0..150);"

    def test_integer_range_min_only(self) -> None:
        """Test @range with only minimum bound."""

        class Score(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", None)

        schema = Score.to_schema_definition()
        assert schema == "attribute Score, value integer @range(0..);"

    def test_integer_range_max_only(self) -> None:
        """Test @range with only maximum bound."""

        class Priority(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = (None, "10")

        schema = Priority.to_schema_definition()
        assert schema == "attribute Priority, value integer @range(..10);"

    def test_double_range_both_bounds(self) -> None:
        """Test @range with double type and both bounds."""

        class Temperature(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("-50.0", "50.0")

        schema = Temperature.to_schema_definition()
        assert schema == "attribute Temperature, value double @range(-50.0..50.0);"

    def test_double_range_min_only(self) -> None:
        """Test @range with double type and only minimum."""

        class Price(Double):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0.0", None)

        schema = Price.to_schema_definition()
        assert schema == "attribute Price, value double @range(0.0..);"

    def test_no_range_no_annotation(self) -> None:
        """Test that attributes without range_constraint don't get @range."""

        class Name(String):
            pass

        schema = Name.to_schema_definition()
        assert schema == "attribute Name, value string;"
        assert "@range" not in schema


class TestRegexAnnotationGeneration:
    """Test @regex annotation generation in schema definitions."""

    def test_simple_regex(self) -> None:
        """Test @regex with a simple pattern."""

        class Email(String):
            regex_pattern = r"^[a-z]+@[a-z]+\.[a-z]+$"

        schema = Email.to_schema_definition()
        assert '@regex("^[a-z]+@[a-z]+\\.[a-z]+$")' in schema

    def test_regex_with_special_chars(self) -> None:
        """Test @regex with special characters."""

        class PhoneNumber(String):
            regex_pattern = r"^\+?[0-9]{10,14}$"

        schema = PhoneNumber.to_schema_definition()
        assert '@regex("^\\+?[0-9]{10,14}$")' in schema


class TestValuesAnnotationGeneration:
    """Test @values annotation generation in schema definitions."""

    def test_simple_values(self) -> None:
        """Test @values with simple string values."""

        class Status(String):
            allowed_values: ClassVar[tuple[str, ...]] = ("active", "inactive")

        schema = Status.to_schema_definition()
        assert schema == 'attribute Status, value string @values("active", "inactive");'

    def test_multiple_values(self) -> None:
        """Test @values with multiple values."""

        class Priority(String):
            allowed_values: ClassVar[tuple[str, ...]] = ("low", "medium", "high", "critical")

        schema = Priority.to_schema_definition()
        assert '@values("low", "medium", "high", "critical")' in schema


class TestMultipleAnnotations:
    """Test combining multiple annotations."""

    def test_range_and_other_annotations(self) -> None:
        """Test that range can be combined with other properties."""

        class BoundedInteger(Integer):
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("1", "100")

        schema = BoundedInteger.to_schema_definition()
        assert schema == "attribute BoundedInteger, value integer @range(1..100);"


class TestAbstractWithAnnotations:
    """Test @abstract with value type annotations."""

    def test_abstract_attribute_format(self) -> None:
        """Test that @abstract comes before value type."""

        class BaseId(String):
            abstract = True

        schema = BaseId.to_schema_definition()
        assert schema == "attribute BaseId @abstract, value string;"

    def test_abstract_with_range(self) -> None:
        """Test @abstract combined with @range."""

        class BaseScore(Integer):
            abstract = True
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "100")

        schema = BaseScore.to_schema_definition()
        assert schema == "attribute BaseScore @abstract, value integer @range(0..100);"


class TestIndependentAnnotationGeneration:
    """Test @independent annotation generation in schema definitions."""

    def test_independent_string_attribute(self) -> None:
        """Test @independent on a string attribute."""

        class Language(String):
            independent = True

        schema = Language.to_schema_definition()
        assert schema == "attribute Language @independent, value string;"

    def test_independent_integer_attribute(self) -> None:
        """Test @independent on an integer attribute."""

        class GlobalCounter(Integer):
            independent = True

        schema = GlobalCounter.to_schema_definition()
        assert schema == "attribute GlobalCounter @independent, value integer;"

    def test_independent_with_range(self) -> None:
        """Test @independent combined with @range."""

        class SharedScore(Integer):
            independent = True
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "100")

        schema = SharedScore.to_schema_definition()
        assert schema == "attribute SharedScore @independent, value integer @range(0..100);"

    def test_independent_with_values(self) -> None:
        """Test @independent combined with @values."""

        class GlobalStatus(String):
            independent = True
            allowed_values: ClassVar[tuple[str, ...]] = ("active", "inactive", "pending")

        schema = GlobalStatus.to_schema_definition()
        assert (
            schema
            == 'attribute GlobalStatus @independent, value string @values("active", "inactive", "pending");'
        )

    def test_abstract_and_independent(self) -> None:
        """Test combining @abstract and @independent."""

        class BaseLanguage(String):
            abstract = True
            independent = True

        schema = BaseLanguage.to_schema_definition()
        assert schema == "attribute BaseLanguage @abstract @independent, value string;"

    def test_abstract_independent_with_range(self) -> None:
        """Test combining @abstract, @independent, and @range."""

        class AbstractCounter(Integer):
            abstract = True
            independent = True
            range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", None)

        schema = AbstractCounter.to_schema_definition()
        assert (
            schema == "attribute AbstractCounter @abstract @independent, value integer @range(0..);"
        )

    def test_is_independent_method(self) -> None:
        """Test the is_independent() class method."""

        class IndependentAttr(String):
            independent = True

        class DependentAttr(String):
            pass

        assert IndependentAttr.is_independent() is True
        assert DependentAttr.is_independent() is False
