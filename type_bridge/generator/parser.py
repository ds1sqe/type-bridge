"""TQL schema parser backed by the Rust type_bridge_core core."""

from __future__ import annotations

import logging
from typing import Any

from .annotations import extract_annotations
from .models import (
    AnnotationValue,
    AttributeSpec,
    Cardinality,
    EntitySpec,
    FunctionSpec,
    ParameterSpec,
    ParsedSchema,
    RelationSpec,
    ReturnTypeItem,
    ReturnTypeSpec,
    RoleSpec,
    StructFieldSpec,
    StructSpec,
)

logger = logging.getLogger(__name__)

# Rust core availability
try:
    from type_bridge_core import TypeSchema as _RustTypeSchema

    _RUST_SCHEMA_AVAILABLE = True
except ImportError:
    _RustTypeSchema = None
    _RUST_SCHEMA_AVAILABLE = False


# ---------------------------------------------------------------------------
# Rust TypeSchema → ParsedSchema conversion
# ---------------------------------------------------------------------------


def _convert_cardinality(card_dict: dict[str, Any] | None) -> Cardinality | None:
    """Convert a Rust cardinality dict ``{min, max}`` to a Python ``Cardinality``."""
    if card_dict is None:
        return None
    return Cardinality(min=card_dict["min"], max=card_dict["max"])


def _convert_owned_attributes(
    owns_list: list[dict[str, Any]],
    owns_order: list[str],
) -> tuple[
    set[str],
    list[str],
    set[str],
    set[str],
    set[str],
    dict[str, str],
    dict[str, Cardinality],
]:
    """Decompose ``Vec<OwnedAttribute>`` dicts into separate Python collections.

    Returns (owns, owns_order, keys, uniques, cascades, subkeys, cardinalities).
    """
    owns: set[str] = set()
    keys: set[str] = set()
    uniques: set[str] = set()
    cascades: set[str] = set()
    subkeys: dict[str, str] = {}
    cardinalities: dict[str, Cardinality] = {}

    for attr_dict in owns_list:
        name = attr_dict["name"]
        owns.add(name)
        if attr_dict["is_key"]:
            keys.add(name)
        if attr_dict["is_unique"]:
            uniques.add(name)
        if attr_dict["is_cascade"]:
            cascades.add(name)
        if attr_dict["subkey_group"] is not None:
            subkeys[name] = attr_dict["subkey_group"]
        card = _convert_cardinality(attr_dict.get("cardinality"))
        if card is not None:
            cardinalities[name] = card

    return owns, list(owns_order), keys, uniques, cascades, subkeys, cardinalities


def _convert_played_roles(
    plays_list: list[dict[str, Any]],
) -> tuple[set[str], dict[str, Cardinality]]:
    """Decompose ``Vec<PlayedRole>`` dicts into plays set + plays_cardinalities dict."""
    plays: set[str] = set()
    plays_cardinalities: dict[str, Cardinality] = {}

    for play_dict in plays_list:
        role_ref = play_dict["role_ref"]
        plays.add(role_ref)
        card = _convert_cardinality(play_dict.get("cardinality"))
        if card is not None:
            plays_cardinalities[role_ref] = card

    return plays, plays_cardinalities


