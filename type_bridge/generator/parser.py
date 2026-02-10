"""TQL schema parser with Rust core acceleration and Lark fallback."""

from __future__ import annotations

import logging
import re
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

# Rust core availability
try:
    from type_bridge_core import TypeSchema as _RustTypeSchema  # type: ignore[import-not-found]

    _RUST_SCHEMA_AVAILABLE = True
except ImportError:
    _RustTypeSchema = None  # type: ignore[assignment, misc]
    _RUST_SCHEMA_AVAILABLE = False

# Pattern to detect function/struct definitions in schema text
_HAS_FUN_OR_STRUCT = re.compile(r"(?:^|\s)(?:fun|struct)\s", re.MULTILINE)


def _unquote_string(raw: str) -> str:
    """Safely parse a quoted string literal.

    Uses ast.literal_eval for safe parsing that handles:
    - Both single and double quotes
    - Escape sequences (\\n, \\t, \\\\, etc.)
    - Unicode escapes

    Args:
        raw: Raw string literal (e.g., '"hello"' or "'hello\\nworld'")

    Returns:
        Unquoted string content with escapes processed

    Raises:
        ValueError: If string is not a valid string literal
    """
    import ast
    import warnings

    try:
        # Suppress SyntaxWarning for invalid escape sequences (e.g., \. in regex)
        # Python 3.13+ raises warnings for escape sequences that are valid in regex
        # but not in Python strings. These are legitimate in TQL @regex annotations.
        with warnings.catch_warnings():
            warnings.filterwarnings("ignore", category=SyntaxWarning)
            result = ast.literal_eval(raw)
        if not isinstance(result, str):
            raise ValueError(f"Expected string literal, got {type(result).__name__}")
        return result
    except (ValueError, SyntaxError) as e:
        raise ValueError(f"Invalid string literal: {raw!r}. Error: {e}") from e


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

        # Extract docstring and annotations
        attr_annots = self.attribute_annotations.get(name, {}).copy()
        docstring = attr_annots.pop("_docstring", None)
        attr_docstring: str | None = docstring if isinstance(docstring, str) else None

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
            docstring=attr_docstring,
            annotations=attr_annots,
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
        import warnings

        raw = str(items[0])
        try:
            pattern = _unquote_string(raw)
        except ValueError as e:
            raise ValueError(f"Invalid @regex annotation: {e}") from e

        # Validate: must be a valid regex pattern
        # Suppress SyntaxWarning for valid regex escape sequences like \.
        try:
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", SyntaxWarning)
                re.compile(pattern)
        except re.error as e:
            raise ValueError(
                f"Invalid @regex pattern: '{pattern}'. "
                f"Must be a valid regular expression. Error: {e}"
            ) from e

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
        return [_unquote_string(str(item)) for item in items]

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

        # Extract docstring and annotations
        entity_annots = self.entity_annotations.get(name, {}).copy()
        docstring = entity_annots.pop("_docstring", None)
        if isinstance(docstring, str):
            entity_docstring: str | None = docstring
        else:
            entity_docstring = None

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
            docstring=entity_docstring,
            annotations=entity_annots,
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

        # Extract docstring and annotations
        rel_annots = self.relation_annotations.get(name, {}).copy()
        docstring = rel_annots.pop("_docstring", None)
        rel_docstring: str | None = docstring if isinstance(docstring, str) else None

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
            docstring=rel_docstring,
            annotations=rel_annots,
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


def _parse_with_lark(
    schema_content: str,
    entity_annots: dict[str, dict[str, AnnotationValue]],
    attr_annots: dict[str, dict[str, AnnotationValue]],
    rel_annots: dict[str, dict[str, AnnotationValue]],
    role_annots: dict[str, dict[str, dict[str, AnnotationValue]]],
) -> ParsedSchema:
    """Parse TQL schema using Lark (original implementation)."""
    with open(GRAMMAR_PATH, encoding="utf-8") as f:
        grammar = f.read()

    lark_parser = Lark(grammar, start="start", parser="lalr")
    tree = lark_parser.parse(schema_content)

    transformer = SchemaTransformer(
        entity_annotations=entity_annots,
        attribute_annotations=attr_annots,
        relation_annotations=rel_annots,
        role_annotations=role_annots,
    )
    return transformer.transform(tree)


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

    # No accumulate_inheritance() needed — Rust already resolved it
    return schema


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def parse_tql_schema(schema_content: str) -> ParsedSchema:
    """Parse a TQL schema string into a :class:`ParsedSchema`.

    Uses the Rust ``TypeSchema`` parser when available for speed, falling
    back to the Lark-based parser otherwise.  Annotations and docstrings
    (extracted from comments) are always applied on the Python side.
    """
    # Step 1: Always extract annotations from comments first
    entity_annots, attr_annots, rel_annots, role_annots = extract_annotations(schema_content)

    # Step 2: Try Rust parser
    if _RUST_SCHEMA_AVAILABLE:
        try:
            rust_schema = _RustTypeSchema.from_typeql(schema_content)  # type: ignore[union-attr]
            schema = _rust_schema_to_parsed(
                rust_schema, entity_annots, attr_annots, rel_annots, role_annots
            )
            logger.debug("Parsed schema using Rust core")

            # Step 3: If functions/structs present, supplement with Lark
            if _HAS_FUN_OR_STRUCT.search(schema_content):
                logger.debug("Schema contains functions/structs, supplementing with Lark parser")
                lark_schema = _parse_with_lark(
                    schema_content, entity_annots, attr_annots, rel_annots, role_annots
                )
                schema.functions = lark_schema.functions
                schema.structs = lark_schema.structs

            return schema
        except Exception:
            logger.warning("Rust parser failed, falling back to Lark parser", exc_info=True)

    # Step 4: Full Lark fallback
    if not _RUST_SCHEMA_AVAILABLE:
        logger.debug("Rust core not available, using Lark parser")
    return _parse_with_lark(schema_content, entity_annots, attr_annots, rel_annots, role_annots)
