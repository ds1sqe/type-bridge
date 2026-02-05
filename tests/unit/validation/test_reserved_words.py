"""Unit tests for TypeQL reserved words validation."""

import pytest

from type_bridge import Entity, Integer, Relation, Role, String, TypeFlags
from type_bridge.reserved_words import get_reserved_words, is_reserved_word
from type_bridge.validation import (
    ReservedWordError,
    ValidationError,
    validate_type_name,
)


class TestReservedWordDetection:
    """Test the reserved word detection utilities."""

    def test_is_reserved_word_case_insensitive(self):
        """Reserved word detection should be case-insensitive."""
        assert is_reserved_word("define")
        assert is_reserved_word("DEFINE")
        assert is_reserved_word("Define")
        assert is_reserved_word("match")
        assert is_reserved_word("MATCH")

    def test_common_reserved_words(self):
        """Check that common TypeQL keywords are recognized as reserved."""
        reserved = [
            "define",
            "match",
            "insert",
            "delete",
            "update",
            "entity",
            "relation",
            "attribute",
            "has",
            "owns",
            "plays",
            "relates",
            "boolean",
            "integer",
            "string",
            "double",
            "true",
            "false",
            "or",
            "not",  # Note: "and" is not a TypeQL keyword
            "count",
            "sum",
            "max",
            "min",
        ]
        for word in reserved:
            assert is_reserved_word(word), f"{word} should be reserved"

    def test_non_reserved_words(self):
        """Check that normal words are not flagged as reserved."""
        not_reserved = [
            "person",
            "company",
            "employee",
            "name",
            "age",
            "address",
            "salary",
            "department",
            "project",
        ]
        for word in not_reserved:
            assert not is_reserved_word(word), f"{word} should not be reserved"

    def test_get_reserved_words_returns_frozenset(self):
        """get_reserved_words should return a frozenset."""
        words = get_reserved_words()
        assert isinstance(words, frozenset)
        assert len(words) > 50  # Should have many reserved words
        assert "define" in words
        assert "match" in words


class TestValidationFunction:
    """Test the validate_type_name function."""

    def test_empty_name_raises_error(self):
        """Empty names should raise ValidationError."""
        with pytest.raises(ValidationError, match="Empty"):
            validate_type_name("", "entity")

    def test_reserved_word_raises_error(self):
        """Reserved words should raise ReservedWordError."""
        with pytest.raises(ReservedWordError) as exc:
            validate_type_name("match", "entity")
        assert "match" in str(exc.value)
        assert "reserved word" in str(exc.value)

    def test_name_starting_with_number_raises_error(self):
        """Names starting with numbers should raise ValidationError."""
        with pytest.raises(ValidationError, match="must start with a letter"):
            validate_type_name("123name", "entity")

    def test_name_with_invalid_chars_raises_error(self):
        """Names with invalid characters should raise ValidationError."""
        with pytest.raises(ValidationError, match="invalid character"):
            validate_type_name("name@test", "entity")

        with pytest.raises(ValidationError, match="invalid character"):
            validate_type_name("name.test", "entity")

        with pytest.raises(ValidationError, match="invalid character"):
            validate_type_name("name#test", "entity")

    def test_valid_names_pass(self):
        """Valid names should pass validation."""
        valid_names = [
            "person",
            "company",
            "employee_name",
            "first-name",
            "Age",
            "EmailAddress",
            "person_123",
            "name_value",
        ]
        for name in valid_names:
            validate_type_name(name, "entity")  # Should not raise