def _rust_schema_to_parsed(
    rust_schema: Any,
    entity_annots: dict[str, dict[str, AnnotationValue]],
    attr_annots: dict[str, dict[str, AnnotationValue]],
    rel_annots: dict[str, dict[str, AnnotationValue]],
    role_annots: dict[str, dict[str, dict[str, AnnotationValue]]],
) -> ParsedSchema:
    """Convert Rust ``TypeSchema`` output to a ``ParsedSchema``.

    The Rust parser returns pythonize'd dicts via PyO3.  This function
    transforms them into the dataclass-based ``ParsedSchema`` that
    renderers expect, including annotation/docstring application.
    """
    schema = ParsedSchema()

    # --- Attributes ---
    for name, attr_dict in rust_schema.attributes.items():
        annots = attr_annots.get(name, {}).copy()
        docstring = annots.pop("_docstring", None)
        attr_docstring: str | None = docstring if isinstance(docstring, str) else None

        allowed_values = attr_dict.get("allowed_values")
        schema.attributes[name] = AttributeSpec(
            name=name,
            value_type=attr_dict.get("value_type", ""),
            parent=attr_dict.get("parent"),
            abstract=attr_dict.get("is_abstract", False),
            independent=attr_dict.get("is_independent", False),
            regex=attr_dict.get("regex"),
            allowed_values=tuple(allowed_values) if allowed_values is not None else None,
            range_min=attr_dict.get("range_min"),
            range_max=attr_dict.get("range_max"),
            docstring=attr_docstring,
            annotations=annots,
        )

    # --- Entities ---
    for name, ent_dict in rust_schema.entities.items():
        owns, owns_order, keys, uniques, cascades, subkeys, cardinalities = (
            _convert_owned_attributes(ent_dict.get("owns", []), ent_dict.get("owns_order", []))
        )
        plays, plays_cardinalities = _convert_played_roles(ent_dict.get("plays", []))

        annots = entity_annots.get(name, {}).copy()
        docstring = annots.pop("_docstring", None)
        entity_docstring: str | None = docstring if isinstance(docstring, str) else None

        schema.entities[name] = EntitySpec(
            name=name,
            parent=ent_dict.get("parent"),
            owns=owns,
            owns_order=owns_order,
            plays=plays,
            abstract=ent_dict.get("is_abstract", False),
            keys=keys,
            uniques=uniques,
            cascades=cascades,
            subkeys=subkeys,
            cardinalities=cardinalities,
            plays_cardinalities=plays_cardinalities,
            docstring=entity_docstring,
            annotations=annots,
        )

    # --- Relations ---
    for name, rel_dict in rust_schema.relations.items():
        owns, owns_order, keys, uniques, cascades, subkeys, cardinalities = (
            _convert_owned_attributes(rel_dict.get("owns", []), rel_dict.get("owns_order", []))
        )

        # Convert role dicts → RoleSpec dataclasses
        roles: list[RoleSpec] = []
        for role_dict in rel_dict.get("roles", []):
            role = RoleSpec(
                name=role_dict["name"],
                overrides=role_dict.get("overrides"),
                cardinality=_convert_cardinality(role_dict.get("cardinality")),
                distinct=role_dict.get("distinct", False),
                is_abstract=role_dict.get("is_abstract", False),
            )
            roles.append(role)

        # Apply role annotations
        r_annots = role_annots.get(name, {})
        for role in roles:
            if role.name in r_annots:
                role.annotations.update(r_annots[role.name])

        annots = rel_annots.get(name, {}).copy()
        docstring = annots.pop("_docstring", None)
        rel_docstring: str | None = docstring if isinstance(docstring, str) else None

        schema.relations[name] = RelationSpec(
            name=name,
            parent=rel_dict.get("parent"),
            roles=roles,
            owns=owns,
            owns_order=owns_order,
            abstract=rel_dict.get("is_abstract", False),
            keys=keys,
            uniques=uniques,
            cascades=cascades,
            subkeys=subkeys,
            cardinalities=cardinalities,
            docstring=rel_docstring,
            annotations=annots,
        )
        # Note: RelationType.plays is ignored — Python RelationSpec has no plays field

    # --- Functions ---
    for name, fn_dict in rust_schema.functions.items():
        parameters = [
            ParameterSpec(name=p["name"], type=p["type"]) for p in fn_dict.get("parameters", [])
        ]
        rt = fn_dict["return_type"]
        return_type = ReturnTypeSpec(
            is_stream=rt["is_stream"],
            types=[ReturnTypeItem(name=t["name"], optional=t["optional"]) for t in rt["types"]],
        )
        schema.functions[name] = FunctionSpec(
            name=name, parameters=parameters, return_type=return_type
        )

    # --- Structs ---
    for name, struct_dict in rust_schema.structs.items():
        fields = [
            StructFieldSpec(name=f["name"], value_type=f["value_type"], optional=f["optional"])
            for f in struct_dict.get("fields", [])
        ]
        schema.structs[name] = StructSpec(name=name, fields=fields)

    # No accumulate_inheritance() needed — Rust already resolved it
    return schema


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def parse_tql_schema(schema_content: str) -> ParsedSchema:
    """Parse a TQL schema string into a :class:`ParsedSchema`.

    Parsing runs entirely through the Rust ``type_bridge_core`` core. Annotations
    and docstrings (extracted from comments) are applied on the Python side.

    Raises:
        RuntimeError: if the Rust core is unavailable (it is a default dependency,
            so this is a guard, not a fallback).
        ValueError: if the Rust parser rejects the schema (propagated from core).
    """
    if not _RUST_SCHEMA_AVAILABLE or _RustTypeSchema is None:
        raise RuntimeError(
            "type_bridge_core is required to parse TQL schemas but is not "
            "available. Reinstall type-bridge with its native core."
        )

    entity_annots, attr_annots, rel_annots, role_annots = extract_annotations(schema_content)
    rust_schema = _RustTypeSchema.from_typeql(schema_content)
    return _rust_schema_to_parsed(rust_schema, entity_annots, attr_annots, rel_annots, role_annots)
