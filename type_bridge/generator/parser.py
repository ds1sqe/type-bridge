"""Lark-based TQL schema parser."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from lark import Lark, Transformer

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
    RoleSpec,
    StructFieldSpec,
    StructSpec,
)

logger = logging.getLogger(__name__)

# Load grammar
GRAMMAR_PATH = Path(__file__).parent / "typeql.lark"


class SchemaTransformer(Transformer):
    """Transform Lark parse tree into TypeBridge schema models."""

    def __init__(
        self,
        entity_annotations: dict[str, dict[str, AnnotationValue]] | None = None,
        attribute_annotations: dict[str, dict[str, AnnotationValue]] | None = None,
        relation_annotations: dict[str, dict[str, AnnotationValue]] | None = None,
        role_annotations: dict[str, dict[str, dict[str, AnnotationValue]]] | None = None,
    ) -> None:
        self.schema = ParsedSchema()
        self.entity_annotations = entity_annotations or {}
        self.attribute_annotations = attribute_annotations or {}
        self.relation_annotations = relation_annotations or {}
        self.role_annotations = role_annotations or {}

    def start(self, items: list[Any]) -> ParsedSchema:
        """Root rule: returns the populated schema."""
        self.schema.accumulate_inheritance()
        return self.schema

    # --- Attributes ---
    def attribute_def(self, items: list[Any]) -> None:
        name_token = items[0]
        name = str(name_token)
        # items[1] is attribute_opts result (list of dicts) if present
        opts_list = items[1] if len(items) > 1 else []

        # Merge all opts dicts
        opts = {}
        for opt in opts_list:
            opts.update(opt)

        attr = AttributeSpec(
            name=name,
            value_type=opts.get("value_type", ""),
            parent=opts.get("parent"),
            abstract=opts.get("abstract", False),
            independent=opts.get("independent", False),
            regex=opts.get("regex"),
            allowed_values=opts.get("values"),
            range_min=opts.get("range_min"),
            range_max=opts.get("range_max"),
            annotations=self.attribute_annotations.get(name, {}),
        )
        self.schema.attributes[attr.name] = attr

    def attribute_opts(self, items: list[Any]) -> list[dict[str, Any]]:
        # Returns list of dicts from children
        return items

    def sub_clause(self, items: list[Any]) -> dict[str, str]:
        return {"parent": str(items[0])}

    def value_type_clause(self, items: list[Any]) -> dict[str, str]:
        return {"value_type": str(items[0])}

    def abstract_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"abstract": True}

    def independent_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"independent": True}

    def regex_annotation(self, items: list[Any]) -> dict[str, str]:
        import re

        raw = str(items[0])
        pattern = raw[1:-1]  # Strip quotes

        # Validate: must be a valid regex pattern
        try:
            re.compile(pattern)
        except re.error as e:
            raise ValueError(
                f"Invalid @regex pattern: '{pattern}'. "
                f"Must be a valid regular expression. Error: {e}"
            )

        return {"regex": pattern}

    def values_annotation(self, items: list[Any]) -> dict[str, tuple[str, ...]]:
        values = items[0]

        # Validate: must have at least one value
        if not values:
            raise ValueError(
                "Invalid @values annotation: must have at least one value. "
                'Example: @values("active", "inactive")'
            )

        # Validate: no duplicate values
        seen: set[str] = set()
        duplicates: list[str] = []
        for v in values:
            if v in seen:
                duplicates.append(v)
            seen.add(v)

        if duplicates:
            raise ValueError(
                f"Invalid @values annotation: duplicate values found: {duplicates}. "
                "Each value must be unique."
            )

        return {"values": tuple(values)}

    def range_annotation(self, items: list[Any]) -> dict[str, str | None]:
        # items[0] is RANGE_EXPR token containing "min..max" or "min.." or "..max"
        expr = str(items[0]).strip()

        # Validate: @range must use .. syntax, not comma
        if ".." not in expr:
            if "," in expr:
                raise ValueError(
                    f"Invalid @range syntax: '@range({expr})'. "
                    f"Use '..' syntax instead of comma, e.g., '@range(1..5)' not '@range(1, 5)'"
                )
            else:
                raise ValueError(
                    f"Invalid @range syntax: '@range({expr})'. "
                    f"Expected 'min..max', 'min..', or '..max' format, e.g., '@range(1..5)'"
                )

        parts = expr.split("..")
        range_min = parts[0].strip() if parts[0].strip() else None
        range_max = parts[1].strip() if len(parts) > 1 and parts[1].strip() else None
        return {"range_min": range_min, "range_max": range_max}

    def string_list(self, items: list[Any]) -> list[str]:
        return [str(item)[1:-1] for item in items]

    def value_type(self, items: list[Any]) -> str:
        return str(items[0])

    # --- Entities ---
    def entity_def(self, items: list[Any]) -> None:
        name = str(items[0])

        # Collect all opts and clauses
        opts = {}
        owns_list = []
        plays_list: list[tuple[str, Cardinality | None]] = []

        # items[1:] contains entity_clauses (dict, tuple)
        for item in items[1:]:
            if isinstance(item, dict):  # sub_clause or abstract_annotation
                opts.update(item)
            elif isinstance(item, tuple):
                # Check if it's owns_statement (6 elements) or plays_statement (2 elements)
                if len(item) == 6:
                    owns_list.append(item)
                elif len(item) == 2:
                    plays_list.append(item)

        # Process owns
        owns_set = set()
        owns_order = []
        keys = set()
        uniques = set()
        cascades = set()
        subkeys: dict[str, str] = {}
        cardinalities = {}

        for attr, card, is_key, is_unique, is_cascade, subkey_group in owns_list:
            owns_set.add(attr)
            owns_order.append(attr)
            if is_key:
                keys.add(attr)
            if is_unique:
                uniques.add(attr)
            if is_cascade:
                cascades.add(attr)
            if subkey_group:
                subkeys[attr] = subkey_group
            if card:
                cardinalities[attr] = card

        # Process plays
        plays_set = set()
        plays_cardinalities: dict[str, Cardinality] = {}

        for role_ref, card in plays_list:
            plays_set.add(role_ref)
            if card:
                plays_cardinalities[role_ref] = card

        entity = EntitySpec(
            name=name,
            parent=opts.get("parent"),
            owns=owns_set,
            owns_order=owns_order,
            plays=plays_set,
            abstract=opts.get("abstract", False),
            keys=keys,
            uniques=uniques,
            cascades=cascades,
            subkeys=subkeys,
            cardinalities=cardinalities,
            plays_cardinalities=plays_cardinalities,
            annotations=self.entity_annotations.get(name, {}),
        )
        self.schema.entities[name] = entity

    def entity_clause(self, items: list[Any]) -> Any:
        return items[0]

    def owns_statement(
        self, items: list[Any]
    ) -> tuple[str, Cardinality | None, bool, bool, bool, str | None]:
        name = str(items[0])
        opts = items[1] or {} if len(items) > 1 else {}
        return (
            name,
            opts.get("card"),
            opts.get("key", False),
            opts.get("unique", False),
            opts.get("cascade", False),
            opts.get("subkey"),
        )

    def owns_opts(self, items: list[Any]) -> dict[str, Any]:
        opts = {}
        for item in items:
            opts.update(item)
        return opts

    def key_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"key": True}

    def unique_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"unique": True}

    def cascade_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"cascade": True}

    def subkey_annotation(self, items: list[Any]) -> dict[str, str]:
        from type_bridge.validation import _is_xid_continue, _is_xid_start

        identifier = str(items[0])

        # Validate: must be a valid TypeDB identifier using XID rules (TypeQL 3.8.0+)
        if not identifier or not _is_xid_start(identifier[0]):
            raise ValueError(
                f"Invalid @subkey identifier: '{identifier}'. "
                "Must start with a letter or underscore."
            )
        for char in identifier[1:]:
            if not _is_xid_continue(char):
                raise ValueError(
                    f"Invalid @subkey identifier: '{identifier}'. "
                    f"Contains invalid character '{char}'."
                )

        return {"subkey": identifier}

    def card_annotation(self, items: list[Any]) -> dict[str, Cardinality]:
        # Filter None (from optional grammar groups)
        real_items = [x for x in items if x is not None]

        # Check for comma syntax error (would appear as multiple items without "..")
        raw_str = " ".join(str(x) for x in real_items)
        if "," in raw_str:
            raise ValueError(
                f"Invalid @card syntax: found comma in '{raw_str}'. "
                "Use '..' syntax for ranges, e.g., '@card(1..5)' not '@card(1, 5)'"
            )

        min_val = int(real_items[0])

        # Validate: min must be non-negative
        if min_val < 0:
            raise ValueError(
                f"Invalid @card annotation: minimum value {min_val} cannot be negative."
            )

        if len(real_items) == 1:
            # @card(x) -> exactly x
            return {"card": Cardinality(min_val, min_val)}

        # Has ".."
        # items could be [min, ".."] or [min, "..", max]
        last = real_items[-1]
        if hasattr(last, "type") and last.type == "INT":
            max_val = int(last)

            # Validate: min must be <= max
            if min_val > max_val:
                raise ValueError(
                    f"Invalid @card annotation: minimum ({min_val}) cannot be greater "
                    f"than maximum ({max_val}). Use '@card({max_val}..{min_val})' instead."
                )
        else:
            max_val = None  # Unbounded

        return {"card": Cardinality(min_val, max_val)}

    def plays_statement(self, items: list[Any]) -> tuple[str, Cardinality | None]:
        # items: [relation_name, role_name?, card_annotation?]
        # Build the role reference
        role_ref: str
        card: Cardinality | None = None

        if len(items) >= 2 and items[1] is not None and isinstance(items[1], str):
            # Has explicit role: plays relation:role
            role_ref = f"{items[0]}:{items[1]}"
            # Check if there's a card annotation (items[2] would be a dict with "card")
            if len(items) >= 3 and isinstance(items[2], dict):
                card = items[2].get("card")
        else:
            role_ref = str(items[0])
            # Check if there's a card annotation (items[1] would be a dict with "card")
            if len(items) >= 2 and isinstance(items[1], dict):
                card = items[1].get("card")

        return (role_ref, card)

    # --- Relations ---
    def relation_def(self, items: list[Any]) -> None:
        name = str(items[0])

        opts = {}
        roles = []
        owns_list = []
        plays_set = set()

        # items[1:] contains relation_clauses (dict, RoleSpec, tuple, or str)
        for item in items[1:]:
            if isinstance(item, dict):  # sub_clause or abstract_annotation
                opts.update(item)
            elif isinstance(item, RoleSpec):  # relates_statement
                roles.append(item)
            elif isinstance(item, tuple):
                # Check if it's owns_statement (6 elements) or plays_statement (2 elements)
                if len(item) == 6:
                    owns_list.append(item)
                elif len(item) == 2:
                    # plays_statement returns (role_ref, card) - just use role_ref
                    plays_set.add(item[0])

        # Process owns
        owns_set = set()
        owns_order = []
        keys = set()
        uniques = set()
        cascades = set()
        subkeys: dict[str, str] = {}
        cardinalities = {}

        for attr, card, is_key, is_unique, is_cascade, subkey_group in owns_list:
            owns_set.add(attr)
            owns_order.append(attr)
            if is_key:
                keys.add(attr)
            if is_unique:
                uniques.add(attr)
            if is_cascade:
                cascades.add(attr)
            if subkey_group:
                subkeys[attr] = subkey_group
            if card:
                cardinalities[attr] = card

        # Apply role annotations
        role_annots = self.role_annotations.get(name, {})
        for role in roles:
            if role.name in role_annots:
                role.annotations.update(role_annots[role.name])

        rel = RelationSpec(
            name=name,
            parent=opts.get("parent"),
            roles=roles,
            owns=owns_set,
            owns_order=owns_order,
            abstract=opts.get("abstract", False),
            keys=keys,
            uniques=uniques,
            cascades=cascades,
            subkeys=subkeys,
            cardinalities=cardinalities,
            annotations=self.relation_annotations.get(name, {}),
        )
        self.schema.relations[name] = rel

    def relation_clause(self, items: list[Any]) -> Any:
        return items[0]

    def relates_statement(self, items: list[Any]) -> RoleSpec:
        # items: [role_name, optional "as" override (Token), optional relates_opts (dict)]
        name = str(items[0])
        overrides: str | None = None
        cardinality: Cardinality | None = None
        distinct: bool = False

        # Parse remaining items - could be: overrides (str), opts (dict), or both
        for item in items[1:]:
            if isinstance(item, str):
                overrides = item
            elif isinstance(item, dict):
                if "card" in item:
                    cardinality = item["card"]
                if "distinct" in item:
                    distinct = item["distinct"]

        return RoleSpec(name=name, overrides=overrides, cardinality=cardinality, distinct=distinct)

    def relates_opts(self, items: list[Any]) -> dict[str, Any]:
        opts = {}
        for item in items:
            opts.update(item)
        return opts

    def distinct_annotation(self, items: list[Any]) -> dict[str, bool]:
        return {"distinct": True}

    # --- Structs ---
    def struct_def(self, items: list[Any]) -> None:
        name = str(items[0])
        fields = items[1] if len(items) > 1 else []

        struct = StructSpec(name=name, fields=fields)
        self.schema.structs[name] = struct

    def struct_fields(self, items: list[Any]) -> list[StructFieldSpec]:
        return items

    def struct_field(self, items: list[Any]) -> StructFieldSpec:
        name = str(items[0])
        value_type = str(items[1])
        optional = len(items) > 2 and items[2] is not None
        return StructFieldSpec(name=name, value_type=value_type, optional=optional)

    # --- Functions ---
    def function_def(self, items: list[Any]) -> None:
        idx = 0
        name = str(items[idx])
        idx += 1

        parameters = []
        if idx < len(items) and isinstance(items[idx], list):
            parameters = items[idx]
            idx += 1

        # Next item is return_type_clause result (string)
        return_type = str(items[idx])

        func = FunctionSpec(name=name, parameters=parameters, return_type=return_type)
        self.schema.functions[name] = func

    def param_list(self, items: list[Any]) -> list[ParameterSpec]:
        return items

    def param(self, items: list[Any]) -> ParameterSpec:
        return ParameterSpec(name=str(items[0]), type=str(items[1]))

    def return_type_clause(self, items: list[Any]) -> str:
        # items[0] is either stream_return or single_return result
        return str(items[0])

    def stream_return(self, items: list[Any]) -> str:
        # Stream type: { types }
        return "{ " + str(items[0]) + " }"

    def single_return(self, items: list[Any]) -> str:
        # Single/tuple type: type or type1, type2
        return str(items[0])

    def return_type_list(self, items: list[Any]) -> str:
        # Join multiple return types with comma
        return ", ".join(str(item) for item in items)

    def return_type(self, items: list[Any]) -> str:
        # items[0] is type name, items[1] (if present) is OPTIONAL "?" token
        type_name = str(items[0])
        if len(items) > 1 and items[1] is not None:
            return type_name + "?"
        return type_name

    def func_body(self, items: list[Any]) -> Any:
        return None  # Ignore body content

    # --- Comments ---
    # Comments are ignored by grammar (%ignore SH_COMMENT),
    # capturing docstrings requires explicit token handling or a separate pass.
    # For now, we accept losing docstrings in the migration or add them later.


def parse_tql_schema(schema_content: str) -> ParsedSchema:
    """Parse TQL schema using Lark.

    First extracts custom annotations from comments (# @key(value)),
    then parses the schema structure and associates annotations with definitions.
    """
    # Extract annotations from comments before parsing
    entity_annots, attr_annots, rel_annots, role_annots = extract_annotations(schema_content)

    with open(GRAMMAR_PATH, encoding="utf-8") as f:
        grammar = f.read()

    parser = Lark(grammar, start="start", parser="lalr")
    tree = parser.parse(schema_content)

    transformer = SchemaTransformer(
        entity_annotations=entity_annots,
        attribute_annotations=attr_annots,
        relation_annotations=rel_annots,
        role_annotations=role_annots,
    )
    return transformer.transform(tree)
