from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest

from type_bridge import Card, Entity, Relation, Role, TypeFlags
from type_bridge._rust_runtime import generate_define_block
from type_bridge.generator.models import Cardinality
from type_bridge.generator.parser import parse_tql_schema
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


class ParityPlaysCardPerson(Entity):
    flags = TypeFlags(name="parity-plays-card-person")


class ParityPlaysCardCompany(Entity):
    flags = TypeFlags(name="parity-plays-card-company")


class ParityPlaysCardEmployment(Relation):
    flags = TypeFlags(name="parity-plays-card-employment")

    employee: Role[ParityPlaysCardPerson] = Role("employee", ParityPlaysCardPerson)
    employer: Role[ParityPlaysCardCompany] = Role(
        "employer",
        ParityPlaysCardCompany,
        cardinality=Card(1, 1),
        plays_cardinality=Card(0, 1),
    )


class ParitySurfacePerson(Entity):
    flags = TypeFlags(name="parity-surface-person")


class ParitySurfaceCompany(Entity):
    flags = TypeFlags(name="parity-surface-company")


class ParitySurfaceContractor(Entity):
    flags = TypeFlags(name="parity-surface-contractor")


class ParitySurfaceContract(Relation):
    flags = TypeFlags(name="parity-surface-contract")

    signer: Role[ParitySurfacePerson] = Role("signer", ParitySurfacePerson)


class ParitySurfaceEmployment(Relation):
    flags = TypeFlags(name="parity-surface-employment")

    definition: Role
    employee: Role[ParitySurfacePerson] = Role("employee", ParitySurfacePerson)
    employer: Role[ParitySurfaceCompany] = Role(
        "employer",
        ParitySurfaceCompany,
        cardinality=Card(1, 1),
        plays_cardinality=Card(0, 1),
    )
    contributor: Role[ParitySurfacePerson | ParitySurfaceContractor] = Role.multi(
        "contributor",
        ParitySurfacePerson,
        ParitySurfaceContractor,
        plays_cardinality=Card(0, 5),
    )
    contract: Role[ParitySurfaceContract] = Role(
        "contract",
        ParitySurfaceContract,
        plays_cardinality=Card(1, 1),
    )


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


def _python_plays_card_typeql() -> str:
    schema = SchemaInfo()
    schema.entities.extend([ParityPlaysCardPerson, ParityPlaysCardCompany])
    schema.relations.append(ParityPlaysCardEmployment)
    return schema.to_typeql()


def _python_final_surface_typeql() -> str:
    schema = SchemaInfo()
    schema.entities.extend([ParitySurfacePerson, ParitySurfaceCompany, ParitySurfaceContractor])
    schema.relations.extend([ParitySurfaceContract, ParitySurfaceEmployment])
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


def _typescript_plays_card_typeql() -> str:
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if not NODE_ENTRY.exists():
        pytest.skip(f"compiled Node package not built ({NODE_ENTRY})")
    if not list(NODE_DIR.glob("type_bridge_node*.node")):
        pytest.skip("native node module not built; run `npm run build:native`")

    script = """
const tb = require("./dist/index.js");
class Person extends tb.Entity("parity-plays-card-person", {}) {}
class Company extends tb.Entity("parity-plays-card-company", {}) {}
class Employment extends tb.Relation("parity-plays-card-employment", {
  employee: tb.role(Person),
  employer: tb.role(Company, {
    cardinality: tb.Card(1, 1),
    playsCardinality: tb.Card(0, 1),
  }),
}) {}
const registry = new tb.DescriptorRegistry();
registry.registerEntity(Person.descriptor());
registry.registerEntity(Company.descriptor());
registry.registerRelation(Employment.descriptor());
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


def _typescript_final_surface_typeql() -> str:
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if not NODE_ENTRY.exists():
        pytest.skip(f"compiled Node package not built ({NODE_ENTRY})")
    if not list(NODE_DIR.glob("type_bridge_node*.node")):
        pytest.skip("native node module not built; run `npm run build:native`")

    script = """
const tb = require("./dist/index.js");
class Person extends tb.Entity("parity-surface-person", {}) {}
class Company extends tb.Entity("parity-surface-company", {}) {}
class Contractor extends tb.Entity("parity-surface-contractor", {}) {}
class Contract extends tb.Relation("parity-surface-contract", {
  signer: tb.role(Person),
}) {}
class Employment extends tb.Relation("parity-surface-employment", {
  definition: tb.role(),
  employee: tb.role(Person),
  employer: tb.role(Company, {
    cardinality: tb.Card(1, 1),
    playsCardinality: tb.Card(0, 1),
  }),
  contributor: tb.role(Person, Contractor, {
    playsCardinality: tb.Card(0, 5),
  }),
  contract: tb.role(Contract, {
    playsCardinality: tb.Card(1, 1),
  }),
}) {}
const registry = new tb.DescriptorRegistry();
registry.registerEntity(Person.descriptor());
registry.registerEntity(Company.descriptor());
registry.registerEntity(Contractor.descriptor());
registry.registerRelation(Contract.descriptor());
registry.registerRelation(Employment.descriptor());
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


