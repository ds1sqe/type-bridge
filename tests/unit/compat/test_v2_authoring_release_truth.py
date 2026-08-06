"""Guard retained low-level and generated Query V2 documentation truth."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
DURABLE_PLAN_ROOT = ROOT.parents[2] / "docs/plans/type-bridge/#170-rust-ssot-schema-query-v2"
REPOSITORY_AUTHORING_TRUTH_FILES = (
    ROOT / "docs/guide/upgrade-v2.md",
    ROOT / "docs/guide/v2-deprecations.md",
    ROOT / "docs/guide/typed-queries.md",
    ROOT / "type-bridge-core/crates/python/src/query_v2_runtime.rs",
    ROOT / "type-bridge-core/crates/node/src/query_v2_runtime.rs",
    ROOT / "type-bridge-core/crates/node/README.md",
)
DURABLE_AUTHORING_TRUTH_FILES = tuple(
    path
    for path in (
        DURABLE_PLAN_ROOT / "02-query-ssot.md",
        DURABLE_PLAN_ROOT / "appendix-A-architecture-overview.md",
    )
    if path.is_file()
)
AUTHORING_TRUTH_FILES = (
    *REPOSITORY_AUTHORING_TRUTH_FILES,
    *DURABLE_AUTHORING_TRUTH_FILES,
)
AUTHORING_SMOKE_FILES = (
    ROOT / "tests/integration/queries/test_query_v2_binding_smoke.py",
    ROOT / "type-bridge-core/crates/node/tests/integration/queries/query-v2-smoke.test.ts",
)
FORBIDDEN_DEFERRAL_TEXT = (
    "plans are authored in rust in 2.0.0",
    "plan authoring is a rust surface in 2.0.0",
    "binding authoring facade over this surface is tracked in issue #195",
    "binding authoring remains deferred",
    "binding authoring will ship after 2.0",
    "do not yet offer idiomatic plan-builder facades",
    "ship in a later `2.0.x` release",
    "python/node ergonomic typed authoring remains the separate post-2.0 plan 08",
    "python/node ergonomic typed plan authoring is the separate post-2.0 plan 08",
    "do not yet expose the ergonomic typed builder facades tracked separately by plan 08",
    "python/node typed authoring remains plan 08",
    "does not yet provide a plan-builder facade",
)


def test_v2_authoring_sources_do_not_restore_historical_deferral_claims() -> None:
    for path in AUTHORING_TRUTH_FILES:
        text = " ".join(path.read_text(encoding="utf-8").lower().split())
        for forbidden in FORBIDDEN_DEFERRAL_TEXT:
            assert forbidden not in text, f"{path} restores stale V2 guidance"


def test_binding_smokes_author_plans_instead_of_embedding_plan_bytes() -> None:
    for path in AUTHORING_SMOKE_FILES:
        text = path.read_text(encoding="utf-8")
        assert "QueryPlanBuilder" in text, (
            f"{path.relative_to(ROOT)} no longer authors its plan through the public facade"
        )
        for forbidden in (
            "PLAN_B64",
            "plan_b64",
            "eyJiaW5kaW5ncyI",
            "base64.b64decode(PLAN",
            "Buffer.from(PLAN",
        ):
            assert forbidden not in text, (
                f"{path.relative_to(ROOT)} restored an embedded query-plan fixture"
            )


def test_binding_smokes_pin_one_advanced_cross_language_authored_identity() -> None:
    identities: list[tuple[str, ...]] = []
    for path in AUTHORING_SMOKE_FILES:
        text = path.read_text(encoding="utf-8")
        for operation in (".input(", ".select(", ".require(", ".distinct(", ".sort("):
            assert operation in text, (
                f"{path.relative_to(ROOT)} lost advanced authored operation {operation}"
            )
        identities.append(tuple(re.findall(r'"([0-9a-f]{64})"', text)))
    assert identities[0] == identities[1]
    assert len(identities[0]) == 2


def test_typed_query_guide_is_projection_scoped_and_names_remote_failure() -> None:
    text = (ROOT / "docs/guide/typed-queries.md").read_text(encoding="utf-8")
    normalized = " ".join(text.split())
    assert text.startswith("# Immutable generated queries")
    assert "verified projection" in normalized
    assert "Only exact registered generated classes and tokens are accepted." in text
    assert "query_remote_v2_native_only_operation" in text
    assert "TypeDBType" not in text
    assert "type_bridge.typed" not in text


def test_documented_python_generated_query_surfaces_have_acceptance_evidence() -> None:
    guide = (ROOT / "docs/guide/typed-queries.md").read_text(encoding="utf-8")
    acceptance = (
        ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/positive.py"
    ).read_text(encoding="utf-8")

    for documented, accepted in (
        ("Person.query(", "Person.query("),
        (".exact(", ".exact("),
        ("subtypes(Model)", ".subtypes("),
        (".reachable(", ".reachable("),
        (".query_as(", ".query_as("),
        (".page_by(", ".page_by("),
        (".count_by(", ".count_by("),
        (".exists_by(", ".exists_by("),
        (".aggregate(", ".aggregate("),
        (".group_by(", ".group_by("),
    ):
        assert documented in guide, f"generated query guide lost {documented}"
        assert accepted in acceptance, f"generated acceptance lost {accepted}"


def test_documented_remote_contract_has_runtime_acceptance() -> None:
    guide = (ROOT / "docs/guide/typed-queries.md").read_text(encoding="utf-8")
    runtime = (
        ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/runtime_check.py"
    ).read_text(encoding="utf-8")
    live = (ROOT / "tests/integration/schema/test_generated_projection_live.py").read_text(
        encoding="utf-8"
    )

    assert "RemoteQuerySession" in guide
    assert "RemoteQuerySession" in runtime
    for terminal in ("rows", "page_by", "count_by", "exists_by"):
        assert f"`{terminal}`" in guide
        assert f".{terminal}(" in runtime
    assert "query_remote_v2_native_only_operation" in guide
    assert "query_remote_v2_native_only_operation" in live
