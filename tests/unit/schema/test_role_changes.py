"""Unit tests for role change dataclasses."""

from type_bridge.migration.diff import (
    AttributeFlagChange,
    RelationChanges,
    RoleCardinalityChange,
    RolePlayerChange,
)


class TestRolePlayerChange:
    """Tests for RolePlayerChange dataclass."""

    def test_has_changes_with_added(self):
        """has_changes should return True when player types added."""
        change = RolePlayerChange(
            role_name="employee",
            added_player_types=["contractor"],
        )

        assert change.has_changes() is True

    def test_has_changes_with_removed(self):
        """has_changes should return True when player types removed."""
        change = RolePlayerChange(
            role_name="employee",
            removed_player_types=["contractor"],
        )

        assert change.has_changes() is True

    def test_has_changes_with_both(self):
        """has_changes should return True when both added and removed."""
        change = RolePlayerChange(
            role_name="employee",
            added_player_types=["contractor"],
            removed_player_types=["intern"],
        )

        assert change.has_changes() is True

    def test_has_changes_empty(self):
        """has_changes should return False when no changes."""
        change = RolePlayerChange(role_name="employee")

        assert change.has_changes() is False

    def test_defaults_empty_lists(self):
        """RolePlayerChange should default to empty lists."""
        change = RolePlayerChange(role_name="employee")

        assert change.added_player_types == []
        assert change.removed_player_types == []


class TestRoleCardinalityChange:
    """Tests for RoleCardinalityChange dataclass."""

    def test_cardinality_tuple_format(self):
        """RoleCardinalityChange should store (min, max) tuples."""
        change = RoleCardinalityChange(
            role_name="employee",
            old_cardinality=(0, None),
            new_cardinality=(1, 5),
        )

        assert change.old_cardinality == (0, None)
        assert change.new_cardinality == (1, 5)

    def test_unbounded_cardinality(self):
        """None should represent unbounded cardinality."""
        change = RoleCardinalityChange(
            role_name="employee",
            old_cardinality=(None, None),
            new_cardinality=(1, None),
        )

        # Old: no constraints
        assert change.old_cardinality[0] is None
        assert change.old_cardinality[1] is None
        # New: minimum of 1, no maximum
        assert change.new_cardinality[0] == 1
        assert change.new_cardinality[1] is None


class TestRelationChangesWithRolePlayerChanges:
    """Tests for RelationChanges role player tracking."""

    def test_has_changes_with_role_player_changes(self):
        """has_changes should return True when role_player_changes exist."""
        changes = RelationChanges(
            modified_role_players=[
                RolePlayerChange(
                    role_name="employee",
                    added_player_types=["contractor"],
                )
            ]
        )

        assert changes.has_changes() is True

    def test_has_changes_empty_role_player_changes(self):
        """has_changes should handle empty role_player_changes."""
        changes = RelationChanges()

        assert changes.has_changes() is False


class TestRelationChangesWithRoleCardinalityChanges:
    """Tests for RelationChanges role cardinality tracking."""

    def test_has_changes_with_role_cardinality_changes(self):
        """has_changes should return True when role_cardinality_changes exist."""
        changes = RelationChanges(
            modified_role_cardinality=[
                RoleCardinalityChange(
                    role_name="employee",
                    old_cardinality=(0, None),
                    new_cardinality=(1, None),
                )
            ]
        )

        assert changes.has_changes() is True

    def test_has_changes_empty_role_cardinality_changes(self):
        """has_changes should handle empty role_cardinality_changes."""
        changes = RelationChanges()

        assert changes.has_changes() is False


class TestRelationChangesWithModifiedAttributes:
    """Tests for RelationChanges attribute flag tracking."""

    def test_has_changes_with_modified_attributes(self):
        """has_changes should return True when modified_attributes exist."""
        changes = RelationChanges(
            modified_attributes=[
                AttributeFlagChange(
                    name="start_date",
                    old_flags="@card(0..1)",
                    new_flags="@card(1..1)",
                )
            ]
        )

        assert changes.has_changes() is True


class TestRelationChangesComprehensive:
    """Comprehensive tests for RelationChanges."""

    def test_all_change_types(self):
        """RelationChanges should track all types of changes."""
        changes = RelationChanges(
            added_roles=["manager"],
            removed_roles=["intern"],
            modified_role_players=[
                RolePlayerChange(
                    role_name="employee",
                    added_player_types=["contractor"],
                    removed_player_types=["temp"],
                )
            ],
            modified_role_cardinality=[
                RoleCardinalityChange(
                    role_name="employer",
                    old_cardinality=(1, 1),
                    new_cardinality=(1, 5),
                )
            ],
            added_attributes=["start_date"],
            removed_attributes=["end_date"],
            modified_attributes=[
                AttributeFlagChange(
                    name="notes",
                    old_flags="@card(0..1)",
                    new_flags="@card(0..5)",
                )
            ],
        )

        assert changes.has_changes() is True
        assert len(changes.added_roles) == 1
        assert len(changes.removed_roles) == 1
        assert len(changes.modified_role_players) == 1
        assert len(changes.modified_role_cardinality) == 1
        assert len(changes.added_attributes) == 1
        assert len(changes.removed_attributes) == 1
        assert len(changes.modified_attributes) == 1

    def test_defaults_empty(self):
        """RelationChanges should default all fields to empty."""
        changes = RelationChanges()

        assert changes.added_roles == []
        assert changes.removed_roles == []
        assert changes.modified_role_players == []
        assert changes.modified_role_cardinality == []
        assert changes.added_attributes == []
        assert changes.removed_attributes == []
        assert changes.modified_attributes == []


class TestAttributeFlagChange:
    """Tests for AttributeFlagChange dataclass."""

    def test_stores_old_and_new_flags(self):
        """AttributeFlagChange should store old and new flag values."""
        change = AttributeFlagChange(
            name="email",
            old_flags="@card(0..1)",
            new_flags="@card(1..1) @unique",
        )

        assert change.name == "email"
        assert change.old_flags == "@card(0..1)"
        assert change.new_flags == "@card(1..1) @unique"
