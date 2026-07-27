"""Live TypeDB smoke for the Phase 3 generated Python runtime projection."""

from __future__ import annotations

import importlib
import json
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

from type_bridge import Database

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
ACCEPTANCE_SCHEMA = CORE / "crates/schema-codegen/tests/acceptance/schema.yaml"
PROVIDER_SCHEMA = CORE / "crates/schema-codegen/tests/acceptance/provider-3.12.1.tql"


@pytest.fixture
def generated_package(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[ModuleType]:
    """Emit and import the real schema-codegen Python acceptance package."""
    stage = tmp_path / "generated-projection"
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-schema-codegen",
            "--example",
            "emit_python_acceptance",
            "--",
            str(ACCEPTANCE_SCHEMA),
            str(stage / "generated_v2"),
        ],
        cwd=ROOT,
        check=True,
    )
    monkeypatch.syspath_prepend(str(stage))
    importlib.invalidate_caches()
    generated = importlib.import_module("generated_v2")
    try:
        yield generated
    finally:
        for module_name in tuple(sys.modules):
            if module_name == "generated_v2" or module_name.startswith("generated_v2."):
                del sys.modules[module_name]


def test_generated_projection_round_trips_live_models(
    clean_db: Database,
    generated_package: ModuleType,
) -> None:
    generated = generated_package
    server_version = clean_db.detected_server_version()
    if server_version is not None:
        major, minor = (int(part) for part in server_version.split(".")[:2])
        if (major, minor) < (3, 12):
            pytest.skip("the generated projection fixture uses TypeDB 3.12+ annotation placements")
    clean_db.execute_query(PROVIDER_SCHEMA.read_text(encoding="utf-8"), transaction_type="schema")

    runtime_projection = json.loads(generated.RUNTIME_PROJECTION_JSON)
    assert runtime_projection["semantic_fingerprint"] == json.loads(
        generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON
    )
    assert runtime_projection["projection_fingerprint"] == json.loads(
        generated.PROJECTION_FINGERPRINT_JSON
    )

    installed = generated.Person.__runtime_projection__
    assert installed is not None
    for model in (
        generated.Aliases,
        generated.Container,
        generated.Employment,
        generated.Event,
        generated.Identifier,
        generated.Membership,
        generated.Nickname,
        generated.Person,
        generated.Robot,
    ):
        assert model.__runtime_projection__ is installed

    person_manager = generated.Person.manager(clean_db)
    person = generated.Person(
        identifier=generated.Identifier("person-1"),
        nickname=generated.Nickname("alice"),
        aliases=[generated.Aliases("alpha"), generated.Aliases("beta")],
    )
    assert person_manager.insert(person) is person
    assert person.iid

    stored_person = person_manager.get_by_iid(person.iid)
    assert type(stored_person) is generated.Person
    assert stored_person.iid == person.iid
    assert type(stored_person.identifier) is generated.Identifier
    assert stored_person.identifier.value == "person-1"
    assert type(stored_person.nickname) is generated.Nickname
    assert stored_person.nickname.value == "alice"
    assert type(stored_person.aliases) is tuple
    assert {alias.value for alias in stored_person.aliases} == {"alpha", "beta"}
    assert all(type(alias) is generated.Aliases for alias in stored_person.aliases)

    membership_manager = generated.Membership.manager(clean_db)
    membership = generated.Membership(member=person)
    assert membership_manager.insert(membership) is membership
    assert membership.iid

    stored_membership = membership_manager.get_by_iid(membership.iid)
    assert type(stored_membership) is generated.Membership
    assert stored_membership.iid == membership.iid
    assert type(stored_membership.member) is generated.Person
    assert stored_membership.member.iid == person.iid

    employment_manager = generated.Employment.manager(clean_db)
    employment = generated.Employment(employee=person)
    assert employment_manager.insert(employment) is employment
    assert employment.iid

    stored_employment = employment_manager.get_by_iid(employment.iid)
    assert type(stored_employment) is generated.Employment
    assert stored_employment.iid == employment.iid
    assert type(stored_employment.employee) is generated.Person
    assert stored_employment.employee.iid == person.iid
    assert not hasattr(stored_employment, "member")

    event_manager = generated.Event.manager(clean_db)
    event = generated.Event(subject=person)
    assert event_manager.insert(event) is event
    assert event.iid
    assert type(event_manager.get_by_iid(event.iid)) is generated.Event

    container_manager = generated.Container.manager(clean_db)
    container = generated.Container(item=[generated.EventRef(event.iid)])
    assert container_manager.insert(container) is container
    assert container.iid

    stored_container = container_manager.get_by_iid(container.iid)
    assert type(stored_container) is generated.Container
    assert stored_container.iid == container.iid
    assert type(stored_container.item) is tuple
    assert len(stored_container.item) == 1
    stored_event = stored_container.item[0]
    assert type(stored_event) is generated.EventRef
    assert not isinstance(stored_event, generated.Event)
    assert stored_event.iid == event.iid
