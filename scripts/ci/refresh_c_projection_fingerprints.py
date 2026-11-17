#!/usr/bin/env python3
"""Re-pin projection evidence after a deliberate generated runtime change.

Runs native emitters, refuses semantic changes, and updates selected digests
and their catalog SHA pins (C by default). Other fingerprints and plan fixtures stay
untouched. Run with TMPDIR on disk and the normal shared CARGO_TARGET_DIR.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTRACTS = ROOT / "tests/contracts/sdk_conformance"


def replace_once(body: str, old: str, new: str) -> str:
    if body.count(old) != 1:
        raise ValueError(f"expected exactly one fingerprint occurrence: {old}")
    return body.replace(old, new, 1)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bindings", nargs="+", choices=("c", "python", "node", "rust"), default=["c"]
    )
    bindings = parser.parse_args().bindings
    if len(set(bindings)) != len(bindings) or ("c" in bindings and len(bindings) != 1):
        parser.error("select C alone or Python/Node/Rust")
    native_test, producer = (
        ("c_projection_live", "produce_c_projection_fingerprints")
        if bindings == ["c"]
        else ("projection_fingerprints", "produce_projection_fingerprints")
    )
    with tempfile.TemporaryDirectory(prefix="typebridge-c-fingerprints-") as directory:
        output = Path(directory) / "fingerprints.json"
        subprocess.run(
            [
                "cargo",
                "+1.94.1",
                "test",
                "--manifest-path",
                "type-bridge-core/Cargo.toml",
                "-p",
                "type-bridge-schema-codegen",
                "--test",
                native_test,
                producer,
                "--",
                "--ignored",
                "--exact",
            ],
            cwd=ROOT,
            check=True,
            env={**os.environ, "TYPE_BRIDGE_C_PROJECTION_FINGERPRINT_OUTPUT": str(output)},
        )
        evidence = json.loads(output.read_text())
    changes: dict[Path, str] = {}
    ledger_path = ROOT / "tests/contracts/c-broad-case-ledger.json"
    ledger = ledger_path.read_text()
    probe_path = ROOT / "tests/unit/compat/test_sdk_models_contract.py"
    probe = probe_path.read_text()
    node_probe_path = (
        ROOT
        / "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/projected_live_check.mjs"
    )
    node_probe = node_probe_path.read_text()
    for suite in ("queries", "models"):
        matrix_path = CONTRACTS / suite / "profiles.json"
        catalog_path = CONTRACTS / suite / "catalog.json"
        matrix_body, catalog_body = matrix_path.read_text(), catalog_path.read_text()
        matrix, catalog = json.loads(matrix_body), json.loads(catalog_body)
        original_catalog = catalog_body
        if set(matrix["profiles"]) != set(evidence):
            raise ValueError("native evidence does not cover exactly the committed profiles")
        for profile, entry in matrix["profiles"].items():
            expected = entry["expected_fingerprints"]
            produced = evidence[profile][suite]
            if expected["semantic"] != produced["semantic"]:
                raise ValueError(f"refusing semantic fingerprint change: {suite} {profile}")
            for binding in bindings:
                old = expected["projections"][binding]["digest"]
                new = produced[binding]["digest"]
                if {**expected["projections"][binding], "digest": new} != produced[binding]:
                    raise ValueError(f"refusing fingerprint metadata change: {suite} {profile}")
                matrix_body = replace_once(matrix_body, old, new)
                if profile == catalog["fixture"]["semantic_profile"]:
                    if catalog["expected_fingerprints"] != expected:
                        raise ValueError("catalog and profile matrix disagree before refresh")
                    catalog_body = replace_once(catalog_body, old, new)
                    if suite == "models":
                        probe = replace_once(probe, old, new)
                        if binding == "node":
                            match = re.search(
                                r'export const PROJECTION_FINGERPRINT = Object.freeze\(\{.*?digest: "([0-9a-f]{64})"',
                                node_probe,
                                re.DOTALL,
                            )
                            if match is None:
                                raise ValueError("Node live producer has no projection fingerprint")
                            node_probe = replace_once(node_probe, match.group(1), new)
                print(f"{suite} {profile} {binding}: {old} -> {new}")
        old_sha = hashlib.sha256(original_catalog.encode()).hexdigest()
        new_sha = hashlib.sha256(catalog_body.encode()).hexdigest()
        ledger = replace_once(ledger, old_sha, new_sha)
        print(f"{suite} catalog SHA: {old_sha} -> {new_sha}")
        changes[matrix_path], changes[catalog_path] = matrix_body, catalog_body
    changes[ledger_path], changes[probe_path] = ledger, probe
    if "node" in bindings:
        changes[node_probe_path] = node_probe
    # All semantic and shape checks complete before changing any committed file.
    for path, body in changes.items():
        path.write_text(body)


if __name__ == "__main__":
    main()
