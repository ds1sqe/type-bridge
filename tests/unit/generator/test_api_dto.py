"""Tests for the API DTO generator."""

from __future__ import annotations

from type_bridge.generator import parse_tql_schema
from type_bridge.generator.api_dto import render_api_dto
from type_bridge.generator.dto_config import (
    BaseClassConfig,
    CompositeEntityConfig,
    CompositeFieldConfig,
    DTOConfig,
    EntityFieldOverride,
    FieldOverride,
    FieldSyncConfig,
    ValidatorConfig,
)


class TestRenderApiDto:
    """Tests for API DTO rendering."""

    def test_basic_entity_dto(self) -> None:
        """Render basic entity DTOs."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            attribute name, value string;
        """)
        source = render_api_dto(schema)

        assert "class PersonOut(BaseDTOOut):" in source
        assert "class PersonCreate(BaseDTOCreate):" in source
        assert "class PersonPatch(BaseDTOPatch):" in source
        assert 'type: Literal["person"] = "person"' in source
        assert "name: Optional[str]" in source

    def test_basic_relation_dto(self) -> None:
        """Render basic relation DTOs."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name,
                plays friendship:friend;
            attribute name, value string;
            relation friendship,
                relates friend @card(2);
        """)
        source = render_api_dto(schema)

        assert "class FriendshipOut(BaseRelationOut):" in source
        assert "class FriendshipCreate(BaseRelationCreate):" in source
        assert "friend_iid: Optional[str]" in source
        assert "friend_id: str" in source  # Required since card(2) means min=2

    def test_exclude_entities(self) -> None:
        """Test that excluded entities are not generated."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            entity internal_counter,
                owns count;
            attribute name, value string;
            attribute count, value integer;
        """)
        config = DTOConfig(exclude_entities=["internal_counter"])
        source = render_api_dto(schema, config)

        assert "class PersonOut" in source
        assert "InternalCounter" not in source

    def test_iid_field_name_default(self) -> None:
        """Test default IID field name."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            attribute name, value string;
        """)
        source = render_api_dto(schema)

        assert "iid: str = Field(description=" in source

    def test_iid_field_name_custom(self) -> None:
        """Test custom IID field name (id instead of iid)."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            attribute name, value string;
        """)
        config = DTOConfig(iid_field_name="id")
        source = render_api_dto(schema, config)

        assert "id: str = Field(description=" in source
        assert "iid: str" not in source

    def test_custom_validator_with_pattern(self) -> None:
        """Test custom validator generation with regex pattern."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns display_id;
            attribute display_id, value string;
        """)
        config = DTOConfig(
            validators=[
                ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,5}-\d+$"),
            ]
        )
        source = render_api_dto(schema, config)

        assert '_DISPLAYID_PATTERN = re.compile(r"^[A-Z]{1,5}-\\d+$")' in source
        assert "def _validate_displayid(value: str) -> str:" in source
        assert "DisplayId = Annotated[str, AfterValidator(_validate_displayid)]" in source

    def test_preamble_injection(self) -> None:
        """Test that custom preamble is injected."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            attribute name, value string;
        """)
        config = DTOConfig(
            preamble="""
# Custom validator
def _validate_custom(value: str) -> str:
    return value.upper()

CustomType = Annotated[str, AfterValidator(_validate_custom)]
"""
        )
        source = render_api_dto(schema, config)

        assert "# Custom validator" in source
        assert "def _validate_custom(value: str) -> str:" in source
        assert "CustomType = Annotated[str, AfterValidator(_validate_custom)]" in source

    def test_custom_base_class(self) -> None:
        """Test custom base class for entity hierarchy."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns display_id,
                owns name,
                owns description;
            entity task sub artifact,
                owns status;
            attribute display_id, value string;
            attribute name, value string;
            attribute description, value string;
            attribute status, value string;
        """)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["display_id", "name", "description"],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Base class should be generated
        assert "class BaseArtifactOut(BaseDTOOut):" in source
        assert "class BaseArtifactCreate(BaseDTOCreate):" in source
        assert "class BaseArtifactPatch(BaseDTOPatch):" in source

        # Task should inherit from BaseArtifact
        assert "class TaskOut(BaseArtifactOut):" in source
        assert "class TaskCreate(BaseArtifactCreate):" in source

        # Task should only have its own attributes, not inherited ones
        assert "status: Optional[str]" in source

    def test_base_class_with_field_sync(self) -> None:
        """Test base class with field sync validator."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns description,
                owns content;
            entity document sub artifact;
            attribute description, value string;
            attribute content, value string;
        """)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["description", "content"],
                    field_syncs=[FieldSyncConfig("description", "content")],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "@model_validator(mode=" in source
        assert "_sync_description_content" in source
        assert "self.content = self.description" in source

    def test_base_class_with_extra_fields(self) -> None:
        """Test base class with extra fields."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity document sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["name"],
                    extra_fields={"version": "int | None = None"},
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "version: int | None = None" in source

    def test_entity_union_name(self) -> None:
        """Test custom entity union name."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name;
            attribute name, value string;
        """)
        config = DTOConfig(entity_union_name="GraphNode")
        source = render_api_dto(schema, config)

        assert "GraphNodeOut = Annotated[" in source
        assert "GraphNodeCreate = Annotated[" in source
        assert "GraphNodePatch = Annotated[" in source

    def test_relation_union_name(self) -> None:
        """Test custom relation union name."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;
            relation friendship,
                relates friend @card(2);
        """)
        config = DTOConfig(relation_union_name="GraphRelation")
        source = render_api_dto(schema, config)

        assert "GraphRelationCreate = Annotated[" in source

    def test_skip_relation_output(self) -> None:
        """Test that relation output DTOs are skipped when configured."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;
            relation friendship,
                relates friend @card(2);
        """)
        config = DTOConfig(skip_relation_output=True)
        source = render_api_dto(schema, config)

        # Create should still be generated
        assert "class FriendshipCreate" in source
        # Out should be skipped
        assert "class FriendshipOut" not in source
        assert "class BaseRelationOut" not in source

    def test_relation_create_base_class(self) -> None:
        """Test custom base class for relation create DTOs."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;
            relation friendship,
                relates friend @card(2);
        """)
        config = DTOConfig(
            relation_create_base_class="CustomRelationBase",
            relation_preamble='''
class CustomRelationBase(BaseDTO):
    """Custom base for relation creates."""
    source_id: str
    target_id: str
''',
        )
        source = render_api_dto(schema, config)

        # Custom preamble should be included
        assert "class CustomRelationBase(BaseDTO):" in source
        assert "source_id: str" in source
        assert "target_id: str" in source

        # Relation should inherit from custom base
        assert "class FriendshipCreate(CustomRelationBase):" in source

        # Role-specific fields should NOT be generated
        assert "friend_id:" not in source

    def test_relation_preamble(self) -> None:
        """Test relation preamble injection."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays friendship:friend;
            relation friendship,
                relates friend @card(2);
        """)
        config = DTOConfig(
            relation_preamble='''
class GraphEdgeOut(BaseDTO):
    """Custom edge output."""
    iid: str
    source: str
    target: str
    type: str
'''
        )
        source = render_api_dto(schema, config)

        assert "class GraphEdgeOut(BaseDTO):" in source
        assert "source: str" in source

    def test_relation_with_owned_attributes(self) -> None:
        """Test relation with owned attributes uses custom base but keeps attrs."""
        schema = parse_tql_schema("""
            define
            entity task,
                plays task_grouping:task;
            entity milestone,
                plays task_grouping:milestone;
            relation task_grouping,
                relates milestone @card(1),
                relates task @card(0..),
                owns sequence_index;
            attribute sequence_index, value integer;
        """)
        config = DTOConfig(
            relation_create_base_class="CustomRelationBase",
            relation_preamble="""
