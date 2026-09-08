"""Hostile tests for sdk-v3 deterministic proof fragments."""

from __future__ import annotations

import copy
import importlib.util
import json
import shutil
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

SOURCE_ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = SOURCE_ROOT / "scripts/ci/sdk_v3_proof_fragments.py"
RUN_NONCE = "9" * 64
PACKAGE_LANES = {
    ("projected_constraint_validation", "diagnostic"),
    ("projection_evidence_integrity", "diagnostic"),
    ("token_package_fencing", "diagnostic"),
}
DATA_LANES = {
    ("complete_connection_policy", "direct_runtime"),
    ("data_operation_cancellation", "direct_runtime"),
    ("data_operation_resource_limits", "direct_runtime"),
    ("data_operation_structured_diagnostic", "diagnostic"),
}
ALLOWED = PACKAGE_LANES | DATA_LANES


def _load_validator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "sdk_v3_proof_fragments",
        VALIDATOR_PATH,
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


validator = _load_validator()


def _authority(tmp_path: Path) -> tuple[str, str]:
    for relative in (validator.PROOF_SCHEMA_RELATIVE, validator.JOURNEY_RELATIVE):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SOURCE_ROOT / relative, target)
    data_source = "tests/proofs/data-provider.py"
    package_source = "tests/proofs/package-provider.py"
    for relative in (data_source, package_source):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(f"# deterministic {relative}\n", encoding="utf-8")
    bindings: dict[str, Any] = {}
    for binding in ("python", "node", "rust", "c"):
        bindings[binding] = [
            {
                "id": f"{binding}.data-proof",
                "sources": [data_source],
                "results": [
                    {
                        "observation_ref": observation_ref,
                        "proof_kind": proof_kind,
                        "test_id": f"{binding}.data-test",
                    }
                    for observation_ref, proof_kind in sorted(DATA_LANES)
                ],
            },
            {
                "id": f"{binding}.package-proof",
                "sources": [package_source],
                "results": [
                    {
                        "observation_ref": observation_ref,
                        "proof_kind": proof_kind,
                        "test_id": f"{binding}.package-test",
                    }
                    for observation_ref, proof_kind in sorted(PACKAGE_LANES)
                ],
            },
        ]
    allowlist = {
        "format": validator.ALLOWLIST_FORMAT,
        "semantic_profile": validator.SEMANTIC_PROFILE,
        "bindings": bindings,
    }
    allowlist_path = tmp_path / validator.ALLOWLIST_RELATIVE
    allowlist_path.parent.mkdir(parents=True, exist_ok=True)
    allowlist_path.write_bytes(validator.canonical_json_bytes(allowlist))
    return data_source, package_source


def _fragment(
    root: Path,
    *,
    producer: str,
    source: str,
    lanes: set[tuple[str, str]],
    test_id: str,
) -> dict[str, Any]:
    return {
        "binding": "python",
        "contract": {
            "allowlist": validator.source_identity(root, validator.ALLOWLIST_RELATIVE),
            "journey": validator.source_identity(root, validator.JOURNEY_RELATIVE),
            "proof_schema": validator.source_identity(root, validator.PROOF_SCHEMA_RELATIVE),
        },
        "format": validator.FRAGMENT_FORMAT,
        "producer": {
            "id": producer,
            "sources": [validator.source_identity(root, source)],
        },
        "results": [
            {
                "observation": {"computed_lane": observation_ref, "observed": True},
                "observation_ref": observation_ref,
                "outcome": "passed",
                "proof_kind": proof_kind,
                "test_id": test_id,
            }
            for observation_ref, proof_kind in sorted(lanes)
        ],
        "run_nonce": RUN_NONCE,
        "semantic_profile": validator.SEMANTIC_PROFILE,
    }


