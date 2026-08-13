"""Hostile tests for workforce-v2 deterministic proof fragments."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

SOURCE_ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = SOURCE_ROOT / "scripts/ci/workforce_v2_proof_fragments.py"
RUN_NONCE = "7" * 64
ALLOWED = {
    ("cancellation_direct", "direct_runtime"),
    ("cancellation_remote", "remote_runtime"),
    ("remote_structured_diagnostic", "diagnostic"),
}


def _load_validator() -> ModuleType:
    spec = importlib.util.spec_from_file_location("workforce_v2_proof_fragments", VALIDATOR_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


validator = _load_validator()


def _source_identity(root: Path, relative: str) -> dict[str, str]:
    return {
        "path": relative,
        "sha256": hashlib.sha256((root / relative).read_bytes()).hexdigest(),
    }


def _authority(tmp_path: Path) -> tuple[Path, str]:
    for relative in (validator.PROOF_SCHEMA_RELATIVE, validator.JOURNEY_RELATIVE):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((SOURCE_ROOT / relative).read_bytes())
    producer_relative = "tests/proofs/python-provider.py"
    producer = tmp_path / producer_relative
    producer.parent.mkdir(parents=True, exist_ok=True)
    producer.write_text("# deterministic test source\n", encoding="utf-8")
    allowlist = {
        "format": validator.ALLOWLIST_FORMAT,
        "semantic_profile": validator.SEMANTIC_PROFILE,
        "bindings": {
            binding: [
                {
                    "id": "python.provider-recording",
                    "sources": [producer_relative],
                    "results": [
                        {
                            "observation_ref": "cancellation_direct",
                            "proof_kind": "direct_runtime",
                            "test_id": "python.match-runtime.cancellation",
                        },
                        {
                            "observation_ref": "cancellation_remote",
                            "proof_kind": "remote_runtime",
                            "test_id": "python.generated.remote-cancellation",
                        },
                        {
                            "observation_ref": "remote_structured_diagnostic",
                            "proof_kind": "diagnostic",
                            "test_id": "python.generated.remote-diagnostic",
                        },
                    ],
                }
            ]
            for binding in ("python", "node", "rust", "c")
        },
    }
    allowlist_path = tmp_path / validator.ALLOWLIST_RELATIVE
    allowlist_path.parent.mkdir(parents=True, exist_ok=True)
    allowlist_path.write_bytes(validator.canonical_json_bytes(allowlist))
    return producer, producer_relative


def _fragment(root: Path, producer_relative: str) -> dict[str, Any]:
    return {
        "binding": "python",
        "contract": {
            "allowlist": _source_identity(root, validator.ALLOWLIST_RELATIVE),
            "journey": _source_identity(root, validator.JOURNEY_RELATIVE),
            "proof_schema": _source_identity(root, validator.PROOF_SCHEMA_RELATIVE),
        },
        "format": validator.FRAGMENT_FORMAT,
        "producer": {
            "id": "python.provider-recording",
            "sources": [_source_identity(root, producer_relative)],
        },
        "results": [
            {
                "observation": {
                    "in_flight": {"provider_await_woken": True},
                    "pre_dispatch": {"provider_calls": 0},
                },
                "observation_ref": "cancellation_direct",
                "outcome": "passed",
                "proof_kind": "direct_runtime",
                "test_id": "python.match-runtime.cancellation",
            },
            {
                "observation": {
                    "before_exchange": {"exchange_count": 0},
                    "during_decode": {"exchange_count": 1},
                },
                "observation_ref": "cancellation_remote",
                "outcome": "passed",
                "proof_kind": "remote_runtime",
                "test_id": "python.generated.remote-cancellation",
            },
            {
                "observation": {"claim_consumed": True, "redacted": True},
                "observation_ref": "remote_structured_diagnostic",
                "outcome": "passed",
                "proof_kind": "diagnostic",
                "test_id": "python.generated.remote-diagnostic",
            },
        ],
        "run_nonce": RUN_NONCE,
        "semantic_profile": validator.SEMANTIC_PROFILE,
    }


def _write_fragment(path: Path, value: Any) -> None:
    path.write_bytes(validator.canonical_json_bytes(value))


def _load(tmp_path: Path, value: dict[str, Any]) -> dict[tuple[str, str], Any]:
    path = tmp_path / "fragment.json"
    _write_fragment(path, value)
    return validator.load_proof_fragments(
        [path],
        expected_binding="python",
        run_nonce=RUN_NONCE,
        allowed_lanes=ALLOWED,
        root=tmp_path,
    )


def test_valid_fragment_returns_only_the_exact_allowed_actual_lanes(tmp_path: Path) -> None:
    _, producer_relative = _authority(tmp_path)
    fragment = _fragment(tmp_path, producer_relative)

    observations = _load(tmp_path, fragment)

    assert set(observations) == ALLOWED
    assert observations[("cancellation_direct", "direct_runtime")]["in_flight"] == {
        "provider_await_woken": True
    }


def test_cli_observation_mode_returns_only_validated_canonical_lanes(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _, producer_relative = _authority(tmp_path)
    fragment = _fragment(tmp_path, producer_relative)
    path = tmp_path / "fragment.json"
    _write_fragment(path, fragment)
    assert (
        validator.main(
            [
                "--binding",
                "python",
                "--run-nonce",
                RUN_NONCE,
                "--observations-json",
                str(path),
            ],
            root=tmp_path,
        )
        == 0
    )
    output = capsys.readouterr().out.encode()
    rows = json.loads(output)
    assert output == validator.canonical_json_bytes(rows)
    assert [(row["observation_ref"], row["proof_kind"]) for row in rows] == sorted(ALLOWED)


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda value: value.update({"binding": "node"}), "binding_mismatch"),
        (lambda value: value.update({"run_nonce": "8" * 64}), "run_nonce_mismatch"),
        (
            lambda value: value["contract"]["journey"].update({"sha256": "0" * 64}),
            "source_digest_mismatch",
        ),
        (
            lambda value: value["producer"]["sources"][0].update({"sha256": "0" * 64}),
            "source_digest_mismatch",
        ),
        (
            lambda value: value["results"][0].update({"observation_ref": "unexpected"}),
            "unexpected_lane",
        ),
        (
            lambda value: value["producer"].update({"id": "python.uncommitted"}),
            "unexpected_producer",
        ),
        (
            lambda value: value["results"][0].update({"test_id": "python.wrong-test"}),
            "test_id_mismatch",
        ),
    ],
)
def test_identity_binding_nonce_and_allowlist_drift_fail_closed(
    tmp_path: Path,
    mutation: Any,
    code: str,
) -> None:
    _, producer_relative = _authority(tmp_path)
    fragment = _fragment(tmp_path, producer_relative)
    mutation(fragment)

    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, fragment)
    assert rejected.value.code == code


def test_missing_lane_and_duplicate_producer_fail_closed(tmp_path: Path) -> None:
    _, producer_relative = _authority(tmp_path)
    missing = _fragment(tmp_path, producer_relative)
    missing["results"].pop()
    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, missing)
    assert rejected.value.code == "producer_lane_coverage_mismatch"

    first = _fragment(tmp_path, producer_relative)
    second = copy.deepcopy(first)
    first_path = tmp_path / "first.json"
    second_path = tmp_path / "second.json"
    _write_fragment(first_path, first)
    _write_fragment(second_path, second)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [first_path, second_path],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "duplicate_producer"


def test_noncanonical_duplicate_key_symlink_and_source_drift_fail_closed(tmp_path: Path) -> None:
    producer, producer_relative = _authority(tmp_path)
    fragment = _fragment(tmp_path, producer_relative)
    path = tmp_path / "fragment.json"

    path.write_text(json.dumps(fragment, indent=2) + "\n", encoding="utf-8")
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [path],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "noncanonical_fragment"

    path.write_text('{"format":"x","format":"y"}\n', encoding="utf-8")
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [path],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "duplicate_json_key"

    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [Path("relative-fragment.json")],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "invalid_fragment_path"

    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [tmp_path / "nested" / ".." / "fragment.json"],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "invalid_fragment_path"

    _write_fragment(path, fragment)
    link = tmp_path / "fragment-link.json"
    link.symlink_to(path)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [link],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "fragment_not_regular"

    real_fragment_dir = tmp_path / "real-fragments"
    real_fragment_dir.mkdir()
    nested_fragment = real_fragment_dir / "fragment.json"
    _write_fragment(nested_fragment, fragment)
    linked_fragment_dir = tmp_path / "linked-fragments"
    linked_fragment_dir.symlink_to(real_fragment_dir, target_is_directory=True)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [linked_fragment_dir / "fragment.json"],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "fragment_not_regular"

    producer.write_text("# changed deterministic test source\n", encoding="utf-8")
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [path],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=ALLOWED,
            root=tmp_path,
        )
    assert rejected.value.code == "source_digest_mismatch"


def test_source_identity_rejects_a_symlinked_parent(tmp_path: Path) -> None:
    _authority(tmp_path)
    real = tmp_path / "real"
    real.mkdir()
    (real / "producer.py").write_text("# source\n", encoding="utf-8")
    link = tmp_path / "linked"
    link.symlink_to(real, target_is_directory=True)

    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.source_identity(tmp_path, "linked/producer.py")
    assert rejected.value.code == "source_not_regular"
