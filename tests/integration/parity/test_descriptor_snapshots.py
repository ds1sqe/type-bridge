"""TypeDB-free descriptor snapshot parity tests for Python and Node."""

from tests.integration.parity.canonical import (
    load_json,
    normalize_descriptor_snapshot,
)
from tests.integration.parity.descriptor_snapshots import (
    assert_descriptor_snapshots_equal,
    node_descriptor_snapshot,
    python_descriptor_snapshot,
)


def test_python_descriptor_snapshot_matches_fixture_without_typedb() -> None:
    expected = normalize_descriptor_snapshot(load_json("descriptors.json"))

    assert_descriptor_snapshots_equal(
        python_descriptor_snapshot(),
        expected,
        actual_name="python descriptor snapshot",
        expected_name="fixture descriptor snapshot",
    )


def test_node_descriptor_snapshot_matches_fixture_without_typedb() -> None:
    expected = normalize_descriptor_snapshot(load_json("descriptors.json"))

    assert_descriptor_snapshots_equal(
        node_descriptor_snapshot(),
        expected,
        actual_name="node descriptor snapshot",
        expected_name="fixture descriptor snapshot",
    )


def test_python_and_node_descriptor_snapshots_match_without_typedb() -> None:
    assert_descriptor_snapshots_equal(
        python_descriptor_snapshot(),
        node_descriptor_snapshot(),
        actual_name="python descriptor snapshot",
        expected_name="node descriptor snapshot",
    )