class TestEntityValidation:
    """Test reserved word validation for Entity classes."""

    def test_entity_with_reserved_name_fails(self):
        """Entity with reserved type name should raise error."""
        with pytest.raises(ReservedWordError, match="match.*entity"):

            class Match(Entity):
                flags = TypeFlags(name="match")

    def test_entity_with_reserved_class_name_formatted_fails(self):
        """Entity class with reserved name (formatted) should fail."""
        with pytest.raises(ReservedWordError):
            # This would become "define" in snake_case
            class Define(Entity):
                pass

    def test_entity_with_typeql_keyword_fails(self):
        """Entity with TypeQL keyword as name should fail."""
        with pytest.raises(ReservedWordError, match="insert"):

            class MyEntity(Entity):
                flags = TypeFlags(name="insert")

    def test_entity_named_entity_fails(self):
        """Entity named 'entity' should fail - TypeDB built-in type.

        Note: Built-in types (entity, relation, attribute, thing) raise ValueError
        from the TYPEDB_BUILTIN_TYPES check, not ReservedWordError.
        """
        with pytest.raises((ValueError, ReservedWordError)):

            class MyEntity(Entity):
                flags = TypeFlags(name="entity")

    def test_entity_named_thing_fails(self):
        """Entity named 'thing' should fail - TypeDB root type.

        Note: Built-in types raise ValueError from TYPEDB_BUILTIN_TYPES check.
        """
        with pytest.raises((ValueError, ReservedWordError)):

            class Thing(Entity):
                pass

    def test_entity_named_attribute_fails(self):
        """Entity named 'attribute' should fail - TypeDB built-in type.

        Note: Built-in types raise ValueError from TYPEDB_BUILTIN_TYPES check.
        """
        with pytest.raises((ValueError, ReservedWordError)):

            class Attribute(Entity):
                flags = TypeFlags(name="attribute")

    def test_entity_with_valid_name_succeeds(self):
        """Entity with valid name should succeed."""

        class Person(Entity):
            flags = TypeFlags(name="person")

        assert Person.get_type_name() == "person"

        class Company(Entity):
            pass

        assert Company.get_type_name() == "Company"

    def test_entity_with_valid_compound_name_succeeds(self):
        """Entity with compound name containing reserved word substring succeeds."""

        # These should work - reserved word is part of a larger name
        class MatchHistory(Entity):
            flags = TypeFlags(name="match_history")

        assert MatchHistory.get_type_name() == "match_history"

        class DeleteLog(Entity):
            flags = TypeFlags(name="delete_log")

        assert DeleteLog.get_type_name() == "delete_log"

        class EntityType(Entity):
            flags = TypeFlags(name="entity_type")

        assert EntityType.get_type_name() == "entity_type"


class TestRelationValidation:
    """Test reserved word validation for Relation classes."""

    def test_relation_with_reserved_name_fails(self):
        """Relation with reserved type name should raise error."""
        with pytest.raises(ReservedWordError, match="delete.*relation"):

            class Delete(Relation):
                flags = TypeFlags(name="delete")

    def test_relation_with_typeql_keyword_fails(self):
        """Relation with TypeQL keyword as name should fail."""
        with pytest.raises(ReservedWordError):

            class MyRelation(Relation):
                flags = TypeFlags(name="update")

    def test_relation_with_valid_name_succeeds(self):
        """Relation with valid name should succeed."""

        class Employment(Relation):
            flags = TypeFlags(name="employment")

        assert Employment.get_type_name() == "employment"


class TestAttributeValidation:
    """Test reserved word validation for Attribute classes."""

    def test_attribute_with_reserved_name_fails(self):
        """Attribute with reserved name should raise error."""
        with pytest.raises(ReservedWordError, match="string.*attribute"):

            class StringAttr(String):
                attr_name = "string"  # 'string' is a TypeQL value type keyword

    def test_attribute_with_typeql_value_type_fails(self):
        """Attribute named after TypeQL value type should fail."""
        with pytest.raises(ReservedWordError):

            class MyBoolean(String):
                attr_name = "boolean"

        with pytest.raises(ReservedWordError):

            class MyInteger(String):
                attr_name = "integer"

    def test_attribute_with_typeql_keyword_fails(self):
        """Attribute with TypeQL keyword as name should fail."""
        with pytest.raises(ReservedWordError):

            class MyAttribute(String):
                attr_name = "count"  # 'count' is a reduction keyword

    def test_attribute_named_value_fails(self):
        """Attribute named 'value' should fail - common mistake."""
        with pytest.raises(ReservedWordError, match="[Vv]alue.*attribute"):

            class Value(Integer):
                pass

        with pytest.raises(ReservedWordError):

            class MyValue(String):
                attr_name = "value"

    def test_attribute_named_label_fails(self):
        """Attribute named 'label' should fail."""
        with pytest.raises(ReservedWordError):

            class Label(String):
                pass

    def test_attribute_named_double_fails(self):
        """Attribute named 'double' (TypeQL value type) should fail."""
        with pytest.raises(ReservedWordError):

            class Double(Integer):
                attr_name = "double"

    def test_attribute_named_datetime_fails(self):
        """Attribute named 'datetime' (TypeQL value type) should fail."""
        with pytest.raises(ReservedWordError):

            class DateTimeAttr(String):
                attr_name = "datetime"

    def test_attribute_with_valid_name_succeeds(self):
        """Attribute with valid name should succeed."""

        class Name(String):
            pass

        assert Name.get_attribute_name() == "Name"

        class Age(Integer):
            pass

        assert Age.get_attribute_name() == "Age"

        class EmailAddress(String):
            attr_name = "email"

        assert EmailAddress.get_attribute_name() == "email"


