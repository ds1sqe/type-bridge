"""Live TypeDB smoke for the Phase 3 generated Python runtime projection."""

from __future__ import annotations

import asyncio
import base64
import importlib
import json
import logging
import os
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from dataclasses import make_dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from types import ModuleType
from typing import Protocol
from urllib import request as urllib_request

import pytest

from type_bridge import Database

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
ACCEPTANCE_DIRECTORY = CORE / "crates/schema-codegen/tests/acceptance"
ACCEPTANCE_SCHEMA = ACCEPTANCE_DIRECTORY / "schema.yaml"
ACCEPTANCE_SCHEMA_3_11 = ACCEPTANCE_DIRECTORY / "schema-3.11.5.yaml"
PROVIDER_SCHEMA = ACCEPTANCE_DIRECTORY / "provider-3.12.1.tql"
PROVIDER_SCHEMA_3_11 = ACCEPTANCE_DIRECTORY / "provider-3.11.5.tql"


class _StringValue(Protocol):
    value: str


class _IntegerValue(Protocol):
    value: int


class _GeneratedPerson(Protocol):
    """Static test view of the emitted person used before its package exists."""

    iid: str | None
    identifier: _StringValue
    nickname: _StringValue | None
    score: _IntegerValue
    aliases: list[_StringValue] | tuple[_StringValue, ...]


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _wait_for_port(port: int, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"generated remote server exited with {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.2)
    raise AssertionError("generated remote server never became reachable")


def _make_generated_person(
    generated: ModuleType,
    identifier: str,
    score: int,
    *,
    nickname: str | None = None,
) -> _GeneratedPerson:
    return generated.Person(
        identifier=generated.Identifier(identifier),
        nickname=None if nickname is None else generated.Nickname(nickname),
        score=generated.Score(score),
        val_bool=generated.ValBool(True),
        val_constrained=generated.ValConstrained(20),
        val_date=generated.ValDate(date(2026, 7, 29)),
        val_datetime=generated.ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=generated.ValDecimal(Decimal("3.5")),
        val_double=generated.ValDouble(3.5),
        val_duration=generated.ValDuration(timedelta(seconds=3)),
    )


def _acceptance_contract(database: Database) -> tuple[Path, Path, str]:
    server_version = database.detected_server_version()
    if server_version is not None:
        major, minor = (int(part) for part in server_version.split(".")[:2])
        if (major, minor) < (3, 12):
            return ACCEPTANCE_SCHEMA_3_11, PROVIDER_SCHEMA_3_11, "typedb-3.11.5/v1"
    return ACCEPTANCE_SCHEMA, PROVIDER_SCHEMA, "typedb-3.12.1/v1"


@pytest.fixture
def generated_package(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    clean_db: Database,
) -> Iterator[ModuleType]:
    """Import a generated package emitted now or supplied as immutable test evidence."""
    _, _, semantic_profile = _acceptance_contract(clean_db)
    supplied_stage = os.environ.get("TYPE_BRIDGE_GENERATED_PYTHON_STAGE")
    if supplied_stage is None:
        stage = tmp_path / "generated-projection"
        generation_environment = os.environ.copy()
        generation_environment["TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"] = semantic_profile
        subprocess.run(
            [
                str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
                "python",
                str(stage),
            ],
            cwd=ROOT,
            env=generation_environment,
            check=True,
        )
    else:
        stage = Path(supplied_stage).resolve()
        for required in (
            stage / "generated_v2" / "__init__.py",
            stage / "schema-authority.json",
        ):
            if not required.is_file() or required.is_symlink():
                raise AssertionError(f"supplied generated Python fixture is incomplete: {required}")
    monkeypatch.syspath_prepend(str(stage))
    importlib.invalidate_caches()
    generated = importlib.import_module("generated_v2")
    generated_semantics = json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON)
    assert generated_semantics["semantic_profile"] == semantic_profile
    try:
        yield generated
    finally:
        for module_name in tuple(sys.modules):
            if module_name == "generated_v2" or module_name.startswith("generated_v2."):
                del sys.modules[module_name]


