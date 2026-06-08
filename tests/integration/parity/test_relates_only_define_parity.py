from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest

from type_bridge import Entity, Relation, Role, TypeFlags
from type_bridge._rust_runtime import generate_define_block
from type_bridge.migration.info import SchemaInfo

REPO_ROOT = Path(__file__).resolve().parents[3]
NODE_DIR = REPO_ROOT / "type-bridge-core" / "crates" / "node"
NODE_ENTRY = NODE_DIR / "dist" / "index.js"

EXPECTED_RELATES_PLUS_BOUND = (
    "define\n\n"
    "entity parity-relates-only-person;\n\n"
    "relation parity-relates-only-rel,\n"
    "    relates definition,\n"
    "    relates actor;\n\n"
    "parity-relates-only-person plays parity-relates-only-rel:actor;"
)


class ParityRelatesOnlyPerson(Entity):
    flags = TypeFlags(name="parity-relates-only-person")


class ParityRelatesOnlyRelation(Relation):
    flags = TypeFlags(name="parity-relates-only-rel")

    definition: Role
    actor: Role[ParityRelatesOnlyPerson] = Role("actor", ParityRelatesOnlyPerson)


def _rust_schema_info() -> dict[str, Any]:
    return {
        "entities": {
            "parity-relates-only-person": {
                "type_name": "parity-relates-only-person",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [],
                "plays_cardinalities": {},
            }
        },
        "relations": {
            "parity-relates-only-rel": {
                "type_name": "parity-relates-only-rel",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [],
                "roles": [
                    {
                        "role_name": "definition",
                        "player_type_names": [],
                        "cardinality": None,
                    },
                    {
                        "role_name": "actor",
                        "player_type_names": ["parity-relates-only-person"],
                        "cardinality": None,
                    },
                ],
                "plays_cardinalities": {},
            }
        },
        "attributes": {},
    }


def _python_typeql() -> str:
    schema = SchemaInfo()
    schema.entities.append(ParityRelatesOnlyPerson)
    schema.relations.append(ParityRelatesOnlyRelation)
    return schema.to_typeql()


def _node_env() -> dict[str, str]:
    env = dict(os.environ)
    candidates = list(NODE_DIR.glob("type_bridge_node*.node"))
    if candidates:
        env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(candidates[0])
    return env


def _typescript_typeql() -> str:
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if not NODE_ENTRY.exists():
        pytest.skip(f"compiled Node package not built ({NODE_ENTRY})")
    if not list(NODE_DIR.glob("type_bridge_node*.node")):
        pytest.skip("native node module not built; run `npm run build:native`")

    script = """
const tb = require("./dist/index.js");
class Person extends tb.Entity("parity-relates-only-person", {}) {}
class Rel extends tb.Relation("parity-relates-only-rel", {
  definition: tb.role(),
  actor: tb.role(Person),
}) {}
const registry = new tb.DescriptorRegistry();
registry.registerEntity(Person.descriptor());
registry.registerRelation(Rel.descriptor());
process.stdout.write(tb.generateDefineBlock(registry.schemaInfo()));
"""
    completed = subprocess.run(
        ["node", "-e", script],
        cwd=NODE_DIR,
        env=_node_env(),
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout


def test_rust_python_and_typescript_relates_only_define_blocks_match() -> None:
    core = pytest.importorskip("type_bridge_core")
    if not hasattr(core, "generate_define_block"):
        pytest.skip("type_bridge_core extension does not expose generate_define_block")

    rust = generate_define_block(_rust_schema_info())
    python = _python_typeql()
    typescript = _typescript_typeql()

    assert rust == EXPECTED_RELATES_PLUS_BOUND
    assert python == rust
    assert typescript == rust


@pytest.mark.integration
def test_live_typedb_accepts_relates_only_roles_with_zero_plays(clean_db: Any) -> None:
    core = pytest.importorskip("type_bridge_core")
    if not hasattr(core, "generate_define_block"):
        pytest.skip("type_bridge_core extension does not expose generate_define_block")
    typeql = generate_define_block(
        {
            "entities": {},
            "relations": {
                "live-relates-only-rel": {
                    "type_name": "live-relates-only-rel",
                    "is_abstract": False,
                    "parent_type": None,
                    "owned_attributes": [],
                    "roles": [
                        {
                            "role_name": "definition",
                            "player_type_names": [],
                            "cardinality": None,
                        },
                        {
                            "role_name": "allowed_value",
                            "player_type_names": [],
                            "cardinality": None,
                        },
                    ],
                    "plays_cardinalities": {},
                }
            },
            "attributes": {},
        }
    )
    assert "plays live-relates-only-rel" not in typeql

    clean_db.execute_query(typeql, transaction_type="schema")

    plays = clean_db.execute_query("match $t plays $r; select $t, $r;", transaction_type="read")
    relates = clean_db.execute_query(
        "match $rel relates $role; select $rel, $role;",
        transaction_type="read",
    )

    assert plays == []
    assert len(relates) == 2