class TestRoleValidation:
    """Test reserved word validation for Role definitions."""

    def test_role_with_reserved_name_fails(self):
        """Role with reserved name should raise error."""

        # Need a valid entity for testing
        class Person(Entity):
            pass

        with pytest.raises(ReservedWordError, match="from.*role"):
            Role("from", Person)  # 'from' is a TypeQL keyword

    def test_role_with_typeql_keyword_fails(self):
        """Role with TypeQL keyword as name should fail."""

        class Company(Entity):
            pass

        with pytest.raises(ReservedWordError):
            Role("has", Company)  # 'has' is a TypeQL statement keyword

        with pytest.raises(ReservedWordError):
            Role("plays", Company)  # 'plays' is a TypeQL statement keyword

    def test_role_with_valid_name_succeeds(self):
        """Role with valid name should succeed."""

        class Person(Entity):
            pass

        class Company(Entity):
            pass

        # These should all succeed
        employee_role = Role("employee", Person)
        assert employee_role.role_name == "employee"

        employer_role = Role("employer", Company)
        assert employer_role.role_name == "employer"

        member_role = Role("member", Person)
        assert member_role.role_name == "member"


class TestErrorMessages:
    """Test that error messages are helpful and include suggestions."""

    def test_reserved_word_error_includes_suggestions(self):
        """ReservedWordError should include helpful suggestions."""
        with pytest.raises(ReservedWordError) as exc:
            validate_type_name("entity", "entity")

        error_msg = str(exc.value)
        assert "Cannot use 'entity'" in error_msg
        assert "Suggestion" in error_msg
        # Should suggest alternatives like 'object', 'item', etc.

    def test_different_contexts_have_different_suggestions(self):
        """Different contexts (entity, relation, attribute, role) should have tailored suggestions."""
        # Entity context
        with pytest.raises(ReservedWordError) as exc:
            validate_type_name("count", "entity")
        assert "count_entity" in str(exc.value) or "my_count" in str(exc.value)

        # Attribute context
        with pytest.raises(ReservedWordError) as exc:
            validate_type_name("string", "attribute")
        assert "text" in str(exc.value) or "str_value" in str(exc.value)

        # Role context
        with pytest.raises(ReservedWordError) as exc:
            validate_type_name("from", "role")
        assert "source" in str(exc.value) or "from_role" in str(exc.value)


class TestIntegrationScenarios:
    """Test real-world scenarios with reserved words."""

    def test_schema_with_multiple_reserved_word_conflicts(self):
        """Test that multiple conflicts are caught correctly."""
        errors = []

        # Try to create entities with reserved names
        try:

            class Match(Entity):
                pass
        except ReservedWordError as e:
            errors.append(("Match entity", e))

        try:

            class Insert(Entity):
                flags = TypeFlags(name="insert")
        except ReservedWordError as e:
            errors.append(("Insert entity", e))

        # Try to create attributes with reserved names
        try:

            class Boolean(String):
                pass
        except ReservedWordError as e:
            errors.append(("Boolean attribute", e))

        # Should have caught all conflicts
        assert len(errors) == 3
        assert all("reserved word" in str(e) for _, e in errors)

    def test_inherited_entity_validates_correctly(self):
        """Inherited entities should also be validated."""

        # Base class with valid name
        class Animal(Entity):
            flags = TypeFlags(abstract=True)

        # This should work
        class Dog(Animal):
            pass

        # This should fail
        with pytest.raises(ReservedWordError):

            class Update(Animal):
                flags = TypeFlags(name="update")


