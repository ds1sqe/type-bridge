"""Marshal retained Python model metadata to the native schema renderer."""

from typing import Any

from type_bridge import _rust_runtime


def _local_descriptor(model: type[Any]) -> dict[str, Any]:
    descriptor = _rust_runtime.descriptor_for_model(model)
    local_attributes = {info.typ.get_attribute_name() for info in model._owned_attrs.values()}
    descriptor["owned_attributes"] = [
        entry for entry in descriptor["owned_attributes"] if entry["attr_name"] in local_attributes
    ]
    if "roles" in descriptor:
        local_roles = {role.role_name for role in model._roles.values()}
        descriptor["roles"] = [
            entry for entry in descriptor["roles"] if entry["role_name"] in local_roles
        ]
    return descriptor


def model_definition(model: type[Any], kind: str) -> str | None:
    if model.is_base():
        return None
    return _rust_runtime.rust_core()._render_schema_definition(kind, _local_descriptor(model))


def attribute_definition(attribute: type[Any], include_doc_meta: bool) -> str:
    entry = _rust_runtime.attribute_schema_entry(attribute)
    if not include_doc_meta:
        entry.pop("doc", None)
        entry.pop("meta", None)
    return _rust_runtime.rust_core()._render_schema_definition("attribute", entry)


def owns_lines(model: type[Any]) -> list[str]:
    return _rust_runtime.rust_core()._render_schema_owns(
        _local_descriptor(model)["owned_attributes"]
    )