class CustomRelationBase(BaseDTO):
    source_id: str
    target_id: str
""",
        )
        source = render_api_dto(schema, config)

        # Should inherit from custom base
        assert "class TaskGroupingCreate(CustomRelationBase):" in source
        # Should still have owned attributes
        assert "sequence_index: Optional[int]" in source

    def test_abstract_entities_excluded_from_union(self) -> None:
        """Test that abstract entities are not in union types."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            attribute name, value string;
        """)
        source = render_api_dto(schema)

        # Abstract entity should not generate Out/Create/Patch classes
        assert "class ArtifactOut" not in source
        # Concrete entity should be generated
        assert "class TaskOut" in source
        # Union should only include concrete types
        assert "TaskOut," in source
        assert "ArtifactOut," not in source

    def test_strict_out_models_default_off(self) -> None:
        """Test that Out models use Optional by default."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name @key;
            attribute name, value string;
        """)
        source = render_api_dto(schema)

        # Even @key fields should be Optional in Out by default
        assert "name: Optional[str]" in source

    def test_strict_out_models_enabled(self) -> None:
        """Test that Out models use required fields when strict_out_models=True."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns name @key,
                owns nickname;
            attribute name, value string;
            attribute nickname, value string;
        """)
        config = DTOConfig(strict_out_models=True)
        source = render_api_dto(schema, config)

        # In PersonOut, @key field should be required (not Optional)
        out_section = source.split("class PersonOut")[1].split("class PersonCreate")[0]
        assert "name: str" in out_section
        # Non-required fields should still be Optional
        assert "nickname: Optional[str]" in out_section

    def test_name_clash_entity_vs_entity_union(self) -> None:
        """Test that entity name clashing with entity_union_name raises error."""
        schema = parse_tql_schema("""
            define
            entity entity,
                owns name;
            attribute name, value string;
        """)
        import pytest

        with pytest.raises(ValueError, match="clashes with entity_union_name"):
            render_api_dto(schema)

    def test_name_clash_custom_union_name(self) -> None:
        """Test that custom union name clashes are detected."""
        schema = parse_tql_schema("""
            define
            entity graph_node,
                owns name;
            attribute name, value string;
        """)
        import pytest

        config = DTOConfig(entity_union_name="GraphNode")
        with pytest.raises(ValueError, match="clashes with entity_union_name"):
            render_api_dto(schema, config)

    def test_name_clash_entity_vs_relation_union(self) -> None:
        """Test entity name clashing with relation_union_name."""
        schema = parse_tql_schema("""
            define
            entity relation,
                owns name;
            attribute name, value string;
        """)
        import pytest

        with pytest.raises(ValueError, match="clashes with relation_union_name"):
            render_api_dto(schema)

    def test_name_clash_relation_vs_entity_union(self) -> None:
        """Test relation name clashing with entity_union_name."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays entity_link:person;
            relation entity,
                relates person;
            attribute name, value string;
        """)
        import pytest

        with pytest.raises(ValueError, match="clashes with entity_union_name"):
            render_api_dto(schema)

    def test_name_clash_relation_vs_relation_union(self) -> None:
        """Test relation name clashing with relation_union_name."""
        schema = parse_tql_schema("""
            define
            entity person,
                plays relation_link:person;
            relation relation,
                relates person;
        """)
        import pytest

        with pytest.raises(ValueError, match="clashes with relation_union_name"):
            render_api_dto(schema)

    def test_unique_attribute_is_optional_without_required_cardinality(self) -> None:
        """Bare @unique retains the default optional cardinality in Create DTOs."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns email @unique,
                owns handle @unique @card(1..1);
            attribute email, value string;
            attribute handle, value string;
        """)
        source = render_api_dto(schema)

        create_section = source.split("class PersonCreate")[1].split("class PersonPatch")[0]
        assert "email: Optional[str] = None" in create_section
        assert "handle: str" in create_section

    def test_cardinality_determines_required(self) -> None:
        """Test that cardinality determines if field is required."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns required_field @card(1..),
                owns optional_field @card(0..1);
            attribute required_field, value string;
            attribute optional_field, value string;
        """)
        source = render_api_dto(schema)

        create_section = source.split("class PersonCreate")[1].split("class PersonPatch")[0]
        # min=1 should be required
        assert "required_field: str" in create_section
        # min=0 should be optional
        assert "optional_field: Optional[str]" in create_section

    def test_deep_inheritance_chain(self) -> None:
        """Test entities with multi-level inheritance."""
        schema = parse_tql_schema("""
            define
            entity thing @abstract,
                owns name;
            entity artifact @abstract sub thing,
                owns description;
            entity task sub artifact,
                owns status;
            attribute name, value string;
            attribute description, value string;
            attribute status, value string;
        """)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="thing",
                    base_name="BaseThing",
                    inherited_attrs=["name"],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Task should eventually inherit from BaseThing
        assert "class TaskOut(BaseThingOut):" in source
        # Task should have its own attrs
        task_section = source.split("class TaskOut")[1].split("class TaskCreate")[0]
        assert "status:" in task_section

    def test_base_class_for_source_entity_itself(self) -> None:
        """Test that source entity uses default base, not custom base."""
        schema = parse_tql_schema("""
            define
            entity artifact,
                owns name;
            attribute name, value string;
        """)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["name"],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # artifact itself should NOT inherit from BaseArtifact (that would be circular)
        # It should inherit from BaseDTOOut
        assert "class ArtifactOut(BaseDTOOut):" in source


class TestDTOConfigMethods:
    """Tests for DTOConfig helper methods."""

    def test_get_base_class_for_entity_direct_match(self) -> None:
        """Test finding base class for direct source entity."""
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["name"],
                ),
            ]
        )
        # Mock schema entities - direct match
        schema_entities = {
            "artifact": type("EntitySpec", (), {"parent": None})(),
        }

        result = config.get_base_class_for_entity("artifact", schema_entities)
        assert result is not None
        assert result.base_name == "BaseArtifact"

    def test_get_base_class_for_entity_inheritance(self) -> None:
        """Test finding base class through inheritance chain."""
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["name"],
                ),
            ]
        )
        # Mock schema entities - task inherits from artifact
        schema_entities = {
            "artifact": type("EntitySpec", (), {"parent": None})(),
            "task": type("EntitySpec", (), {"parent": "artifact"})(),
        }

        result = config.get_base_class_for_entity("task", schema_entities)
        assert result is not None
        assert result.base_name == "BaseArtifact"

    def test_get_base_class_for_entity_no_match(self) -> None:
        """Test when no base class matches."""
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["name"],
                ),
            ]
        )
        # Mock schema entities - person doesn't inherit from artifact
        schema_entities = {
            "person": type("EntitySpec", (), {"parent": None})(),
        }

        result = config.get_base_class_for_entity("person", schema_entities)
        assert result is None

    def test_get_base_class_for_entity_empty_config(self) -> None:
        """Test with no base classes configured."""
        config = DTOConfig()
        schema_entities = {
            "person": type("EntitySpec", (), {"parent": None})(),
        }

        result = config.get_base_class_for_entity("person", schema_entities)
        assert result is None


class TestCompositeEntityConfig:
    """Tests for composite entity DTOs."""

    def test_composite_entity_basic(self) -> None:
        """Test basic composite entity generation."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact,
                owns status;
            entity epic sub artifact,
                owns target_date;
            attribute name, value string;
            attribute status, value string;
            attribute target_date, value date;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Composite DTOs should be generated
        assert "class GraphNodeOut(BaseDTOOut):" in source
        assert "class GraphNodeCreate(BaseDTOCreate):" in source
        assert "class GraphNodePatch(BaseDTOPatch):" in source

        # Type literal should be generated
        assert (
            'GraphNodeType = Literal["task", "epic"]' in source
            or 'GraphNodeType = Literal["epic", "task"]' in source
        )

    def test_composite_entity_with_common_fields(self) -> None:
        """Test composite with explicitly configured common fields."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name,
                owns description;
            entity task sub artifact;
            attribute name, value string;
            attribute description, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                        CompositeFieldConfig("description", "str", default="''"),
                    ],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # In Create, required field should have no default
        assert "name: str" in source
        # Optional field should have default in Create
        assert "description: Optional[str] = ''" in source

    def test_composite_entity_with_extra_fields(self) -> None:
        """Test composite with extra fields not in schema."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                    extra_fields={"version": "int | None = None"},
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "version: int | None = None" in source

    def test_composite_entity_with_field_sync(self) -> None:
        """Test composite with field sync validator."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns description,
                owns content;
            entity task sub artifact;
            attribute description, value string;
            attribute content, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                    field_syncs=[FieldSyncConfig("description", "content")],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "@model_validator(mode=" in source
        assert "_sync_description_content" in source

    def test_composite_entity_exclude_entities(self) -> None:
        """Test composite with excluded entities."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            entity internal_type sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                    exclude_entities=["internal_type"],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Only task should be in the type literal
        assert 'GraphNodeType = Literal["task"]' in source
        assert "internal_type" not in source.split("GraphNodeType")[1].split("\n")[0]

    def test_composite_entity_include_entities(self) -> None:
        """Test composite with explicit include list."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            entity epic sub artifact;
            entity story sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Only specified entities in type literal
        type_literal_line = [
            line for line in source.split("\n") if "GraphNodeType = Literal" in line
        ][0]
        assert "task" in type_literal_line
        assert "epic" in type_literal_line
        assert "story" not in type_literal_line

    def test_composite_entity_custom_field_names(self) -> None:
        """Test composite with custom id and type field names."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                    id_field_name="node_id",
                    type_field_name="node_type",
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Check type field name is used
        assert "node_type: GraphNodeType" in source

    def test_composite_entity_merges_attributes_from_all_entities(self) -> None:
        """Test that composite collects attributes from all included entities."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact,
                owns status,
                owns priority;
            entity epic sub artifact,
                owns target_date;
            attribute name, value string;
            attribute status, value string;
            attribute priority, value integer;
            attribute target_date, value date;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # All attributes from all entities should be present in composite
        # Check in the GraphNodeOut section
        graphnode_section = source.split("class GraphNodeOut")[1].split("class ")[0]
        assert "name:" in graphnode_section
        assert "status:" in graphnode_section
        assert "priority:" in graphnode_section
        assert "target_date:" in graphnode_section

    def test_composite_entity_alongside_per_entity_dtos(self) -> None:
        """Test that composite DTOs are generated alongside per-entity DTOs."""
        schema = parse_tql_schema("""
            define
            entity artifact @abstract,
                owns name;
            entity task sub artifact;
            attribute name, value string;
        """)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    base_entity="artifact",
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Per-entity DTOs should still be generated
        assert "class TaskOut(BaseDTOOut):" in source
        assert "class TaskCreate(BaseDTOCreate):" in source
        assert "class TaskPatch(BaseDTOPatch):" in source

        # Composite DTOs should also be generated
        assert "class GraphNodeOut(BaseDTOOut):" in source
        assert "class GraphNodeCreate(BaseDTOCreate):" in source
        assert "class GraphNodePatch(BaseDTOPatch):" in source


class TestCompositeEntityConfigMethods:
    """Tests for CompositeEntityConfig helper methods."""

    def test_get_included_entities_with_base_entity(self) -> None:
        """Test getting included entities via inheritance from base_entity."""
        comp = CompositeEntityConfig(
            name="GraphNode",
            base_entity="artifact",
        )
        # Mock schema entities
        mock_abstract = type("EntitySpec", (), {"parent": None, "abstract": True})()
        mock_task = type("EntitySpec", (), {"parent": "artifact", "abstract": False})()
        mock_epic = type("EntitySpec", (), {"parent": "artifact", "abstract": False})()

        schema_entities = {
            "artifact": mock_abstract,
            "task": mock_task,
            "epic": mock_epic,
        }

        result = comp.get_included_entities(schema_entities)
        assert "task" in result
        assert "epic" in result
        assert "artifact" not in result  # Abstract, so excluded

    def test_get_included_entities_with_exclude(self) -> None:
        """Test excluding specific entities."""
        comp = CompositeEntityConfig(
            name="GraphNode",
            base_entity="artifact",
            exclude_entities=["epic"],
        )
        mock_abstract = type("EntitySpec", (), {"parent": None, "abstract": True})()
        mock_task = type("EntitySpec", (), {"parent": "artifact", "abstract": False})()
        mock_epic = type("EntitySpec", (), {"parent": "artifact", "abstract": False})()

        schema_entities = {
            "artifact": mock_abstract,
            "task": mock_task,
            "epic": mock_epic,
        }

        result = comp.get_included_entities(schema_entities)
        assert "task" in result
        assert "epic" not in result

    def test_get_included_entities_with_include_list(self) -> None:
        """Test explicit include list."""
        comp = CompositeEntityConfig(
            name="GraphNode",
            include_entities=["task", "epic"],
        )
        # Note: with explicit include_entities, we don't need the schema
        schema_entities = {}

        result = comp.get_included_entities(schema_entities)
        assert result == ["task", "epic"]

    def test_get_included_entities_include_with_exclude(self) -> None:
        """Test include list with exclusions."""
        comp = CompositeEntityConfig(
            name="GraphNode",
            include_entities=["task", "epic", "story"],
            exclude_entities=["story"],
        )
        schema_entities = {}

        result = comp.get_included_entities(schema_entities)
        assert "task" in result
        assert "epic" in result
        assert "story" not in result


class TestFieldOverrides:
    """Tests for per-variant field requiredness overrides."""

    SCHEMA_WITH_KEY = """
        define
        entity artifact @abstract,
            owns display_id @key,
            owns name;
        entity task sub artifact,
            owns status;
        attribute display_id, value string;
        attribute name, value string;
        attribute status, value string;
    """

    def test_base_class_override_makes_key_optional_on_create(self) -> None:
        """Base class create_field_overrides makes @key optional on Create."""
        schema = parse_tql_schema(self.SCHEMA_WITH_KEY)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["display_id", "name"],
                    create_field_overrides={
                        "display_id": FieldOverride(required=False),
                    },
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # In BaseArtifactCreate, display_id should be Optional (overridden)
        create_section = source.split("class BaseArtifactCreate")[1].split("class ")[0]
        assert "display_id: Optional[str]" in create_section

    def test_base_class_override_with_custom_default(self) -> None:
        """Base class create_field_overrides with a custom default value."""
        schema = parse_tql_schema(self.SCHEMA_WITH_KEY)
        config = DTOConfig(
            base_classes=[
                BaseClassConfig(
                    source_entity="artifact",
                    base_name="BaseArtifact",
                    inherited_attrs=["display_id", "name"],
                    create_field_overrides={
                        "display_id": FieldOverride(required=False, default='"AUTO"'),
                    },
                ),
            ]
        )
        source = render_api_dto(schema, config)

        create_section = source.split("class BaseArtifactCreate")[1].split("class ")[0]
        assert 'display_id: Optional[str] = "AUTO"' in create_section

    def test_per_entity_override_on_create(self) -> None:
        """Per-entity override makes @key optional on Create for a specific entity."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns email @key,
                owns name;
            attribute email, value string;
            attribute name, value string;
        """)
        config = DTOConfig(
            entity_field_overrides=[
                EntityFieldOverride(
                    entity="person",
                    field="email",
                    variant="create",
                    required=False,
                ),
            ]
        )
        source = render_api_dto(schema, config)

        create_section = source.split("class PersonCreate")[1].split("class PersonPatch")[0]
        assert "email: Optional[str]" in create_section

    def test_per_entity_override_on_out_with_strict(self) -> None:
        """Per-entity override on Out variant with strict_out_models."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns email @key,
                owns name;
            attribute email, value string;
            attribute name, value string;
        """)
        config = DTOConfig(
            strict_out_models=True,
            entity_field_overrides=[
                EntityFieldOverride(
                    entity="person",
                    field="email",
                    variant="out",
                    required=False,
                ),
            ],
        )
        source = render_api_dto(schema, config)

        out_section = source.split("class PersonOut")[1].split("class PersonCreate")[0]
        # email should be Optional in Out even though strict_out_models=True
        assert "email: Optional[str]" in out_section
        # name should still be Optional (not @key, strict doesn't affect non-required)
        assert "name: Optional[str]" in out_section

    def test_override_scoped_to_one_entity(self) -> None:
        """Override scoped to one entity doesn't leak to another."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns email @key;
            entity company,
                owns email @key;
            attribute email, value string;
        """)
        config = DTOConfig(
            entity_field_overrides=[
                EntityFieldOverride(
                    entity="person",
                    field="email",
                    variant="create",
                    required=False,
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # Person's email should be optional on Create
        person_create = source.split("class PersonCreate")[1].split("class PersonPatch")[0]
        assert "email: Optional[str]" in person_create

        # Company's email should still be required on Create
        company_create = source.split("class CompanyCreate")[1].split("class CompanyPatch")[0]
        assert "email: str" in company_create

    def test_create_override_does_not_bleed_into_out_or_patch(self) -> None:
        """Create override doesn't affect Out or Patch variants."""
        schema = parse_tql_schema("""
            define
            entity person,
                owns email @key;
            attribute email, value string;
        """)
        config = DTOConfig(
            strict_out_models=True,
            entity_field_overrides=[
                EntityFieldOverride(
                    entity="person",
                    field="email",
                    variant="create",
                    required=False,
                ),
            ],
        )
        source = render_api_dto(schema, config)

        # Create: email should be Optional (overridden)
        create_section = source.split("class PersonCreate")[1].split("class PersonPatch")[0]
        assert "email: Optional[str]" in create_section

        # Out: email should still be required (strict_out_models + @key)
        out_section = source.split("class PersonOut")[1].split("class PersonCreate")[0]
        assert "email: str" in out_section

        # Patch: always Optional, unchanged
        patch_section = source.split("class PersonPatch")[1].split("\n\n\n")[0]
        assert "email: Optional[str] = None" in patch_section


