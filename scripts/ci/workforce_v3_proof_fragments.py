#!/usr/bin/env python3
"""Validate same-run deterministic proof fragments for workforce-v3 reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
from collections.abc import Iterable
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
PROOF_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-schema-v1.json"
ALLOWLIST_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-allowlist-v1.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"

FRAGMENT_FORMAT = "typebridge.workforce-v3-proof-fragment/v1"
ALLOWLIST_FORMAT = "typebridge.workforce-v3-proof-fragment-allowlist/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
REPORT_BINDINGS = frozenset({"python", "node", "rust", "c"})
PROOF_KINDS = frozenset({"direct_runtime", "diagnostic"})
MAX_FRAGMENT_BYTES = 64 * 1024
MAX_RESULTS = 4
MAX_SOURCES = 16

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PORTABLE_PATH_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")
PRODUCER_ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
OBSERVATION_REF_RE = re.compile(r"^[a-z][a-z0-9_]{0,95}$")
TEST_ID_RE = re.compile(r"^[a-z0-9][a-z0-9._:-]{0,191}$")

ObservationLane = tuple[str, str]
ProducerAuthority = tuple[tuple[str, ...], dict[ObservationLane, str]]


class ProofFragmentError(ValueError):
    """A stable fail-closed proof-fragment rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def canonical_json_bytes(value: Any) -> bytes:
    """Return the compact, sorted, UTF-8 JSON form required for fragments."""

    return (
        json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def _reject_constant(value: str) -> None:
    raise ProofFragmentError("invalid_json_number", f"non-finite JSON number {value!r}")


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ProofFragmentError("duplicate_json_key", f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _exact_keys(value: Any, expected: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ProofFragmentError("invalid_shape", f"{context} must be an object")
    actual = set(value)
    if actual != expected:
        raise ProofFragmentError(
            "invalid_shape",
            f"{context} keys differ: expected {sorted(expected)!r}, got {sorted(actual)!r}",
        )
    return value


def _portable_relative_path(value: Any, context: str) -> str:
    if not isinstance(value, str) or not PORTABLE_PATH_RE.fullmatch(value):
        raise ProofFragmentError("invalid_source_path", f"{context} is not a portable path")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or path.as_posix() != value
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise ProofFragmentError("invalid_source_path", f"{context} escapes the source root")
    if len(value) > 512:
        raise ProofFragmentError("invalid_source_path", f"{context} exceeds 512 bytes")
    return value


def _regular_source_bytes(root: Path, relative: str) -> bytes:
    relative_path = PurePosixPath(_portable_relative_path(relative, "source path"))
    path = root
    metadata = None
    for component in relative_path.parts:
        path = path / component
        try:
            metadata = path.lstat()
        except OSError as error:
            raise ProofFragmentError(
                "source_unreadable", f"source {relative!r} is not inspectable: {error}"
            ) from error
        if path.is_symlink():
            raise ProofFragmentError(
                "source_not_regular", f"source {relative!r} traverses a symlink"
            )
    if metadata is None or not stat.S_ISREG(metadata.st_mode):
        raise ProofFragmentError(
            "source_not_regular", f"source {relative!r} must be a regular non-symlink file"
        )
    try:
        return path.read_bytes()
    except OSError as error:
        raise ProofFragmentError(
            "source_unreadable", f"source {relative!r} is not readable: {error}"
        ) from error


def source_identity(root: Path, relative: str) -> dict[str, str]:
    """Return the exact raw source identity used by a proof fragment."""

    relative = _portable_relative_path(relative, "source path")
    return {
        "path": relative,
        "sha256": hashlib.sha256(_regular_source_bytes(root, relative)).hexdigest(),
    }


def _validate_source_identity(value: Any, root: Path, context: str) -> dict[str, str]:
    identity = _exact_keys(value, {"path", "sha256"}, context)
    relative = _portable_relative_path(identity["path"], f"{context}.path")
    digest = identity["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        raise ProofFragmentError("invalid_source_digest", f"{context}.sha256 is malformed")
    expected = source_identity(root, relative)
    if identity != expected:
        raise ProofFragmentError("source_digest_mismatch", f"{context} does not match {relative}")
    return expected


def _inspect_fragment_path(path: Path) -> tuple[Path, os.stat_result]:
    """Inspect a fragment without accepting a symlink at any path component."""

    if not path.is_absolute() or ".." in path.parts:
        raise ProofFragmentError(
            "invalid_fragment_path",
            f"fragment {path} must use a normalized absolute path",
        )
    absolute = path
    chain = list(reversed((absolute, *absolute.parents)))
    metadata = None
    for component in chain:
        try:
            metadata = component.lstat()
        except OSError as error:
            raise ProofFragmentError(
                "fragment_unreadable", f"fragment {path} is not inspectable: {error}"
            ) from error
        if stat.S_ISLNK(metadata.st_mode):
            raise ProofFragmentError("fragment_not_regular", f"fragment {path} traverses a symlink")
    if metadata is None:
        raise ProofFragmentError("fragment_unreadable", f"fragment {path} is not inspectable")
    return absolute, metadata


def _load_fragment(path: Path) -> dict[str, Any]:
    inspected_path, metadata = _inspect_fragment_path(path)
    if not stat.S_ISREG(metadata.st_mode):
        raise ProofFragmentError(
            "fragment_not_regular", f"fragment {path} must be a regular non-symlink file"
        )
    if metadata.st_size > MAX_FRAGMENT_BYTES:
        raise ProofFragmentError(
            "fragment_too_large", f"fragment {path} exceeds {MAX_FRAGMENT_BYTES} bytes"
        )
    try:
        raw = inspected_path.read_bytes()
    except OSError as error:
        raise ProofFragmentError(
            "fragment_unreadable", f"fragment {path} is not readable: {error}"
        ) from error
    try:
        value = json.loads(
            raw,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_constant,
        )
    except ProofFragmentError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProofFragmentError(
            "invalid_json", f"fragment {path} is invalid JSON: {error}"
        ) from error
    if raw != canonical_json_bytes(value):
        raise ProofFragmentError(
            "noncanonical_fragment", f"fragment {path} is not compact canonical JSON plus one LF"
        )
    return _exact_keys(
        value,
        {"format", "binding", "semantic_profile", "run_nonce", "contract", "producer", "results"},
        f"fragment {path}",
    )


def _validate_lane(value: Any, context: str) -> ObservationLane:
    if (
        not isinstance(value, tuple)
        or len(value) != 2
        or not isinstance(value[0], str)
        or not isinstance(value[1], str)
    ):
        raise ProofFragmentError("invalid_allowlist", f"{context} must be a string pair")
    observation_ref, proof_kind = value
    if not OBSERVATION_REF_RE.fullmatch(observation_ref) or proof_kind not in PROOF_KINDS:
        raise ProofFragmentError("invalid_allowlist", f"{context} is not a valid observation lane")
    return value


def proof_fragment_authority(root: Path, binding: str) -> dict[str, ProducerAuthority]:
    """Load the exact committed producer/source/test authority for one binding."""

    if binding not in REPORT_BINDINGS:
        raise ProofFragmentError("invalid_binding", f"unknown binding {binding!r}")
    raw = _regular_source_bytes(root, ALLOWLIST_RELATIVE)
    try:
        value = json.loads(
            raw,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_constant,
        )
    except ProofFragmentError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProofFragmentError(
            "invalid_allowlist", "committed allowlist is invalid JSON"
        ) from error
    allowlist = _exact_keys(value, {"format", "semantic_profile", "bindings"}, "allowlist")
    if allowlist["format"] != ALLOWLIST_FORMAT or allowlist["semantic_profile"] != SEMANTIC_PROFILE:
        raise ProofFragmentError("invalid_allowlist", "committed allowlist authority is not v1")
    bindings = _exact_keys(allowlist["bindings"], set(REPORT_BINDINGS), "allowlist bindings")
    producers = bindings[binding]
    if not isinstance(producers, list) or not producers:
        raise ProofFragmentError("invalid_allowlist", f"{binding} allowlist is empty or malformed")
    authority: dict[str, ProducerAuthority] = {}
    claimed_lanes: set[ObservationLane] = set()
    producer_ids: list[str] = []
    for producer_index, item in enumerate(producers):
        producer = _exact_keys(
            item,
            {"id", "sources", "results"},
            f"allowlist producer[{producer_index}]",
        )
        producer_id = producer["id"]
        if not isinstance(producer_id, str) or not PRODUCER_ID_RE.fullmatch(producer_id):
            raise ProofFragmentError("invalid_allowlist", "allowlist producer ID is invalid")
        sources = producer["sources"]
        if not isinstance(sources, list) or not 1 <= len(sources) <= MAX_SOURCES:
            raise ProofFragmentError(
                "invalid_allowlist", "allowlist producer sources are malformed"
            )
        source_paths = tuple(
            _portable_relative_path(source, f"allowlist producer[{producer_index}] source")
            for source in sources
        )
        if list(source_paths) != sorted(source_paths) or len(set(source_paths)) != len(
            source_paths
        ):
            raise ProofFragmentError(
                "invalid_allowlist", "allowlist producer sources are not unique and sorted"
            )
        results = producer["results"]
        if not isinstance(results, list) or not 1 <= len(results) <= MAX_RESULTS:
            raise ProofFragmentError(
                "invalid_allowlist", "allowlist producer results are malformed"
            )
        result_authority: dict[ObservationLane, str] = {}
        ordered_lanes: list[ObservationLane] = []
        for result_index, item in enumerate(results):
            result = _exact_keys(
                item,
                {"observation_ref", "proof_kind", "test_id"},
                f"allowlist producer[{producer_index}].results[{result_index}]",
            )
            lane = _validate_lane(
                (result["observation_ref"], result["proof_kind"]),
                f"allowlist producer[{producer_index}].results[{result_index}]",
            )
            test_id = result["test_id"]
            if not isinstance(test_id, str) or not TEST_ID_RE.fullmatch(test_id):
                raise ProofFragmentError("invalid_allowlist", "allowlist test ID is invalid")
            if lane in result_authority or lane in claimed_lanes:
                raise ProofFragmentError("invalid_allowlist", "allowlist lane is claimed twice")
            result_authority[lane] = test_id
            claimed_lanes.add(lane)
            ordered_lanes.append(lane)
        if ordered_lanes != sorted(ordered_lanes):
            raise ProofFragmentError(
                "invalid_allowlist", "allowlist producer results are not sorted"
            )
        if producer_id in authority:
            raise ProofFragmentError("invalid_allowlist", "allowlist producer ID is duplicated")
        authority[producer_id] = (source_paths, result_authority)
        producer_ids.append(producer_id)
    if producer_ids != sorted(producer_ids):
        raise ProofFragmentError(
            "invalid_allowlist", f"{binding} allowlist producers are not unique and sorted"
        )
    return authority


def proof_fragment_lanes(root: Path, binding: str) -> set[ObservationLane]:
    """Return the exact committed deterministic lane set for one binding."""

    return {
        lane
        for _, result_authority in proof_fragment_authority(root, binding).values()
        for lane in result_authority
    }


def load_proof_fragments(
    fragment_paths: Iterable[Path],
    *,
    expected_binding: str,
    run_nonce: str,
    allowed_lanes: Iterable[ObservationLane] | None = None,
    root: Path = ROOT,
) -> dict[ObservationLane, Any]:
    """Validate fragments and return their exact, closed observation lane map."""

    if expected_binding not in REPORT_BINDINGS:
        raise ProofFragmentError("invalid_binding", f"unknown binding {expected_binding!r}")
    if not SHA256_RE.fullmatch(run_nonce):
        raise ProofFragmentError(
            "invalid_run_nonce", "run nonce must be 64 lowercase hex characters"
        )
    producer_authority = proof_fragment_authority(root, expected_binding)
    committed_lanes = {
        lane for _, result_authority in producer_authority.values() for lane in result_authority
    }
    if allowed_lanes is None:
        allowed = committed_lanes
    else:
        allowed = {_validate_lane(lane, "allowed lane") for lane in allowed_lanes}
        if allowed != committed_lanes:
            raise ProofFragmentError(
                "invalid_allowlist", "caller lane allowlist differs from committed authority"
            )

    paths = [Path(path) for path in fragment_paths]
    if not paths:
        raise ProofFragmentError("missing_fragment", "at least one proof fragment is required")
    normalized_paths = [path.resolve(strict=False) for path in paths]
    if len(set(normalized_paths)) != len(normalized_paths):
        raise ProofFragmentError(
            "duplicate_fragment", "one fragment path was supplied more than once"
        )

    expected_contract = {
        "proof_schema": source_identity(root, PROOF_SCHEMA_RELATIVE),
        "allowlist": source_identity(root, ALLOWLIST_RELATIVE),
        "journey": source_identity(root, JOURNEY_RELATIVE),
    }
    observations: dict[ObservationLane, Any] = {}
    producer_ids: set[str] = set()

    for path in paths:
        fragment = _load_fragment(path)
        if fragment["format"] != FRAGMENT_FORMAT:
            raise ProofFragmentError("format_mismatch", f"fragment {path} has the wrong format")
        if fragment["binding"] != expected_binding:
            raise ProofFragmentError(
                "binding_mismatch", f"fragment {path} belongs to another binding"
            )
        if fragment["semantic_profile"] != SEMANTIC_PROFILE:
            raise ProofFragmentError("profile_mismatch", f"fragment {path} has the wrong profile")
        if fragment["run_nonce"] != run_nonce:
            raise ProofFragmentError("run_nonce_mismatch", f"fragment {path} is from another run")

        contract = _exact_keys(
            fragment["contract"], {"proof_schema", "allowlist", "journey"}, "contract"
        )
        for name, identity in contract.items():
            _validate_source_identity(identity, root, f"contract.{name}")
        if contract != expected_contract:
            raise ProofFragmentError("contract_mismatch", f"fragment {path} binds another contract")

        producer = _exact_keys(fragment["producer"], {"id", "sources"}, "producer")
        producer_id = producer["id"]
        if not isinstance(producer_id, str) or not PRODUCER_ID_RE.fullmatch(producer_id):
            raise ProofFragmentError(
                "invalid_producer", f"fragment {path} has an invalid producer ID"
            )
        if producer_id in producer_ids:
            raise ProofFragmentError(
                "duplicate_producer", f"producer {producer_id!r} emitted twice"
            )
        producer_ids.add(producer_id)
        authority = producer_authority.get(producer_id)
        if authority is None:
            raise ProofFragmentError(
                "unexpected_producer",
                f"producer {producer_id!r} is not committed for {expected_binding}",
            )
        expected_sources, expected_results = authority
        sources = producer["sources"]
        if not isinstance(sources, list) or not 1 <= len(sources) <= MAX_SOURCES:
            raise ProofFragmentError(
                "invalid_producer", "producer sources must contain 1..16 items"
            )
        validated_sources = [
            _validate_source_identity(source, root, f"producer.sources[{index}]")
            for index, source in enumerate(sources)
        ]
        source_paths = [source["path"] for source in validated_sources]
        if source_paths != sorted(source_paths) or len(set(source_paths)) != len(source_paths):
            raise ProofFragmentError(
                "invalid_producer", "producer source identities must be unique and path-sorted"
            )
        if tuple(source_paths) != expected_sources:
            raise ProofFragmentError(
                "producer_source_mismatch",
                f"producer {producer_id!r} source paths differ from committed authority",
            )

        results = fragment["results"]
        if not isinstance(results, list) or not 1 <= len(results) <= MAX_RESULTS:
            raise ProofFragmentError("invalid_results", "fragment results must contain 1..16 items")
        result_lanes: list[ObservationLane] = []
        for index, item in enumerate(results):
            result = _exact_keys(
                item,
                {"observation_ref", "proof_kind", "test_id", "outcome", "observation"},
                f"results[{index}]",
            )
            observation_ref = result["observation_ref"]
            proof_kind = result["proof_kind"]
            test_id = result["test_id"]
            if not isinstance(observation_ref, str) or not OBSERVATION_REF_RE.fullmatch(
                observation_ref
            ):
                raise ProofFragmentError("invalid_result", f"results[{index}] has an invalid ref")
            if proof_kind not in PROOF_KINDS:
                raise ProofFragmentError("invalid_result", f"results[{index}] has an invalid kind")
            if not isinstance(test_id, str) or not TEST_ID_RE.fullmatch(test_id):
                raise ProofFragmentError(
                    "invalid_result", f"results[{index}] has an invalid test ID"
                )
            if result["outcome"] != "passed" or not isinstance(result["observation"], dict):
                raise ProofFragmentError(
                    "invalid_result", f"results[{index}] did not pass with an object"
                )
            lane = (observation_ref, proof_kind)
            if lane not in allowed:
                raise ProofFragmentError("unexpected_lane", f"fragment {path} carries {lane!r}")
            expected_test_id = expected_results.get(lane)
            if expected_test_id is None:
                raise ProofFragmentError(
                    "producer_lane_mismatch",
                    f"producer {producer_id!r} does not own lane {lane!r}",
                )
            if test_id != expected_test_id:
                raise ProofFragmentError(
                    "test_id_mismatch",
                    f"producer {producer_id!r} lane {lane!r} has the wrong test ID",
                )
            if lane in observations:
                raise ProofFragmentError("duplicate_lane", f"proof lane {lane!r} was emitted twice")
            observations[lane] = result["observation"]
            result_lanes.append(lane)
        if result_lanes != sorted(result_lanes):
            raise ProofFragmentError("invalid_results", "fragment results must be lane-sorted")
        if set(result_lanes) != set(expected_results):
            raise ProofFragmentError(
                "producer_lane_coverage_mismatch",
                f"producer {producer_id!r} did not emit its exact committed lanes",
            )

    actual = set(observations)
    if actual != allowed:
        missing = sorted(allowed - actual)
        extra = sorted(actual - allowed)
        raise ProofFragmentError(
            "lane_coverage_mismatch", f"proof lanes differ: missing={missing!r}, extra={extra!r}"
        )
    if producer_ids != set(producer_authority):
        raise ProofFragmentError(
            "producer_coverage_mismatch", "proof fragments did not cover every committed producer"
        )
    return observations


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
        print(f"workforce-v3 proof fragments rejected: {error}")
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
        print(f"validated {len(observations)} workforce-v3 proof lanes for {args.binding}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
