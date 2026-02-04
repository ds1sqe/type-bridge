"""Integration tests for role player change detection."""

import pytest
from type_bridge import (
    BreakingChangeAnalyzer,
    ChangeCategory,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge.attribute import AttributeFlags


# Shared attributes
class Name(String):
    flags = AttributeFlags(name="name")


# Version 1: Basic entities
class PersonV1(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)


class CompanyV1(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)


class ContractorV1(Entity):
    flags = TypeFlags(name="contractor")
    name: Name = Flag(Key)


# Version 1: Employment with only Person as employee
class EmploymentV1(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[PersonV1] = Role("employee", PersonV1)
    employer: Role[CompanyV1] = Role("employer", CompanyV1)


# Version 2: Employment with Person OR Contractor as employee (added player)
class EmploymentV2(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[PersonV1 | ContractorV1] = Role("employee", PersonV1, ContractorV1)
    employer: Role[CompanyV1] = Role("employer", CompanyV1)


# Version 3: Employment with only Contractor (removed Person)
class EmploymentV3(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[ContractorV1] = Role("employee", ContractorV1)
    employer: Role[CompanyV1] = Role("employer", CompanyV1)


@pytest.mark.integration
@pytest.mark.order(250)
def test_detect_added_role_player(clean_db):
    """SchemaDiff should detect when a role player type is added."""
    # Arrange - create initial schema
    manager1 = SchemaManager(clean_db)
    manager1.register(PersonV1, CompanyV1, ContractorV1, EmploymentV1)
    manager1.sync_schema(force=True)

    old_info = manager1.collect_schema_info()

    # Create new schema manager with expanded role
    manager2 = SchemaManager(clean_db)
    manager2.register(PersonV1, CompanyV1, ContractorV1, EmploymentV2)
    new_info = manager2.collect_schema_info()

    # Act
    diff = old_info.compare(new_info)

    # Assert
    assert EmploymentV2 in diff.modified_relations
    rel_changes = diff.modified_relations[EmploymentV2]

    # Find role player changes for 'employee' role
    employee_changes = [
        rpc for rpc in rel_changes.modified_role_players if rpc.role_name == "employee"
    ]
    assert len(employee_changes) == 1
    assert "contractor" in employee_changes[0].added_player_types


@pytest.mark.integration
@pytest.mark.order(251)
def test_detect_removed_role_player(clean_db):
    """SchemaDiff should detect when a role player type is removed."""
    # Arrange - create schema with multiple players
    manager1 = SchemaManager(clean_db)
    manager1.register(PersonV1, CompanyV1, ContractorV1, EmploymentV2)
    manager1.sync_schema(force=True)

    old_info = manager1.collect_schema_info()

    # Create new schema with one player removed
    manager2 = SchemaManager(clean_db)
    manager2.register(PersonV1, CompanyV1, ContractorV1, EmploymentV3)
    new_info = manager2.collect_schema_info()

    # Act
    diff = old_info.compare(new_info)

    # Assert
    assert EmploymentV3 in diff.modified_relations
    rel_changes = diff.modified_relations[EmploymentV3]

    employee_changes = [
        rpc for rpc in rel_changes.modified_role_players if rpc.role_name == "employee"
    ]
    assert len(employee_changes) == 1
    assert "person" in employee_changes[0].removed_player_types


@pytest.mark.integration
@pytest.mark.order(252)
def test_breaking_change_analyzer_with_diff(clean_db):
    """BreakingChangeAnalyzer should classify role player changes correctly."""
    # Arrange - create schema with multiple players
    manager1 = SchemaManager(clean_db)
    manager1.register(PersonV1, CompanyV1, ContractorV1, EmploymentV2)
    manager1.sync_schema(force=True)

    old_info = manager1.collect_schema_info()

    # Create new schema with narrowed role
    manager2 = SchemaManager(clean_db)
    manager2.register(PersonV1, CompanyV1, ContractorV1, EmploymentV3)
    new_info = manager2.collect_schema_info()

    # Act
    diff = old_info.compare(new_info)
    analyzer = BreakingChangeAnalyzer()
    changes = analyzer.analyze(diff)

    # Assert - removing role player should be BREAKING
    breaking_changes = [c for c in changes if c.category == ChangeCategory.BREAKING]
    assert len(breaking_changes) >= 1

    # Find the specific change
    role_player_change = [c for c in breaking_changes if "Remove player type" in c.description]
    assert len(role_player_change) == 1
    assert "person" in role_player_change[0].description


@pytest.mark.integration
@pytest.mark.order(253)
def test_widening_role_player_is_safe(clean_db):
    """Adding a role player type should be classified as SAFE."""
    # Arrange - create initial schema
    manager1 = SchemaManager(clean_db)
    manager1.register(PersonV1, CompanyV1, ContractorV1, EmploymentV1)
    manager1.sync_schema(force=True)

    old_info = manager1.collect_schema_info()

    # Create new schema with additional player
    manager2 = SchemaManager(clean_db)
    manager2.register(PersonV1, CompanyV1, ContractorV1, EmploymentV2)
    new_info = manager2.collect_schema_info()

    # Act
    diff = old_info.compare(new_info)
    analyzer = BreakingChangeAnalyzer()

    # Assert
    assert not analyzer.has_breaking_changes(diff)

    changes = analyzer.analyze(diff)
    safe_changes = [c for c in changes if c.category == ChangeCategory.SAFE]
    assert len(safe_changes) >= 1

    add_player_change = [c for c in safe_changes if "Add player type" in c.description]
    assert len(add_player_change) == 1
