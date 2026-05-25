from __future__ import annotations

# pyright: reportMissingImports=false
from typing import Any, cast

import pytest

from type_bridge import Card, Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.crud.hooks import HookCancelled
from type_bridge.crud.rust_manager import RustTypeDBManager
from type_bridge.expressions import AggregateExpr
from type_bridge.session import Database


class RustManagerName(String):
    pass


class RustManagerAge(Integer):
    pass


class RustManagerScore(Integer):
    pass


class RustManagerPerson(Entity):
    flags = TypeFlags(name="rust-manager-person")

    name: RustManagerName = Flag(Key)


class RustManagerAgedPerson(Entity):
    flags = TypeFlags(name="rust-manager-aged-person")

    name: RustManagerName = Flag(Key)
    age: RustManagerAge
    scores: list[RustManagerScore] = Flag(Card(1, 4))


class RustManagerCompany(Entity):
    flags = TypeFlags(name="rust-manager-company")

    name: RustManagerName = Flag(Key)


class RustManagerPosition(String):
    pass


class RustManagerEmployment(Relation):
    flags = TypeFlags(name="rust-manager-employment")

    employee: Role[RustManagerPerson] = Role("employee", RustManagerPerson)
    employer: Role[RustManagerCompany] = Role("employer", RustManagerCompany)
    reviewer: Role[RustManagerPerson] = Role("reviewer", RustManagerPerson, cardinality=Card(1))
    position: RustManagerPosition


class RecordingHook:
    def __init__(self) -> None:
        self.calls: list[tuple[str, type[Any], Any]] = []

    def pre_insert(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("pre_insert", sender, instance))

    def post_insert(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("post_insert", sender, instance))

    def pre_update(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("pre_update", sender, instance))

    def post_update(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("post_update", sender, instance))

    def pre_delete(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("pre_delete", sender, instance))

    def post_delete(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("post_delete", sender, instance))

    def pre_put(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("pre_put", sender, instance))

    def post_put(self, sender: type[Any], instance: Any) -> None:
        self.calls.append(("post_put", sender, instance))


class FakeRustEntityManager:
    def __init__(self) -> None:
        self.inserted: dict[str, Any] | None = None
        self.insert_many_attributes: list[dict[str, Any]] | None = None
        self.put_attributes: dict[str, Any] | None = None
        self.put_many_attributes: list[dict[str, Any]] | None = None
        self.updated: tuple[dict[str, Any], str | None] | None = None
        self.update_calls: list[tuple[dict[str, Any], str | None]] = []
        self.deleted: str | None = None
        self.aggregate_call: tuple[list[dict[str, Any]], dict[str, Any]] | None = None
        self.group_by_call: tuple[list[str], list[dict[str, Any]], dict[str, Any]] | None = None

    def insert(self, attributes: dict[str, Any]) -> str:
        self.inserted = attributes
        return "0xabc"

    def insert_many(self, attributes: list[dict[str, Any]]) -> list[str]:
        self.insert_many_attributes = attributes
        return [f"0xinsert{i}" for i in range(len(attributes))]

    def put(self, attributes: dict[str, Any]) -> str:
        self.put_attributes = attributes
        return "0xput"

    def put_many(self, attributes: list[dict[str, Any]]) -> list[str]:
        self.put_many_attributes = attributes
        return [f"0xput{i}" for i in range(len(attributes))]

    def update(self, attributes: dict[str, Any], iid: str | None = None) -> None:
        self.updated = (attributes, iid)
        self.update_calls.append((attributes, iid))

    def get(self, filters: dict[str, Any]) -> list[dict[str, Any]]:
        return [{"name": "Alice", "_iid": "0xabc", "_type": "rust-manager-person"}]

    def get_by_iid(self, iid: str) -> dict[str, Any] | None:
        if iid == "0xmissing":
            return None
        return {"name": "Alice", "_iid": iid, "_type": "rust-manager-person"}

    def all(self) -> list[dict[str, Any]]:
        return self.get({})

    def count(self, filters: dict[str, Any]) -> int:
        return 1

    def aggregate(
        self,
        aggregates: list[dict[str, Any]],
        filters: dict[str, Any],
    ) -> list[dict[str, Any]]:
        self.aggregate_call = (aggregates, filters)
        return [{"$count": {"value": 2}, "$avg_age": {"value": 31.5}}]

    def group_by_aggregate(
        self,
        group_fields: list[str],
        aggregates: list[dict[str, Any]],
        filters: dict[str, Any],
    ) -> list[dict[str, Any]]:
        self.group_by_call = (group_fields, aggregates, filters)
        return [
            {"$group0": {"value": "Alice"}, "$count": {"value": 1}},
            {"$group0": {"value": "Bob"}, "$count": {"value": 1}},
        ]

    def delete_by_iid(self, iid: str) -> None:
        self.deleted = iid


