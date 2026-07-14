"""Generated Python models expose owner-aware references without runtime drift."""

from __future__ import annotations

import importlib
import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

REPO_ROOT = Path(__file__).resolve().parents[3]
CORE_MANIFEST = REPO_ROOT / "type-bridge-core" / "Cargo.toml"
NEGATIVE_MARKER = "# generated-ref-error:"
SCHEMA = """\
define
attribute owner-name, value string;
attribute owner-age, value integer;
attribute owner-active, value boolean;
attribute owner-tag, value string;
attribute owner-code, value string;
attribute owner-email, value string;
attribute owner-handle, value string;
attribute owner-alias, value string;
attribute owner-ordered, value string;
attribute owner-bounded, value string;
entity owner-party @abstract,
    owns owner-name @key @doc("Generated owner name") @meta("source", "typed-probe");
entity owner-person sub owner-party,
    owns owner-age,
    owns owner-active,
    owns owner-tag @card(0..),
    plays owner-employment:employee,
    plays owner-employment:actor;
entity owner-company, owns owner-name @key;
entity owner-bot, owns owner-name @key, plays owner-employment:actor;
entity owner-unique-probe,
    owns owner-email @unique,
    owns owner-handle @unique @card(1..1),
    owns owner-alias @unique @card(0..3),
    owns owner-ordered[] @distinct,
    owns owner-bounded[] @unique @card(0..5) @distinct;
relation owner-employment,
    owns owner-code @key,
    relates employee,
    relates actor @card(0..);
"""

POSITIVE = """\
from typing import assert_type

from generated_owner_models.attributes import OwnerActive, OwnerAge, OwnerName, OwnerTag
from generated_owner_models.entities import OwnerBot, OwnerCompany, OwnerParty, OwnerPerson
from generated_owner_models.relations import OwnerEmployment
from type_bridge import Database
from type_bridge.fields import FieldRef, NumericFieldRef, StringFieldRef
from type_bridge.fields.role import RoleRef
from type_bridge.typed import BoundRole, Predicate, QuerySession

assert_type(OwnerParty.owner_name, StringFieldRef[OwnerName, OwnerParty])
assert_type(OwnerPerson.owner_name, StringFieldRef[OwnerName, OwnerPerson])
assert_type(OwnerPerson.owner_age, NumericFieldRef[OwnerAge, OwnerPerson])
assert_type(OwnerPerson.owner_active, FieldRef[OwnerActive, OwnerPerson])
assert_type(
    OwnerEmployment.employee,
    RoleRef[OwnerPerson, OwnerEmployment],
)
assert_type(
    OwnerEmployment.actor,
    RoleRef[OwnerBot | OwnerPerson, OwnerEmployment],
)

# Required inherited keys stay required, while optional/list defaults may be omitted.
person_value = OwnerPerson(owner_name=OwnerName("Alice"))
assert_type(person_value.owner_name, OwnerName)
assert_type(person_value.owner_age, OwnerAge | None)
assert_type(person_value.owner_active, OwnerActive | None)
assert_type(person_value.owner_tag, list[OwnerTag])

def check_owner_aware_references(database: Database) -> None:
    session = QuerySession(database)
    person = session.var(OwnerPerson)
    company = session.var(OwnerCompany)
    employment = session.var(OwnerEmployment)
    bot = session.var(OwnerBot)
    assert_type(
        person.field(OwnerPerson.owner_name).contains(OwnerName("li")),
        Predicate,
    )
    assert_type(
        person.field(OwnerParty.owner_name).contains(OwnerName("li")),
        Predicate,
    )
    assert_type(
        person.field(OwnerPerson.owner_age).gte(OwnerAge(18)),
        Predicate,
    )
    employee_role = employment.role(OwnerEmployment.employee)
    assert_type(employee_role, BoundRole[OwnerPerson])
    assert_type(employee_role.connects(person), Predicate)
    assert_type(employment.role(OwnerEmployment.actor).connects(bot), Predicate)
    del company
"""

