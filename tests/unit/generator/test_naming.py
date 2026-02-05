"""Tests for naming convention utilities."""

from __future__ import annotations

import pytest

from type_bridge.generator.naming import (
    build_class_name_map,
    render_all_export,
    to_class_name,
    to_python_name,
)


class TestToClassName:
    """Tests for to_class_name conversion."""

    @pytest.mark.parametrize(
        ("input_name", "expected"),
        [
            ("person", "Person"),
            ("isbn-13", "Isbn13"),
            ("order-line", "OrderLine"),
            ("birth-date", "BirthDate"),
            ("user-action-log", "UserActionLog"),
            ("a", "A"),
            ("ABC", "Abc"),
        ],
    )
    def test_conversion(self, input_name: str, expected: str) -> None:
        """Test various kebab-case to PascalCase conversions."""
        assert to_class_name(input_name) == expected


class TestToPythonName:
    """Tests for to_python_name conversion."""

    @pytest.mark.parametrize(
        ("input_name", "expected"),
        [
            ("person", "person"),
            ("isbn-13", "isbn_13"),
            ("order-line", "order_line"),
            ("birth-date", "birth_date"),
            ("a", "a"),
        ],
    )
    def test_conversion(self, input_name: str, expected: str) -> None:
        """Test various kebab-case to snake_case conversions."""
        assert to_python_name(input_name) == expected


class TestBuildClassNameMap:
    """Tests for build_class_name_map."""

    def test_builds_mapping(self) -> None:
        """Build mapping from TypeDB names to class names."""
        names = {"person": None, "order-line": None, "isbn-13": None}
        result = build_class_name_map(names)
        assert result == {
            "person": "Person",
            "order-line": "OrderLine",
            "isbn-13": "Isbn13",
        }

    def test_empty_dict(self) -> None:
        """Empty input returns empty output."""
        assert build_class_name_map({}) == {}


class TestRenderAllExport:
    """Tests for __all__ list rendering."""

    def test_basic_export(self) -> None:
        """Render basic __all__ export."""
        lines = render_all_export(["Foo", "Bar", "Baz"])
        expected = [
            "__all__ = [",
            '    "Bar",',
            '    "Baz",',
            '    "Foo",',  # Sorted alphabetically
            "]",
            "",
        ]
        assert lines == expected

    def test_with_extras(self) -> None:
        """Render __all__ with extra exports."""
        lines = render_all_export(["Foo"], extras=["helper"])
        expected = [
            "__all__ = [",
            '    "Foo",',
            '    "helper",',
            "]",
            "",
        ]
        assert lines == expected

    def test_empty_list(self) -> None:
        """Empty list still renders valid __all__."""
        lines = render_all_export([])
        expected = [
            "__all__ = [",
            "]",
            "",
        ]
        assert lines == expected
