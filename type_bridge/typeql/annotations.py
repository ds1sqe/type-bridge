"""Shared TypeQL annotation formatting utilities.

This module centralizes the formatting logic for TypeQL annotations
to avoid duplication across the codebase.
"""


def format_card_annotation(
    min_val: int | None,
    max_val: int | None,
) -> str | None:
    """Format a @card(min..max) annotation string.

    Args:
        min_val: Minimum cardinality (None means unspecified, defaults to 0)
        max_val: Maximum cardinality (None means unbounded)

    Returns:
        Formatted annotation string like "@card(1..5)" or "@card(2..)",
        or None if both min and max are None.

    Examples:
        >>> format_card_annotation(1, 5)
        '@card(1..5)'
        >>> format_card_annotation(2, None)
        '@card(2..)'
        >>> format_card_annotation(0, 1)
        '@card(0..1)'
        >>> format_card_annotation(None, None)
        None
    """
    if min_val is None and max_val is None:
        return None

    min_v = min_val if min_val is not None else 0
    if max_val is None:
        return f"@card({min_v}..)"
    return f"@card({min_v}..{max_val})"


def format_type_annotations(
    *,
    abstract: bool = False,
    independent: bool = False,
) -> list[str]:
    """Format type-level annotations (@abstract, @independent).

    Args:
        abstract: Whether to include @abstract annotation
        independent: Whether to include @independent annotation

    Returns:
        List of annotation strings (may be empty)

    Examples:
        >>> format_type_annotations(abstract=True)
        ['@abstract']
        >>> format_type_annotations(abstract=True, independent=True)
        ['@abstract', '@independent']
        >>> format_type_annotations()
        []
    """
    annotations = []
    if abstract:
        annotations.append("@abstract")
    if independent:
        annotations.append("@independent")
    return annotations


def escape_annotation_string(value: str) -> str:
    """Render a TypeQL string literal for ``@doc`` / ``@meta`` values.

    Escapes backslashes, quotes, and control characters exactly the way
    TypeDB's schema export renders them, mirroring the Rust core's
    ``escaped_string_literal`` so both lowering paths emit identical text.

    Examples:
        >>> escape_annotation_string('plain')
        '"plain"'
        >>> escape_annotation_string('line1\\nline2')
        '"line1\\\\nline2"'
    """
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\t", "\\t")
        .replace("\r", "\\r")
    )
    return f'"{escaped}"'


def format_doc_meta_annotations(doc: str | None, meta: dict[str, str]) -> list[str]:
    """Format TypeDB 3.12+ ``@doc`` / ``@meta`` annotations.

    Emits ``@doc`` before ``@meta`` with meta keys sorted, matching the
    canonical annotation order of TypeDB's schema export (and the Rust
    core's ``append_doc_meta_annotations``).

    Examples:
        >>> format_doc_meta_annotations("a person", {"icon": "p.png"})
        ['@doc("a person")', '@meta("icon", "p.png")']
        >>> format_doc_meta_annotations(None, {})
        []
    """
    annotations = []
    if doc is not None:
        annotations.append(f"@doc({escape_annotation_string(doc)})")
    for key in sorted(meta):
        annotations.append(
            f"@meta({escape_annotation_string(key)}, {escape_annotation_string(meta[key])})"
        )
    return annotations
