#!/usr/bin/env python3
"""Validate same-run deterministic proof fragments for sdk-v3 reports."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from proof_fragments import (  # noqa: E402
    REPORT_BINDINGS,
    ProofContract,
    ProofFragmentError,
    canonical_json_bytes,
)
from proof_fragments import (
    SEMANTIC_PROFILE as SEMANTIC_PROFILE,
)
from proof_fragments import (
    source_identity as source_identity,
)

ROOT = Path(__file__).resolve().parents[2]
PROOF_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-schema-v1.json"
ALLOWLIST_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-allowlist-v1.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"
FRAGMENT_FORMAT = "typebridge.sdk-v3-proof-fragment/v1"
ALLOWLIST_FORMAT = "typebridge.sdk-v3-proof-fragment-allowlist/v1"
PROOF_KINDS = frozenset({"direct_runtime", "diagnostic"})
MAX_RESULTS = 4

CONTRACT = ProofContract(
    proof_schema_relative=PROOF_SCHEMA_RELATIVE,
    allowlist_relative=ALLOWLIST_RELATIVE,
    journey_relative=JOURNEY_RELATIVE,
    fragment_format=FRAGMENT_FORMAT,
    allowlist_format=ALLOWLIST_FORMAT,
    proof_kinds=PROOF_KINDS,
    max_results=MAX_RESULTS,
)
proof_fragment_authority = CONTRACT.proof_fragment_authority
proof_fragment_lanes = CONTRACT.proof_fragment_lanes
load_proof_fragments = CONTRACT.load_proof_fragments


def main(argv: list[str] | None = None, *, root: Path = ROOT) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=sorted(REPORT_BINDINGS))
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument(
        "--observations-json",
        action="store_true",
        help="write the validated closed observation lanes as canonical JSON",
    )
    parser.add_argument("fragments", nargs="+", type=Path)
    args = parser.parse_args(argv)
    try:
        observations = load_proof_fragments(
            args.fragments,
            expected_binding=args.binding,
            run_nonce=args.run_nonce,
            root=root,
        )
    except ProofFragmentError as error:
        print(f"sdk-v3 proof fragments rejected: {error}")
        return 1
    if args.observations_json:
        rows = [
            {
                "observation": observation,
                "observation_ref": lane[0],
                "proof_kind": lane[1],
            }
            for lane, observation in observations.items()
        ]
        print(canonical_json_bytes(rows).decode("utf-8"), end="")
    else:
        print(f"validated {len(observations)} sdk-v3 proof lanes for {args.binding}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
