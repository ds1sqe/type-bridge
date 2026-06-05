from __future__ import annotations

# pyright: reportMissingImports=false
import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge._rust_runtime import (
    descriptor_registry,
    model_schema_info,
    register_model_descriptor,
)
from type_bridge.attribute import AttributeFlags


class Name(String):
    flags = AttributeFlags(name="name")


class PersonV1(Entity):
    flags = TypeFlags(name="person")

    name: Name = Flag(Key)


class CompanyV1(Entity):
    flags = TypeFlags(name="company")

    name: Name = Flag(Key)


class EmploymentV3(Relation):
    flags = TypeFlags(name="employment")

    employee: Role[PersonV1] = Role("employee", PersonV1)
    employer: Role[CompanyV1] = Role("employer", CompanyV1)


def test_registered_model_descriptors_lower_to_schema_info() -> None:
    pytest.importorskip("type_bridge_core")
    descriptor_registry.cache_clear()

    try:
        register_model_descriptor(PersonV1)
        register_model_descriptor(CompanyV1)
        register_model_descriptor(EmploymentV3)

        result = model_schema_info()

        person = result["entities"]["person"]
        assert person["is_abstract"] is False
        assert person["owned_attributes"][0]["attr_name"] == "name"
        assert "Key" in person["owned_attributes"][0]["annotations"]

        roles = result["relations"]["employment"]["roles"]
        assert {role["role_name"] for role in roles} == {"employee", "employer"}
        assert "name" in result["attributes"]
    finally:
        descriptor_registry.cache_clear()
