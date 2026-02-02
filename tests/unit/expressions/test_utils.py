"""Unit tests for expression utilities (variable generation)."""

from type_bridge import Integer, String
from type_bridge.attribute.flags import AttributeFlags
from type_bridge.expressions.utils import generate_attr_var, generate_has_pattern


# Test attributes
class Name(String):
    pass


class Age(Integer):
    pass


class NameStatus(String):
    """Attribute that could cause collision with var_name + status."""

    flags = AttributeFlags(name="name_status")


class BirthDate(String):
    """Attribute with kebab-case TypeDB name."""

    flags = AttributeFlags(name="birth-date")


class TestGenerateAttrVar:
    """Tests for generate_attr_var function."""

    def test_basic_variable_generation(self):
        """Basic variable generation works correctly."""
        result = generate_attr_var("$e", Name)
        assert result == "$e__name"

    def test_longer_prefix(self):
        """Works with longer variable prefixes."""
        result = generate_attr_var("$person", Age)
        assert result == "$person__age"

    def test_double_underscore_prevents_collision(self):
        """Double underscore prevents collision between similar vars/attrs.

        Without double underscore separator:
        - $actor_name + status = $actor_name_status
        - $actor + name_status = $actor_name_status  (COLLISION!)

        With double underscore separator:
        - $actor_name + status = $actor_name__status
        - $actor + name_status = $actor__name_status  (NO COLLISION!)
        """
        # These should produce DIFFERENT variable names
        var1 = generate_attr_var("$actor_name", Age)  # $actor_name__age
        var2 = generate_attr_var("$actor", NameStatus)  # $actor__name_status

        # Verify they are different (no collision)
        assert var1 != var2
        assert var1 == "$actor_name__age"
        assert var2 == "$actor__name_status"

    def test_hyphen_sanitization(self):
        """Hyphens in attribute names are converted to underscores.

        TypeDB variables cannot contain hyphens, so we must sanitize.
        """
        result = generate_attr_var("$e", BirthDate)
        # birth-date -> birth_date
        assert result == "$e__birth_date"
        assert "-" not in result  # No hyphens in variable name

    def test_preserves_dollar_sign(self):
        """Generated variable always starts with $."""
        result = generate_attr_var("$x", Name)
        assert result.startswith("$")

    def test_handles_already_stripped_prefix(self):
        """Works even if input accidentally has no $."""
        # This shouldn't happen in practice, but handle it gracefully
        result = generate_attr_var("actor", Name)
        assert result == "$actor__name"


class TestGenerateHasPattern:
    """Tests for generate_has_pattern function."""

    def test_basic_pattern_generation(self):
        """Basic pattern generation works correctly."""
        attr_var, pattern = generate_has_pattern("$e", Name)
        assert attr_var == "$e__name"
        assert pattern == "$e has Name $e__name"

    def test_kebab_case_attribute_in_pattern(self):
        """Pattern uses original attribute name (kebab-case) but sanitized var."""
        attr_var, pattern = generate_has_pattern("$e", BirthDate)
        # Variable is sanitized (no hyphens)
        assert attr_var == "$e__birth_date"
        # Pattern uses original TypeDB attribute name (with hyphen)
        assert pattern == "$e has birth-date $e__birth_date"


class TestVariableScopingDocumentation:
    """Tests that document the variable scoping behavior for TypeDB 3.x."""

    def test_different_entities_get_different_vars(self):
        """Different entity variables produce different attribute variables.

        This is critical for TypeDB 3.x where using the same variable twice
        creates an implicit equality constraint.
        """
        actor_var = generate_attr_var("$actor", Name)
        target_var = generate_attr_var("$target", Name)

        # Must be different to avoid implicit equality constraint
        assert actor_var != target_var
        assert actor_var == "$actor__name"
        assert target_var == "$target__name"