def _final_surface_toml() -> str:
    return """
[entities.parity-surface-person]
plays = [
    { relation = "parity-surface-contract", role = "signer" },
    { relation = "parity-surface-employment", role = "employee" },
    { relation = "parity-surface-employment", role = "contributor", card = "0..5" },
]

[entities.parity-surface-company]
plays = [
    { relation = "parity-surface-employment", role = "employer", card = "0..1" },
]

[entities.parity-surface-contractor]
plays = [
    { relation = "parity-surface-employment", role = "contributor", card = "0..5" },
]

[relations.parity-surface-contract]
roles = [{ name = "signer" }]
plays = [
    { relation = "parity-surface-employment", role = "contract", card = "1" },
]

[relations.parity-surface-employment]
roles = [
    { name = "definition" },
    { name = "employee" },
    { name = "employer", card = "1..1" },
    { name = "contributor" },
    { name = "contract" },
]
"""


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


def test_python_and_typescript_plays_card_define_blocks_match() -> None:
    core = pytest.importorskip("type_bridge_core")
    if not hasattr(core, "generate_define_block"):
        pytest.skip("type_bridge_core extension does not expose generate_define_block")

    python = _python_plays_card_typeql()
    typescript = _typescript_plays_card_typeql()

    assert python == typescript
    assert (
        "parity-plays-card-company plays parity-plays-card-employment:employer @card(0..1);"
        in python
    )
    assert "relates employer @card(1..1)" in python
    assert "parity-plays-card-person plays parity-plays-card-employment:employee;" in python


def test_python_and_typescript_final_surface_define_blocks_match() -> None:
    core = pytest.importorskip("type_bridge_core")
    if not hasattr(core, "generate_define_block"):
        pytest.skip("type_bridge_core extension does not expose generate_define_block")

    python = _python_final_surface_typeql()
    typescript = _typescript_final_surface_typeql()

    assert python == typescript
    assert "relates definition" in python
    assert "parity-surface-employment:definition" not in python
    assert "relates employer @card(1..1)" in python
    assert "parity-surface-person plays parity-surface-employment:employee;" in python
    assert "parity-surface-company plays parity-surface-employment:employer @card(0..1);" in python
    assert (
        "parity-surface-person plays parity-surface-employment:contributor @card(0..5);" in python
    )
    assert (
        "parity-surface-contractor plays parity-surface-employment:contributor @card(0..5);"
        in python
    )
    assert "parity-surface-contract plays parity-surface-employment:contract @card(1..1);" in python


def test_toml_final_surface_matches_core_semantics() -> None:
    core = pytest.importorskip("type_bridge_core")
    TypeSchema = core.TypeSchema
    toml_to_typeql = core.toml_to_typeql

    typeql = toml_to_typeql(_final_surface_toml())
    parsed = parse_tql_schema(typeql)

    person = parsed.entities["parity-surface-person"]
    company = parsed.entities["parity-surface-company"]
    contractor = parsed.entities["parity-surface-contractor"]
    relation_roles = {
        role.name: role for role in parsed.relations["parity-surface-employment"].roles
    }

    assert "parity-surface-employment:employee" not in person.plays_cardinalities
    assert person.plays_cardinalities["parity-surface-employment:contributor"] == Cardinality(0, 5)
    assert company.plays_cardinalities["parity-surface-employment:employer"] == Cardinality(0, 1)
    assert contractor.plays_cardinalities["parity-surface-employment:contributor"] == Cardinality(
        0, 5
    )
    assert relation_roles["definition"].cardinality is None
    assert relation_roles["employer"].cardinality == Cardinality(1, 1)

    schema = TypeSchema.from_typeql(typeql)
    contract_plays = {
        entry["role_ref"]: entry["cardinality"]
        for entry in schema.get_all_plays_roles("parity-surface-contract")
    }
    assert contract_plays["parity-surface-employment:contract"] == {
        "min": 1,
        "max": 1,
    }


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


@pytest.mark.integration
def test_live_typedb_accepts_final_plays_cardinality_surface(clean_db: Any) -> None:
    core = pytest.importorskip("type_bridge_core")
    if not hasattr(core, "generate_define_block"):
        pytest.skip("type_bridge_core extension does not expose generate_define_block")

    typeql = _python_final_surface_typeql()

    clean_db.execute_query(typeql, transaction_type="schema")
