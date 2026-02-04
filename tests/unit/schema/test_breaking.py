"""Unit tests for breaking change analyzer."""

from type_bridge import Entity, Integer, Relation, Role, String, TypeFlags
from type_bridge.migration.breaking import (
    BreakingChangeAnalyzer,
    ChangeCategory,
    ClassifiedChange,
)
from type_bridge.migration.diff import (
    AttributeFlagChange,
    EntityChanges,
    RelationChanges,
    RoleCardinalityChange,
    RolePlayerChange,
    SchemaDiff,
)


# Test fixtures
class Name(String):
    pass


class Age(Integer):
    pass


class Email(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)


class TestChangeCategory:
    """Tests for ChangeCategory enum."""

    def test_enum_values(self):
        """ChangeCategory should have correct values."""
        assert ChangeCategory.SAFE.value == "safe"
        assert ChangeCategory.WARNING.value == "warning"
        assert ChangeCategory.BREAKING.value == "breaking"


class TestClassifiedChange:
    """Tests for ClassifiedChange dataclass."""

    def test_fields(self):
        """ClassifiedChange should have correct fields."""
        change = ClassifiedChange(
            description="Add entity: person",
            category=ChangeCategory.SAFE,
            recommendation="No action needed",
        )

        assert change.description == "Add entity: person"
        assert change.category == ChangeCategory.SAFE
        assert change.recommendation == "No action needed"


class TestBreakingChangeAnalyzerEntities:
    """Tests for entity change classification."""

    def test_added_entity_is_safe(self):
        """Adding an entity should be classified as SAFE."""
        diff = SchemaDiff(added_entities={Person})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.SAFE
        assert "Add entity" in changes[0].description

    def test_removed_entity_is_breaking(self):
        """Removing an entity should be classified as BREAKING."""
        diff = SchemaDiff(removed_entities={Person})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove entity" in changes[0].description


class TestBreakingChangeAnalyzerAttributes:
    """Tests for attribute change classification."""

    def test_added_attribute_type_is_safe(self):
        """Adding an attribute type should be classified as SAFE."""
        diff = SchemaDiff(added_attributes={Name})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.SAFE
        assert "Add attribute type" in changes[0].description

    def test_removed_attribute_type_is_breaking(self):
        """Removing an attribute type should be classified as BREAKING."""
        diff = SchemaDiff(removed_attributes={Name})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove attribute type" in changes[0].description


class TestBreakingChangeAnalyzerRelations:
    """Tests for relation change classification."""

    def test_added_relation_is_safe(self):
        """Adding a relation should be classified as SAFE."""
        diff = SchemaDiff(added_relations={Employment})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.SAFE
        assert "Add relation" in changes[0].description

    def test_removed_relation_is_breaking(self):
        """Removing a relation should be classified as BREAKING."""
        diff = SchemaDiff(removed_relations={Employment})

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove relation" in changes[0].description


