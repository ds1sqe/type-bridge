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