class FakeRustTransaction:
    def __init__(self) -> None:
        self.executed: list[str] = []
        self.committed = False
        self.rolled_back = False
        self.closed = False

    def execute(self, query: str) -> list[dict[str, Any]]:
        self.executed.append(query)
        return []

    def commit(self) -> None:
        self.committed = True

    def rollback(self) -> None:
        self.rolled_back = True

    def close(self) -> None:
        self.closed = True


class FakeRustDatabase:
    def __init__(self) -> None:
        self.tx = FakeRustTransaction()
        self.tx_type: str | None = None

    def transaction(self, transaction_type: str) -> FakeRustTransaction:
        self.tx_type = transaction_type
        return self.tx


class FakeRustRelationManager:
    def __init__(self) -> None:
        self.inserted: tuple[dict[str, Any], list[dict[str, Any]]] | None = None
        self.insert_many_items: list[dict[str, Any]] | None = None
        self.put_relation: tuple[dict[str, Any], list[dict[str, Any]]] | None = None
        self.put_many_items: list[dict[str, Any]] | None = None
        self.updated: tuple[dict[str, Any], list[dict[str, Any]], str | None] | None = None
        self.update_calls: list[tuple[dict[str, Any], list[dict[str, Any]], str | None]] = []
        self.deleted: str | None = None
        self.aggregate_call: tuple[list[dict[str, Any]], dict[str, Any]] | None = None
        self.group_by_call: tuple[list[str], list[dict[str, Any]], dict[str, Any]] | None = None

    def get(self, filters: dict[str, Any]) -> list[dict[str, Any]]:
        return [
            {
                "_iid": "0xrel",
                "_type": "rust-manager-employment",
                RustManagerPosition.get_attribute_name(): "Engineer",
                "role_players": [
                    {
                        "role_name": "employee",
                        "player_iid": "0xalice",
                        "player_type_name": "rust-manager-person",
                        "attributes": {RustManagerName.get_attribute_name(): "Alice"},
                    },
                    {
                        "role_name": "employer",
                        "player_iid": "0xacme",
                        "player_type_name": "rust-manager-company",
                        "attributes": {RustManagerName.get_attribute_name(): "Acme"},
                    },
                    {
                        "role_name": "reviewer",
                        "player_iid": "0xbob",
                        "player_type_name": "rust-manager-person",
                        "attributes": {RustManagerName.get_attribute_name(): "Bob"},
                    },
                    {
                        "role_name": "reviewer",
                        "player_iid": "0xcarol",
                        "player_type_name": "rust-manager-person",
                        "attributes": {RustManagerName.get_attribute_name(): "Carol"},
                    },
                ],
            }
        ]

    def get_by_iid(self, iid: str) -> dict[str, Any] | None:
        if iid == "0xmissing":
            return None
        row = self.get({})[0]
        row["_iid"] = iid
        return row

    def all(self) -> list[dict[str, Any]]:
        return self.get({})

    def count(self, filters: dict[str, Any]) -> int:
        return 1

    def aggregate(
        self,
        aggregates: list[dict[str, Any]],
        filters: dict[str, Any],
    ) -> list[dict[str, Any]]:
        self.aggregate_call = (aggregates, filters)
        return [{"$count": {"value": 2}}]

    def group_by_aggregate(
        self,
        group_fields: list[str],
        aggregates: list[dict[str, Any]],
        filters: dict[str, Any],
    ) -> list[dict[str, Any]]:
        self.group_by_call = (group_fields, aggregates, filters)
        return [{"$group0": {"value": "Engineer"}, "$count": {"value": 2}}]

    def insert(self, attributes: dict[str, Any], role_players: list[dict[str, Any]]) -> str:
        self.inserted = (attributes, role_players)
        return "0xrel"

    def insert_many(self, items: list[dict[str, Any]]) -> list[str]:
        self.insert_many_items = items
        return [f"0xrelinsert{i}" for i in range(len(items))]

    def put(self, attributes: dict[str, Any], role_players: list[dict[str, Any]]) -> str:
        self.put_relation = (attributes, role_players)
        return "0xputrel"

    def put_many(self, items: list[dict[str, Any]]) -> list[str]:
        self.put_many_items = items
        return [f"0xrelput{i}" for i in range(len(items))]

    def update(
        self,
        attributes: dict[str, Any],
        role_players: list[dict[str, Any]],
        iid: str | None = None,
    ) -> None:
        self.updated = (attributes, role_players, iid)
        self.update_calls.append((attributes, role_players, iid))

    def delete_by_iid(self, iid: str) -> None:
        self.deleted = iid
        return None