class TestBreakingChangeAnalyzerRoles:
    """Tests for role change classification."""

    def test_added_role_is_safe(self):
        """Adding a role should be classified as SAFE."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(added_roles=["manager"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.SAFE
        assert "Add role" in changes[0].description

    def test_removed_role_is_breaking(self):
        """Removing a role should be classified as BREAKING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(removed_roles=["manager"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove role" in changes[0].description


class TestBreakingChangeAnalyzerRolePlayers:
    """Tests for role player change classification."""

    def test_added_role_player_is_safe(self):
        """Adding a role player type should be classified as SAFE (widening)."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_role_players=[
                        RolePlayerChange(
                            role_name="employee",
                            added_player_types=["contractor"],
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.SAFE
        assert "Add player type" in changes[0].description

    def test_removed_role_player_is_breaking(self):
        """Removing a role player type should be classified as BREAKING (narrowing)."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_role_players=[
                        RolePlayerChange(
                            role_name="employee",
                            removed_player_types=["contractor"],
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove player type" in changes[0].description


class TestBreakingChangeAnalyzerOwnership:
    """Tests for ownership change classification."""

    def test_added_entity_attribute_is_warning(self):
        """Adding attribute ownership to entity should be WARNING."""
        diff = SchemaDiff(
            modified_entities={
                Person: EntityChanges(added_attributes=["email"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.WARNING
        assert "Add attribute" in changes[0].description

    def test_removed_entity_attribute_is_breaking(self):
        """Removing attribute ownership from entity should be BREAKING."""
        diff = SchemaDiff(
            modified_entities={
                Person: EntityChanges(removed_attributes=["email"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Remove attribute" in changes[0].description


class TestBreakingChangeAnalyzerCardinality:
    """Tests for cardinality change classification."""

    def test_increased_minimum_is_breaking(self):
        """Increasing minimum cardinality should be BREAKING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_role_cardinality=[
                        RoleCardinalityChange(
                            role_name="employee",
                            old_cardinality=(0, None),
                            new_cardinality=(1, None),
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Increase minimum" in changes[0].description

    def test_decreased_maximum_is_breaking(self):
        """Decreasing maximum cardinality should be BREAKING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_role_cardinality=[
                        RoleCardinalityChange(
                            role_name="employee",
                            old_cardinality=(0, 10),
                            new_cardinality=(0, 5),
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
        assert "Decrease maximum" in changes[0].description

    def test_relaxed_cardinality_is_safe(self):
        """Relaxing cardinality should be SAFE."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_role_cardinality=[
                        RoleCardinalityChange(
                            role_name="employee",
                            old_cardinality=(1, 5),
                            new_cardinality=(0, 10),
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        # Two changes: decreased min (safe) and increased max (safe)
        safe_changes = [c for c in changes if c.category == ChangeCategory.SAFE]
        assert len(safe_changes) >= 1


class TestBreakingChangeAnalyzerModifiedAttributeFlags:
    """Tests for modified attribute flag classification."""

    def test_modified_entity_attribute_flags_is_warning(self):
        """Modifying entity attribute flags should be WARNING."""
        diff = SchemaDiff(
            modified_entities={
                Person: EntityChanges(
                    modified_attributes=[
                        AttributeFlagChange(
                            name="name", old_flags="@card(0..1)", new_flags="@card(1..1)"
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.WARNING
        assert "Modify attribute flags" in changes[0].description

    def test_modified_relation_attribute_flags_is_warning(self):
        """Modifying relation attribute flags should be WARNING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(
                    modified_attributes=[
                        AttributeFlagChange(
                            name="start_date",
                            old_flags="@card(0..1)",
                            new_flags="@card(1..1)",
                        )
                    ]
                ),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.WARNING
        assert "Modify attribute flags" in changes[0].description


class TestBreakingChangeAnalyzerHelpers:
    """Tests for BreakingChangeAnalyzer helper methods."""

    def test_has_breaking_changes_true(self):
        """has_breaking_changes should return True when breaking changes exist."""
        diff = SchemaDiff(removed_entities={Person})

        analyzer = BreakingChangeAnalyzer()

        assert analyzer.has_breaking_changes(diff) is True

    def test_has_breaking_changes_false(self):
        """has_breaking_changes should return False when no breaking changes."""
        diff = SchemaDiff(added_entities={Person})

        analyzer = BreakingChangeAnalyzer()

        assert analyzer.has_breaking_changes(diff) is False

    def test_has_warnings_true(self):
        """has_warnings should return True when warnings exist."""
        diff = SchemaDiff(
            modified_entities={
                Person: EntityChanges(added_attributes=["email"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()

        assert analyzer.has_warnings(diff) is True

    def test_has_warnings_false(self):
        """has_warnings should return False when no warnings."""
        diff = SchemaDiff(added_entities={Person})

        analyzer = BreakingChangeAnalyzer()

        assert analyzer.has_warnings(diff) is False

    def test_get_breaking_changes(self):
        """get_breaking_changes should return only breaking changes."""
        diff = SchemaDiff(
            added_entities={Company},  # SAFE
            removed_entities={Person},  # BREAKING
        )

        analyzer = BreakingChangeAnalyzer()
        breaking = analyzer.get_breaking_changes(diff)

        assert len(breaking) == 1
        assert breaking[0].category == ChangeCategory.BREAKING


class TestBreakingChangeAnalyzerSummary:
    """Tests for BreakingChangeAnalyzer.summary()."""

    def test_summary_no_changes(self):
        """Summary should indicate no changes for empty diff."""
        diff = SchemaDiff()

        analyzer = BreakingChangeAnalyzer()
        summary = analyzer.summary(diff)

        assert "No schema changes detected" in summary

    def test_summary_grouped_by_category(self):
        """Summary should group changes by category."""
        diff = SchemaDiff(
            added_entities={Company},  # SAFE
            removed_entities={Person},  # BREAKING
            modified_entities={
                Person: EntityChanges(added_attributes=["email"]),  # WARNING
            },
        )

        analyzer = BreakingChangeAnalyzer()
        summary = analyzer.summary(diff)

        assert "[BREAKING]" in summary
        assert "[WARNING]" in summary
        assert "[SAFE]" in summary

    def test_summary_includes_recommendations(self):
        """Summary should include recommendations for breaking changes."""
        diff = SchemaDiff(removed_entities={Person})

        analyzer = BreakingChangeAnalyzer()
        summary = analyzer.summary(diff)

        assert "Recommendation" in summary


class TestBreakingChangeAnalyzerRelationAttributes:
    """Tests for relation attribute change classification."""

    def test_added_relation_attribute_is_warning(self):
        """Adding attribute to relation should be WARNING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(added_attributes=["start_date"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.WARNING

    def test_removed_relation_attribute_is_breaking(self):
        """Removing attribute from relation should be BREAKING."""
        diff = SchemaDiff(
            modified_relations={
                Employment: RelationChanges(removed_attributes=["start_date"]),
            }
        )

        analyzer = BreakingChangeAnalyzer()
        changes = analyzer.analyze(diff)

        assert len(changes) == 1
        assert changes[0].category == ChangeCategory.BREAKING