class TestSkipVariants:
    """Tests for CompositeEntityConfig.skip_variants."""

    SCHEMA = """
        define
        entity task,
            owns name,
            owns status;
        entity epic,
            owns name;
        attribute name, value string;
        attribute status, value string;
    """

    def test_skip_all_variants_keeps_type_enum(self) -> None:
        """skip_variants={out, create, patch} still generates the type Literal."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    skip_variants={"out", "create", "patch"},
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "GraphNodeType = Literal[" in source
        assert "class GraphNodeOut" not in source
        assert "class GraphNodeCreate" not in source
        assert "class GraphNodePatch" not in source

    def test_skip_out_only(self) -> None:
        """skip_variants={out} skips Out but keeps Create and Patch."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                    ],
                    skip_variants={"out"},
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "GraphNodeType = Literal[" in source
        assert "class GraphNodeOut" not in source
        assert "class GraphNodeCreate" in source
        assert "class GraphNodePatch" in source

    def test_skip_create_and_patch(self) -> None:
        """skip_variants={create, patch} skips Create/Patch but keeps Out."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                    ],
                    skip_variants={"create", "patch"},
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "GraphNodeType = Literal[" in source
        assert "class GraphNodeOut" in source
        assert "class GraphNodeCreate" not in source
        assert "class GraphNodePatch" not in source

    def test_empty_skip_variants_generates_all(self) -> None:
        """Empty skip_variants generates all three variant classes."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                    ],
                ),
            ]
        )
        source = render_api_dto(schema, config)

        assert "class GraphNodeOut" in source
        assert "class GraphNodeCreate" in source
        assert "class GraphNodePatch" in source


