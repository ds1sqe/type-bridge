"""Typed-surface cross-language value parity (Plan 11 Phase 2).

These tests lock the typed TypeScript ``toDict()`` serialization into the same
``expected-canonical.json`` value oracle the dynamic ``node_reader.cjs`` already
satisfies, reusing the single canonicalizer in ``cross_language.py``.

This is the VALUE-parity gate. Plan 10's descriptor byte-identity check is the
SHAPE-parity gate. They are complementary: Plan 10 proves the typed factory
emits byte-identical descriptors; this proves typed serialization emits the
byte-identical canonical value shape Python ``to_dict()`` produces.

The offline test needs no database (it builds instances from ``write-data.json``);
the live test writes the fixture through Python and reads it back through the
typed Node manager, mirroring ``test_python_writer_node_reader.py``.
"""

from __future__ import annotations

import pytest

from tests.integration.parity.cross_language import (
    assert_typed_node_output_matches_expected,
    assert_typed_relation_attributes_match_expected,
    load_parity_schema,
    read_with_typed_node,
    write_fixture_with_python,
)


def test_typed_todict_offline_matches_canonical_value_oracle():
    """Offline: typed toDict() output normalizes to expected-canonical.json.

    No database — instances are built directly from write-data.json. Skips if
    node or the compiled typed reader is unavailable.
    """
    raw_output = read_with_typed_node(offline=True)
    assert_typed_node_output_matches_expected(raw_output)
    assert_typed_relation_attributes_match_expected(raw_output)


@pytest.mark.integration
def test_python_writer_typed_node_reader_matches_canonical_fixture(clean_db, monkeypatch):
    """Live: Python writes the fixture, the typed Node reader round-trips entities
    through toDict() to the same canonical value oracle the dynamic reader hits."""
    monkeypatch.delenv("TYPE_BRIDGE_BACKEND", raising=False)
    load_parity_schema(clean_db)

    write_fixture_with_python(clean_db)

    raw_output = read_with_typed_node(clean_db.address, clean_db.database_name)
    assert_typed_node_output_matches_expected(raw_output)