class TestBaseClassEscapeHatch:
    """Test that base=True allows bypassing reserved word validation."""

    def test_entity_with_base_true_bypasses_validation(self):
        """Entity with base=True should not be validated for reserved words."""

        # This should succeed even though 'match' is reserved
        class Match(Entity):
            flags = TypeFlags(base=True, name="match")

        assert Match.get_type_name() == "match"
        assert Match.is_base() is True

        # Another example with a different reserved word
        class Define(Entity):
            flags = TypeFlags(base=True)

        assert Define.is_base() is True

    def test_relation_with_base_true_bypasses_validation(self):
        """Relation with base=True should not be validated for reserved words."""

        # This should succeed even though 'insert' is reserved
        class Insert(Relation):
            flags = TypeFlags(base=True, name="insert")

        assert Insert.get_type_name() == "insert"
        assert Insert.is_base() is True

    def test_base_class_not_in_schema(self):
        """Base classes should return None for schema definition."""

        class Update(Entity):
            flags = TypeFlags(base=True, name="update")

        # Base classes don't appear in schema
        assert Update.to_schema_definition() is None

    def test_inherited_from_base_class_is_validated(self):
        """Classes inheriting from base classes should still be validated."""

        # Base class with reserved name (allowed)
        class Match(Entity):
            flags = TypeFlags(base=True, name="match")

        # Inheriting class with valid name (should work)
        class Person(Match):
            flags = TypeFlags(name="person")

        assert Person.get_type_name() == "person"

        # Inheriting class with reserved name (should fail)
        with pytest.raises(ReservedWordError):

            class Delete(Match):
                flags = TypeFlags(name="delete")

    def test_base_entity_and_relation_classes_bypass_validation(self):
        """The abstract base Entity and Relation classes themselves should bypass validation."""
        # This tests that the actual Entity and Relation base classes from type_bridge
        # don't trigger validation errors even though "entity" and "relation" are TypeDB built-ins
        from type_bridge.models.entity import Entity as BaseEntity
        from type_bridge.models.relation import Relation as BaseRelation

        # These should exist without errors
        assert BaseEntity.__name__ == "Entity"
        assert BaseRelation.__name__ == "Relation"


