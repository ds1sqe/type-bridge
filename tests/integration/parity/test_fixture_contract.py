"""Non-DB smoke tests for the cross-language parity fixture contract."""

import json

from tests.integration.parity.canonical import (
    canonical_json,
    load_fixture_contract,
    validate_fixture_contract,
)
from tests.integration.parity.cross_language import parse_npm_pack_manifest


def test_phase5_fixture_contract_loads_and_validates_without_typedb() -> None:
    contract = load_fixture_contract()
    validate_fixture_contract(contract)


def test_canonical_json_output_is_stable() -> None:
    assert (
        canonical_json({"b": 1, "a": [2, 1]}) == '{\n  "a": [\n    2,\n    1\n  ],\n  "b": 1\n}\n'
    )


def test_npm_pack_manifest_accepts_array_and_workspace_object_shapes() -> None:
    manifest = {"filename": "package.tgz", "files": [{"path": "package.json"}]}
    assert parse_npm_pack_manifest(json.dumps([manifest])) == manifest
    assert parse_npm_pack_manifest(json.dumps({"@type-bridge/node": manifest})) == manifest