def _fragments(tmp_path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    data_source, package_source = _authority(tmp_path)
    return (
        _fragment(
            tmp_path,
            producer="python.data-proof",
            source=data_source,
            lanes=DATA_LANES,
            test_id="python.data-test",
        ),
        _fragment(
            tmp_path,
            producer="python.package-proof",
            source=package_source,
            lanes=PACKAGE_LANES,
            test_id="python.package-test",
        ),
    )


def _write_fragments(tmp_path: Path, values: tuple[dict[str, Any], ...]) -> list[Path]:
    paths = []
    for index, value in enumerate(values):
        path = tmp_path / f"fragment-{index}.json"
        path.write_bytes(validator.canonical_json_bytes(value))
        paths.append(path)
    return paths


def _load(tmp_path: Path, values: tuple[dict[str, Any], ...]) -> dict[tuple[str, str], Any]:
    return validator.load_proof_fragments(
        _write_fragments(tmp_path, values),
        expected_binding="python",
        run_nonce=RUN_NONCE,
        allowed_lanes=ALLOWED,
        root=tmp_path,
    )


def test_valid_fragments_require_exactly_two_producers_and_seven_actual_lanes(
    tmp_path: Path,
) -> None:
    observations = _load(tmp_path, _fragments(tmp_path))

    assert set(observations) == ALLOWED
    assert len(observations) == 7
    assert observations[("complete_connection_policy", "direct_runtime")] == {
        "computed_lane": "complete_connection_policy",
        "observed": True,
    }


def test_committed_phase0_allowlist_has_exact_two_by_seven_authority() -> None:
    for binding in validator.REPORT_BINDINGS:
        authority = validator.proof_fragment_authority(SOURCE_ROOT, binding)
        assert len(authority) == 2
        lanes = {lane for _, producer_lanes in authority.values() for lane in producer_lanes}
        assert lanes == ALLOWED
        assert sorted(len(producer_lanes) for _, producer_lanes in authority.values()) == [3, 4]


def test_cli_observation_mode_returns_only_validated_canonical_lanes(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    paths = _write_fragments(tmp_path, _fragments(tmp_path))
    assert (
        validator.main(
            [
                "--binding",
                "python",
                "--run-nonce",
                RUN_NONCE,
                "--observations-json",
                *(str(path) for path in paths),
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
    data, package = _fragments(tmp_path)
    mutation(data)

    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, (data, package))
    assert rejected.value.code == code


def test_missing_lane_producer_and_duplicate_producer_fail_closed(tmp_path: Path) -> None:
    data, package = _fragments(tmp_path)
    package["results"].pop()
    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, (data, package))
    assert rejected.value.code == "producer_lane_coverage_mismatch"

    data, package = _fragments(tmp_path)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, (data,))
    assert rejected.value.code == "lane_coverage_mismatch"

    data, package = _fragments(tmp_path)
    duplicate = copy.deepcopy(data)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        _load(tmp_path, (data, duplicate, package))
    assert rejected.value.code == "duplicate_producer"


def test_noncanonical_symlink_and_source_drift_fail_closed(tmp_path: Path) -> None:
    data, package = _fragments(tmp_path)
    paths = _write_fragments(tmp_path, (data, package))
    paths[0].write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            paths,
            expected_binding="python",
            run_nonce=RUN_NONCE,
            root=tmp_path,
        )
    assert rejected.value.code == "noncanonical_fragment"

    paths = _write_fragments(tmp_path, (data, package))
    link = tmp_path / "fragment-link.json"
    link.symlink_to(paths[0])
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            [link, paths[1]],
            expected_binding="python",
            run_nonce=RUN_NONCE,
            root=tmp_path,
        )
    assert rejected.value.code == "fragment_not_regular"

    (tmp_path / "tests/proofs/data-provider.py").write_text(
        "# changed deterministic source\n", encoding="utf-8"
    )
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.load_proof_fragments(
            paths,
            expected_binding="python",
            run_nonce=RUN_NONCE,
            root=tmp_path,
        )
    assert rejected.value.code == "source_digest_mismatch"