def test_generated_package_preserves_application_operation_outcomes_live(
    clean_db: Database,
    generated_package: ModuleType,
    caplog: pytest.LogCaptureFixture,
) -> None:
    generated = generated_package
    _, provider_schema, _ = _acceptance_contract(clean_db)
    clean_db.execute_query(provider_schema.read_text(encoding="utf-8"), transaction_type="schema")

    person_manager = generated.Person.manager(clean_db)

    hook_events: list[tuple[str, str, str]] = []

    class TraceHook(generated.CrudHook):
        def __init__(self, name: str) -> None:
            self.name = name

        def pre_put(self, sender, instance) -> None:
            hook_events.append((self.name, "pre_put", instance.identifier.value))

        def post_put(self, sender, instance) -> None:
            hook_events.append((self.name, "post_put", instance.identifier.value))

    first_hook = TraceHook("first")
    second_hook = TraceHook("second")
    hooked_manager = generated.Person.manager(clean_db).add_hook(first_hook).add_hook(second_hook)
    hooked_person = _make_generated_person(generated, "parity-hooked", 10)
    assert hooked_manager.put(hooked_person) is hooked_person
    assert hook_events == [
        ("first", "pre_put", "parity-hooked"),
        ("second", "pre_put", "parity-hooked"),
        ("second", "post_put", "parity-hooked"),
        ("first", "post_put", "parity-hooked"),
    ]
    hooked_manager.remove_hook(second_hook)
    hooked_manager.remove_hook(first_hook)

    filtered_hook_events: list[str] = []

    class PutOnlyTraceHook(generated.CrudHook):
        def should_run(self, event, sender) -> bool:
            return event in {generated.CrudEvent.PRE_PUT, generated.CrudEvent.POST_PUT}

        def pre_put(self, sender, instance) -> None:
            filtered_hook_events.append("pre_put")

        def post_put(self, sender, instance) -> None:
            filtered_hook_events.append("post_put")

        def pre_update(self, sender, instance) -> None:
            filtered_hook_events.append("unexpected_pre_update")

    put_only_manager = generated.Person.manager(clean_db).add_hook(PutOnlyTraceHook())
    assert put_only_manager.put(hooked_person) is hooked_person
    assert filtered_hook_events == ["pre_put", "post_put"]
    assert put_only_manager.update(hooked_person) is hooked_person
    assert filtered_hook_events == ["pre_put", "post_put"]

    class FailingPostHook(generated.CrudHook):
        def post_insert(self, sender, instance) -> None:
            raise RuntimeError("post hook sentinel")

    post_failure_person = _make_generated_person(generated, "parity-post-failure", 11)
    with caplog.at_level(logging.ERROR, logger="generated_v2._runtime"):
        assert (
            generated.Person.manager(clean_db)
            .add_hook(FailingPostHook())
            .insert(post_failure_person)
            is post_failure_person
        )
    assert "generated CRUD post-hook failed for post_insert" in caplog.text
    assert person_manager.get_by_iid(post_failure_person.iid) is not None

    class CancelInsertHook(generated.CrudHook):
        def pre_insert(self, sender, instance) -> None:
            raise generated.HookCancelled("insert cancelled")

    cancelled_person = _make_generated_person(generated, "parity-cancelled", 12)
    cancelling_hook = CancelInsertHook()
    with pytest.raises(generated.HookCancelled, match="insert cancelled") as cancelled:
        generated.Person.manager(clean_db).add_hook(cancelling_hook).insert(cancelled_person)
    assert cancelled.value.event is generated.CrudEvent.PRE_INSERT
    assert cancelled.value.hook is cancelling_hook
    assert (
        person_manager.filter(identifier=generated.Identifier("parity-cancelled")).exists() is False
    )

    batch_events: list[tuple[str, str]] = []

    class BatchTraceHook(generated.CrudHook):
        def pre_insert(self, sender, instance) -> None:
            batch_events.append(("pre_insert", instance.identifier.value))

        def post_insert(self, sender, instance) -> None:
            batch_events.append(("post_insert", instance.identifier.value))

        def pre_update(self, sender, instance) -> None:
            batch_events.append(("pre_update", instance.identifier.value))

        def post_update(self, sender, instance) -> None:
            batch_events.append(("post_update", instance.identifier.value))

    batch_people = [
        _make_generated_person(generated, "parity-batch-a", 20),
        _make_generated_person(generated, "parity-batch-b", 21),
    ]
    batch_manager = generated.Person.manager(clean_db).add_hook(BatchTraceHook())
    assert batch_manager.insert_many([]) == []
    assert batch_manager.put_many([]) == []
    assert batch_manager.update_many([]) == []
    assert batch_manager.delete_many([]) == []
    assert batch_events == []
    assert batch_manager.insert_many(batch_people) == batch_people
    assert batch_events == [
        ("pre_insert", "parity-batch-a"),
        ("pre_insert", "parity-batch-b"),
        ("post_insert", "parity-batch-a"),
        ("post_insert", "parity-batch-b"),
    ]
    batch_events.clear()
    batch_people[0].score = generated.Score(30)
    batch_people[1].score = generated.Score(31)
    assert batch_manager.update_many(batch_people) == batch_people
    assert batch_events == [
        ("pre_update", "parity-batch-a"),
        ("pre_update", "parity-batch-b"),
        ("post_update", "parity-batch-a"),
        ("post_update", "parity-batch-b"),
    ]

    key_mutation = _make_generated_person(generated, "parity-key-preserved", 32)
    person_manager.insert(key_mutation)
    key_mutation.identifier = generated.Identifier("parity-key-mutated")
    assert person_manager.update(key_mutation) is key_mutation
    assert key_mutation.identifier.value == "parity-key-preserved"
    assert person_manager.get_by_iid(key_mutation.iid).identifier.value == "parity-key-preserved"

    stale_update = _make_generated_person(generated, "parity-stale-update", 33)
    person_manager.insert(stale_update)
    person_manager.delete(stale_update)
    stale_update.score = generated.Score(34)
    with pytest.raises(RuntimeError, match="not found after update"):
        person_manager.update(stale_update)

    detached_update = _make_generated_person(
        generated,
        "parity-batch-a",
        40,
        nickname="detached-update",
    )
    assert person_manager.update(detached_update) is detached_update
    assert detached_update.iid == batch_people[0].iid
    detached_stored = person_manager.get_by_iid(batch_people[0].iid)
    assert detached_stored.score.value == 40
    assert detached_stored.nickname.value == "detached-update"

    missing_update = _make_generated_person(generated, "parity-missing-update", 99)
    assert person_manager.update(missing_update) is missing_update
    assert missing_update.iid is None
    assert (
        person_manager.filter(identifier=generated.Identifier("parity-missing-update")).exists()
        is False
    )

    missing_delete = _make_generated_person(generated, "parity-missing-delete", 99)
    assert person_manager.delete(missing_delete) is missing_delete
    assert missing_delete.iid is None
    assert person_manager.delete_many([missing_delete]) == []

    mutation_edge = _make_generated_person(
        generated,
        "parity-ownership-update",
        98,
        nickname="remove-me",
    )
    mutation_edge.aliases = [generated.Aliases("initial-a"), generated.Aliases("initial-b")]
    assert person_manager.insert(mutation_edge) is mutation_edge
    mutation_edge.nickname = None
    special_alias = "quote'\"\\line\nunicode-λ"
    mutation_edge.aliases = [generated.Aliases(special_alias)]
    assert person_manager.update(mutation_edge) is mutation_edge
    replaced_ownerships = person_manager.get_by_iid(mutation_edge.iid)
    assert replaced_ownerships.nickname is None
    assert [alias.value for alias in replaced_ownerships.aliases] == [special_alias]
    mutation_edge.aliases = []
    assert person_manager.update(mutation_edge) is mutation_edge
    cleared_ownerships = person_manager.get_by_iid(mutation_edge.iid)
    assert cleared_ownerships.nickname is None
    assert cleared_ownerships.aliases == ()

    strict_missing = _make_generated_person(generated, "parity-strict-missing", 99)
    with pytest.raises(generated.ProjectedModelNotFoundError, match="not found"):
        person_manager.delete_many([batch_people[0], strict_missing], strict=True)
    assert person_manager.get_by_iid(batch_people[0].iid) is not None

    updated = person_manager.filter(
        identifier__in=[
            generated.Identifier("parity-batch-a"),
            generated.Identifier("parity-batch-b"),
        ]
    ).update_with(lambda value: setattr(value, "nickname", generated.Nickname("filtered")))
    assert {value.iid for value in updated} == {value.iid for value in batch_people}
    assert {
        value.nickname.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    } == {"filtered"}

    scores_before_callback_failure = {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    }

    def fail_callback(value) -> None:
        value.score = generated.Score(value.score.value + 100)
        if value.identifier.value == "parity-batch-b":
            raise RuntimeError("callback failure sentinel")

    with pytest.raises(RuntimeError, match="callback failure sentinel"):
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).update_with(fail_callback)
    assert {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    } == scores_before_callback_failure

    transaction_people = [
        _make_generated_person(generated, "parity-transaction-a", 50),
        _make_generated_person(generated, "parity-transaction-b", 51),
    ]
    person_manager.insert_many(transaction_people)
    transaction_people[0].score = generated.Score(60)
    transaction_people[1].score = generated.Score(61)

    class CancelSecondUpdate(generated.CrudHook):
        def __init__(self) -> None:
            self.calls = 0

        def pre_update(self, sender, instance) -> None:
            self.calls += 1
            if self.calls == 2:
                raise generated.HookCancelled("second update cancelled")

    with pytest.raises(generated.HookCancelled, match="second update cancelled"):
        generated.Person.manager(clean_db).add_hook(CancelSecondUpdate()).update_many(
            transaction_people
        )
    assert {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).all()
    } == {"parity-transaction-a": 50, "parity-transaction-b": 51}

    atomic_people = [
        _make_generated_person(generated, "parity-atomic-a", 52),
        _make_generated_person(generated, "parity-atomic-b", 53),
    ]
    person_manager.insert_many(atomic_people)
    stale_atomic_iid = atomic_people[1].iid
    person_manager.delete(atomic_people[1])
    atomic_people[0].score = generated.Score(62)
    atomic_people[1].score = generated.Score(63)
    with pytest.raises(RuntimeError, match="not found after update"):
        person_manager.update_many(atomic_people)
    assert person_manager.get_by_iid(atomic_people[0].iid).score.value == 52
    assert person_manager.get_by_iid(stale_atomic_iid) is None
    person_manager.delete(atomic_people[0])

    class CancelSecondDelete(generated.CrudHook):
        def __init__(self) -> None:
            self.calls = 0

        def pre_delete(self, sender, instance) -> None:
            self.calls += 1
            if self.calls == 2:
                raise generated.HookCancelled("second delete cancelled")

    with pytest.raises(generated.HookCancelled, match="second delete cancelled"):
        generated.Person.manager(clean_db).add_hook(CancelSecondDelete()).filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).delete()
    assert (
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).count()
        == 2
    )
    assert (
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).delete()
        == 2
    )

    detached_delete_source = _make_generated_person(generated, "parity-detached-delete", 70)
    person_manager.insert(detached_delete_source)
    detached_delete = _make_generated_person(generated, "parity-detached-delete", 70)
    assert person_manager.delete(detached_delete) is detached_delete
    assert detached_delete.iid == detached_delete_source.iid
    assert person_manager.get_by_iid(detached_delete_source.iid) is None

    role_player = hooked_person
    membership_manager = generated.Membership.manager(clean_db)
    membership = generated.Membership(member=role_player)
    membership_manager.insert(membership)
    detached_membership_update = generated.Membership(member=role_player)
    assert membership_manager.update(detached_membership_update) is detached_membership_update
    assert detached_membership_update.iid == membership.iid
    detached_membership_delete = generated.Membership(member=role_player)
    assert membership_manager.delete(detached_membership_delete) is detached_membership_delete
    assert detached_membership_delete.iid == membership.iid
    assert membership_manager.get_by_iid(membership.iid) is None
    missing_membership = generated.Membership(member=role_player)
    assert membership_manager.delete(missing_membership) is missing_membership
    assert missing_membership.iid is None
    assert membership_manager.insert_many([]) == []
    assert membership_manager.put_many([]) == []
    assert membership_manager.update_many([]) == []
    assert membership_manager.delete_many([]) == []

    relation_batch_events: list[tuple[str, str]] = []

    class RelationBatchTraceHook(generated.CrudHook):
        def pre_update(self, sender, instance) -> None:
            relation_batch_events.append(("pre_update", instance.identifier.value))

        def post_update(self, sender, instance) -> None:
            relation_batch_events.append(("post_update", instance.identifier.value))

        def pre_delete(self, sender, instance) -> None:
            relation_batch_events.append(("pre_delete", instance.identifier.value))

        def post_delete(self, sender, instance) -> None:
            relation_batch_events.append(("post_delete", instance.identifier.value))

    network_manager = generated.NetworkLink.manager(clean_db)
    network_batch_manager = generated.NetworkLink.manager(clean_db).add_hook(
        RelationBatchTraceHook()
    )
    assert network_batch_manager.insert_many([]) == []
    assert network_batch_manager.put_many([]) == []
    assert network_batch_manager.update_many([]) == []
    assert network_batch_manager.delete_many([]) == []
    assert relation_batch_events == []
    networks = [
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-a"),
            origin=hooked_person,
            destination=batch_people[0],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-b"),
            origin=batch_people[0],
            destination=hooked_person,
        ),
    ]
    network_manager.insert_many(networks)
    networks[0].nickname = generated.Nickname("batch-updated-a")
    networks[1].nickname = generated.Nickname("batch-updated-b")
    assert network_batch_manager.update_many(networks) == networks
    assert relation_batch_events == [
        ("pre_update", "parity-network-a"),
        ("pre_update", "parity-network-b"),
        ("post_update", "parity-network-a"),
        ("post_update", "parity-network-b"),
    ]
    detached_network = generated.NetworkLink(
        identifier=generated.Identifier("parity-network-a"),
        nickname=generated.Nickname("detached-relation"),
        origin=hooked_person,
        destination=batch_people[0],
    )
    assert network_manager.update(detached_network) is detached_network
    assert detached_network.iid == networks[0].iid
    assert network_manager.get_by_iid(networks[0].iid).nickname.value == "detached-relation"

    updated_networks = network_manager.filter(
        identifier__in=[
            generated.Identifier("parity-network-a"),
            generated.Identifier("parity-network-b"),
        ]
    ).update_with(lambda value: setattr(value, "nickname", generated.Nickname("filtered-relation")))
    assert {value.iid for value in updated_networks} == {value.iid for value in networks}
    assert {
        value.nickname.value
        for value in network_manager.filter(
            identifier__in=[
                generated.Identifier("parity-network-a"),
                generated.Identifier("parity-network-b"),
            ]
        ).all()
    } == {"filtered-relation"}
    assert (
        network_manager.filter(
            identifier__in=[
                generated.Identifier("parity-network-a"),
                generated.Identifier("parity-network-b"),
            ]
        ).delete()
        == 2
    )

    relation_delete_batch = [
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-delete-a"),
            origin=hooked_person,
            destination=batch_people[0],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-delete-b"),
            origin=batch_people[0],
            destination=hooked_person,
        ),
    ]
    network_manager.insert_many(relation_delete_batch)
    relation_batch_events.clear()
    assert network_batch_manager.delete_many(relation_delete_batch) == relation_delete_batch
    assert relation_batch_events == [
        ("pre_delete", "parity-network-delete-a"),
        ("pre_delete", "parity-network-delete-b"),
        ("post_delete", "parity-network-delete-a"),
        ("post_delete", "parity-network-delete-b"),
    ]

    assert person_manager.delete_many(batch_people) == batch_people
    for value in batch_people:
        assert person_manager.get_by_iid(value.iid) is None
    for value in (hooked_person, post_failure_person, mutation_edge, key_mutation):
        person_manager.delete(value)
        assert person_manager.get_by_iid(value.iid) is None


