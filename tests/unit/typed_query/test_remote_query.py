"""Remote typed-query delegation, one-exchange, and pre-I/O parity."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Awaitable, Callable
from pathlib import Path

import pytest
from type_bridge_core import (
    MatchRequestError,
    QueryV2Authority,
    QueryV2Error,
    query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_rows,
    query_v2_remote_model_context,
)

from tests.unit.query.test_query_v2_remote_runtime import (
    _fingerprint as _remote_fingerprint,
)
from tests.unit.query.test_query_v2_remote_runtime import (
    _signed_reply as _remote_signed_reply,
)
from tests.unit.typed_query._support import diagnostic_session, invoke_untyped
from tests.utils.handwritten import AttributeFlags, Entity, Flag, Key, String, TypeFlags
from type_bridge.typed import (
    BoundVar,
    Page,
    Query,
    QueryOrder,
    RemoteQuery,
    RemoteQueryLimits,
    RemoteQuerySession,
)
from type_bridge.typed import _remote_terminal as remote_terminal
from type_bridge.typed import remote_session as remote_session_module
from type_bridge.typed._remote_terminal import _RemoteRuntime


class RemoteName(String):
    pass


class RemotePerson(Entity):
    flags = TypeFlags(name="typed-remote-person")
    name: RemoteName = Flag(Key)


class RemoteSmokeName(String):
    flags = AttributeFlags(name="smoke-name")


class RemoteSmokePerson(Entity):
    flags = TypeFlags(name="smoke-person")
    name: RemoteSmokeName = Flag(Key)


class _Pending:
    def __init__(self, request: bytes | bytearray = b"request") -> None:
        self.request = request
        self.request_calls = 0
        self.decode_calls = 0
        self.responses: list[bytes] = []

    def request_bytes(self) -> bytes | bytearray:
        self.request_calls += 1
        return self.request

    def decode_reply(self, response: bytes) -> object:
        self.decode_calls += 1
        self.responses.append(response)
        return object()


def _remote_query(
    exchange: object,
) -> tuple[
    Query[RemotePerson],
    RemoteQuery[RemotePerson],
    BoundVar[RemotePerson],
]:
    session = diagnostic_session()
    person = session.var(RemotePerson)
    direct = session.query(person)
    runtime = invoke_untyped(_RemoteRuntime, object(), exchange)
    remote = invoke_untyped(RemoteQuery._from_direct, direct, runtime)
    assert isinstance(remote, RemoteQuery)
    return direct, remote, person


async def _invoke_async_untyped(
    function: object,
    /,
    *args: object,
    **kwargs: object,
) -> object:
    pending = invoke_untyped(function, *args, **kwargs)
    if not isinstance(pending, Awaitable):
        raise TypeError("test boundary expected an awaitable")
    return await pending


def _failure(
    callback: Callable[[], object],
) -> tuple[type[BaseException], str, object, object, object, object]:
    try:
        callback()
    except BaseException as error:
        return (
            type(error),
            str(error),
            getattr(error, "category", None),
            getattr(error, "code", None),
            getattr(error, "path", None),
            getattr(error, "details", None),
        )
    raise AssertionError("callback unexpectedly succeeded")


def test_rows_snapshots_request_and_performs_exactly_one_exchange(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exchanges: list[bytes] = []
    pending = _Pending(bytearray(b"request"))

    async def exchange(request: bytes) -> bytes:
        assert type(request) is bytes
        exchanges.append(request)
        return b"response"

    _, remote, _ = _remote_query(exchange)
    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_rows",
        lambda *_args: pending,
    )
    monkeypatch.setattr(
        remote_terminal,
        "_materialize_rows",
        lambda *_args: ["hydrated"],
    )

    assert asyncio.run(remote.rows(limit=5)) == ["hydrated"]
    assert exchanges == [b"request"]
    assert pending.request_calls == 1
    assert pending.decode_calls == 1
    assert pending.responses == [b"response"]


def test_preparation_failure_makes_zero_exchange_calls(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exchanges = 0
    expected = RuntimeError("preparation failed")

    async def exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return b"unreachable"

    _, remote, _ = _remote_query(exchange)

    def fail_prepare(*_args: object) -> object:
        raise expected

    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_rows",
        fail_prepare,
    )
    with pytest.raises(RuntimeError) as raised:
        asyncio.run(remote.rows(limit=1))
    assert raised.value is expected
    assert exchanges == 0


def test_public_direct_and_remote_order_iterables_stop_at_first_excess_term() -> None:
    exchanges = 0

    async def exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return b"unreachable"

    direct, remote, person = _remote_query(exchange)
    order = person.field(RemoteName).asc()

    class InfiniteOrderProbe:
        def __init__(self) -> None:
            self.consumed = 0

        def __iter__(self) -> InfiniteOrderProbe:
            return self

        def __next__(self) -> QueryOrder:
            self.consumed += 1
            if self.consumed > 65:
                raise AssertionError("order iterable was consumed past the first excess term")
            return order

    direct_orders = InfiniteOrderProbe()
    direct_failure = _failure(
        lambda: direct.rows(limit=1, order_by=direct_orders),
    )
    remote_orders = InfiniteOrderProbe()
    remote_failure = _failure(
        lambda: asyncio.run(remote.rows(limit=1, order_by=remote_orders)),
    )

    assert remote_failure == direct_failure
    assert direct_failure[2:] == (
        "resource_limit",
        "structural_limit_exceeded",
        [{"kind": "operation"}],
        {
            "actual": {"kind": "unsigned", "value": 65},
            "limit": {"kind": "text", "value": "order_terms"},
            "maximum": {"kind": "unsigned", "value": 64},
        },
    )
    assert direct_orders.consumed == 65
    assert remote_orders.consumed == 65
    assert exchanges == 0


def test_callback_failure_is_not_retried_or_decoded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    attempts = 0
    expected = RuntimeError("transport failed")
    pending = _Pending()

    async def exchange(_request: bytes) -> bytes:
        nonlocal attempts
        attempts += 1
        raise expected

    _, remote, _ = _remote_query(exchange)
    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_rows",
        lambda *_args: pending,
    )
    with pytest.raises(RuntimeError) as raised:
        asyncio.run(remote.rows(limit=1))
    assert raised.value is expected
    assert attempts == 1
    assert pending.decode_calls == 0


def test_callback_requires_exact_bytes_before_native_decode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pending = _Pending()

    async def exchange(_request: bytes) -> bytearray:
        return bytearray(b"mutable")

    _, remote, _ = _remote_query(exchange)
    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_rows",
        lambda *_args: pending,
    )
    with pytest.raises(TypeError, match="must resolve to bytes"):
        asyncio.run(remote.rows(limit=1))
    assert pending.decode_calls == 0


def test_page_with_total_remains_one_exchange(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exchanges = 0
    pending = _Pending()

    async def exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return b"response"

    _, remote, person = _remote_query(exchange)

    def prepare(*arguments: object) -> _Pending:
        assert arguments[-1] is True
        return pending

    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_page",
        prepare,
    )
    monkeypatch.setattr(
        remote_terminal,
        "_materialize_page",
        lambda *_args: Page([], offset=0, limit=5, total=0),
    )

    page = asyncio.run(
        _invoke_async_untyped(
            remote.page_by,
            person,
            limit=5,
            include_total=True,
        )
    )
    assert isinstance(page, Page)
    assert exchanges == 1
    assert pending.decode_calls == 1


@pytest.mark.parametrize(
    ("direct_call", "remote_call"),
    [
        (
            lambda direct, _person: direct.rows(limit=0),
            lambda remote, _person: asyncio.run(remote.rows(limit=0)),
        ),
        (
            lambda direct, _person: direct.rows(limit=1, offset=True),
            lambda remote, _person: asyncio.run(remote.rows(limit=1, offset=True)),
        ),
        (
            lambda direct, _person: invoke_untyped(
                direct.rows,
                limit=1,
                order_by=(object(),),
            ),
            lambda remote, _person: asyncio.run(
                _invoke_async_untyped(
                    remote.rows,
                    limit=1,
                    order_by=(object(),),
                )
            ),
        ),
        (
            lambda direct, _person: invoke_untyped(direct.page_by, object(), limit=1),
            lambda remote, _person: asyncio.run(
                _invoke_async_untyped(remote.page_by, object(), limit=1)
            ),
        ),
        (
            lambda direct, person: invoke_untyped(
                direct.page_by,
                person,
                limit=1,
                include_total=1,
            ),
            lambda remote, person: asyncio.run(
                _invoke_async_untyped(
                    remote.page_by,
                    person,
                    limit=1,
                    include_total=1,
                )
            ),
        ),
        (
            lambda direct, _person: invoke_untyped(direct.count_by, object()),
            lambda remote, _person: asyncio.run(_invoke_async_untyped(remote.count_by, object())),
        ),
        (
            lambda direct, _person: invoke_untyped(direct.exists_by, object()),
            lambda remote, _person: asyncio.run(_invoke_async_untyped(remote.exists_by, object())),
        ),
    ],
)
def test_direct_and_remote_pre_io_terminal_errors_are_exact(
    direct_call: Callable[[Query[RemotePerson], object], object],
    remote_call: Callable[[RemoteQuery[RemotePerson], object], object],
) -> None:
    async def unreachable(_request: bytes) -> bytes:
        raise AssertionError("pre-I/O failure reached exchange")

    direct, remote, person = _remote_query(unreachable)
    assert _failure(lambda: direct_call(direct, person)) == _failure(
        lambda: remote_call(remote, person)
    )


_REMOTE_CAPABILITIES = (
    "query.execution.batch-identity-rebind",
    "query.execution.same-snapshot-hydration",
    "query.operation.distinct-count",
    "query.operation.distinct-exists",
    "query.operation.exactly-one",
    "query.operation.page",
    "query.order.stable-collection",
    "query.order.stable-root",
    "query.order.stable-selected",
    "query.output.collect",
    "query.output.collect-distinct",
    "query.output.hydrated",
    "query.output.named",
    "query.output.rows",
    "query.pattern.has",
    "query.pattern.isa",
    "query.pattern.isa-subtypes",
    "query.plan",
    "query.plan.v2",
    "query.remote.envelope-v2",
    "query.remote.structured-diagnostic",
    "query.stage.distinct",
    "query.stage.limit",
    "query.stage.offset",
    "query.stage.require",
    "query.stage.select",
    "query.stage.sort",
)
_REMOTE_REPLY_KEY = "2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12"
_REMOTE_REPLY_KEY_ID = "1ac6aeee69f9aba088cb35c15414bcfdcdf6b9fd2ebfa228c9f9535cc25503c5"


def _model_advertisement(
    capabilities: tuple[str, ...] = _REMOTE_CAPABILITIES,
) -> bytes:
    return json.dumps(
        {
            "capabilities": capabilities,
            "executor": {
                "epoch": "python-model-epoch-0001",
                "identity": "python-model-executor",
            },
            "format": "typebridge.query-remote-capabilities/v1",
            "reply_key": _REMOTE_REPLY_KEY,
            "reply_key_id": _REMOTE_REPLY_KEY_ID,
        },
        separators=(",", ":"),
    ).encode()


def _model_authority() -> QueryV2Authority:
    declared = (
        Path("tests/fixtures/query-v2-model-remote-declared.json").read_bytes().removesuffix(b"\n")
    )
    return QueryV2Authority(
        declared,
        "python-model-remote",
        "typedb-3.12.1/v1",
    )


def test_public_remote_session_snapshots_context_and_hydrates_success(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    authority = _model_authority()
    advertisement = b"exact-advertisement"
    limits = RemoteQueryLimits(11, 12, 13, 14, 15, 16, 17)
    native_context = object()
    context_calls: list[tuple[object, ...]] = []

    def context(*arguments: object) -> object:
        context_calls.append(arguments)
        return native_context

    monkeypatch.setattr(
        remote_session_module,
        "query_v2_remote_model_context",
        context,
    )
    exchanges: list[bytes] = []

    async def exchange(request: bytes) -> bytes:
        exchanges.append(request)
        return b"exact-reply"

    session = RemoteQuerySession(authority, advertisement, exchange, limits)
    assert context_calls == [
        (
            authority,
            advertisement,
            11,
            12,
            13,
            14,
            15,
            16,
            17,
        )
    ]
    person = session.var(RemoteSmokePerson)
    query = session.query(person)
    assert exchanges == []

    pending = _Pending(bytearray(b"exact-v2-request"))
    expected = RemoteSmokePerson(name=RemoteSmokeName("Alice"))

    def prepare(*arguments: object) -> _Pending:
        assert arguments[1] is native_context
        return pending

    monkeypatch.setattr(
        remote_terminal,
        "query_v2_prepare_remote_model_rows",
        prepare,
    )
    monkeypatch.setattr(
        remote_terminal,
        "_materialize_one",
        lambda *_args: expected,
    )

    assert asyncio.run(query.one()) is expected
    assert exchanges == [b"exact-v2-request"]
    assert pending.request_calls == 1
    assert pending.decode_calls == 1
    assert pending.responses == [b"exact-reply"]


def test_public_remote_session_emits_exact_v2_contract_for_every_terminal() -> None:
    requests: list[bytes] = []

    async def exchange(request: bytes) -> bytes:
        requests.append(request)
        return b"{}"

    session = RemoteQuerySession(
        _model_authority(),
        _model_advertisement(),
        exchange,
        RemoteQueryLimits(
            max_items=11,
            max_bytes=1 << 20,
            max_collection_members=12,
            max_graph_nodes=13,
            max_attribute_values=14,
            max_role_players=15,
            deadline_ms=5_000,
        ),
    )
    person = session.var(RemoteSmokePerson)
    query = session.query(person)
    assert requests == []

    async def invoke_all() -> None:
        operations: tuple[
            tuple[Callable[[], Awaitable[object]], str, str],
            ...,
        ] = (
            (query.one, "hydrated_rows", "exactly_one"),
            (
                lambda: query.rows(limit=2, offset=1),
                "hydrated_rows",
                "bounded_many",
            ),
            (
                lambda: query.page_by(person, limit=2, include_total=True),
                "hydrated_page",
                "page",
            ),
            (lambda: query.count_by(person), "distinct_count", "distinct_count"),
            (lambda: query.exists_by(person), "distinct_exists", "distinct_exists"),
        )
        for invoke, expected_result, expected_model_kind in operations:
            before = len(requests)
            with pytest.raises(QueryV2Error) as raised:
                await invoke()
            assert raised.value.code == "query_remote_reply_malformed"
            assert len(requests) == before + 1
            raw = requests[-1]
            request = json.loads(raw)
            assert json.dumps(request, separators=(",", ":")).encode() == raw
            assert request["format"] == "typebridge.query-remote-request/v2"
            assert request["result"] == expected_result
            model = request["plan"]["compatibility"]["model_query"]
            if expected_model_kind in {"exactly_one", "bounded_many"}:
                assert model["kind"] == "rows"
                assert model["cardinality"] == expected_model_kind
            else:
                assert model["kind"] == expected_model_kind
            assert request["limits"] == {
                "deadline_ms": 5_000,
                "max_attribute_values": 14,
                "max_bytes": 1 << 20,
                "max_collection_members": 12,
                "max_graph_nodes": 13,
                "max_items": 11,
                "max_role_players": 15,
            }

    asyncio.run(invoke_all())


def test_public_remote_session_preserves_authenticated_structured_failure() -> None:
    advertisement = _model_advertisement()
    exchanges = 0

    async def exchange(request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        parsed_request = json.loads(request)
        nonce = parsed_request["nonce"]
        assert isinstance(nonce, str)
        request_fingerprint = _remote_fingerprint(
            b"typebridge.query.remote-request",
            b"typebridge.query-remote-request/v2",
            request,
        )
        return _remote_signed_reply(
            {
                "category": "invalid_contract",
                "code": "remote_application_failure",
                "details": {
                    "attempt": {"kind": "long", "value": "7"},
                    "expected": {
                        "kind": "text_list",
                        "value": ["person", "employee"],
                    },
                    "retryable": {"kind": "boolean", "value": False},
                    "subject": {"kind": "text", "value": "smoke-person"},
                },
                "format": "typebridge.query-remote-failure/v2",
                "message": "the remote application rejected this query",
                "nonce": nonce,
                "path": [
                    {"kind": "field", "value": "plan"},
                    {"kind": "index", "value": 0},
                    {"kind": "identifier", "value": "smoke-person"},
                ],
                "request": request_fingerprint,
            },
            advertisement,
        )

    session = RemoteQuerySession(
        _model_authority(),
        advertisement,
        exchange,
        RemoteQueryLimits(
            max_items=11,
            max_bytes=1 << 20,
            max_collection_members=12,
            max_graph_nodes=13,
            max_attribute_values=14,
            max_role_players=15,
            deadline_ms=5_000,
        ),
    )
    person = session.var(RemoteSmokePerson)

    with pytest.raises(QueryV2Error) as raised:
        asyncio.run(session.query(person).one())

    error = raised.value
    assert isinstance(error, QueryV2Error)
    assert error.category == "invalid_contract"
    assert error.code == "remote_application_failure"
    assert error.message == "the remote application rejected this query"
    assert error.path == [
        {"kind": "field", "value": "plan"},
        {"kind": "index", "value": 0},
        {"kind": "identifier", "value": "smoke-person"},
    ]
    assert error.details == {
        "attempt": {"kind": "long", "value": "7"},
        "expected": {"kind": "text_list", "value": ["person", "employee"]},
        "retryable": {"kind": "boolean", "value": False},
        "subject": {"kind": "text", "value": "smoke-person"},
    }
    assert exchanges == 1


def test_public_remote_session_rejects_missing_capability_before_exchange() -> None:
    exchanges = 0

    async def exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return b"unreachable"

    capabilities = tuple(
        capability for capability in _REMOTE_CAPABILITIES if capability != "query.output.hydrated"
    )
    session = RemoteQuerySession(
        _model_authority(),
        _model_advertisement(capabilities),
        exchange,
        RemoteQueryLimits(1, 1 << 20, 1, 1, 1, 1),
    )
    person = session.var(RemoteSmokePerson)
    with pytest.raises(QueryV2Error) as raised:
        asyncio.run(session.query(person).one())
    assert raised.value.category == "unsupported_capability"
    assert raised.value.code == "unsupported_required_capability"
    assert exchanges == 0


def test_native_remote_pending_claims_before_response_type_or_replay_inspection() -> None:
    context = query_v2_remote_model_context(
        _model_authority(),
        _model_advertisement(),
        1,
        1 << 20,
        1,
        1,
        1,
        1,
        None,
    )
    session = diagnostic_session()
    person = session.var(RemoteSmokePerson)
    query = session.query(person)
    pending = query_v2_prepare_remote_model_rows(
        query._native_query(),
        context,
        [],
        0,
        1,
        "exactly_one",
    )

    with pytest.raises(TypeError, match="response.*bytes or bytearray"):
        invoke_untyped(pending.decode_reply, object())

    class HostileReplay:
        def __getattribute__(self, name: str) -> object:
            raise AssertionError(f"replayed response inspected {name}")

    with pytest.raises(QueryV2Error) as replayed:
        invoke_untyped(pending.decode_reply, HostileReplay())
    assert replayed.value.category == "integrity"
    assert replayed.value.code == "query_remote_v2_reply_replayed"


def test_native_remote_order_sequences_are_bounded_at_the_raw_ffi_boundary() -> None:
    context = query_v2_remote_model_context(
        _model_authority(),
        _model_advertisement(),
        1,
        1 << 20,
        1,
        1,
        1,
        1,
        None,
    )
    session = diagnostic_session()
    person = session.var(RemoteSmokePerson)
    query = session.query(person)
    order = person.field(RemoteSmokeName).asc()._native_order()
    oversized = [order] * 65

    preparations = (
        lambda: query_v2_prepare_remote_model_rows(
            query._native_query(),
            context,
            oversized,
            0,
            1,
            "bounded_many",
        ),
        lambda: query_v2_prepare_remote_model_page(
            query._native_query(),
            context,
            person._native_binding(),
            oversized,
            0,
            1,
            False,
        ),
    )
    for prepare in preparations:
        with pytest.raises(MatchRequestError) as raised:
            prepare()
        assert raised.value.category == "resource_limit"
        assert raised.value.code == "structural_limit_exceeded"
        assert raised.value.path == [{"kind": "operation"}]
        assert raised.value.details == {
            "actual": {"kind": "unsigned", "value": 65},
            "limit": {"kind": "text", "value": "order_terms"},
            "maximum": {"kind": "unsigned", "value": 64},
        }


@pytest.mark.parametrize("invalid_limit", [True, -1, 1 << 64, 1.5])
def test_public_remote_session_rejects_invalid_limits_before_exchange(
    invalid_limit: object,
) -> None:
    exchanges = 0

    async def exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return b"unreachable"

    limits = invoke_untyped(
        RemoteQueryLimits,
        invalid_limit,
        1 << 20,
        1,
        1,
        1,
        1,
    )
    with pytest.raises(QueryV2Error) as raised:
        invoke_untyped(
            RemoteQuerySession,
            _model_authority(),
            _model_advertisement(),
            exchange,
            limits,
        )
    assert raised.value.code == "query_remote_limit_invalid"
    assert exchanges == 0