NEGATIVE = """\
from generated_owner_models.entities import OwnerCompany, OwnerParty, OwnerPerson
from generated_owner_models.relations import OwnerEmployment
from type_bridge import Database
from type_bridge.typed import QuerySession

OwnerPerson()  # generated-ref-error: required-inherited-key

def reject_invalid_references(database: Database) -> None:
    session = QuerySession(database)
    party = session.var(OwnerParty)
    person = session.var(OwnerPerson)
    company = session.var(OwnerCompany)
    employment = session.var(OwnerEmployment)

    person.field(OwnerCompany.owner_name)  # generated-ref-error: cross-owner-field
    party.field(OwnerPerson.owner_age)  # generated-ref-error: subtype-field-on-base-binding
    person.field(OwnerPerson.owner_active).lt(  # generated-ref-error: invalid-boolean-operator
        OwnerPerson.owner_active
    )
    employment.role(OwnerEmployment.employee).connects(
        company  # generated-ref-error: wrong-role-player
    )
"""


def _render_package(destination: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo executable is required for Rust bindgen coverage")
    schema_path = destination.parent / "owner-aware.tql"
    schema_path.write_text(SCHEMA, encoding="utf-8")
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CORE_MANIFEST),
            "-p",
            "type-bridge-core-lib",
            "--example",
            "bindgen_render",
            "--",
            str(schema_path),
            "python",
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Rust bindgen render failed\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    payload: dict[str, Any] = json.loads(completed.stdout)
    destination.mkdir()
    for generated in payload["files"]:
        target = destination / str(generated["path"])
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(str(generated["contents"]), encoding="utf-8")


