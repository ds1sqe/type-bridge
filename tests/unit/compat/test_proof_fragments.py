"""Shared hostile-input coverage for the two measured SDK proof contracts."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "proof_fragments", ROOT / "scripts/ci/proof_fragments.py"
)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = validator
SPEC.loader.exec_module(validator)
RUN_NONCE = "7" * 64
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
QUERY_RESULTS = [
    {
        "observation_ref": "cancellation_direct",
        "proof_kind": "direct_runtime",
        "test_id": "python.match-runtime.cancellation",
        "outcome": "passed",
        "observation": {
            "in_flight": {"provider_await_woken": True},
            "pre_dispatch": {"provider_calls": 0},
        },
    },
    {
        "observation_ref": "cancellation_remote",
        "proof_kind": "remote_runtime",
        "test_id": "python.generated.remote-cancellation",
        "outcome": "passed",
        "observation": {
            "before_exchange": {"exchange_count": 0},
            "during_decode": {"exchange_count": 1},
        },
    },
    {
        "observation_ref": "remote_structured_diagnostic",
        "proof_kind": "diagnostic",
        "test_id": "python.generated.remote-diagnostic",
        "outcome": "passed",
        "observation": {"claim_consumed": True, "redacted": True},
    },
]


@dataclass
class Proofs:
    version: int
    root: Path
    values: list[dict[str, Any]]
    sources: list[Path]
    lanes: set[tuple[str, str]]

    def write(self, values: list[dict[str, Any]] | None = None) -> list[Path]:
        paths = []
        for index, value in enumerate(self.values if values is None else values):
            path = self.root / f"fragment-{index}.json"
            path.write_bytes(validator.canonical_json_bytes(value))
            paths.append(path)
        return paths

    def load(self, paths: list[Path] | None = None) -> dict[tuple[str, str], Any]:
        return validator.CONTRACTS[self.version].load_proof_fragments(
            self.write() if paths is None else paths,
            expected_binding="python",
            run_nonce=RUN_NONCE,
            allowed_lanes=self.lanes,
            root=self.root,
        )


@pytest.fixture(params=[2, 3], ids=["queries", "models"])
def proofs(request: pytest.FixtureRequest, tmp_path: Path) -> Proofs:
    version = request.param
    contract = validator.CONTRACTS[version]
    for relative in (contract.proof_schema_relative, contract.journey_relative):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((ROOT / relative).read_bytes())
    groups = (
        [("provider-recording", copy.deepcopy(QUERY_RESULTS))]
        if version == 2
        else [
            (
                kind,
                [
                    {
                        "observation_ref": ref,
                        "proof_kind": proof,
                        "test_id": f"python.{kind}-test",
                        "outcome": "passed",
                        "observation": {"computed_lane": ref, "observed": True},
                    }
                    for ref, proof in sorted(lanes)
                ],
            )
            for kind, lanes in (("data-proof", DATA_LANES), ("package-proof", PACKAGE_LANES))
        ]
    )
    producers = []
    sources = []
    for kind, results in groups:
        source = tmp_path / f"tests/proofs/{kind}.py"
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text("# deterministic test source\n")
        sources.append(source)
        producers.append(
            {
                "id": f"python.{kind}",
                "sources": [source.relative_to(tmp_path).as_posix()],
                "results": [
                    {key: item[key] for key in ("observation_ref", "proof_kind", "test_id")}
                    for item in results
                ],
            }
        )
    allowlist = {
        "format": contract.allowlist_format,
        "semantic_profile": validator.SEMANTIC_PROFILE,
        "bindings": {binding: producers for binding in validator.REPORT_BINDINGS},
    }
    target = tmp_path / contract.allowlist_relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(validator.canonical_json_bytes(allowlist))
    values = [
        {
            "binding": "python",
            "format": contract.fragment_format,
            "contract": {
                name: validator.source_identity(tmp_path, relative)
                for name, relative in (
                    ("allowlist", contract.allowlist_relative),
                    ("journey", contract.journey_relative),
                    ("proof_schema", contract.proof_schema_relative),
                )
            },
            "producer": {
                "id": producer["id"],
                "sources": [
                    validator.source_identity(tmp_path, path) for path in producer["sources"]
                ],
            },
            "results": results,
            "run_nonce": RUN_NONCE,
            "semantic_profile": validator.SEMANTIC_PROFILE,
        }
        for producer, (_, results) in zip(producers, groups, strict=True)
    ]
    return Proofs(
        version,
        tmp_path,
        values,
        sources,
        {(r["observation_ref"], r["proof_kind"]) for value in values for r in value["results"]},
    )


def test_only_exact_measured_lanes_are_returned(proofs: Proofs) -> None:
    observed = proofs.load()
    assert set(observed) == proofs.lanes
    if proofs.version == 2:
        assert len(observed) == 3
        assert observed[("cancellation_direct", "direct_runtime")]["in_flight"] == {
            "provider_await_woken": True
        }
    else:
        assert len(proofs.values) == 2 and len(observed) == 7
        assert observed[("complete_connection_policy", "direct_runtime")] == {
            "computed_lane": "complete_connection_policy",
            "observed": True,
        }


def test_committed_model_authority_has_two_producers_and_seven_lanes() -> None:
    for binding in validator.REPORT_BINDINGS:
        authority = validator.CONTRACTS[3].proof_fragment_authority(ROOT, binding)
        assert len(authority) == 2
        assert {
            lane for _, lanes in authority.values() for lane in lanes
        } == PACKAGE_LANES | DATA_LANES
        assert sorted(len(lanes) for _, lanes in authority.values()) == [3, 4]


def test_cli_emits_only_validated_canonical_lanes(
    proofs: Proofs, capsys: pytest.CaptureFixture[str]
) -> None:
    paths = proofs.write()
    assert (
        validator.main(
            [
                "--sdk",
                str(proofs.version),
                "--binding",
                "python",
                "--run-nonce",
                RUN_NONCE,
                "--observations-json",
                *(str(path) for path in paths),
            ],
            root=proofs.root,
        )
        == 0
    )
    raw = capsys.readouterr().out.encode()
    rows = json.loads(raw)
    assert raw == validator.canonical_json_bytes(rows)
    assert [(row["observation_ref"], row["proof_kind"]) for row in rows] == sorted(proofs.lanes)


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda value: value.update(binding="node"), "binding_mismatch"),
        (lambda value: value.update(run_nonce="8" * 64), "run_nonce_mismatch"),
        (
            lambda value: value["contract"]["journey"].update(sha256="0" * 64),
            "source_digest_mismatch",
        ),
        (
            lambda value: value["producer"]["sources"][0].update(sha256="0" * 64),
            "source_digest_mismatch",
        ),
        (lambda value: value["results"][0].update(observation_ref="unexpected"), "unexpected_lane"),
        (lambda value: value["producer"].update(id="python.uncommitted"), "unexpected_producer"),
        (lambda value: value["results"][0].update(test_id="python.wrong-test"), "test_id_mismatch"),
    ],
)
def test_identity_and_authority_drift_fail_closed(proofs: Proofs, mutation: Any, code: str) -> None:
    mutation(proofs.values[0])
    with pytest.raises(validator.ProofFragmentError) as rejected:
        proofs.load()
    assert rejected.value.code == code


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda values: values[-1]["results"].pop(), "producer_lane_coverage_mismatch"),
        (lambda values: values.insert(1, copy.deepcopy(values[0])), "duplicate_producer"),
    ],
)
def test_missing_lane_and_duplicate_producer_fail_closed(
    proofs: Proofs, mutation: Any, code: str
) -> None:
    mutation(proofs.values)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        proofs.load()
    assert rejected.value.code == code


@pytest.mark.parametrize("proofs", [3], indirect=True)
def test_model_contract_requires_both_producers(proofs: Proofs) -> None:
    paths = proofs.write()[:1]
    with pytest.raises(validator.ProofFragmentError) as rejected:
        proofs.load(paths)
    assert rejected.value.code == "lane_coverage_mismatch"


@pytest.mark.parametrize(
    ("hostile", "code"),
    [
        ("formatting", "noncanonical_fragment"),
        ("duplicate-key", "duplicate_json_key"),
        ("relative", "invalid_fragment_path"),
        ("parent-traversal", "invalid_fragment_path"),
        ("symlink", "fragment_not_regular"),
        ("symlink-parent", "fragment_not_regular"),
        ("source-drift", "source_digest_mismatch"),
    ],
)
def test_file_admission_fails_closed(proofs: Proofs, hostile: str, code: str) -> None:
    paths = proofs.write()
    if hostile == "formatting":
        paths[0].write_text(json.dumps(proofs.values[0], indent=2) + "\n")
    elif hostile == "duplicate-key":
        paths[0].write_text('{"format":"x","format":"y"}\n')
    elif hostile == "relative":
        paths[0] = Path("relative-fragment.json")
    elif hostile == "parent-traversal":
        paths[0] = proofs.root / "nested/../fragment-0.json"
    elif hostile == "symlink":
        link = proofs.root / "fragment-link.json"
        link.symlink_to(paths[0])
        paths[0] = link
    elif hostile == "symlink-parent":
        real = proofs.root / "real-fragments"
        real.mkdir()
        (real / "fragment.json").write_bytes(paths[0].read_bytes())
        link = proofs.root / "linked-fragments"
        link.symlink_to(real, target_is_directory=True)
        paths[0] = link / "fragment.json"
    elif hostile == "source-drift":
        proofs.sources[0].write_text("# changed deterministic source\n")
    with pytest.raises(validator.ProofFragmentError) as rejected:
        proofs.load(paths)
    assert rejected.value.code == code


def test_source_identity_rejects_a_symlinked_parent(tmp_path: Path) -> None:
    real = tmp_path / "real"
    real.mkdir()
    (real / "producer.py").write_text("# source\n")
    link = tmp_path / "linked"
    link.symlink_to(real, target_is_directory=True)
    with pytest.raises(validator.ProofFragmentError) as rejected:
        validator.source_identity(tmp_path, "linked/producer.py")
    assert rejected.value.code == "source_not_regular"