def test_backend_iid_setter_uses_private_storage() -> None:
    person = RustManagerPerson(name=RustManagerName("Alice"))

    person._set_backend_iid("0xabc")

    assert person._iid == "0xabc"
    assert person.model_dump() == {"name": "Alice"}


def test_rust_manager_marshals_entity_insert_get_count_delete(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerPerson)
    person = RustManagerPerson(name=RustManagerName("Alice"))

    inserted = manager.insert(person)
    fetched = manager.get(name=RustManagerName("Alice"))
    by_iid = manager.get_by_iid("0xabc")
    count = manager.count(name="Alice")
    manager.delete(person)
    put_person = manager.put(RustManagerPerson(name=RustManagerName("Bob")))
    manager.update(person)

    assert inserted is person
    assert person._iid == "0xabc"
    assert fake.inserted == {"name": "Alice"}
    assert put_person._iid == "0xput"
    assert fake.put_attributes == {"name": "Bob"}
    assert fake.updated == ({"name": "Alice"}, "0xabc")
    assert fetched[0].name == RustManagerName("Alice")
    assert fetched[0]._iid == "0xabc"
    assert by_iid is not None
    assert by_iid.name == RustManagerName("Alice")
    assert by_iid._iid == "0xabc"
    assert manager.get_by_iid("0xmissing") is None
    assert manager.get_by_iid("not-an-iid") is None
    assert count == 1
    assert fake.deleted == "0xabc"


def test_rust_manager_marshals_entity_batch_and_exact_filter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerPerson)
    people = [
        RustManagerPerson(name=RustManagerName("Alice")),
        RustManagerPerson(name=RustManagerName("Bob")),
    ]
    put_people = [
        RustManagerPerson(name=RustManagerName("Carol")),
        RustManagerPerson(name=RustManagerName("Diana")),
    ]

    inserted = manager.insert_many(people)
    put_result = manager.put_many(put_people)
    people[0].name = RustManagerName("Alice Updated")
    people[1].name = RustManagerName("Bob Updated")
    update_result = manager.update_many(people)
    query = manager.filter(name=RustManagerName("Alice")).filter()
    filtered = query.offset(0).limit(1).execute()
    first = manager.filter(name="Alice").first()
    count = manager.filter(name="Alice").count()

    assert inserted is people
    assert [person._iid for person in people] == ["0xinsert0", "0xinsert1"]
    assert fake.insert_many_attributes == [{"name": "Alice"}, {"name": "Bob"}]
    assert put_result is put_people
    assert [person._iid for person in put_people] == ["0xput0", "0xput1"]
    assert fake.put_many_attributes == [{"name": "Carol"}, {"name": "Diana"}]
    assert update_result is people
    assert fake.update_calls == [
        ({"name": "Alice Updated"}, "0xinsert0"),
        ({"name": "Bob Updated"}, "0xinsert1"),
    ]
    assert filtered[0].name == RustManagerName("Alice")
    assert first is not None
    assert first.name == RustManagerName("Alice")
    assert count == 1
    assert query.exists()


