"""Cross-language integration test: Python writer, Node reader."""

from __future__ import annotations

import pytest

from tests.integration.parity.cross_language import (
    assert_node_output_matches_expected,
    load_parity_schema,
    read_with_node,
    write_fixture_with_python,
)


@pytest.mark.integration
def test_python_writer_node_reader_matches_canonical_fixture(clean_db, monkeypatch):
    monkeypatch.setenv("TYPE_BRIDGE_BACKEND", "python")
    load_parity_schema(clean_db)

    monkeypatch.setenv("TYPE_BRIDGE_BACKEND", "rust")
    write_fixture_with_python(clean_db)

    raw_output = read_with_node(clean_db.address, clean_db.database_name)
    assert_node_output_matches_expected(raw_output)