@pytest.fixture(scope="module")
def generated_package(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("owner-aware-bindgen")
    package = root / "generated_owner_models"
    _render_package(package)
    return package


def _pyright_payload(project: Path) -> tuple[int, dict[str, Any], str]:
    pyright = shutil.which("pyright")
    if pyright is None:
        pytest.skip("pyright executable is required for generated typing coverage")
    completed = subprocess.run(
        [pyright, "--outputjson", "--project", str(project), "--pythonpath", sys.executable],
        cwd=project.parent,
        check=False,
        capture_output=True,
        text=True,
    )
    return completed.returncode, json.loads(completed.stdout), completed.stderr


@pytest.mark.integration
def test_generated_package_pyright_contract(generated_package: Path) -> None:
    consumer_root = generated_package.parent
    generated_sources = "\n".join(
        path.read_text(encoding="utf-8") for path in generated_package.rglob("*.py")
    )
    relations_source = (generated_package / "relations.py").read_text(encoding="utf-8")
    assert "cast(" not in generated_sources
    assert "# type: ignore" not in generated_sources
    assert "pyright: ignore" not in generated_sources
    assert "employee: Role[entities.OwnerPerson]" in relations_source
    assert "actor: Role[entities.OwnerBot | entities.OwnerPerson]" in relations_source

    positive = consumer_root / "positive.py"
    negative = consumer_root / "negative.py"
    positive.write_text(POSITIVE, encoding="utf-8")
    negative.write_text(NEGATIVE, encoding="utf-8")

    for label, consumer in (("positive", positive), ("negative", negative)):
        config = consumer_root / f"pyrightconfig.{label}.json"
        config.write_text(
            json.dumps(
                {
                    "include": [consumer.name],
                    "pythonVersion": "3.12",
                    "typeCheckingMode": "standard",
                    "reportMissingModuleSource": "none",
                    "executionEnvironments": [
                        {
                            "root": str(consumer_root),
                            "extraPaths": [str(REPO_ROOT)],
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        returncode, payload, stderr = _pyright_payload(config)
        errors = [
            diagnostic
            for diagnostic in payload.get("generalDiagnostics", [])
            if diagnostic.get("severity") == "error"
        ]
        if label == "positive":
            assert returncode == 0 and not errors, (
                f"positive generated consumer failed:\n{json.dumps(payload, indent=2)}\n{stderr}"
            )
            continue

        expected_lines = {
            line_number
            for line_number, line in enumerate(negative.read_text(encoding="utf-8").splitlines(), 1)
            if NEGATIVE_MARKER in line
        }
        actual_lines = {
            int(diagnostic["range"]["start"]["line"]) + 1
            for diagnostic in errors
            if Path(str(diagnostic.get("file", ""))).resolve() == negative.resolve()
        }
        foreign = [
            diagnostic
            for diagnostic in errors
            if Path(str(diagnostic.get("file", ""))).resolve() != negative.resolve()
        ]
        assert returncode != 0 and actual_lines == expected_lines and not foreign, (
            "negative generated consumer drifted:\n"
            f"expected={sorted(expected_lines)} actual={sorted(actual_lines)}\n"
            f"foreign={foreign}\n{json.dumps(payload, indent=2)}\n{stderr}"
        )


@pytest.mark.integration
def test_generated_package_preserves_runtime_field_behavior(generated_package: Path) -> None:
    import_root = str(generated_package.parent)
    sys.path.insert(0, import_root)
    try:
        attributes = importlib.import_module("generated_owner_models.attributes")
        entities = importlib.import_module("generated_owner_models.entities")

        person_class = entities.OwnerPerson
        assert person_class.model_fields["owner_name"].is_required()
        with pytest.raises(ValidationError):
            person_class()

        person = person_class(owner_name=attributes.OwnerName("Alice"))
        assert person.owner_name == attributes.OwnerName("Alice")
        assert person.owner_age is None
        assert person.owner_active is None
        assert person.owner_tag == []

        from type_bridge.fields import StringFieldRef

        inherited_reference = person_class.owner_name
        assert isinstance(inherited_reference, StringFieldRef)
        assert inherited_reference.entity_type is person_class

        from type_bridge._rust_runtime import descriptor_for_model

        unique_probe = entities.OwnerUniqueProbe
        owned = {
            attribute["field_name"]: attribute
            for attribute in descriptor_for_model(unique_probe)["owned_attributes"]
        }
        assert owned["owner_email"]["annotations"] == [
            "Unique",
            {"Card": [0, 1]},
        ]
        assert owned["owner_handle"]["annotations"] == [
            "Unique",
            {"Card": [1, 1]},
        ]
        assert owned["owner_alias"]["annotations"] == [
            "Unique",
            {"Card": [0, 3]},
        ]
        assert owned["owner_ordered"] == {
            "field_name": "owner_ordered",
            "attr_name": "owner-ordered",
            "value_type": "string",
            "annotations": ["Distinct"],
            "is_optional": True,
            "is_ordered": True,
        }
        assert owned["owner_bounded"] == {
            "field_name": "owner_bounded",
            "attr_name": "owner-bounded",
            "value_type": "string",
            "annotations": ["Unique", "Distinct", {"Card": [0, 5]}],
            "is_optional": True,
            "is_ordered": True,
        }

        unique_value = unique_probe(owner_handle=attributes.OwnerHandle("handle"))
        assert unique_value.owner_ordered == []
        assert unique_value.owner_bounded == []

        typeql = unique_probe.to_schema_definition()
        assert "owns owner-email @unique @card(0..1)" in typeql
        assert "owns owner-handle @unique @card(1..1)" in typeql
        assert "owns owner-alias @unique @card(0..3)" in typeql
        assert "owns owner-ordered[] @distinct" in typeql
        assert "owns owner-bounded[] @unique @distinct @card(0..5)" in typeql
    finally:
        sys.path.remove(import_root)
        for module_name in [
            name
            for name in sys.modules
            if name == "generated_owner_models" or name.startswith("generated_owner_models.")
        ]:
            del sys.modules[module_name]
