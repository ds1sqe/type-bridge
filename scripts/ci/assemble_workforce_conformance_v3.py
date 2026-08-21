#!/usr/bin/env python3
"""Assemble one Workforce V3 report from live observations and proof fragments."""

from __future__ import annotations

import argparse
import json
import os
import secrets
import stat
import sys
from pathlib import Path
from typing import Any

import compare_workforce_conformance_v3 as conformance
import workforce_v3_proof_fragments as fragments

ROOT = Path(__file__).resolve().parents[2]
LIVE_FORMAT = "typebridge.workforce-v3-live-observations/v1"
REPORT_FORMAT = "typebridge.sdk-conformance-report/v3"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
MAX_LIVE_BYTES = 512 * 1024
MAX_REPORT_BYTES = 1024 * 1024


class AssemblyError(ValueError):
    """A stable fail-closed V3 report-assembly rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def _exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise AssemblyError("invalid_object", f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        raise AssemblyError(
            "invalid_object_fields",
            f"{label} fields differ; missing={sorted(keys - actual)}, "
            f"extra={sorted(actual - keys)}",
        )
    return value


def _regular_bytes(path: Path, label: str, limit: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise AssemblyError("invalid_input_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise AssemblyError("invalid_input_file", f"{label} must be a regular non-symlink file")
    if metadata.st_size > limit:
        raise AssemblyError("input_size_limit", f"{label} exceeds {limit} bytes")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise AssemblyError("invalid_input_file", f"{label} cannot be read") from error
    if len(raw) > limit:
        raise AssemblyError("input_size_limit", f"{label} exceeds {limit} bytes")
    return raw


def _load_live(path: Path) -> dict[str, Any]:
    raw = _regular_bytes(path, "live observation bundle", MAX_LIVE_BYTES)
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=conformance._duplicate_key_object,
            parse_constant=lambda value: (_ for _ in ()).throw(
                AssemblyError("invalid_json_number", f"non-finite number {value!r}")
            ),
        )
    except AssemblyError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise AssemblyError("malformed_live_bundle", "live bundle is not bounded JSON") from error
    if raw != conformance.canonical_json_bytes(value):
        raise AssemblyError(
            "noncanonical_live_bundle",
            "live bundle must be compact key-sorted JSON with one trailing newline",
        )
    return _exact_object(
        value,
        {
            "format",
            "binding",
            "semantic_profile",
            "producer",
            "semantic_fingerprint",
            "projection_fingerprint",
            "results",
        },
        "live observation bundle",
    )


def _live_observations(
    live: dict[str, Any],
    *,
    binding: str,
    contracts: conformance.Contracts,
    fragment_lanes: set[tuple[str, str]],
) -> dict[tuple[str, str], dict[str, Any]]:
    if live["format"] != LIVE_FORMAT:
        raise AssemblyError("invalid_live_format", "live bundle format is not V3")
    if live["binding"] != binding or live["semantic_profile"] != SEMANTIC_PROFILE:
        raise AssemblyError("live_identity_mismatch", "live binding or semantic profile differs")
    if live["producer"] != contracts.report_producers[binding]["id"]:
        raise AssemblyError("producer_identity_mismatch", "live producer identity is not frozen")
    if live["semantic_fingerprint"] != contracts.semantic_fingerprint:
        raise AssemblyError("semantic_fingerprint_mismatch", "live semantic fingerprint drifted")
    if live["projection_fingerprint"] != contracts.projection_fingerprints[binding]:
        raise AssemblyError(
            "projection_fingerprint_mismatch", "live projection fingerprint drifted"
        )
    if type(live["results"]) is not list:
        raise AssemblyError("invalid_results", "live results must be an array")

    expected = {
        (observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected
    } - fragment_lanes
    observations: dict[tuple[str, str], dict[str, Any]] = {}
    ordered_lanes: list[tuple[str, str]] = []
    for index, value in enumerate(live["results"]):
        result = _exact_object(
            value,
            {"observation_ref", "proof_kind", "outcome", "observation"},
            f"live results[{index}]",
        )
        lane = (result["observation_ref"], result["proof_kind"])
        if lane not in expected:
            raise AssemblyError("unexpected_live_lane", f"live result carries {lane!r}")
        if result["outcome"] != "passed" or type(result["observation"]) is not dict:
            raise AssemblyError("invalid_live_result", f"live result {lane!r} did not pass")
        if lane in observations:
            raise AssemblyError("duplicate_live_lane", f"live result {lane!r} occurs twice")
        observations[lane] = result["observation"]
        ordered_lanes.append(lane)
    if ordered_lanes != sorted(ordered_lanes):
        raise AssemblyError("unordered_live_results", "live results must be lane-sorted")
    if set(observations) != expected:
        raise AssemblyError("live_lane_coverage_mismatch", "live results do not cover exact lanes")
    return observations


def _source_identity(relative: str, loaded: conformance.LoadedJson) -> dict[str, str]:
    return {"path": relative, "sha256": loaded.sha256}


def assemble_report(
    live: dict[str, Any],
    *,
    binding: str,
    contracts: conformance.Contracts,
    proof_observations: dict[tuple[str, str], dict[str, Any]],
) -> dict[str, Any]:
    live_observations = _live_observations(
        live,
        binding=binding,
        contracts=contracts,
        fragment_lanes=set(proof_observations),
    )
    observations = {**live_observations, **proof_observations}
    selected_lanes = {
        (observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected
    }
    if set(observations) != selected_lanes:
        raise AssemblyError("lane_coverage_mismatch", "combined observations are incomplete")

    results = []
    for case_id, proof_kind, observation_ref in contracts.selected:
        results.append(
            {
                "case_id": case_id,
                "capability_id": contracts.cases[case_id]["capability"]["id"],
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observations[(observation_ref, proof_kind)],
            }
        )
    return {
        "format": REPORT_FORMAT,
        "binding": binding,
        "manifest": _source_identity(conformance.MANIFEST_RELATIVE, contracts.manifest),
        "catalog": _source_identity(conformance.CATALOG_RELATIVE, contracts.catalog),
        "fixture": {
            "id": "workforce-v3",
            "version": 3,
            "semantic_profile": SEMANTIC_PROFILE,
            "schema": _source_identity(conformance.SCHEMA_RELATIVE, contracts.schema),
            "provider_schema": _source_identity(
                conformance.PROVIDER_SCHEMA_RELATIVE, contracts.provider_schema
            ),
            "journey": _source_identity(conformance.JOURNEY_RELATIVE, contracts.journey),
            "semantic_fingerprint": contracts.semantic_fingerprint,
            "projection_target": contracts.projection_targets[binding],
            "projection_fingerprint": contracts.projection_fingerprints[binding],
        },
        "results": results,
    }


def _publish(path: Path, report: dict[str, Any]) -> None:
    if not path.is_absolute():
        raise AssemblyError("invalid_output_path", "output path must be absolute")
    parent = path.parent
    try:
        metadata = parent.lstat()
    except OSError as error:
        raise AssemblyError("invalid_output_path", "output parent cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise AssemblyError("invalid_output_path", "output parent must be a real directory")
    raw = conformance.canonical_json_bytes(report)
    if len(raw) > MAX_REPORT_BYTES:
        raise AssemblyError("report_size_limit", "assembled report exceeds its size limit")
    temporary = parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    try:
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, path, follow_symlinks=False)
    except OSError as error:
        raise AssemblyError("report_publication_failed", "report cannot be created") from error
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.REPORT_BINDINGS)
    parser.add_argument("--live-observations", required=True, type=Path)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("fragments", nargs="+", type=Path)
    arguments = parser.parse_args(argv)
    try:
        contracts = conformance.load_contracts()
        proof_observations = fragments.load_proof_fragments(
            arguments.fragments,
            expected_binding=arguments.binding,
            run_nonce=arguments.run_nonce,
            root=ROOT,
        )
        live = _load_live(arguments.live_observations)
        report = assemble_report(
            live,
            binding=arguments.binding,
            contracts=contracts,
            proof_observations=proof_observations,
        )
        conformance._validate_report(
            conformance.LoadedJson(
                path=arguments.output,
                raw=conformance.canonical_json_bytes(report),
                value=report,
            ),
            contracts,
        )
        _publish(arguments.output, report)
    except (AssemblyError, conformance.ContractError, fragments.ProofFragmentError) as error:
        print(f"workforce-v3 report assembly rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