def test_rust_manager_rejects_lookup_filters(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerPerson)

    with pytest.raises(NotImplementedError, match="lookup filters"):
        manager.filter(name__startswith="A")


def test_rust_manager_chainable_delete_and_update_with(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerPerson)
    deleted_count = manager.filter(name="Alice").delete()
    updated = manager.filter(name="Alice").update_with(
        lambda person: setattr(person, "name", RustManagerName("Updated"))
    )

    assert deleted_count == 1
    assert fake.deleted == "0xabc"
    assert len(updated) == 1
    assert updated[0].name == RustManagerName("Updated")
    assert fake.updated == ({"name": "Updated"}, "0xabc")


def test_rust_manager_marshals_entity_aggregates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerAgedPerson)
    aggregate = manager.filter(name="Alice").aggregate(RustManagerAgedPerson.age.avg())
    scores = cast(Any, RustManagerAgedPerson.scores)
    multi_value_aggregate = manager.filter(name="Alice").aggregate(scores.sum())
    grouped = manager.group_by(RustManagerAgedPerson.name).aggregate(
        AggregateExpr(attr_type=None, function="count")
    )

    assert aggregate == {"count": 2, "avg_age": 31.5}
    assert multi_value_aggregate == {"count": 2, "avg_age": 31.5}
    assert fake.aggregate_call == (
        [
            {
                "result_key": "sum_scores",
                "function": "sum",
                "attr_name": "RustManagerScore",
            }
        ],
        {"name": "Alice"},
    )
    assert grouped == {"Alice": {"count": 1}, "Bob": {"count": 1}}
    assert fake.group_by_call == (
        ["RustManagerName"],
        [{"result_key": "count", "function": "count", "attr_name": None}],
        {},
    )