class TestAllReservedWords:
    """Test that ALL reserved words are properly rejected."""

    # Group reserved words by category for organized testing
    SCHEMA_KEYWORDS = ["define", "undefine", "redefine"]
    DATA_MANIPULATION = ["match", "fetch", "insert", "delete", "update", "put"]
    STREAM_OPERATORS = ["select", "require", "sort", "limit", "offset", "reduce"]
    PATTERN_LOGIC = ["or", "not", "try", "with"]
    TYPE_DEFINITIONS = ["entity", "relation", "attribute", "struct", "fun"]
    CONSTRAINTS = ["sub", "relates", "plays", "value", "owns", "alias"]
    INSTANCE_STATEMENTS = ["isa", "links", "has", "is", "let", "contains", "like"]
    IDENTITY = ["label", "iid"]
    ANNOTATIONS = [
        "card",
        "cascade",
        "independent",
        "abstract",
        "key",
        "subkey",
        "unique",
        "values",
        "range",
        "regex",
        "distinct",
    ]
    REDUCTIONS = ["check", "first", "count", "max", "min", "mean", "median", "std", "sum", "list"]
    VALUE_TYPES = [
        "boolean",
        "integer",
        "double",
        "decimal",
        # Note: "datetime-tz" is omitted because hyphens are not valid in Python identifiers
        "datetime_tz",
        "datetime",
        "date",
        "duration",
        "string",
    ]
    FUNCTIONS = ["round", "ceil", "floor", "abs", "length"]
    LITERALS = ["true", "false"]
    MISC = ["asc", "desc", "return", "of", "from", "in", "as"]

    @pytest.mark.parametrize("word", SCHEMA_KEYWORDS)
    def test_schema_keywords_rejected(self, word):
        """Schema keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", DATA_MANIPULATION)
    def test_data_manipulation_keywords_rejected(self, word):
        """Data manipulation keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", STREAM_OPERATORS)
    def test_stream_operators_rejected(self, word):
        """Stream operators should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", PATTERN_LOGIC)
    def test_pattern_logic_keywords_rejected(self, word):
        """Pattern logic keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", TYPE_DEFINITIONS)
    def test_type_definition_keywords_rejected(self, word):
        """Type definition keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "relation")

    @pytest.mark.parametrize("word", CONSTRAINTS)
    def test_constraint_keywords_rejected(self, word):
        """Constraint keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", INSTANCE_STATEMENTS)
    def test_instance_statement_keywords_rejected(self, word):
        """Instance statement keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", IDENTITY)
    def test_identity_keywords_rejected(self, word):
        """Identity keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", ANNOTATIONS)
    def test_annotation_keywords_rejected(self, word):
        """Annotation keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", REDUCTIONS)
    def test_reduction_keywords_rejected(self, word):
        """Reduction keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", VALUE_TYPES)
    def test_value_type_keywords_rejected(self, word):
        """Value type keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", FUNCTIONS)
    def test_function_keywords_rejected(self, word):
        """Function keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "entity")

    @pytest.mark.parametrize("word", LITERALS)
    def test_literal_keywords_rejected(self, word):
        """Literal keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "attribute")

    @pytest.mark.parametrize("word", MISC)
    def test_misc_keywords_rejected(self, word):
        """Miscellaneous keywords should be rejected."""
        with pytest.raises(ReservedWordError):
            validate_type_name(word, "role")

    # Reserved words that aren't valid Python identifiers (contain hyphens)
    # These can never be used as type names in Python, so we don't test them
    INVALID_PYTHON_IDENTIFIERS = {"datetime-tz"}

    def test_all_reserved_words_covered(self):
        """Verify that test coverage includes all reserved words."""
        all_tested = set(
            self.SCHEMA_KEYWORDS
            + self.DATA_MANIPULATION
            + self.STREAM_OPERATORS
            + self.PATTERN_LOGIC
            + self.TYPE_DEFINITIONS
            + self.CONSTRAINTS
            + self.INSTANCE_STATEMENTS
            + self.IDENTITY
            + self.ANNOTATIONS
            + self.REDUCTIONS
            + self.VALUE_TYPES
            + self.FUNCTIONS
            + self.LITERALS
            + self.MISC
        )
        actual_reserved = get_reserved_words()

        # Exclude reserved words that aren't valid Python identifiers
        testable_reserved = actual_reserved - self.INVALID_PYTHON_IDENTIFIERS

        missing = testable_reserved - all_tested
        extra = all_tested - actual_reserved

        # Report any discrepancies
        assert not missing, f"Reserved words not tested: {missing}"
        assert not extra, f"Tested words not in reserved list: {extra}"

    def test_case_insensitive_all_reserved(self):
        """All reserved words should be rejected regardless of case."""
        sample_words = ["DEFINE", "Match", "INSERT", "Entity", "VALUE", "Boolean"]
        for word in sample_words:
            with pytest.raises(ReservedWordError):
                validate_type_name(word, "entity")


class TestReservedWordContexts:
    """Test reserved words in all validation contexts."""

    @pytest.mark.parametrize("context", ["entity", "relation", "attribute", "role"])
    def test_common_reserved_rejected_in_all_contexts(self, context):
        """Common reserved words should be rejected in all contexts."""
        common_words = ["match", "insert", "delete", "entity", "attribute", "value"]
        for word in common_words:
            with pytest.raises(ReservedWordError):
                validate_type_name(word, context)

    @pytest.mark.parametrize("context", ["entity", "relation", "attribute", "role"])
    def test_valid_names_pass_in_all_contexts(self, context):
        """Valid names should pass in all contexts."""
        valid_names = [
            "person",
            "company",
            "employee",
            "customer",
            "name",
            "age",
            "email",
            "address",
            "my_entity",
            "user_role",
            "product_id",
        ]
        for name in valid_names:
            validate_type_name(name, context)  # Should not raise
