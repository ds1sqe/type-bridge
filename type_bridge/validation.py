"""Validation utilities for TypeBridge.

This module provides validation functions to ensure type names, attribute names,
and role names don't conflict with TypeQL reserved words/keywords.
"""

import logging
import unicodedata
from typing import Literal

from type_bridge.reserved_words import is_reserved_word

logger = logging.getLogger(__name__)


def _is_xid_start(char: str) -> bool:
    """Check if character is valid as identifier start (XID_Start).

    XID_Start includes Unicode letters (Lu, Ll, Lt, Lm, Lo, Nl) and underscore.
    This matches TypeQL 3.8.0's XID_START character class.
    """
    if char == "_":
        return True
    category = unicodedata.category(char)
    return category in ("Lu", "Ll", "Lt", "Lm", "Lo", "Nl")


def _is_xid_continue(char: str) -> bool:
    """Check if character is valid in identifier body (XID_Continue).

    XID_Continue includes XID_Start plus combining marks (Mn, Mc), digits (Nd),
    and connector punctuation (Pc). TypeQL also allows hyphens.
    This matches TypeQL 3.8.0's identifier rules.
    """
    if char == "-":  # TypeQL allows hyphens in identifiers
        return True
    if _is_xid_start(char):
        return True
    category = unicodedata.category(char)
    return category in ("Mn", "Mc", "Nd", "Pc")


class ValidationError(ValueError):
    """Base class for validation errors in TypeBridge."""


class ReservedWordError(ValidationError):
    """Raised when a type name conflicts with a TypeQL reserved word.

    This error is raised when attempting to use a TypeQL keyword as a:
    - Entity type name
    - Relation type name
    - Attribute type name
    - Role name
    """

    def __init__(
        self,
        word: str,
        context: Literal["entity", "relation", "attribute", "role"],
        suggestion: str | None = None,
    ):
        """Initialize ReservedWordError.

        Args:
            word: The reserved word that was used
            context: What kind of name was being validated
            suggestion: Optional suggested alternative name
        """
        self.word = word
        self.context = context
        self.suggestion = suggestion

        message = self._build_message()
        super().__init__(message)

    def _build_message(self) -> str:
        """Build a helpful error message."""
        lines = [f"Cannot use '{self.word}' as {self.context} name: it's a TypeQL reserved word!"]

        if self.suggestion:
            lines.append(f"Suggestion: Use '{self.suggestion}' instead")
        else:
            # Generate automatic suggestions
            suggestions = self._generate_suggestions()
            if suggestions:
                lines.append(f"Suggestions: {', '.join(suggestions)}")

        lines.append("")
        lines.append("TypeQL reserved words include: define, match, entity, relation, attribute,")
        lines.append("insert, delete, update, has, owns, plays, boolean, integer, string, etc.")

        return "\n".join(lines)

    def _generate_suggestions(self) -> list[str]:
        """Generate alternative name suggestions."""
        suggestions = []

        # Add prefix/suffix based on context
        if self.context == "entity":
            suggestions.append(f"{self.word}_entity")
            suggestions.append(f"my_{self.word}")
        elif self.context == "relation":
            suggestions.append(f"{self.word}_relation")
            suggestions.append(f"{self.word}_rel")
        elif self.context == "attribute":
            suggestions.append(f"{self.word}_attr")
            suggestions.append(f"{self.word}_value")
        elif self.context == "role":
            suggestions.append(f"{self.word}_role")
            suggestions.append(f"as_{self.word}")

        # Add underscore if it makes sense
        if self.word.lower() in {"count", "sum", "max", "min", "mean"}:
            suggestions.append(f"{self.word}_value")

        # For common conflicts, provide specific alternatives
        specific_alternatives = {
            "entity": ["object", "item", "record"],
            "relation": ["relationship", "connection", "link"],
            "attribute": ["property", "field", "trait"],
            "string": ["text", "str_value", "text_value"],
            "integer": ["int_value", "number", "num"],
            "boolean": ["bool_value", "flag", "is_enabled"],
            "double": ["float_value", "decimal_value", "num"],
            "date": ["date_value", "calendar_date", "day"],
            "datetime": ["timestamp", "datetime_value", "moment"],
            "duration": ["time_span", "interval", "period"],
            "count": ["total", "quantity", "amount"],
            "sum": ["total", "aggregate", "amount"],
            "max": ["maximum", "highest", "peak"],
            "min": ["minimum", "lowest", "floor"],
            "mean": ["average", "avg"],
            "first": ["initial", "primary", "earliest"],
            "check": ["verify", "validate", "test"],
            "value": ["data", "content", "val"],
            "label": ["name", "title", "identifier"],
            "from": ["source", "origin", "start"],
            "as": ["alias", "name"],
            "has": ["contains", "includes", "holds"],
        }

        if self.word.lower() in specific_alternatives:
            suggestions.extend(specific_alternatives[self.word.lower()])

        # Return unique suggestions
        return list(dict.fromkeys(suggestions))[:3]  # Limit to 3 suggestions


def validate_type_name(
    name: str, context: Literal["entity", "relation", "attribute", "role"]
) -> None:
    """Validate that a type name doesn't conflict with TypeQL reserved words.

    Args:
        name: The type name to validate
        context: What kind of name is being validated

    Raises:
        ReservedWordError: If the name is a TypeQL reserved word
        ValidationError: If the name is invalid for other reasons
    """
    logger.debug(f"Validating {context} name: {name}")

    if not name:
        logger.warning(f"Empty {context} name attempted")
        raise ValidationError(f"Empty {context} name is not allowed")

    # Check for reserved words (case-insensitive to be safe)
    if is_reserved_word(name):
        logger.warning(f"Reserved word used as {context} name: {name}")
        raise ReservedWordError(name, context)

    # TypeQL 3.8.0 uses XID_START and XID_CONTINUE for identifier validation
    # XID_START: Unicode letters (Lu, Ll, Lt, Lm, Lo, Nl) and underscore
    # XID_CONTINUE: XID_START + combining marks (Mn, Mc), digits (Nd), connector (Pc), hyphen
    if not _is_xid_start(name[0]):
        logger.warning(
            f"{context.capitalize()} name '{name}' does not start with a valid character"
        )
        raise ValidationError(
            f"{context.capitalize()} name '{name}' must start with a letter or underscore"
        )

    # Check remaining characters
    for char in name[1:]:
        if not _is_xid_continue(char):
            logger.warning(
                f"{context.capitalize()} name '{name}' contains invalid character: {char}"
            )
            raise ValidationError(
                f"{context.capitalize()} name '{name}' contains invalid character '{char}'. "
                f"Only letters, numbers, underscores, hyphens, and combining marks are allowed."
            )
