"""Cross-language generator parity.

The Node typed-model generator must be EQUIVALENT to the Python generator: run
both on the same TypeDB schema and assert the descriptors they emit are
byte-identical after the single shared canonicalizer. This is the correctness
oracle for the Node generator — NOT the hand-authored ``descriptors.json``,
whose human field names (``id``, ``tags``) match no mechanical generator.

The Node side is exercised end to end: the generator writes ``.ts`` source, it
is compiled by ``tsc``, and the compiled classes' ``descriptor()`` output is
read back. The test skips cleanly when the Node toolchain or build artifacts are
absent; run ``npm run build:typed-integration`` in the node crate to prepare them.
"""

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import pytest

from type_bridge import Entity, Relation
from type_bridge._rust_runtime import descriptor_for_model
from type_bridge.generator import generate_models

from .canonical import normalize_descriptor_snapshot

REPO_ROOT = Path(__file__).resolve().parents[3]
SCHEMA = REPO_ROOT / "tests" / "integration" / "parity" / "fixtures" / "schema.tql"
# The generator's own fixtures broaden coverage beyond the parity corpus:
# multi-level inheritance, @regex/@unique attributes, more value types, and
# (in bookstore) functions — which the generator does not emit, exercising that
# functions never leak into entity/relation descriptors.
GENERATOR_FIXTURES = REPO_ROOT / "tests" / "integration" / "generator" / "fixtures"
SCHEMAS = [
    SCHEMA,
    GENERATOR_FIXTURES / "bookstore.tql",
    GENERATOR_FIXTURES / "social_media.tql",
    GENERATOR_FIXTURES / "role_cardinality.tql",
    GENERATOR_FIXTURES / "type_theoretic.tql",
]
NODE_DIR = REPO_ROOT / "type-bridge-core" / "crates" / "node"
DUMP_HELPER = NODE_DIR / "tests" / "parity" / "generator-descriptor-dump.cjs"
COMPILED_GENERATOR = (
    REPO_ROOT / "tmp" / "node-typed-integration" / "typescript" / "generator" / "index.js"
)
GENERATED_DIR = NODE_DIR / "tests" / "parity" / "generated"


def _native_env() -> dict[str, str]:
    import os

    env = dict(os.environ)
    candidates = list(NODE_DIR.glob("type_bridge_node*.node"))
    if candidates:
        env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(candidates[0])
    return env


def _require_node_toolchain() -> None:
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if shutil.which("npm") is None:
        pytest.skip("npm executable is not installed")
    if not COMPILED_GENERATOR.exists():
        pytest.skip(
            f"compiled generator not built ({COMPILED_GENERATOR}); "
            "run `npm run build:typed-integration` in the node crate first"
        )
    if not list(NODE_DIR.glob("type_bridge_node*.node")):
        pytest.skip("native node module not built; run `npm run build:native`")


def _python_snapshot(schema_text: str) -> dict[str, Any]:
    """Generate the Python surface and collect its model descriptors."""
    out = Path(tempfile.mkdtemp())
    generate_models(schema_text, str(out))
    sys.path.insert(0, str(out.parent))
    try:
        spec = importlib.util.spec_from_file_location(out.name, out / "__init__.py")
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules[out.name] = module
        spec.loader.exec_module(module)
        entities: list[dict[str, Any]] = []
        relations: list[dict[str, Any]] = []
        for submodule in (module.entities, module.relations):
            for name in dir(submodule):
                obj = getattr(submodule, name)
                if not isinstance(obj, type):
                    continue
                if not issubclass(obj, (Entity, Relation)) or obj in (Entity, Relation):
                    continue
                descriptor = descriptor_for_model(obj)
                (relations if "roles" in descriptor else entities).append(descriptor)
        return {"version": "1.0.0", "entities": entities, "relations": relations}
    finally:
        sys.path.remove(str(out.parent))
        sys.modules.pop(out.name, None)


def _typescript_snapshot(schema_path: Path) -> dict[str, Any]:
    """Generate the TS surface, compile it, and read its descriptors back."""
    env = _native_env()
    subprocess.run(
        ["node", str(DUMP_HELPER), "generate", str(schema_path), str(GENERATED_DIR)],
        cwd=NODE_DIR,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        ["npm", "run", "build:generator-parity"],
        cwd=NODE_DIR,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    dumped = subprocess.run(
        ["node", str(DUMP_HELPER), "dump"],
        cwd=NODE_DIR,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(dumped.stdout)


@pytest.mark.parametrize("schema_path", SCHEMAS, ids=lambda p: p.stem)
def test_typescript_generator_matches_python_generator(schema_path: Path) -> None:
    """The TS generator emits the same descriptors as the Python generator."""
    _require_node_toolchain()
    schema_text = schema_path.read_text(encoding="utf-8")

    python = normalize_descriptor_snapshot(_python_snapshot(schema_text))
    typescript = normalize_descriptor_snapshot(_typescript_snapshot(schema_path))

    # Compare per type so a mismatch names the offending entity/relation.
    py_entities = {e["type_name"]: e for e in python["entities"]}
    ts_entities = {e["type_name"]: e for e in typescript["entities"]}
    assert ts_entities.keys() == py_entities.keys()
    for name in py_entities:
        assert ts_entities[name] == py_entities[name], f"entity {name} descriptor differs"

    py_relations = {r["type_name"]: r for r in python["relations"]}
    ts_relations = {r["type_name"]: r for r in typescript["relations"]}
    assert ts_relations.keys() == py_relations.keys()
    for name in py_relations:
        assert ts_relations[name] == py_relations[name], f"relation {name} descriptor differs"
