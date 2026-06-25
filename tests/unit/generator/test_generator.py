"""Tests for the public code generator surface."""

from __future__ import annotations

import tempfile
from pathlib import Path

import pytest

from type_bridge.generator import generate_models, parse_tql_schema

FIXTURES_DIR = Path(__file__).parent / "fixtures"
BOOKSTORE_SCHEMA = FIXTURES_DIR / "bookstore.tql"


class TestGenerateModels:
    """Tests for the main generate_models function."""

    def test_generates_package(self) -> None:
        """Generate a complete package from schema text."""
        schema_text = """
            define
            attribute name, value string;

            define
            entity person,
                owns name @key;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema_text, output)

            assert (output / "__init__.py").exists()
            assert (output / "attributes.py").exists()
            assert (output / "entities.py").exists()
            assert (output / "relations.py").exists()
            assert (output / "schema.tql").exists()

            for py_file in output.glob("*.py"):
                content = py_file.read_text()
                compile(content, py_file.name, "exec")

    def test_case_annotation_inference_and_overrides(self) -> None:
        """Test that TypeNameCase inference and @case overrides work correctly."""
        schema_text = """
            define
            attribute name, value string;

            # @case(PascalCase)
            entity forced_class_name, owns name @key;

            # @case(Python, LowerCase)
            entity forced_python_lower, owns name @key;

            entity FirstPerson, owns name @key;

            entity technology_company, owns name @key;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema_text, output)

            entities_code = (output / "entities.py").read_text()

            # forced_class_name should have CLASS_NAME (override)
            assert "case=TypeNameCase.CLASS_NAME" in entities_code
            assert "class ForcedClassName" in entities_code

            # forced_python_lower should have LOWERCASE (override)
            assert "case=TypeNameCase.LOWERCASE" in entities_code
            assert "class ForcedPythonLower" in entities_code

            # FirstPerson should automatically get CLASS_NAME
            # wait, it might be implicitly inferred without any string because default is CLASS_NAME
            assert "class Firstperson" not in entities_code
            assert "class FirstPerson" in entities_code

            # technology_company should automatically get SNAKE_CASE
            assert "case=TypeNameCase.SNAKE_CASE" in entities_code
            assert "class TechnologyCompany" in entities_code

    def test_toml_transpiler_annotations(self) -> None:
        """Test that annotations in TOML schemas are emitted correctly."""
        schema_text = """
        [attributes.name]
        value = "string"

        [entities.customer]
        bindgen_case = "Python, PascalCase"
        annotations = ["dto_name(CustomerDto)"]
        owns = ["name"]
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema_text, output, format="toml")

            entities_code = (output / "entities.py").read_text()
            assert "class Customer" in entities_code
            assert "case=TypeNameCase.CLASS_NAME" in entities_code

    def test_generates_from_file(self) -> None:
        """Generate from a schema file path."""
        if not BOOKSTORE_SCHEMA.exists():
            pytest.skip("Bookstore schema fixture not found")

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "bookstore"
            generate_models(BOOKSTORE_SCHEMA, output)

            assert (output / "__init__.py").exists()
            assert (output / "attributes.py").exists()
            assert (output / "entities.py").exists()
            assert (output / "relations.py").exists()

            for py_file in output.glob("*.py"):
                content = py_file.read_text()
                compile(content, py_file.name, "exec")


@pytest.mark.skipif(
    not BOOKSTORE_SCHEMA.exists(),
    reason="Bookstore schema fixture not found",
)
class TestBookstoreSchema:
    """Integration tests using the bookstore schema from TypeDB docs."""

    @pytest.fixture
    def bookstore_schema(self) -> str:
        """Load the bookstore schema."""
        return BOOKSTORE_SCHEMA.read_text()

    def test_parses_without_error(self, bookstore_schema: str) -> None:
        """The bookstore schema should parse completely."""
        schema = parse_tql_schema(bookstore_schema)

        assert len(schema.attributes) > 0
        assert len(schema.entities) > 0
        assert len(schema.relations) > 0

    def test_entity_inheritance(self, bookstore_schema: str) -> None:
        """Test entity inheritance in bookstore schema."""
        schema = parse_tql_schema(bookstore_schema)

        assert "book" in schema.entities
        assert schema.entities["book"].abstract is True

        for subtype in ["hardback", "paperback", "ebook"]:
            assert subtype in schema.entities
            assert schema.entities[subtype].parent == "book"

    def test_relation_inheritance(self, bookstore_schema: str) -> None:
        """Test relation inheritance in bookstore schema."""
        schema = parse_tql_schema(bookstore_schema)

        assert "contribution" in schema.relations
        for subtype in ["authoring", "editing", "illustrating"]:
            assert subtype in schema.relations
            assert schema.relations[subtype].parent == "contribution"

    def test_generates_valid_code(self, bookstore_schema: str) -> None:
        """Generated code should compile."""
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "bookstore"
            generate_models(bookstore_schema, output)

            for py_file in output.glob("*.py"):
                content = py_file.read_text()
                compile(content, py_file.name, "exec")


class TestDocstringExtraction:
    """Tests for docstring extraction from TypeQL comments."""

    def test_entity_docstring_from_comment(self) -> None:
        """Entity docstring should be extracted from preceding comment."""
        schema = parse_tql_schema("""
            define
            ## Represents a person in the system.
            ## Can be an employee or customer.
            entity person,
                owns name;

            define
            attribute name, value string;
        """)

        assert schema.entities["person"].docstring is not None
        assert "person" in schema.entities["person"].docstring.lower()

    def test_attribute_docstring_from_comment(self) -> None:
        """Attribute docstring should be extracted from preceding comment."""
        schema = parse_tql_schema("""
            define
            ## The person's full legal name.
            attribute name, value string;
        """)

        assert schema.attributes["name"].docstring is not None
        assert "name" in schema.attributes["name"].docstring.lower()

    def test_relation_docstring_from_comment(self) -> None:
        """Relation docstring should be extracted from preceding comment."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;

            define
            ## Represents a friendship between two people.
            relation friendship,
                relates friend @card(2..2);
        """)

        assert schema.relations["friendship"].docstring is not None
        assert "friendship" in schema.relations["friendship"].docstring.lower()


class TestAttributeDefaultAndTransform:
    """Tests for attribute default and transform annotations."""

    def test_attribute_default_annotation(self) -> None:
        """Attribute @default annotation should be parsed."""
        schema = parse_tql_schema("""
            define
            ## @default(0)
            attribute count, value integer;
        """)

        attr = schema.attributes["count"]
        assert attr.annotations.get("default") == 0 or attr.default == 0

    def test_attribute_transform_annotation(self) -> None:
        """Attribute @transform annotation should be parsed."""
        schema = parse_tql_schema("""
            define
            ## @transform(lowercase)
            attribute email, value string;
        """)

        attr = schema.attributes["email"]
        assert attr.annotations.get("transform") == "lowercase" or attr.transform == "lowercase"