def test_rust_manager_runs_entity_hooks_before_marshalling(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    class MutatingHook(RecordingHook):
        def pre_insert(self, sender: type[Any], instance: Any) -> None:
            super().pre_insert(sender, instance)
            instance.name = RustManagerName("Hooked")

    hook = MutatingHook()
    manager = RustTypeDBManager(Database(), RustManagerPerson).add_hook(hook)
    person = RustManagerPerson(name=RustManagerName("Alice"))

    inserted = manager.insert(person)

    assert inserted is person
    assert fake.inserted == {"name": "Hooked"}
    assert hook.calls == [
        ("pre_insert", RustManagerPerson, person),
        ("post_insert", RustManagerPerson, person),
    ]


def test_rust_manager_batch_hook_cancellation_prevents_write(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    class CancelSecondHook(RecordingHook):
        def pre_insert(self, sender: type[Any], instance: Any) -> None:
            super().pre_insert(sender, instance)
            if instance.name == RustManagerName("Bob"):
                raise HookCancelled("stop batch")

    hook = CancelSecondHook()
    manager = RustTypeDBManager(Database(), RustManagerPerson).add_hook(hook)
    people = [
        RustManagerPerson(name=RustManagerName("Alice")),
        RustManagerPerson(name=RustManagerName("Bob")),
    ]

    with pytest.raises(HookCancelled, match="stop batch"):
        manager.insert_many(people)

    assert fake.insert_many_attributes is None
    assert [call[0] for call in hook.calls] == ["pre_insert", "pre_insert"]
    assert [person._iid for person in people] == [None, None]


def test_transaction_context_uses_rust_transaction_adapter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_db = FakeRustDatabase()
    fake_manager = FakeRustEntityManager()
    seen_connection: dict[str, Any] = {}
    monkeypatch.setenv("TYPE_BRIDGE_BACKEND", "rust")
    monkeypatch.setattr(
        "type_bridge._rust_runtime.rust_database_for",
        lambda connection: fake_db,
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )

    def manager_for_entity(connection: Any, descriptor: dict[str, Any]) -> FakeRustEntityManager:
        del descriptor
        seen_connection["connection"] = connection
        seen_connection["rust_tx"] = connection._rust_tx
        return fake_manager

    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        manager_for_entity,
    )

    with Database().transaction("write") as tx:
        manager = cast(RustTypeDBManager[RustManagerPerson], tx.manager(RustManagerPerson))
        person = manager.insert(RustManagerPerson(name=RustManagerName("Alice")))

    assert fake_db.tx_type == "write"
    assert seen_connection["rust_tx"] is fake_db.tx
    assert person._iid == "0xabc"
    assert fake_db.tx.committed is True
    assert fake_db.tx.rolled_back is False
    assert fake_db.tx.closed is True


def test_transaction_context_rolls_back_rust_adapter_on_exception(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_db = FakeRustDatabase()
    monkeypatch.setenv("TYPE_BRIDGE_BACKEND", "rust")
    monkeypatch.setattr(
        "type_bridge._rust_runtime.rust_database_for",
        lambda connection: fake_db,
    )

    with pytest.raises(RuntimeError, match="boom"):
        with Database().transaction("write"):
            raise RuntimeError("boom")

    assert fake_db.tx.committed is False
    assert fake_db.tx.rolled_back is True
    assert fake_db.tx.closed is True


def test_rust_manager_hydrates_relation_role_players(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: FakeRustRelationManager(),
    )

    manager = RustTypeDBManager(Database(), RustManagerEmployment)

    relations = manager.all()
    by_iid = manager.get_by_iid("0x123")

    assert len(relations) == 1
    relation = relations[0]
    assert relation._iid == "0xrel"
    assert relation.position == RustManagerPosition("Engineer")
    assert relation.employee.name == RustManagerName("Alice")
    assert relation.employee._iid == "0xalice"
    assert relation.employer.name == RustManagerName("Acme")
    assert relation.employer._iid == "0xacme"
    reviewers = cast(list[RustManagerPerson], relation.reviewer)
    assert [reviewer.name for reviewer in reviewers] == [
        RustManagerName("Bob"),
        RustManagerName("Carol"),
    ]
    assert [reviewer._iid for reviewer in reviewers] == ["0xbob", "0xcarol"]
    assert by_iid is not None
    assert by_iid._iid == "0x123"
    assert by_iid.position == RustManagerPosition("Engineer")
    assert by_iid.employee._iid == "0xalice"


def test_rust_manager_marshals_relation_put_update_delete(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustRelationManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: fake,
    )
    manager = RustTypeDBManager(Database(), RustManagerEmployment)
    employee = RustManagerPerson(name=RustManagerName("Alice"))
    employee._set_backend_iid("0xalice")
    employer = RustManagerCompany(name=RustManagerName("Acme"))
    employer._set_backend_iid("0xacme")
    reviewer = RustManagerPerson(name=RustManagerName("Bob"))
    reviewer._set_backend_iid("0xbob")
    relation = RustManagerEmployment(
        employee=employee,
        employer=employer,
        reviewer=reviewer,
        position=RustManagerPosition("Engineer"),
    )

    inserted = manager.insert(relation)
    relation.position = RustManagerPosition("Staff Engineer")
    updated = manager.update(relation)
    put_relation = RustManagerEmployment(
        employee=employee,
        employer=employer,
        reviewer=reviewer,
        position=RustManagerPosition("Principal Engineer"),
    )
    put_result = manager.put(put_relation)
    manager.delete(relation)

    expected_players = [
        {
            "role_name": "employee",
            "player_type_name": "rust-manager-person",
            "iid": "0xalice",
        },
        {
            "role_name": "employer",
            "player_type_name": "rust-manager-company",
            "iid": "0xacme",
        },
        {
            "role_name": "reviewer",
            "player_type_name": "rust-manager-person",
            "iid": "0xbob",
        },
    ]
    assert inserted is relation
    assert updated is relation
    assert relation._iid == "0xrel"
    assert put_result is put_relation
    assert put_relation._iid == "0xputrel"
    assert fake.inserted == ({"position": "Engineer"}, expected_players)
    assert fake.updated == ({"position": "Staff Engineer"}, expected_players, "0xrel")
    assert fake.put_relation == ({"position": "Principal Engineer"}, expected_players)
    assert fake.deleted == "0xrel"


def test_rust_manager_runs_relation_hooks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustRelationManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: fake,
    )
    employee = RustManagerPerson(name=RustManagerName("Alice"))
    employee._set_backend_iid("0xalice")
    employer = RustManagerCompany(name=RustManagerName("Acme"))
    employer._set_backend_iid("0xacme")
    reviewer = RustManagerPerson(name=RustManagerName("Bob"))
    reviewer._set_backend_iid("0xbob")
    relation = RustManagerEmployment(
        employee=employee,
        employer=employer,
        reviewer=reviewer,
        position=RustManagerPosition("Engineer"),
    )
    hook = RecordingHook()
    manager = RustTypeDBManager(Database(), RustManagerEmployment).add_hook(hook)

    manager.insert(relation)
    relation.position = RustManagerPosition("Staff Engineer")
    manager.update(relation)
    manager.put(relation)
    manager.delete(relation)

    assert [call[0] for call in hook.calls] == [
        "pre_insert",
        "post_insert",
        "pre_update",
        "post_update",
        "pre_put",
        "post_put",
        "pre_delete",
        "post_delete",
    ]
    assert all(call[1] is RustManagerEmployment for call in hook.calls)
    assert all(call[2] is relation for call in hook.calls)


def test_rust_manager_marshals_relation_batch_methods(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustRelationManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: fake,
    )
    manager = RustTypeDBManager(Database(), RustManagerEmployment)
    employee = RustManagerPerson(name=RustManagerName("Alice"))
    employee._set_backend_iid("0xalice")
    employer = RustManagerCompany(name=RustManagerName("Acme"))
    employer._set_backend_iid("0xacme")
    reviewer = RustManagerPerson(name=RustManagerName("Bob"))
    reviewer._set_backend_iid("0xbob")
    relations = [
        RustManagerEmployment(
            employee=employee,
            employer=employer,
            reviewer=reviewer,
            position=RustManagerPosition("Engineer"),
        ),
        RustManagerEmployment(
            employee=employee,
            employer=employer,
            reviewer=reviewer,
            position=RustManagerPosition("Manager"),
        ),
    ]

    inserted = manager.insert_many(relations)
    put_result = manager.put_many(relations)
    update_result = manager.update_many(relations)

    assert inserted is relations
    assert put_result is relations
    assert update_result is relations
    assert [relation._iid for relation in relations] == ["0xrelput0", "0xrelput1"]
    assert fake.insert_many_items is not None
    assert fake.insert_many_items[0]["attributes"] == {"position": "Engineer"}
    assert fake.insert_many_items[1]["attributes"] == {"position": "Manager"}
    assert fake.put_many_items is not None
    assert fake.put_many_items[0]["role_players"][0]["iid"] == "0xalice"
    assert fake.update_calls[-2][0] == {"position": "Engineer"}
    assert fake.update_calls[-1][0] == {"position": "Manager"}


def test_rust_manager_marshals_relation_group_by_aggregates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustRelationManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerEmployment)
    aggregate = manager.filter(position="Engineer").aggregate(
        AggregateExpr(attr_type=None, function="count")
    )
    grouped = manager.group_by(RustManagerEmployment.position).aggregate(
        AggregateExpr(attr_type=None, function="count")
    )

    assert aggregate == {"count": 2}
    assert grouped == {"Engineer": {"count": 2}}
    assert fake.aggregate_call == (
        [{"result_key": "count", "function": "count", "attr_name": None}],
        {"position": "Engineer"},
    )
    assert fake.group_by_call == (
        ["RustManagerPosition"],
        [{"result_key": "count", "function": "count", "attr_name": None}],
        {},
    )


def test_rust_manager_reports_unsupported_methods(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_relation",
        lambda connection, descriptor: FakeRustRelationManager(),
    )
    manager = RustTypeDBManager(Database(), RustManagerEmployment)

    with pytest.raises(NotImplementedError, match="Phase 3"):
        manager.filter(object()).all()