def test_generated_projection_round_trips_live_models(
    clean_db: Database,
    generated_package: ModuleType,
) -> None:
    generated = generated_package
    _, provider_schema, _ = _acceptance_contract(clean_db)
    clean_db.execute_query(provider_schema.read_text(encoding="utf-8"), transaction_type="schema")

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
        generated.Actor,
        generated.Aliases,
        generated.Container,
        generated.Counter,
        generated.CounterValue,
        generated.Employment,
        generated.Event,
        generated.Identifier,
        generated.Interaction,
        generated.Membership,
        generated.NetworkLink,
        generated.Nickname,
        generated.Party,
        generated.Person,
        generated.PlainActivity,
        generated.Robot,
        generated.RobotId,
    ):
        assert model.__runtime_projection__ is installed

    person_manager = generated.Person.manager(clean_db)
    assert person_manager.get_by_iid("not-a-valid-iid") is None
    assert person_manager.get_by_iid("0xdeadbeefdeadbeefdeadbeef") is None

    person = generated.Person(
        identifier=generated.Identifier("person-1"),
        nickname=generated.Nickname("alice"),
        aliases=[generated.Aliases("alpha"), generated.Aliases("beta")],
        score=generated.Score(3),
        foo__bar=generated.FooBar(7),
        score__gte=generated.ScoreGte(8),
        val_bool=generated.ValBool(True),
        val_constrained=generated.ValConstrained(20),
        val_date=generated.ValDate(date(2026, 7, 29)),
        val_datetime=generated.ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=generated.ValDecimal(Decimal("3.5")),
        val_double=generated.ValDouble(3.5),
        val_duration=generated.ValDuration(timedelta(seconds=3)),
    )
    assert person_manager.put(person) is person
    assert person.iid

    query_session = generated.Person.query(clean_db)
    person_var = query_session.exact(generated.Person)

    scalar_predicates = (
        (
            "identifier",
            person_var.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
        ),
        ("boolean", person_var.field(generated.Person.val_bool).eq(generated.ValBool(True))),
        (
            "double",
            person_var.field(generated.Person.val_double).gte(generated.ValDouble(3.5)),
        ),
        (
            "decimal",
            person_var.field(generated.Person.val_decimal).gte(
                generated.ValDecimal(Decimal("3.5"))
            ),
        ),
        (
            "date",
            person_var.field(generated.Person.val_date).gte(generated.ValDate(date(2026, 7, 29))),
        ),
        (
            "datetime",
            person_var.field(generated.Person.val_datetime).gte(
                generated.ValDatetime(datetime(2026, 7, 29))
            ),
        ),
        (
            "datetime-tz",
            person_var.field(generated.Person.val_datetime_tz).gte(
                generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC))
            ),
        ),
        (
            "duration-equality",
            person_var.field(generated.Person.val_duration).eq(
                generated.ValDuration(timedelta(seconds=3))
            ),
        ),
    )
    for scalar_name, predicate in scalar_predicates:
        assert query_session.query(person_var).where(predicate).count_by(person_var) == 1, (
            scalar_name
        )
    scalar_domain_person = (
        query_session.query(person_var)
        .where(*(predicate for _, predicate in scalar_predicates))
        .one()
    )
    assert scalar_domain_person.iid == person.iid
    assert [
        candidate.iid
        for candidate in person_manager.filter(
            val_bool=generated.ValBool(True),
            val_double__gte=generated.ValDouble(3.5),
            val_decimal__gte=generated.ValDecimal(Decimal("3.5")),
            val_date__gte=generated.ValDate(date(2026, 7, 29)),
            val_datetime__gte=generated.ValDatetime(datetime(2026, 7, 29)),
            val_datetime_tz__gte=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
            val_duration=generated.ValDuration(timedelta(seconds=3)),
        ).all()
    ] == [person.iid]

    counter_manager = generated.Counter.manager(clean_db)
    detached_counter = generated.Counter(counter_value=generated.CounterValue(42))
    with pytest.raises(ValueError, match="attached IID or projected key"):
        counter_manager.delete(detached_counter)
    assert counter_manager.insert(detached_counter) is detached_counter
    assert detached_counter.iid
    stored_counter = counter_manager.get_by_iid(detached_counter.iid)
    assert type(stored_counter) is generated.Counter
    assert type(stored_counter.counter_value) is generated.CounterValue
    assert stored_counter.counter_value.value == 42
    counter_var = query_session.exact(generated.Counter)
    counter_query = query_session.query(counter_var).where(
        counter_var.field(generated.Counter.counter_value).eq(generated.CounterValue(42))
    )
    queried_counter = counter_query.one()
    assert type(queried_counter) is generated.Counter
    assert queried_counter.iid == detached_counter.iid
    with pytest.raises(
        Exception,
        match="bounded result identity requires a present unique scalar descriptor field",
    ) as bounded_counter_error:
        counter_query.rows(limit=2)
    assert type(bounded_counter_error.value).__name__ == "MatchRequestError"
    counter_manager.delete(detached_counter)
    assert counter_manager.get_by_iid(detached_counter.iid) is None
    assert counter_manager.count() == 0

    plain_activity_manager = generated.PlainActivity.manager(clean_db)
    plain_activity = generated.PlainActivity(participant=person)
    assert plain_activity_manager.insert(plain_activity) is plain_activity
    assert plain_activity.iid
    stored_plain_activity = plain_activity_manager.get_by_iid(plain_activity.iid)
    assert type(stored_plain_activity) is generated.PlainActivity
    assert type(stored_plain_activity.participant) is generated.Person
    assert stored_plain_activity.participant.iid == person.iid
    plain_activity_var = query_session.exact(generated.PlainActivity)
    plain_participant_var = query_session.exact(generated.Person)
    queried_plain_activity, queried_plain_participant = (
        query_session.query(plain_activity_var, plain_participant_var)
        .where(
            plain_activity_var.role(generated.PlainActivity.participant).connects(
                plain_participant_var
            ),
            plain_participant_var.field(generated.Person.identifier).eq(
                generated.Identifier("person-1")
            ),
        )
        .one()
    )
    assert queried_plain_activity.iid == plain_activity.iid
    assert queried_plain_participant.iid == person.iid
    plain_activity_manager.delete(plain_activity)
    assert plain_activity_manager.get_by_iid(plain_activity.iid) is None

    matching_people = query_session.query(person_var).where(
        person_var.field(generated.Person.score).gte(generated.Score(3))
    )
    assert matching_people.count_by(person_var) == 1
    assert matching_people.exists_by(person_var) is True
    query_rows = matching_people.rows(limit=10)
    assert len(query_rows) == 1
    assert type(query_rows[0]) is generated.Person
    assert query_rows[0].iid == person.iid
    assert type(query_rows[0].score) is generated.Score
    assert query_rows[0].score.value == 3
    query_one = matching_people.one()
    assert type(query_one) is generated.Person
    assert query_one.iid == person.iid
    present_aliases = query_session.query(person_var).where(
        person_var.field(generated.Person.aliases).is_present()
    )
    assert [candidate.iid for candidate in present_aliases.rows(limit=10)] == [person.iid]
    assert query_session.query(person_var).where(person_var.iid(person.iid)).one().iid == person.iid
    assert [
        candidate.iid
        for candidate in query_session.query(person_var)
        .where(person_var.iid_in([person.iid]))
        .rows(limit=10)
    ] == [person.iid]
    missing_people = query_session.query(person_var).where(
        person_var.field(generated.Person.score).gt(generated.Score(3))
    )
    assert missing_people.count_by(person_var) == 0
    assert missing_people.exists_by(person_var) is False

    filtered_people = person_manager.filter(score__gte=generated.Score(3)).all()
    assert [candidate.iid for candidate in filtered_people] == [person.iid]
    assert [
        candidate.iid
        for candidate in person_manager.filter(
            score__in=[generated.Score(2), generated.Score(3)]
        ).all()
    ] == [person.iid]
    assert [candidate.iid for candidate in person_manager.filter(aliases__isnull=False).all()] == [
        person.iid
    ]
    assert [candidate.iid for candidate in person_manager.filter(iid__in=[person.iid]).all()] == [
        person.iid
    ]
    filtered_manager = person_manager.filter(score__gte=generated.Score(3))
    assert filtered_manager.first().iid == person.iid
    assert filtered_manager.count() == 1
    assert filtered_manager.exists() is True
    assert person_manager.filter(score__gt=generated.Score(3)).first() is None
    assert person_manager.filter(score__gt=generated.Score(3)).count() == 0
    assert person_manager.filter(score__gt=generated.Score(3)).exists() is False
    assert [candidate.iid for candidate in person_manager.filter(score__gte=3).all()] == [
        person.iid
    ]
    assert [
        candidate.iid for candidate in person_manager.filter(score__gte=generated.Score(3)).all()
    ] == [person.iid]
    assert [
        candidate.iid
        for candidate in person_manager.filter(**{"score__gte__eq": generated.ScoreGte(8)}).all()
    ] == [person.iid]
    generated_dunder = person_manager.filter(**{"foo__bar": generated.FooBar(7)}).all()
    assert [candidate.iid for candidate in generated_dunder] == [person.iid]
    with pytest.raises(TypeError, match="exact attribute wrapper"):
        person_manager.filter(score__gte=generated.Identifier("wrong-wrapper"))
    with pytest.raises(ValueError, match="unsupported generated manager lookup"):
        person_manager.filter(score__contains=generated.Score(3))

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

    person.nickname = generated.Nickname("ada")
    assert person_manager.update(person) is person
    updated_person = person_manager.get_by_iid(person.iid)
    assert type(updated_person) is generated.Person
    assert updated_person.nickname.value == "ada"

    batch_people = [
        _make_generated_person(generated, "person-2", 5),
        _make_generated_person(generated, "person-3", 7),
    ]
    assert person_manager.insert_many(batch_people) == batch_people
    assert all(candidate.iid for candidate in batch_people)
    batch_iids = [candidate.iid for candidate in batch_people]
    assert {
        candidate.iid for candidate in person_manager.filter(aliases__isnull=True).all()
    } == set(batch_iids)
    missing_aliases = query_session.query(person_var).where(
        person_var.field(generated.Person.aliases).is_missing()
    )
    assert {candidate.iid for candidate in missing_aliases.rows(limit=10)} == set(batch_iids)
    assert [candidate.iid for candidate in person_manager.put_many(batch_people)] == batch_iids

    with clean_db.transaction("write") as transaction:
        transaction_person = _make_generated_person(generated, "person-4", 9)
        transaction_manager = generated.Person.manager(transaction)
        assert transaction_manager.insert(transaction_person) is transaction_person

    rolled_back_person = _make_generated_person(generated, "person-rollback", 11)
    with pytest.raises(RuntimeError, match="generated rollback sentinel"):
        with clean_db.transaction("write") as transaction:
            rollback_manager = generated.Person.manager(transaction)
            assert rollback_manager.insert(rolled_back_person) is rolled_back_person
            assert rollback_manager.get_by_iid(rolled_back_person.iid).iid == rolled_back_person.iid
            raise RuntimeError("generated rollback sentinel")
    assert (
        person_manager.filter(identifier=generated.Identifier("person-rollback")).exists() is False
    )

    with clean_db.transaction("read") as transaction:
        transaction_session = generated.Person.query(transaction)
        transaction_person_var = transaction_session.exact(generated.Person)
        transaction_query = transaction_session.query(transaction_person_var).where(
            transaction_person_var.field(generated.Person.identifier).eq(
                generated.Identifier("person-4")
            )
        )
        assert transaction_query.count_by(transaction_person_var) == 1
        assert transaction_query.first().iid == transaction_person.iid

    assert person_manager.count() == 4
    all_people_query = query_session.query(person_var)
    assert (
        all_people_query.first(order_by=(person_var.field(generated.Person.identifier).asc(),)).iid
        == person.iid
    )
    assert [
        candidate.identifier.value
        for candidate in all_people_query.rows(
            limit=2,
            offset=1,
            order_by=(person_var.field(generated.Person.identifier).asc(),),
        )
    ] == ["person-2", "person-3"]
    identifier_field = person_var.field(generated.Person.identifier)
    identifier_value = generated.Identifier("person-1")
    generated_expression = (
        identifier_field.starts_with(generated.Identifier("person-"))
        & identifier_field.contains(generated.Identifier("son-"))
        & identifier_field.ends_with(generated.Identifier("-1"))
        & identifier_field.regex(generated.Identifier("^person-1$"))
        & ~identifier_field.neq(identifier_value)
        & (
            identifier_field.eq(identifier_value)
            | identifier_field.eq(generated.Identifier("does-not-exist"))
        )
    )
    expression_person = all_people_query.where(generated_expression).one()
    assert type(expression_person) is generated.Person
    assert expression_person.iid == person.iid
    same_person_var = query_session.exact(generated.Person)
    same_identifier_pair = (
        query_session.query(person_var, same_person_var)
        .where(
            identifier_field.eq(same_person_var.field(generated.Person.identifier)),
            identifier_field.eq(identifier_value),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in same_identifier_pair) == (person.iid, person.iid)
    cross_left = query_session.exact(generated.Person)
    cross_right = query_session.exact(generated.Person)
    cross_pair = (
        query_session.query(cross_left, cross_right)
        .allow_cross_join(cross_left, cross_right)
        .where(
            cross_left.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
            cross_right.field(generated.Person.identifier).eq(generated.Identifier("person-2")),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in cross_pair) == (person.iid, batch_people[0].iid)
    score_field = person_var.field(generated.Person.score)
    direct_aggregate = all_people_query.aggregate(
        person_var,
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
        generated.aggregate.min(score_field),
        generated.aggregate.max(score_field),
        generated.aggregate.mean(score_field),
        generated.aggregate.median(score_field),
        generated.aggregate.std(score_field),
    )
    assert direct_aggregate[:6] == (4, 24, 3, 9, 6.0, 6.0)
    assert isinstance(direct_aggregate[6], float)
    direct_field_grouped = all_people_query.group_by(
        person_var,
        person_var.field(generated.Person.val_bool),
    ).aggregate(
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
    )
    assert len(direct_field_grouped) == 1
    direct_field_group, direct_field_values = direct_field_grouped[0]
    assert type(direct_field_group) is generated.ValBool
    assert direct_field_group.value is True
    assert direct_field_values == (4, 24)
    direct_tuple_field_grouped = all_people_query.group_by(
        person_var,
        person_var.field(generated.Person.val_bool),
        score_field,
    ).aggregate(
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
    )
    assert [
        (bool_group.value, score_group.value, values)
        for (bool_group, score_group), values in direct_tuple_field_grouped
    ] == [
        (True, 3, (1, 3)),
        (True, 5, (1, 5)),
        (True, 7, (1, 7)),
        (True, 9, (1, 9)),
    ]
    with pytest.raises(Exception, match="long|double|numeric|reduc"):
        all_people_query.aggregate(
            person_var,
            generated.aggregate.sum(person_var.field(generated.Person.identifier)),
        )

    employee = generated.Employee(
        identifier=generated.Identifier("employee-1"),
        party_name=generated.PartyName("employee"),
        rank=generated.Rank(1),
    )
    manager = generated.Manager(
        identifier=generated.Identifier("manager-1"),
        manager_note=generated.ManagerNote("lead"),
        party_name=generated.PartyName("manager"),
        rank=generated.Rank(2),
    )
    generated.Employee.manager(clean_db).insert(employee)
    generated.Manager.manager(clean_db).insert(manager)
    party_var = query_session.subtypes(generated.Party)
    party_rows = query_session.query(party_var).rows(
        limit=10,
        order_by=(party_var.field(generated.Party.identifier).asc(),),
    )
    assert [type(candidate) for candidate in party_rows] == [
        generated.Employee,
        generated.Manager,
    ]
    assert [candidate.iid for candidate in party_rows] == [employee.iid, manager.iid]

    membership_manager = generated.Membership.manager(clean_db)
    membership = generated.Membership(member=person)
    assert membership_manager.insert(membership) is membership
    assert membership.iid

    stored_membership = membership_manager.get_by_iid(membership.iid)
    assert type(stored_membership) is generated.Membership
    assert stored_membership.iid == membership.iid
    assert type(stored_membership.member) is generated.Person
    assert stored_membership.member.iid == person.iid

    membership_session = generated.Membership.query(clean_db)
    membership_var = membership_session.exact(generated.Membership)
    # Membership is intentionally keyless. The shared Query V2 contract permits
    # singular execution but rejects bounded-many windows without a stable key.
    queried_membership = membership_session.query(membership_var).one()
    assert type(queried_membership) is generated.Membership
    assert queried_membership.iid == membership.iid
    assert type(queried_membership.member) is generated.Person
    assert queried_membership.member.iid == person.iid

    robot_manager = generated.Robot.manager(clean_db)
    integer_key_values = (-42, 1, 100, 9999)
    robots = [
        generated.Robot(
            nickname=generated.Nickname("actor-robot") if value == -42 else None,
            robot_id=generated.RobotId(value),
            val_constrained=generated.ValConstrained(index + 1),
        )
        for index, value in enumerate(integer_key_values)
    ]
    assert robot_manager.insert_many(robots) == robots
    assert robot_manager.count() == len(integer_key_values)
    for expected, value in zip(robots, integer_key_values, strict=True):
        integer_key_match = robot_manager.filter(robot_id=generated.RobotId(value))
        assert integer_key_match.count() == 1
        assert integer_key_match.first().iid == expected.iid
        assert integer_key_match.first().robot_id.value == value
    assert {
        candidate.robot_id.value
        for candidate in robot_manager.filter(
            robot_id__in=[generated.RobotId(-42), generated.RobotId(9999)]
        ).all()
    } == {-42, 9999}

    robot = robots[0]
    robot_membership = generated.Membership(member=robot)
    assert membership_manager.insert(robot_membership) is robot_membership
    stored_robot_membership = membership_manager.get_by_iid(robot_membership.iid)
    assert type(stored_robot_membership) is generated.Membership
    assert type(stored_robot_membership.member) is generated.Robot
    assert stored_robot_membership.member.iid == robot.iid
    assert stored_robot_membership.member.robot_id.value == -42

    interaction_manager = generated.Interaction.manager(clean_db)
    robot_interaction = generated.Interaction(
        identifier=generated.Identifier("interaction-robot"),
        nickname=generated.Nickname("assist"),
        actor=robot,
        target=person,
    )
    person_interaction = generated.Interaction(
        identifier=generated.Identifier("interaction-person"),
        nickname=generated.Nickname("read"),
        actor=person,
        target=batch_people[0],
    )
    assert interaction_manager.insert_many([robot_interaction, person_interaction]) == [
        robot_interaction,
        person_interaction,
    ]

    actor_var = query_session.subtypes(generated.Actor)
    interaction_var = query_session.exact(generated.Interaction)
    polymorphic_actor_rows = (
        query_session.query(interaction_var)
        .match(actor_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(actor_var),
            actor_var.field(generated.Actor.nickname).contains(generated.Nickname("a")),
        )
        .rows(
            limit=10,
            order_by=(interaction_var.field(generated.Interaction.identifier).asc(),),
        )
    )
    assert {type(relation.actor) for relation in polymorphic_actor_rows} == {
        generated.Person,
        generated.Robot,
    }
    assert {relation.iid for relation in polymorphic_actor_rows} == {
        robot_interaction.iid,
        person_interaction.iid,
    }

    robot_var = query_session.exact(generated.Robot)
    target_var = query_session.exact(generated.Person)
    queried_robot_interaction, queried_robot, queried_target = (
        query_session.query(interaction_var, robot_var, target_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(robot_var),
            interaction_var.role(generated.Interaction.target).connects(target_var),
            interaction_var.field(generated.Interaction.nickname).eq(generated.Nickname("assist")),
            robot_var.field(generated.Robot.robot_id).eq(generated.RobotId(-42)),
            robot_var.field(generated.Robot.val_constrained).lt(generated.ValConstrained(10)),
            target_var.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
        )
        .one()
    )
    assert queried_robot_interaction.iid == robot_interaction.iid
    assert type(queried_robot) is generated.Robot
    assert queried_robot.iid == robot.iid
    assert queried_target.iid == person.iid

    person_actor_var = query_session.exact(generated.Person)
    queried_person_interaction, queried_person_actor = (
        query_session.query(interaction_var, person_actor_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(person_actor_var),
            interaction_var.field(generated.Interaction.nickname).eq(generated.Nickname("read")),
            person_actor_var.field(generated.Person.score).gte(generated.Score(3)),
        )
        .one()
    )
    assert queried_person_interaction.iid == person_interaction.iid
    assert queried_person_actor.iid == person.iid
    interaction_manager.delete(queried_person_interaction)
    assert interaction_manager.get_by_iid(person_interaction.iid) is None

    membership_manager.delete(robot_membership)
    assert membership_manager.get_by_iid(robot_membership.iid) is None
    robot_manager.delete(robot)
    assert robot_manager.get_by_iid(robot.iid) is None
    surviving_interaction = interaction_manager.get_by_iid(robot_interaction.iid)
    assert type(surviving_interaction) is generated.Interaction
    assert surviving_interaction.iid == robot_interaction.iid
    assert surviving_interaction.actor is None
    assert type(surviving_interaction.target) is generated.Person
    assert surviving_interaction.target.iid == person.iid
    interaction_manager.delete(surviving_interaction)
    assert interaction_manager.get_by_iid(robot_interaction.iid) is None
    assert robot_manager.delete_many(robots[1:]) == robots[1:]
    assert robot_manager.count() == 0

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

    employment_session = generated.Employment.query(clean_db)
    employment_var = employment_session.exact(generated.Employment)
    queried_employment = employment_session.query(employment_var).one()
    assert type(queried_employment) is generated.Employment
    assert queried_employment.iid == employment.iid
    assert type(queried_employment.employee) is generated.Person
    assert queried_employment.employee.iid == person.iid
    assert not hasattr(queried_employment, "member")

    membership_subtype_var = membership_session.subtypes(generated.Membership)
    membership_family = membership_session.query(membership_subtype_var)
    assert membership_family.count_by(membership_subtype_var) == 2
    queried_base_relation = membership_family.where(
        membership_subtype_var.iid(membership.iid)
    ).one()
    assert type(queried_base_relation) is generated.Membership
    assert queried_base_relation.member.iid == person.iid
    queried_subtype_relation = membership_family.where(
        membership_subtype_var.iid(employment.iid)
    ).one()
    assert type(queried_subtype_relation) is generated.Employment
    assert queried_subtype_relation.employee.iid == person.iid
    assert not hasattr(queried_subtype_relation, "member")

    aggregate_employment_var = query_session.exact(generated.Employment)
    direct_grouped_aggregate = (
        query_session.query(person_var, aggregate_employment_var)
        .where(aggregate_employment_var.role(generated.Employment.employee).connects(person_var))
        .group_by(person_var, aggregate_employment_var)
        .aggregate(
            generated.aggregate.count(),
            generated.aggregate.sum(score_field),
        )
    )
    assert len(direct_grouped_aggregate) == 1
    direct_group, direct_group_values = direct_grouped_aggregate[0]
    assert type(direct_group) is generated.Employment
    assert direct_group.iid == employment.iid
    assert direct_group_values == (1, 3)

    network_manager = generated.NetworkLink.manager(clean_db)
    network = generated.NetworkLink(
        identifier=generated.Identifier("network-1"),
        nickname=generated.Nickname("primary"),
        origin=person,
        destination=batch_people[0],
        participant=[person, batch_people[0]],
    )
    assert network_manager.insert(network) is network
    assert network.iid
    network_iid = network.iid
    assert network_manager.put(network) is network
    assert network.iid == network_iid
    network.nickname = generated.Nickname("updated")
    assert network_manager.update(network) is network
    stored_network = network_manager.get_by_iid(network_iid)
    assert type(stored_network) is generated.NetworkLink
    assert stored_network.nickname.value == "updated"
    filtered_networks = network_manager.filter(identifier=generated.Identifier("network-1"))
    assert [candidate.iid for candidate in filtered_networks.all()] == [network_iid]
    assert filtered_networks.first().iid == network_iid
    assert filtered_networks.count() == 1
    assert filtered_networks.exists() is True

    cross_type_entity_owners = generated.Identifier.owners(
        clean_db,
        "person-1",
        kind="entity",
    )
    assert [type(candidate) for candidate in cross_type_entity_owners] == [generated.Person]
    assert [candidate.iid for candidate in cross_type_entity_owners] == [person.iid]
    assert {
        candidate.iid
        for candidate in generated.Identifier.owners(
            clean_db,
            "person-",
            kind="entity",
            lookup="startswith",
        )
    } == {person.iid, *batch_iids, transaction_person.iid}
    party_attribute_owners = generated.Party.has(
        clean_db,
        generated.Identifier,
        lookup="present",
    )
    assert [type(candidate) for candidate in party_attribute_owners] == [
        generated.Employee,
        generated.Manager,
    ]
    assert {candidate.iid for candidate in party_attribute_owners} == {
        employee.iid,
        manager.iid,
    }
    narrowed_person_owners = generated.Person.has(
        clean_db,
        generated.Identifier,
        generated.Identifier("person-1"),
    )
    assert [candidate.iid for candidate in narrowed_person_owners] == [person.iid]
    relation_attribute_owners = generated.Identifier.owners(
        clean_db,
        generated.Identifier("network-1"),
        kind="relation",
    )
    assert [type(candidate) for candidate in relation_attribute_owners] == [generated.NetworkLink]
    assert [candidate.iid for candidate in relation_attribute_owners] == [network_iid]
    assert relation_attribute_owners[0].origin.iid == person.iid
    assert relation_attribute_owners[0].destination.iid == batch_people[0].iid

    network_session = generated.NetworkLink.query(clean_db)
    network_var = network_session.exact(generated.NetworkLink)
    queried_network = (
        network_session.query(network_var)
        .where(
            network_var.iid(network_iid),
            network_var.field(generated.NetworkLink.nickname).is_present(),
        )
        .one()
    )
    assert type(queried_network) is generated.NetworkLink
    assert queried_network.iid == network_iid
    participant_var = network_session.exact(generated.Person)
    participant = network_var.role(generated.NetworkLink.participant).connects(participant_var)
    network_rows = network_session.query(network_var).rows(
        limit=10,
        order_by=(network_var.field(generated.NetworkLink.identifier).asc(),),
    )
    assert [candidate.iid for candidate in network_rows] == [network.iid]

    reachable_source = network_session.exact(generated.Person)
    reachable_target = network_session.exact(generated.Person)
    reachable = network_session.reachable(
        reachable_source,
        reachable_target,
        generated.NetworkLink,
        generated.NetworkLink.origin,
        generated.NetworkLink.destination,
        min_depth=1,
        max_depth=1,
    )
    reachable_pair = (
        network_session.query(reachable_source, reachable_target)
        .where(
            reachable,
            reachable_source.field(generated.Person.identifier).eq(
                generated.Identifier("person-1")
            ),
            reachable_target.field(generated.Person.identifier).eq(
                generated.Identifier("person-2")
            ),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in reachable_pair) == (
        person.iid,
        batch_people[0].iid,
    )

    network_row = make_dataclass(
        "NetworkRow",
        [
            ("network", generated.NetworkLink),
            ("participants", tuple[generated.Person, ...]),
        ],
        frozen=True,
        slots=True,
    )
    network_page = (
        network_session.query_as(
            network_row,
            network=network_var,
            participants=participant_var.collect()
            .distinct()
            .order_by(participant_var.field(generated.Person.identifier).asc()),
        )
        .where(participant)
        .page_by(
            network_var,
            limit=10,
            order_by=(network_var.field(generated.NetworkLink.identifier).asc(),),
            include_total=True,
        )
    )
    assert network_page.total == 1
    assert len(network_page.items) == 1
    named_network = network_page.items[0]
    assert type(named_network) is network_row
    assert type(named_network.network) is generated.NetworkLink
    assert named_network.network.iid == network.iid
    assert len(named_network.participants) == 2
    assert all(type(candidate) is generated.Person for candidate in named_network.participants)
    assert tuple(candidate.iid for candidate in named_network.participants) == (
        person.iid,
        batch_people[0].iid,
    )

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

    container_session = generated.Container.query(clean_db)
    container_var = container_session.exact(generated.Container)
    queried_container = container_session.query(container_var).one()
    assert type(queried_container) is generated.Container
    assert queried_container.iid == container.iid
    assert type(queried_container.item) is tuple
    assert len(queried_container.item) == 1
    assert type(queried_container.item[0]) is generated.EventRef
    assert queried_container.item[0].iid == event.iid

    generated_file = generated.__file__
    assert generated_file is not None
    generated_path = Path(generated_file).resolve()
    authority = generated_path.parent.parent.joinpath("schema-authority.json").read_bytes()
    remote_port = _free_port()
    supplied_server = os.environ.get("TYPE_BRIDGE_V2_SMOKE_SERVER")
    if supplied_server is None:
        server_command = [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "type-bridge-server",
            "--features",
            "v2-query",
            "--example",
            "v2_smoke_server",
        ]
        server_cwd = CORE
    else:
        server_path = Path(supplied_server).resolve()
        if not server_path.is_file() or server_path.is_symlink():
            raise AssertionError(f"supplied V2 smoke server is invalid: {server_path}")
        server_command = [str(server_path)]
        server_cwd = ROOT
    server = subprocess.Popen(
        server_command,
        cwd=server_cwd,
        env={
            **os.environ,
            "SMOKE_TYPEDB_ADDRESS": clean_db.address,
            "SMOKE_TYPEDB_USERNAME": clean_db.username or "admin",
            "SMOKE_TYPEDB_PASSWORD": clean_db.password or "password",
            "SMOKE_TYPEDB_HTTP_PORT": str(clean_db.http_port),
            "SMOKE_DATABASE": clean_db.database_name,
            "SMOKE_AUTHORITY_B64": base64.b64encode(authority).decode(),
            "SMOKE_PORT": str(remote_port),
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_for_port(remote_port, server, timeout=300)
        with urllib_request.urlopen(
            f"http://127.0.0.1:{remote_port}/v2/capabilities",
            timeout=30,
        ) as response:
            advertisement = response.read()

        remote_requests: list[bytes] = []

        async def exchange(request: bytes) -> bytes:
            remote_requests.append(request)

            def post() -> bytes:
                http_request = urllib_request.Request(
                    f"http://127.0.0.1:{remote_port}/v2/query",
                    data=request,
                    headers={"content-type": "application/json"},
                    method="POST",
                )
                with urllib_request.urlopen(http_request, timeout=30) as response:
                    return response.read()

            return await asyncio.to_thread(post)

        remote_session = generated.RemoteQuerySession(
            advertisement,
            exchange,
            generated.RemoteQueryLimits(
                max_items=10,
                max_bytes=1 << 20,
                max_collection_members=10,
                max_graph_nodes=30,
                max_attribute_values=1_000,
                max_role_players=30,
                deadline_ms=30_000,
            ),
        )
        remote_person = remote_session.exact(generated.Person)
        remote_people = remote_session.query(remote_person).where(
            remote_person.field(generated.Person.identifier).eq(generated.Identifier("person-1"))
        )
        remote_person_one = asyncio.run(remote_people.one())
        assert type(remote_person_one) is generated.Person
        assert remote_person_one.iid == person.iid
        remote_person_first = asyncio.run(
            remote_people.first(order_by=(remote_person.field(generated.Person.identifier).asc(),))
        )
        assert type(remote_person_first) is generated.Person
        assert remote_person_first.iid == person.iid
        remote_present_aliases = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.field(generated.Person.aliases).is_present())
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_present_aliases] == [person.iid]
        remote_missing_aliases = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.field(generated.Person.aliases).is_missing())
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert {candidate.iid for candidate in remote_missing_aliases} == {
            *batch_iids,
            transaction_person.iid,
        }
        assert (
            asyncio.run(
                remote_session.query(remote_person).where(remote_person.iid(person.iid)).one()
            ).iid
            == person.iid
        )
        remote_iid_set = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.iid_in((person.iid, batch_people[0].iid)))
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_iid_set] == [
            person.iid,
            batch_people[0].iid,
        ]
        remote_network = remote_session.exact(generated.NetworkLink)
        remote_network_one = asyncio.run(
            remote_session.query(remote_network)
            .where(
                remote_network.iid(network_iid),
                remote_network.field(generated.NetworkLink.nickname).is_present(),
            )
            .one()
        )
        assert type(remote_network_one) is generated.NetworkLink
        assert remote_network_one.iid == network_iid
        remote_person_rows = asyncio.run(
            remote_people.rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_person_rows] == [person.iid]
        remote_person_page = asyncio.run(
            remote_people.page_by(
                remote_person,
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert [candidate.iid for candidate in remote_person_page.items] == [person.iid]
        assert remote_person_page.total == 1
        assert asyncio.run(remote_people.count_by(remote_person)) == 1
        assert asyncio.run(remote_people.exists_by(remote_person)) is True

        remote_party = remote_session.subtypes(generated.Party)
        remote_party_rows = asyncio.run(
            remote_session.query(remote_party).rows(
                limit=10,
                order_by=(remote_party.field(generated.Party.identifier).asc(),),
            )
        )
        assert [type(candidate) for candidate in remote_party_rows] == [
            generated.Employee,
            generated.Manager,
        ]
        assert [candidate.iid for candidate in remote_party_rows] == [employee.iid, manager.iid]

        remote_all_people = remote_session.query(remote_person)
        remote_score_field = remote_person.field(generated.Person.score)
        requests_before_reduction = len(remote_requests)
        with pytest.raises(Exception, match="query_remote_v2_native_only_operation"):
            asyncio.run(
                remote_all_people.aggregate(
                    remote_person,
                    generated.aggregate.count(),
                    generated.aggregate.sum(remote_score_field),
                    generated.aggregate.min(remote_score_field),
                    generated.aggregate.max(remote_score_field),
                    generated.aggregate.mean(remote_score_field),
                    generated.aggregate.median(remote_score_field),
                    generated.aggregate.std(remote_score_field),
                )
            )
        assert len(remote_requests) == requests_before_reduction

        remote_employment = remote_session.exact(generated.Employment)
        remote_person_employment = remote_session.query(
            remote_person,
            remote_employment,
        ).where(remote_employment.role(generated.Employment.employee).connects(remote_person))
        remote_person_value, remote_employment_value = asyncio.run(remote_person_employment.one())
        assert type(remote_person_value) is generated.Person
        assert remote_person_value.iid == person.iid
        assert type(remote_employment_value) is generated.Employment
        assert remote_employment_value.iid == employment.iid
        requests_before_grouped_reduction = len(remote_requests)
        with pytest.raises(Exception, match="query_remote_v2_native_only_operation"):
            asyncio.run(
                remote_person_employment.group_by(remote_person, remote_employment).aggregate(
                    generated.aggregate.count(),
                    generated.aggregate.sum(remote_score_field),
                )
            )
        assert len(remote_requests) == requests_before_grouped_reduction

        remote_network = remote_session.exact(generated.NetworkLink)
        remote_participant = remote_session.exact(generated.Person)
        remote_network_page = asyncio.run(
            remote_session.query_as(
                network_row,
                network=remote_network,
                participants=remote_participant.collect()
                .distinct()
                .order_by(remote_participant.field(generated.Person.identifier).asc()),
            )
            .where(
                remote_network.role(generated.NetworkLink.participant).connects(remote_participant)
            )
            .page_by(
                remote_network,
                limit=10,
                order_by=(remote_network.field(generated.NetworkLink.identifier).asc(),),
                include_total=True,
            )
        )
        assert remote_network_page.total == network_page.total
        assert len(remote_network_page.items) == len(network_page.items)
        remote_named_network = remote_network_page.items[0]
        assert type(remote_named_network) is network_row
        assert type(remote_named_network.network) is generated.NetworkLink
        assert remote_named_network.network.iid == named_network.network.iid
        assert tuple(value.iid for value in remote_named_network.participants) == tuple(
            value.iid for value in named_network.participants
        )
        assert all(type(value) is generated.Person for value in remote_named_network.participants)

        remote_source = remote_session.exact(generated.Person)
        remote_target = remote_session.exact(generated.Person)
        remote_reachable = remote_session.reachable(
            remote_source,
            remote_target,
            generated.NetworkLink,
            generated.NetworkLink.origin,
            generated.NetworkLink.destination,
            min_depth=1,
            max_depth=1,
        )
        remote_reachable_pair = asyncio.run(
            remote_session.query(remote_source, remote_target)
            .where(
                remote_reachable,
                remote_source.field(generated.Person.identifier).eq(
                    generated.Identifier("person-1")
                ),
                remote_target.field(generated.Person.identifier).eq(
                    generated.Identifier("person-2")
                ),
            )
            .one()
        )
        assert tuple(candidate.iid for candidate in remote_reachable_pair) == tuple(
            candidate.iid for candidate in reachable_pair
        )
        remote_cross_left = remote_session.exact(generated.Person)
        remote_cross_right = remote_session.exact(generated.Person)
        remote_cross_pair = asyncio.run(
            remote_session.query(remote_cross_left, remote_cross_right)
            .allow_cross_join(remote_cross_left, remote_cross_right)
            .where(
                remote_cross_left.field(generated.Person.identifier).eq(
                    generated.Identifier("person-1")
                ),
                remote_cross_right.field(generated.Person.identifier).eq(
                    generated.Identifier("person-2")
                ),
            )
            .one()
        )
        assert tuple(candidate.iid for candidate in remote_cross_pair) == tuple(
            candidate.iid for candidate in cross_pair
        )
    finally:
        server.terminate()
        try:
            server.wait(timeout=30)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=30)

    relation_batch = [
        generated.NetworkLink(
            identifier=generated.Identifier("network-2"),
            origin=batch_people[0],
            destination=batch_people[1],
            participant=[batch_people[0], batch_people[1]],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("network-3"),
            origin=batch_people[1],
            destination=person,
            participant=[batch_people[1], person],
        ),
    ]
    assert network_manager.insert_many(relation_batch) == relation_batch
    relation_batch_iids = [candidate.iid for candidate in relation_batch]
    assert all(relation_batch_iids)
    assert [candidate.iid for candidate in network_manager.put_many(relation_batch)] == (
        relation_batch_iids
    )
    for relation in relation_batch:
        network_manager.delete(relation)
        assert network_manager.get_by_iid(relation.iid) is None

    membership_manager.delete(membership)
    assert membership_manager.get_by_iid(membership.iid) is None
    network_manager.delete(network)
    assert network_manager.get_by_iid(network.iid) is None
    person_manager.delete(transaction_person)
    assert person_manager.get_by_iid(transaction_person.iid) is None
