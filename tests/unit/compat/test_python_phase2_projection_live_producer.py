"""Fail-closed tests for Python's exact-3.12.3 Phase-2 live producer."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
PRODUCER_PATH = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/phase2_live_check.py"
)


def _load_producer() -> ModuleType:
    spec = importlib.util.spec_from_file_location("python_phase2_live_producer", PRODUCER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


producer = _load_producer()


class _FakeDatabase:
    def __init__(
        self,
        *,
        address: str,
        database: str,
        http_port: int,
        events: list[str],
        preexisting: bool = False,
        version: str | None = "3.12.3",
        schema_failure: bool = False,
        delete_failure: bool = False,
    ) -> None:
        self.address = address
        self.database = database
        self.http_port = http_port
        self.events = events
        self.exists = preexisting
        self.version = version
        self.schema_failure = schema_failure
        self.delete_failure = delete_failure

    def connect(self) -> None:
        self.events.append("connect")

    def detected_server_version(self) -> str | None:
        self.events.append("version")
        return self.version

    def database_exists(self) -> bool:
        self.events.append("exists")
        return self.exists

    def create_database(self) -> None:
        self.events.append("create")
        self.exists = True

    def execute_query(self, query: str, *, transaction_type: str) -> list[object]:
        assert query == "define entity person;"
        assert transaction_type == "schema"
        self.events.append("schema")
        if self.schema_failure:
            raise RuntimeError("injected schema failure")
        return []

    def delete_database(self) -> None:
        self.events.append("delete")
        if self.delete_failure:
            raise RuntimeError("injected delete failure")
        self.exists = False

    def close(self) -> None:
        self.events.append("close")


def _factory(
    events: list[str],
    *,
    preexisting: bool = False,
    version: str | None = "3.12.3",
    schema_failure: bool = False,
    delete_failure: bool = False,
) -> Any:
    return lambda **kwargs: _FakeDatabase(
        **kwargs,
        events=events,
        preexisting=preexisting,
        version=version,
        schema_failure=schema_failure,
        delete_failure=delete_failure,
    )


def _journey_result(*_arguments: object) -> dict[str, object]:
    return {"observed": True}


def test_report_is_canonical_bounded_and_create_new(tmp_path: Path) -> None:
    report = {"binding": "python", "observations": {"value": 38}}
    destination = tmp_path / "python.json"

    producer._publish_report(destination, report)

    assert destination.read_bytes() == producer._canonical(report)
    with pytest.raises(producer.ProducerError) as exists:
        producer._publish_report(destination, report)
    assert exists.value.code == "output_exists"
    with pytest.raises(producer.ProducerError) as bounded:
        producer._publish_report(
            tmp_path / "oversized.json",
            {"value": "x" * producer.MAX_REPORT_BYTES},
        )
    assert bounded.value.code == "report_size_limit"


def test_report_envelope_and_observation_ledger_are_exact() -> None:
    observations = {name: {} for name in producer.OBSERVATION_REFS}

    report = producer._build_report(
        {
            name: {"path": producer.SOURCE_PATHS[name], "sha256": digest}
            for name, digest in producer.SOURCE_SHA256.items()
        },
        observations,
    )

    assert report == {
        "authority": {
            name: {"path": producer.SOURCE_PATHS[name], "sha256": digest}
            for name, digest in producer.SOURCE_SHA256.items()
        },
        "binding": "python",
        "format": "typebridge.phase2-projected-live-report/v1",
        "observations": observations,
        "semantic_profile": "typedb-3.12.1/v1",
    }
    assert set(observations) == {
        "canonical_scalar_values",
        "cleanup",
        "inherited_plain_activity_role_lifecycle",
        "integer_key_polymorphic_optional_role",
        "relation_as_player",
    }


def test_generated_package_with_foreign_semantic_digest_is_rejected(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    package_directory = tmp_path / "generated_phase2"
    package_directory.mkdir()
    initializer = package_directory / "__init__.py"
    initializer.write_text("", encoding="utf-8")
    projection = object()

    def model(name: str) -> type:
        return type(name, (), {"__runtime_projection__": projection})

    semantic = dict(producer.SEMANTIC_SCHEMA_FINGERPRINT)
    semantic["digest"] = "a98b9c10a2942f263eb723a4e532cced5221d0e0ecaf3296ee4017412e8fa5b0"
    package = SimpleNamespace(
        __file__=str(initializer),
        SEMANTIC_SCHEMA_FINGERPRINT_JSON=json.dumps(semantic),
        Container=model("Container"),
        Event=model("Event"),
        Interaction=model("Interaction"),
        Person=model("Person"),
        PlainActivity=model("PlainActivity"),
        Robot=model("Robot"),
    )
    monkeypatch.setattr(producer.importlib, "import_module", lambda _name: package)

    with pytest.raises(producer.ProducerError) as rejection:
        producer._load_package(tmp_path)

    assert rejection.value.code == "generated_package_identity"


def test_preexisting_database_is_never_deleted() -> None:
    events: list[str] = []

    with pytest.raises(producer.ProducerError) as rejection:
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "already_present",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=_factory(events, preexisting=True),
            journey_runner=_journey_result,
        )

    assert rejection.value.code == "database_preexisting"
    assert events == ["connect", "version", "exists", "close"]


def test_create_failure_is_never_claimed_or_deleted() -> None:
    events: list[str] = []

    class CreateFailure(_FakeDatabase):
        def create_database(self) -> None:
            self.events.append("create")
            raise RuntimeError("create failed")

    def factory(**arguments: object) -> CreateFailure:
        return CreateFailure(**arguments, events=events)

    with pytest.raises(RuntimeError, match="create failed"):
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "raced_database",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=factory,
            journey_runner=_journey_result,
        )

    assert events == ["connect", "version", "exists", "create", "close"]


def test_wrong_server_version_fails_before_database_administration() -> None:
    events: list[str] = []

    with pytest.raises(producer.ProducerError) as rejection:
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "phase2_live",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=_factory(events, version="3.12.0"),
            journey_runner=_journey_result,
        )

    assert rejection.value.code == "server_version_mismatch"
    assert events == ["connect", "version", "close"]


def test_journey_failure_still_deletes_owned_database_and_closes() -> None:
    events: list[str] = []

    def fail(*_arguments: object) -> dict[str, object]:
        events.append("journey")
        raise RuntimeError("injected live failure")

    with pytest.raises(RuntimeError, match="injected live failure"):
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "phase2_live",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=_factory(events),
            journey_runner=fail,
        )

    assert events == [
        "connect",
        "version",
        "exists",
        "create",
        "exists",
        "schema",
        "journey",
        "delete",
        "exists",
        "close",
    ]


def test_schema_failure_still_deletes_owned_database_and_closes() -> None:
    events: list[str] = []

    with pytest.raises(RuntimeError, match="injected schema failure"):
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "phase2_live",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=_factory(events, schema_failure=True),
            journey_runner=_journey_result,
        )

    assert events == [
        "connect",
        "version",
        "exists",
        "create",
        "exists",
        "schema",
        "delete",
        "exists",
        "close",
    ]


def test_delete_failure_takes_precedence_but_still_closes() -> None:
    events: list[str] = []

    def fail(*_arguments: object) -> dict[str, object]:
        events.append("journey")
        raise RuntimeError("injected journey failure")

    with pytest.raises(RuntimeError, match="injected delete failure") as rejection:
        producer._run_owned_database(
            object(),
            "127.0.0.1:1729",
            "phase2_live",
            32943,
            "define entity person;",
            {},
            [],
            [],
            database_factory=_factory(events, delete_failure=True),
            journey_runner=fail,
        )

    assert rejection.value.__context__ is not None
    assert str(rejection.value.__context__) == "injected journey failure"
    assert events == [
        "connect",
        "version",
        "exists",
        "create",
        "exists",
        "schema",
        "journey",
        "delete",
        "close",
    ]


def test_success_returns_only_after_database_teardown() -> None:
    events: list[str] = []

    result = producer._run_owned_database(
        object(),
        "127.0.0.1:1729",
        "phase2_live",
        32943,
        "define entity person;",
        {},
        [],
        [],
        database_factory=_factory(events),
        journey_runner=_journey_result,
    )

    assert result == {"observed": True}
    assert events[-3:] == ["delete", "exists", "close"]


@pytest.mark.parametrize("failure_code", ["invalid_hydration", "cleanup_failed"])
def test_main_never_publishes_after_journey_or_cleanup_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure_code: str,
) -> None:
    destination = tmp_path / "python-live.json"
    environment = {
        producer.OUTPUT_ENV: str(destination),
        producer.ADDRESS_ENV: "127.0.0.1:1729",
        producer.DATABASE_ENV: "phase2_live",
        producer.HTTP_PORT_ENV: "32943",
        producer.PACKAGE_ROOT_ENV: str(tmp_path),
        producer.REPOSITORY_ROOT_ENV: str(ROOT),
    }
    authority = {
        name: {"path": producer.SOURCE_PATHS[name], "sha256": digest}
        for name, digest in producer.SOURCE_SHA256.items()
    }
    monkeypatch.setattr(
        producer,
        "_source_authority",
        lambda _root: (authority, {"journey": b"{}", "provider": b"define entity person;"}),
    )
    monkeypatch.setattr(producer, "_load_package", lambda _root: object())
    monkeypatch.setattr(producer, "_json_object", lambda _raw, _label: {})
    monkeypatch.setattr(producer, "_record_index", lambda _journey: ({}, [], []))

    def fail(*_arguments: object) -> dict[str, object]:
        raise producer.ProducerError(failure_code, "injected live failure")

    monkeypatch.setattr(producer, "_run_owned_database", fail)

    assert producer.main(environment) == 1
    assert not destination.exists()


def test_http_port_is_bounded_and_forwarded() -> None:
    for value, expected in (("1", 1), ("32943", 32943), ("65535", 65535)):
        assert producer._http_port({producer.HTTP_PORT_ENV: value}) == expected

    for value in ("0", "65536", "-1", "+1", " 32943", "32943 ", "３２９４３"):
        with pytest.raises(producer.ProducerError) as rejection:
            producer._http_port({producer.HTTP_PORT_ENV: value})
        assert rejection.value.code == "invalid_http_port"

    with pytest.raises(producer.ProducerError) as missing:
        producer._http_port({})
    assert missing.value.code == "missing_environment"

    events: list[str] = []
    constructed: list[dict[str, object]] = []

    def factory(**arguments: object) -> _FakeDatabase:
        constructed.append(arguments)
        return _FakeDatabase(**arguments, events=events)

    producer._run_owned_database(
        object(),
        "127.0.0.1:32942",
        "phase2_live",
        32943,
        "define entity person;",
        {},
        [],
        [],
        database_factory=factory,
        journey_runner=_journey_result,
    )

    assert constructed == [
        {"address": "127.0.0.1:32942", "database": "phase2_live", "http_port": 32943}
    ]


def test_source_uses_only_schema_raw_query_and_no_committed_result_seed() -> None:
    source = PRODUCER_PATH.read_text()

    for forbidden in (
        "compare_phase2_projection_live",
        "compare_phase2_projection_parity",
        "expected_observations",
        "expected_report",
        "load_contract",
    ):
        assert forbidden not in source
    assert source.count("execute_query(") == 1
    assert 'transaction_type="schema"' in source
    assert source.index("database.create_database()") < source.index("owns_database = True")
    assert "TYPE_BRIDGE_PHASE2_LIVE_REPORT" in source
    assert "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS" in source
    assert "TYPE_BRIDGE_PHASE2_LIVE_DATABASE" in source
    assert "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT" in source
    assert '"address":' not in source
    assert '"database":' not in source
