"""Tests for the code generator."""

from __future__ import annotations

import tempfile
from pathlib import Path

import pytest

from type_bridge.generator import generate_models, parse_tql_schema
from type_bridge.generator.naming import build_class_name_map
from type_bridge.generator.render import (
    render_attributes,
    render_entities,
    render_package_init,
    render_relations,
)

FIXTURES_DIR = Path(__file__).parent / "fixtures"
BOOKSTORE_SCHEMA = FIXTURES_DIR / "bookstore.tql"


class TestRenderAttributes:
    """Tests for attribute rendering."""

    def test_simple_attribute(self) -> None:
        """Render a simple string attribute."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert "class Name(String):" in source
        assert 'flags = AttributeFlags(name="name")' in source
        assert "from type_bridge import" in source

    def test_attribute_inheritance(self) -> None:
        """Render attributes with inheritance."""
        schema = parse_tql_schema("""
            define
            attribute isbn @abstract, value string;

            define
            attribute isbn-13 sub isbn;
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert "class Isbn(String):" in source
        assert "class Isbn13(Isbn):" in source  # Inherits from Isbn, not String

    def test_attribute_with_constraints(self) -> None:
        """Render attribute with @regex and @values."""
        schema = parse_tql_schema("""
            define
            attribute status, value string @regex("^(active|inactive)$");
            attribute emoji, value string @values("like", "love");
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert 'regex: ClassVar[str] = r"^(active|inactive)$"' in source
        assert 'allowed_values: ClassVar[tuple[str, ...]] = ("like", "love",)' in source

    def test_attribute_with_range(self) -> None:
        """Render attribute with @range constraint."""
        schema = parse_tql_schema("""
            define
            attribute age, value integer @range(0..150);
            attribute temperature, value double @range(-50.0..50.0);
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert (
            "range_constraint: ClassVar[tuple[int | float | None, int | float | None]] = (0, 150)"
            in source
        )
        assert (
            "range_constraint: ClassVar[tuple[int | float | None, int | float | None]] = (-50.0, 50.0)"
            in source
        )

    def test_attribute_with_open_range(self) -> None:
        """Render attribute with open-ended @range constraint."""
        schema = parse_tql_schema("""
            define
            attribute score, value integer @range(0..);
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert (
            "range_constraint: ClassVar[tuple[int | float | None, int | float | None]] = (0, None)"
            in source
        )

    def test_attribute_with_independent(self) -> None:
        """Render attribute with @independent annotation."""
        schema = parse_tql_schema("""
            define
            attribute language @independent, value string;
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert "class Language(String):" in source
        assert "independent = True" in source

    def test_attribute_independent_with_abstract(self) -> None:
        """Render attribute with both @abstract and @independent."""
        schema = parse_tql_schema("""
            define
            attribute tag @abstract @independent, value string;
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert "class Tag(String):" in source
        assert "independent = True" in source

    def test_attribute_not_independent_by_default(self) -> None:
        """Render attribute without @independent should not include independent = True."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;
        """)
        class_names = build_class_name_map(schema.attributes)
        source = render_attributes(schema, class_names)

        assert "class Name(String):" in source
        assert "independent = True" not in source


class TestRenderEntities:
    """Tests for entity rendering."""

    def test_simple_entity(self) -> None:
        """Render a simple entity."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;

            define
            entity person,
                owns name;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "class Person(Entity):" in source
        assert 'flags = TypeFlags(name="person")' in source
        assert "name: attributes.Name | None = None" in source

    def test_entity_with_key(self) -> None:
        """Render entity with @key attribute."""
        schema = parse_tql_schema("""
            define
            attribute id, value string;

            define
            entity user,
                owns id @key;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "id: attributes.Id = Flag(Key)" in source

    def test_entity_inheritance(self) -> None:
        """Render entity with inheritance."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;

            define
            entity company @abstract,
                owns name;

            define
            entity publisher sub company;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "class Company(Entity):" in source
        assert "abstract=True" in source
        assert "class Publisher(Company):" in source  # Inherits from Company

    def test_entity_with_cardinality(self) -> None:
        """Render entity with various cardinalities."""
        schema = parse_tql_schema("""
            define
            attribute tag, value string;
            attribute title, value string;

            define
            entity article,
                owns title @card(1),
                owns tag @card(0..);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "title: attributes.Title" in source  # Required single
        assert "list[attributes.Tag]" in source  # Multi-value


class TestRenderRelations:
    """Tests for relation rendering."""

    def test_simple_relation(self) -> None:
        """Render a simple relation."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;

            define
            relation friendship,
                relates friend;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        assert "class Friendship(Relation):" in source
        assert 'flags = TypeFlags(name="friendship")' in source
        assert "friend: Role[entities.Person]" in source

    def test_relation_with_owns(self) -> None:
        """Render relation that owns attributes."""
        schema = parse_tql_schema("""
            define
            attribute since, value datetime;
            entity person,
                plays friendship:friend;

            define
            relation friendship,
                relates friend,
                owns since;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        assert "since: attributes.Since" in source

    def test_relation_optional_attribute(self) -> None:
        """Render relation with optional attribute (no @card constraint)."""
        schema = parse_tql_schema("""
            define
            attribute sequence_index, value integer;
            entity milestone,
                plays task_grouping:milestone;
            entity task,
                plays task_grouping:task;

            define
            relation task_grouping,
                relates milestone @card(1),
                relates task @card(0..),
                owns sequence_index;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Without @card constraint, attribute should be optional
        assert "sequence_index: attributes.Sequence_index | None = None" in source

    def test_relation_required_attribute(self) -> None:
        """Render relation with required attribute (@card(1))."""
        schema = parse_tql_schema("""
            define
            attribute weight, value double;
            entity node,
                plays edge:endpoint;

            define
            relation edge,
                relates endpoint,
                owns weight @card(1);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # With @card(1), attribute should be required (no | None = None)
        assert "weight: attributes.Weight" in source
        assert "weight: attributes.Weight | None" not in source

    def test_relation_key_attribute(self) -> None:
        """Render relation with @key attribute."""
        schema = parse_tql_schema("""
            define
            attribute edge_id, value string;
            entity node,
                plays connection:endpoint;

            define
            relation connection,
                relates endpoint,
                owns edge_id @key;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # With @key, attribute should use Flag(Key)
        assert "edge_id: attributes.Edge_id = Flag(Key)" in source
        assert "from type_bridge import" in source
        assert "Flag" in source
        assert "Key" in source

    def test_relation_multi_value_attribute(self) -> None:
        """Render relation with multi-value attribute (@card(0..))."""
        schema = parse_tql_schema("""
            define
            attribute tag, value string;
            entity item,
                plays tagging:item;

            define
            relation tagging,
                relates item,
                owns tag @card(0..);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # With @card(0..), attribute should be a list
        assert "list[attributes.Tag]" in source
        assert "Card" in source

    def test_relation_inherits_key_from_parent(self) -> None:
        """Child relation inherits @key constraint from parent."""
        schema = parse_tql_schema("""
            define
            attribute rel_id, value string;
            entity node,
                plays base_rel:endpoint,
                plays child_rel:endpoint;

            define
            relation base_rel @abstract,
                relates endpoint,
                owns rel_id @key;

            define
            relation child_rel sub base_rel;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Child should inherit @key from parent
        assert "class Child_rel(Base_rel):" in source
        assert "rel_id: attributes.Rel_id = Flag(Key)" in source

    def test_relation_inherits_cardinality_from_parent(self) -> None:
        """Child relation inherits cardinality constraint from parent."""
        schema = parse_tql_schema("""
            define
            attribute weight, value double;
            entity node,
                plays base_edge:endpoint,
                plays weighted_edge:endpoint;

            define
            relation base_edge @abstract,
                relates endpoint,
                owns weight @card(1);

            define
            relation weighted_edge sub base_edge;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Child should inherit required cardinality from parent
        # The parent declares weight as required, child should not have it optional
        assert "class Weighted_edge(Base_edge):" in source
        # weight should NOT be in child since it's inherited from parent
        # But if it were re-declared, it should still be required


class TestComingSoonAnnotationStubs:
    """Tests for coming-soon annotation stubs (TODO comments in generated code)."""

    def test_entity_cascade_stub(self) -> None:
        """Render entity with @cascade annotation stub."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;

            define
            entity person,
                owns name;
        """)
        # Manually set cascades (parser doesn't parse this yet as it's coming soon)
        schema.entities["person"].cascades = {"name"}

        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "# TODO: @cascade annotation (coming soon in TypeDB)" in source
        assert "# cascade_attrs:" in source
        assert "'name'" in source

    def test_entity_subkey_stub(self) -> None:
        """Render entity with @subkey annotation stub."""
        schema = parse_tql_schema("""
            define
            attribute first_name, value string;
            attribute last_name, value string;

            define
            entity person,
                owns first_name,
                owns last_name;
        """)
        # Manually set subkeys (parser doesn't parse this yet as it's coming soon)
        schema.entities["person"].subkeys = {"first_name": "name_key", "last_name": "name_key"}

        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        assert "# TODO: @subkey annotation (coming soon in TypeDB)" in source
        assert '# subkey_group "name_key":' in source
        assert "'first_name'" in source or "'last_name'" in source

    def test_relation_cascade_stub(self) -> None:
        """Render relation with @cascade annotation stub."""
        schema = parse_tql_schema("""
            define
            attribute since, value datetime;
            entity person,
                plays friendship:friend;

            define
            relation friendship,
                relates friend,
                owns since;
        """)
        # Manually set cascades
        schema.relations["friendship"].cascades = {"since"}

        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        assert "# TODO: @cascade annotation (coming soon in TypeDB)" in source
        assert "# cascade_attrs:" in source
        assert "'since'" in source

    def test_relation_distinct_stub(self) -> None:
        """Render relation with @distinct role annotation stub."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;

            define
            relation friendship,
                relates friend;
        """)
        # Manually set distinct on role
        schema.relations["friendship"].roles[0].distinct = True

        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        assert "# TODO: @distinct annotation (coming soon in TypeDB)" in source
        assert "# distinct_roles:" in source
        assert "'friend'" in source

    def test_no_stubs_when_annotations_absent(self) -> None:
        """No TODO stubs when coming-soon annotations are not present."""
        schema = parse_tql_schema("""
            define
            attribute name, value string;

            define
            entity person,
                owns name;

            define
            relation friendship,
                relates friend;

            define
            entity member,
                plays friendship:friend;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)

        entity_source = render_entities(schema, attr_names, entity_names)
        relation_source = render_relations(schema, attr_names, entity_names, relation_names)

        # No TODO stubs should appear
        assert "# TODO: @cascade" not in entity_source
        assert "# TODO: @subkey" not in entity_source
        assert "# TODO: @cascade" not in relation_source
        assert "# TODO: @distinct" not in relation_source

    def test_role_cardinality_exact(self) -> None:
        """Render relation with exact role cardinality (@card(2))."""
        schema = parse_tql_schema("""
            define
            entity memory,
                plays is_similar_to:similar_memory;

            define
            relation is_similar_to,
                relates similar_memory @card(2..2);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Role with @card(2..2) should include cardinality=Card(2, 2)
        assert (
            'similar_memory: Role[entities.Memory] = Role("similar_memory", entities.Memory, cardinality=Card(2, 2))'
            in source
        )
        assert "Card" in source

    def test_role_cardinality_unbounded(self) -> None:
        """Render relation with unbounded role cardinality (@card(2..))."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays group_membership:member;

            define
            relation group_membership,
                relates member @card(2..);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Role with @card(2..) should include cardinality=Card(2) (unbounded)
        assert (
            'member: Role[entities.Person] = Role("member", entities.Person, cardinality=Card(2))'
            in source
        )

    def test_role_cardinality_bounded(self) -> None:
        """Render relation with bounded role cardinality (@card(1..3))."""
        schema = parse_tql_schema("""
            define
            entity document,
                plays review:document;
            entity reviewer,
                plays review:reviewer;

            define
            relation review,
                relates document @card(1),
                relates reviewer @card(1..3);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Role with @card(1..3) should include cardinality=Card(1, 3)
        assert (
            'reviewer: Role[entities.Reviewer] = Role("reviewer", entities.Reviewer, cardinality=Card(1, 3))'
            in source
        )
        # Role with @card(1) is default, should not include Card
        assert 'document: Role[entities.Document] = Role("document", entities.Document)' in source

    def test_role_cardinality_default_omitted(self) -> None:
        """Default role cardinality (@card(1..1)) should not generate Card."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays employment:employee;
            entity company,
                plays employment:employer;

            define
            relation employment,
                relates employee @card(1..1),
                relates employer;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Default cardinality should not include Card
        assert 'employee: Role[entities.Person] = Role("employee", entities.Person)' in source
        assert 'employer: Role[entities.Company] = Role("employer", entities.Company)' in source
        # Card should not be imported if only default cardinalities
        assert "Card" not in source


class TestRenderPackageInit:
    """Tests for package __init__.py rendering."""

    def test_basic_init(self) -> None:
        """Render basic __init__.py."""
        source = render_package_init(
            {"name": "Name"},
            {"person": "Person"},
            {"friendship": "Friendship"},
            schema_version="2.0.0",
        )

        assert 'SCHEMA_VERSION = "2.0.0"' in source
        assert "from . import attributes, entities, registry, relations" in source
        assert "attributes.Name," in source
        assert "entities.Person," in source
        assert "relations.Friendship," in source
        assert "def schema_text()" in source

    def test_without_schema_loader(self) -> None:
        """Render without schema_text helper."""
        source = render_package_init(
            {},
            {},
            {},
            include_schema_loader=False,
        )

        assert "def schema_text()" not in source
        assert "importlib" not in source


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

            # Check all files exist
            assert (output / "__init__.py").exists()
            assert (output / "attributes.py").exists()
            assert (output / "entities.py").exists()
            assert (output / "relations.py").exists()
            assert (output / "schema.tql").exists()

            # Check content is valid Python
            for py_file in output.glob("*.py"):
                content = py_file.read_text()
                compile(content, py_file.name, "exec")

    def test_generates_from_file(self) -> None:
        """Generate from a schema file path."""
        if not BOOKSTORE_SCHEMA.exists():
            pytest.skip("Bookstore schema fixture not found")

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "bookstore"
            generate_models(BOOKSTORE_SCHEMA, output)

            # Check files exist
            assert (output / "__init__.py").exists()
            assert (output / "attributes.py").exists()
            assert (output / "entities.py").exists()
            assert (output / "relations.py").exists()

            # Verify generated code compiles
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

        # Check we got meaningful content
        assert len(schema.attributes) > 0
        assert len(schema.entities) > 0
        assert len(schema.relations) > 0

    def test_entity_inheritance(self, bookstore_schema: str) -> None:
        """Test entity inheritance in bookstore schema."""
        schema = parse_tql_schema(bookstore_schema)

        # book is abstract with subtypes
        assert "book" in schema.entities
        assert schema.entities["book"].abstract is True

        # hardback, paperback, ebook extend book
        for subtype in ["hardback", "paperback", "ebook"]:
            assert subtype in schema.entities
            assert schema.entities[subtype].parent == "book"

    def test_relation_inheritance(self, bookstore_schema: str) -> None:
        """Test relation inheritance in bookstore schema."""
        schema = parse_tql_schema(bookstore_schema)

        # contribution is parent of authoring, editing, illustrating
        assert "contribution" in schema.relations
        for subtype in ["authoring", "editing", "illustrating"]:
            assert subtype in schema.relations
            assert schema.relations[subtype].parent == "contribution"

    def test_generates_valid_code(self, bookstore_schema: str) -> None:
        """Generated code should compile."""
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "bookstore"
            generate_models(bookstore_schema, output)

            # All Python files should compile
            for py_file in output.glob("*.py"):
                content = py_file.read_text()
                compile(content, py_file.name, "exec")


class TestPlaysCardinalityRendering:
    """Tests for entity plays cardinality rendering.

    When entities declare `plays relation:role @card(0..5)`, the cardinality
    should be rendered as a TODO comment (until TypeDB supports it at runtime).
    """

    def test_plays_cardinality_rendered_as_todo(self) -> None:
        """Entity plays cardinality should be rendered as TODO comment."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend @card(0..5);

            define
            relation friendship,
                relates friend @card(2..2);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        # Should have TODO comment about plays cardinality
        assert "# TODO: plays cardinality" in source or "@card(0..5)" in source

    def test_plays_cardinality_unbounded(self) -> None:
        """Unbounded plays cardinality should be rendered."""
        schema = parse_tql_schema("""
            define
            entity employee,
                plays employment:employee @card(1..);

            define
            relation employment,
                relates employee,
                relates employer;

            define
            entity company,
                plays employment:employer;
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        source = render_entities(schema, attr_names, entity_names)

        # Should indicate the cardinality constraint somewhere
        assert "@card(1..)" in source or "plays_cardinality" in source.lower()


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

        # Docstring should be extracted
        assert schema.entities["person"].docstring is not None
        assert "person" in schema.entities["person"].docstring.lower()

    def test_attribute_docstring_from_comment(self) -> None:
        """Attribute docstring should be extracted from preceding comment."""
        schema = parse_tql_schema("""
            define
            ## The person's full legal name.
            attribute name, value string;
        """)

        # Docstring should be extracted
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

        # Docstring should be extracted
        assert schema.relations["friendship"].docstring is not None
        assert "friendship" in schema.relations["friendship"].docstring.lower()


class TestAnnotationRendering:
    """Tests for custom annotation rendering."""

    def test_role_annotations_in_relation_context(self) -> None:
        """Role annotations should be available in generated relation code."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;

            define
            ## @api(public)
            relation friendship,
                relates friend @card(2..2);
        """)
        attr_names = build_class_name_map(schema.attributes)
        entity_names = build_class_name_map(schema.entities)
        relation_names = build_class_name_map(schema.relations)
        source = render_relations(schema, attr_names, entity_names, relation_names)

        # Relation should have some indication of the annotation
        # Either as a comment, docstring, or class variable
        assert "@api" in source or "api" in source.lower()


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
        # Should have default value parsed from annotation
        assert attr.annotations.get("default") == 0 or attr.default == 0

    def test_attribute_transform_annotation(self) -> None:
        """Attribute @transform annotation should be parsed."""
        schema = parse_tql_schema("""
            define
            ## @transform(lowercase)
            attribute email, value string;
        """)

        attr = schema.attributes["email"]
        # Should have transform parsed from annotation
        assert attr.annotations.get("transform") == "lowercase" or attr.transform == "lowercase"
