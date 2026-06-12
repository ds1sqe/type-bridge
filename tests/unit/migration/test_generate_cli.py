"""Unit tests for the _generate CLI argument parser.

Verifies that --http-port is accepted and forwarded to the Database
constructor.  No TypeDB connection required — Database and the generator
are patched at their source modules because _generate imports them lazily
inside main().
"""

from __future__ import annotations

from unittest.mock import MagicMock, patch


def _run_main(argv: list[str]) -> list[int]:
    """Run _generate.main with patched Database/generator; return recorded ports."""
    from type_bridge.migration._generate import main

    recorded: list[int] = []

    def _fake_database(*, address, database, http_port=8000, **kwargs):
        recorded.append(http_port)
        db = MagicMock()
        db.connect.return_value = None
        db.close.return_value = None
        return db

    with (
        patch("type_bridge.session.Database", side_effect=_fake_database),
        patch("type_bridge.migration.generator.MigrationGenerator") as mock_gen,
    ):
        mock_gen.return_value.generate.return_value = "migrations/0001_test.py"
        result = main(argv)

    assert result == 0, f"main() exited {result}, expected 0"
    return recorded


def test_http_port_argument_default() -> None:
    """--http-port defaults to 8000 when omitted."""
    recorded = _run_main(["--name", "test_migration", "--empty"])
    assert recorded == [8000], f"Expected default http_port=8000, got {recorded}"


def test_http_port_argument_custom() -> None:
    """--http-port 9123 is parsed and forwarded to Database as http_port=9123."""
    recorded = _run_main(["--name", "test_migration", "--empty", "--http-port", "9123"])
    assert recorded == [9123], f"Expected http_port=9123 forwarded to Database, got {recorded}"
