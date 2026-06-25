"""Tests for generator naming helpers."""

from __future__ import annotations

from type_bridge.generator.naming import (
    build_class_name_map,
    render_all_export,
    to_class_name,
    to_python_name,
)


def test_to_class_name_preserves_mixed_case_segments() -> None:
    """Mixed-case segments should preserve their internal casing."""
    assert to_class_name("person-name") == "PersonName"
    assert to_class_name("person_NAME") == "PersonName"
    assert to_class_name("firstPerson") == "FirstPerson"
    assert to_class_name("api_KEY") == "ApiKey"


def test_to_python_name_replaces_hyphens_with_underscores() -> None:
    assert to_python_name("birth-date") == "birth_date"


def test_build_class_name_map_uses_to_class_name() -> None:
    result = build_class_name_map({"person-name": 1, "birth-date": 1, "firstPerson": 1})
    assert result == {
        "person-name": "PersonName",
        "birth-date": "BirthDate",
        "firstPerson": "FirstPerson",
    }


def test_render_all_export_sorts_names() -> None:
    assert render_all_export(["beta", "alpha", "gamma"]) == [
        "__all__ = [",
        '    "alpha",',
        '    "beta",',
        '    "gamma",',
        "]",
        "",
    ]


def test_render_all_export_adds_extras() -> None:
    assert render_all_export(["beta", "alpha"], extras=["All"]) == [
        "__all__ = [",
        '    "alpha",',
        '    "beta",',
        '    "All",',
        "]",
        "",
    ]
