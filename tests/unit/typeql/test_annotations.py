"""Tests for TypeQL annotation formatting utilities."""

from type_bridge.typeql.annotations import format_card_annotation, format_type_annotations


class TestFormatCardAnnotation:
    """Tests for format_card_annotation."""

    def test_both_min_and_max(self) -> None:
        """Test with both min and max specified."""
        assert format_card_annotation(1, 5) == "@card(1..5)"
        assert format_card_annotation(0, 1) == "@card(0..1)"
        assert format_card_annotation(2, 10) == "@card(2..10)"

    def test_unbounded_max(self) -> None:
        """Test with unbounded max (None)."""
        assert format_card_annotation(0, None) == "@card(0..)"
        assert format_card_annotation(1, None) == "@card(1..)"
        assert format_card_annotation(2, None) == "@card(2..)"

    def test_none_min_defaults_to_zero(self) -> None:
        """Test that None min defaults to 0."""
        assert format_card_annotation(None, 5) == "@card(0..5)"
        assert format_card_annotation(None, 1) == "@card(0..1)"

    def test_both_none_returns_none(self) -> None:
        """Test that both None returns None."""
        assert format_card_annotation(None, None) is None

    def test_edge_cases(self) -> None:
        """Test edge cases."""
        assert format_card_annotation(0, 0) == "@card(0..0)"
        assert format_card_annotation(100, 1000) == "@card(100..1000)"


class TestFormatTypeAnnotations:
    """Tests for format_type_annotations."""

    def test_no_annotations(self) -> None:
        """Test with no annotations."""
        assert format_type_annotations() == []
        assert format_type_annotations(abstract=False) == []
        assert format_type_annotations(independent=False) == []
        assert format_type_annotations(abstract=False, independent=False) == []

    def test_abstract_only(self) -> None:
        """Test with only @abstract."""
        assert format_type_annotations(abstract=True) == ["@abstract"]

    def test_independent_only(self) -> None:
        """Test with only @independent."""
        assert format_type_annotations(independent=True) == ["@independent"]

    def test_both_annotations(self) -> None:
        """Test with both annotations."""
        result = format_type_annotations(abstract=True, independent=True)
        assert result == ["@abstract", "@independent"]

    def test_order_is_consistent(self) -> None:
        """Test that order is always @abstract before @independent."""
        result = format_type_annotations(abstract=True, independent=True)
        assert result[0] == "@abstract"
        assert result[1] == "@independent"