class TestExtraFieldsOut:
    """Tests for CompositeEntityConfig.extra_fields_out."""

    SCHEMA = """
        define
        entity task,
            owns name;
        entity epic,
            owns name;
        attribute name, value string;
    """

    def test_extra_fields_out_overrides_on_out_variant(self) -> None:
        """extra_fields_out overrides the annotation on Out but not Create/Patch."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                    ],
                    extra_fields={
                        "id": "Optional[str] = None",
                        "version": "int | None = None",
                    },
                    extra_fields_out={
                        "id": "str",  # Required on Out
                    },
                ),
            ]
        )
        source = render_api_dto(schema, config)

        out_section = source.split("class GraphNodeOut")[1].split("class GraphNodeCreate")[0]
        create_section = source.split("class GraphNodeCreate")[1].split("class GraphNodePatch")[0]

        # Out uses the override
        assert "id: str" in out_section
        # Out still has version from extra_fields (not overridden)
        assert "version: int | None = None" in out_section

        # Create uses the default from extra_fields
        assert "id: Optional[str] = None" in create_section
        assert "version: int | None = None" in create_section

    def test_extra_fields_out_without_base_extra_field(self) -> None:
        """extra_fields_out for a field not in extra_fields is ignored."""
        schema = parse_tql_schema(self.SCHEMA)
        config = DTOConfig(
            composite_entities=[
                CompositeEntityConfig(
                    name="GraphNode",
                    include_entities=["task", "epic"],
                    common_fields=[
                        CompositeFieldConfig("name", "str"),
                    ],
                    extra_fields={
                        "version": "int | None = None",
                    },
                    extra_fields_out={
                        "phantom": "str",  # Not in extra_fields
                    },
                ),
            ]
        )
        source = render_api_dto(schema, config)

        # phantom should not appear (only overrides, doesn't add)
        out_section = source.split("class GraphNodeOut")[1].split("class GraphNodeCreate")[0]
        assert "phantom" not in out_section
