"""Guard the pre-stable V2 authoring promise against stale deferral text."""

import base64
import json
import re
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

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


def _fenced_example(after: str, language: str) -> str:
    text = (ROOT / "docs/guide/typed-queries.md").read_text(encoding="utf-8")
    tail = text.split(after, 1)[1]
    body = tail.split(f"```{language}\n", 1)[1]
    return body.split("\n```", 1)[0] + "\n"


def _marked_source(path: Path, start: str, end: str) -> str:
    text = path.read_text(encoding="utf-8")
    body = text.split(start, 1)[1].split(end, 1)[0]
    return textwrap.dedent(body).strip("\n") + "\n"


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


def test_typed_query_guide_keeps_match_and_v2_error_taxonomies_distinct() -> None:
    text = (ROOT / "docs/guide/typed-queries.md").read_text(encoding="utf-8")
    assert "Python `MatchRequestError` or Node `TypedMatchError`" in text
    assert "`invalid_plan`, `cardinality`, `unsupported_capability`, `stale_schema`," in text
    assert "Low-level plan authoring and the V2 remote envelope use `QueryV2Error`" in text
    assert "Import it from `type_bridge_core` in Python" in text
    assert "`@type-bridge/node` package root in Node" in text
    assert "Its categories are `invalid_contract`, `unsupported_capability`," in text
    assert "they do not use the model-query `invalid_plan` taxonomy" in text


def test_documented_python_low_level_example_typechecks_and_executes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    snippet = _fenced_example(
        "This Python example authors a typed-input row plan:",
        "python",
    )
    corpus = json.loads(
        (ROOT / "tests/fixtures/query-v2-remote-failures.json").read_text(encoding="utf-8")
    )
    (tmp_path / "declared-schema.json").write_bytes(base64.b64decode(corpus["declared_b64"]))
    example = tmp_path / "typed_query_authoring_example.py"
    example.write_text(snippet, encoding="utf-8")

    pyright = shutil.which("pyright")
    assert pyright is not None, "the docs example gate requires the project Pyright tool"
    checked = subprocess.run(
        [
            pyright,
            "--pythonpath",
            sys.executable,
            "--outputjson",
            str(example),
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert checked.returncode == 0, checked.stdout + checked.stderr

    namespace: dict[str, object] = {}
    monkeypatch.chdir(tmp_path)
    exec(compile(snippet, str(example), "exec"), namespace)
    plan = namespace["plan"]
    invocation = namespace["invocation"]
    assert getattr(plan, "format") == "typebridge.query-plan/v2"
    assert getattr(invocation, "plan_fingerprint") == getattr(plan, "fingerprint")


def test_documented_remote_examples_are_exact_live_parity_regions() -> None:
    documented_python = _fenced_example(
        "This Python fragment is extracted verbatim from the live local/remote parity",
        "python",
    )
    live_python = _marked_source(
        ROOT / "tests/integration/queries/test_remote_query_session_parity.py",
        "# docs: remote-query-python:start",
        "# docs: remote-query-python:end",
    )
    assert documented_python == live_python

    documented_node = _fenced_example(
        "The equivalent fragment is extracted verbatim from the Node live parity test.",
        "typescript",
    )
    live_node = _marked_source(
        ROOT
        / "type-bridge-core/crates/node/tests/integration/queries"
        / "typed-remote-query-parity.test.ts",
        "// docs: remote-query-node:start",
        "// docs: remote-query-node:end",
    )
    assert documented_node == live_node
