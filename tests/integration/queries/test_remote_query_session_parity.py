"""Public direct/remote typed-session parity over one three-binding model query."""

from __future__ import annotations

import asyncio
import base64
import json
import os
import socket
import ssl
import subprocess
import time
import uuid
from pathlib import Path
from typing import overload
from urllib import request as urllib_request

import pytest
import type_bridge_core as core
from type_bridge_core import QueryV2Authority

from tests.integration.parity.cross_language import (
    read_v2_authoring_with_packed_node,
)
from type_bridge import (
    AttributeFlags,
    Card,
    Database,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.query_v2 import AuthoredQueryInvocation, AuthoredQueryPlan, QueryPlanBuilder
from type_bridge.typed import (
    BoundVar,
    Query,
    QuerySession,
    RemoteQuery,
    RemoteQueryLimits,
    RemoteQuerySession,
)

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE_DIR = ROOT / "type-bridge-core"
DECLARED_PATH = ROOT / "tests/fixtures/query-v2-model-remote-parity-declared.json"
SCOPE = "model-remote-parity"
PROFILE = "typedb-3.12.1/v1"
ADVANCED_PLAN_FINGERPRINT = "85c9504dca956286b46336510af3b24980bba1a72e79465069b7a24e7d52e26f"

SCHEMA = """
define
attribute parity-person-name, value string;
attribute parity-project-name, value string;
attribute parity-assignment-id, value string;
entity parity-person @abstract,
    owns parity-person-name @key,
    plays parity-assignment:employee;
entity parity-employee sub parity-person;
entity parity-project,
    owns parity-project-name @key,
    plays parity-assignment:project;
relation parity-assignment,
    owns parity-assignment-id @key,
    relates employee @card(1),
    relates project @card(1);
"""

DATA = """
insert
$alice isa parity-employee, has parity-person-name "Alice";
$bob isa parity-employee, has parity-person-name "Bob";
$alpha isa parity-project, has parity-project-name "Alpha";
$beta isa parity-project, has parity-project-name "Beta";
$first isa parity-assignment,
    links (employee: $alice, project: $alpha),
    has parity-assignment-id "assignment-1";
$second isa parity-assignment,
    links (employee: $bob, project: $beta),
    has parity-assignment-id "assignment-2";
"""


class ParityPersonName(String):
    flags = AttributeFlags(name="parity-person-name")


class ParityProjectName(String):
    flags = AttributeFlags(name="parity-project-name")


class ParityAssignmentId(String):
    flags = AttributeFlags(name="parity-assignment-id")


class ParityPerson(Entity):
    flags = TypeFlags(name="parity-person", abstract=True)
    name: ParityPersonName = Flag(Key)


class ParityEmployee(ParityPerson):
    flags = TypeFlags(name="parity-employee")


class ParityProject(Entity):
    flags = TypeFlags(name="parity-project")
    name: ParityProjectName = Flag(Key)


class ParityAssignment(Relation):
    flags = TypeFlags(name="parity-assignment")
    assignment_id: ParityAssignmentId = Flag(Key)
    employee: Role[ParityPerson] = Role(
        "employee",
        ParityPerson,
        cardinality=Card(min=1, max=1),
    )
    project: Role[ParityProject] = Role(
        "project",
        ParityProject,
        cardinality=Card(min=1, max=1),
    )


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _wait_for_port(port: int, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"smoke server exited early with code {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.2)
    raise AssertionError("smoke server never became reachable")


@overload
def _query(
    session: QuerySession,
) -> tuple[
    Query[ParityPerson, ParityProject, ParityAssignment],
    BoundVar[ParityPerson],
]: ...


@overload
def _query(
    session: RemoteQuerySession,
) -> tuple[
    RemoteQuery[ParityPerson, ParityProject, ParityAssignment],
    BoundVar[ParityPerson],
]: ...


def _query(
    session: QuerySession | RemoteQuerySession,
) -> tuple[
    Query[ParityPerson, ParityProject, ParityAssignment]
    | RemoteQuery[ParityPerson, ParityProject, ParityAssignment],
    BoundVar[ParityPerson],
]:
    employee = session.var(ParityPerson, subtypes=True)
    project = session.var(ParityProject)
    assignment = session.var(ParityAssignment)
    query = session.query(employee, project, assignment).where(
        assignment.role(ParityAssignment.employee).connects(employee),
        assignment.role(ParityAssignment.project).connects(project),
    )
    return query, employee


def _advanced_plan(
    authority: QueryV2Authority,
) -> tuple[AuthoredQueryPlan, AuthoredQueryInvocation]:
    """Author the cross-binding FunctionCall/Try/Reduce pipeline."""
    builder = QueryPlanBuilder(authority)
    local_person = builder.binding("lp")
    local_name = builder.binding("ln")
    local_function = builder.local_function(
        "local_name_count",
        (local_name, local_person),
        (local_person,),
        ("parity-person",),
        (
            builder.isa(local_person, "entity", "parity-person", True),
            builder.has(local_person, local_name, "parity-person-name"),
        ),
        builder.local_return("count", local_name, "long"),
    )

    person = builder.binding("person")
    name = builder.binding("name")
    optional_name = builder.binding("optional_name")
    local_result = builder.binding("local_result")
    count_result = builder.binding("count_result")
    wanted_name = builder.input("wanted_name", "string", False)
    name_operand = builder.binding_operand(name)
    nobody = builder.literal_operand("string", "nobody")
    equal = builder.value(
        "equal",
        name_operand,
        builder.input_operand(wanted_name),
    )
    not_equal = builder.value("not_equal", name_operand, nobody)
    builder.match(
        (
            builder.isa(person, "entity", "parity-person", True),
            builder.has(person, name, "parity-person-name"),
            builder.or_(((equal,), (not_equal,))),
            builder.not_((builder.value("equal", name_operand, nobody),)),
            builder.try_((builder.has(person, optional_name, "parity-person-name"),)),
            builder.function_call(
                local_result,
                (builder.binding_operand(person),),
                local_function=local_function,
            ),
        )
    )
    builder.select((person, name, local_result))
    builder.require((name,))
    builder.distinct()
    count = builder.reduce_assignment(count_result, "count")
    builder.reduce((count,), (name,))
    builder.sort(
        (
            builder.order(name, "ascending"),
            builder.order(count_result, "descending"),
        )
    )
    builder.offset(0)
    builder.limit(10)
    plan = builder.finalize_rows((name, count_result))
    assert plan.fingerprint == ADVANCED_PLAN_FINGERPRINT
    return plan, plan.rows((("Alice",),))


def _normalized(
    rows: list[tuple[ParityPerson, ParityProject, ParityAssignment]],
) -> list[tuple[str, str, str, str]]:
    return [
        (
            type(employee).__name__,
            employee.name.value,
            project.name.value,
            assignment.assignment_id.value,
        )
        for employee, project, assignment in rows
    ]


def test_public_remote_query_session_matches_direct_subtype_hydration() -> None:
    tls_values = {
        "TYPEDB_TLS_ADDRESS": os.getenv("TYPEDB_TLS_ADDRESS"),
        "TYPEDB_TLS_HTTP_PORT": os.getenv("TYPEDB_TLS_HTTP_PORT"),
        "TYPEDB_TLS_ROOT_CA": os.getenv("TYPEDB_TLS_ROOT_CA"),
    }
    tls_enabled = any(value is not None for value in tls_values.values())
    if tls_enabled and not all(tls_values.values()):
        pytest.fail(
            "TLS model-remote smoke requires TYPEDB_TLS_ADDRESS, "
            "TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA together"
        )
    if tls_enabled:
        address = tls_values["TYPEDB_TLS_ADDRESS"]
        tls_http_port = tls_values["TYPEDB_TLS_HTTP_PORT"]
        tls_root_ca = tls_values["TYPEDB_TLS_ROOT_CA"]
        assert address is not None and tls_http_port is not None and tls_root_ca is not None
        http_port = int(tls_http_port)
    else:
        address = os.getenv("TYPEDB_ADDRESS", "localhost:1730")
        http_port = int(os.getenv("TYPEDB_HTTP_PORT", "8000"))
        tls_root_ca = None
    username = os.getenv("TYPEDB_USERNAME", "admin")
    password = os.getenv("TYPEDB_PASSWORD", "password")
    database_name = f"tb_v2_model_remote_{uuid.uuid4().hex[:12]}"

    server_tls: dict[str, str] = {}
    ssl_context: ssl.SSLContext | None = None
    remote_scheme = "http"
    node_typedb_tls_root_ca: Path | None = None
    node_remote_tls_root_ca: Path | None = None
    if tls_enabled:
        server_cert = os.getenv("SMOKE_TLS_CERT")
        server_key = os.getenv("SMOKE_TLS_KEY")
        inbound_root_ca = os.getenv("SMOKE_TLS_ROOT_CA")
        if not server_cert or not server_key or not inbound_root_ca:
            pytest.fail(
                "TLS model-remote smoke requires SMOKE_TLS_CERT, "
                "SMOKE_TLS_KEY, and SMOKE_TLS_ROOT_CA"
            )
        assert tls_root_ca is not None
        server_tls = {
            "SMOKE_TYPEDB_TLS": "true",
            "SMOKE_TYPEDB_TLS_ROOT_CA": tls_root_ca,
            "SMOKE_TLS_CERT": server_cert,
            "SMOKE_TLS_KEY": server_key,
        }
        ssl_context = ssl.create_default_context(cafile=inbound_root_ca)
        remote_scheme = "https"
        node_typedb_tls_root_ca = Path(tls_root_ca)
        node_remote_tls_root_ca = Path(inbound_root_ca)

    declared = DECLARED_PATH.read_bytes().removesuffix(b"\n")
    database = Database(
        address,
        database_name,
        username,
        password,
        http_port=http_port,
        tls=True if tls_enabled else None,
        tls_root_ca=tls_root_ca,
    )
    server_version = database.detected_server_version()
    if server_version is not None:
        major, minor = (int(part) for part in server_version.split(".")[:2])
        if (major, minor) < (3, 12):
            pytest.skip("the remote parity smoke prepares the typedb-3.12.1/v1 semantic profile")
    database.create_database()
    try:
        with database.transaction("schema") as transaction:
            transaction.execute(SCHEMA)
        with database.transaction("write") as transaction:
            transaction.execute(DATA)

        advanced_authority = QueryV2Authority(declared, SCOPE, PROFILE)
        advanced_plan, advanced_invocation = _advanced_plan(advanced_authority)
        rust_database = core.PyRustDatabase.connect(
            address,
            database_name,
            username,
            password,
            http_port=http_port,
            tls=True if tls_enabled else None,
            tls_root_ca=tls_root_ca,
        )
        try:
            local_authority = QueryV2Authority.query_only(
                rust_database,
                declared,
                SCOPE,
                PROFILE,
            )
            advanced_local = core.query_v2_execute_local(
                rust_database,
                local_authority,
                advanced_plan.canonical_bytes,
                advanced_invocation.canonical_bytes.decode(),
            )
        finally:
            rust_database.close()
        assert json.loads(advanced_local) == {
            "kind": "rows",
            "rows": [
                [
                    {
                        "kind": "attribute",
                        "type_id": {
                            "kind": "attribute",
                            "label": "parity-person-name",
                        },
                        "value": {"kind": "string", "value": "Alice"},
                    },
                    {"kind": "value", "value": {"kind": "long", "value": "1"}},
                ],
                [
                    {
                        "kind": "attribute",
                        "type_id": {
                            "kind": "attribute",
                            "label": "parity-person-name",
                        },
                        "value": {"kind": "string", "value": "Bob"},
                    },
                    {"kind": "value", "value": {"kind": "long", "value": "1"}},
                ],
            ],
        }

        direct_query, direct_employee = _query(QuerySession(database))
        direct_rows = direct_query.rows(
            limit=10,
            order_by=(direct_employee.field(ParityPersonName).asc(),),
        )
        assert _normalized(direct_rows) == [
            ("ParityEmployee", "Alice", "Alpha", "assignment-1"),
            ("ParityEmployee", "Bob", "Beta", "assignment-2"),
        ]

        port = _free_port()
        smoke_server = os.getenv("TYPE_BRIDGE_V2_SMOKE_SERVER")
        server_command = (
            [smoke_server]
            if smoke_server is not None
            else [
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
        )
        server = subprocess.Popen(
            server_command,
            cwd=None if smoke_server is not None else CORE_DIR,
            env={
                **os.environ,
                "SMOKE_TYPEDB_ADDRESS": address,
                "SMOKE_TYPEDB_USERNAME": username,
                "SMOKE_TYPEDB_PASSWORD": password,
                "SMOKE_TYPEDB_HTTP_PORT": str(http_port),
                "SMOKE_DATABASE": database_name,
                "SMOKE_DECLARED_B64": base64.b64encode(declared).decode(),
                "SMOKE_SCOPE": SCOPE,
                "SMOKE_PROFILE": PROFILE,
                "SMOKE_PORT": str(port),
                **server_tls,
            },
        )
        try:
            _wait_for_port(port, server, timeout=300)
            with urllib_request.urlopen(
                f"{remote_scheme}://127.0.0.1:{port}/v2/capabilities",
                timeout=30,
                context=ssl_context,
            ) as response:
                advertisement = response.read()

            advanced_exchanges = 0

            def post_advanced(request: bytes) -> bytes:
                nonlocal advanced_exchanges
                advanced_exchanges += 1
                http_request = urllib_request.Request(
                    f"{remote_scheme}://127.0.0.1:{port}/v2/query",
                    data=request,
                    headers={"content-type": "application/json"},
                    method="POST",
                )
                with urllib_request.urlopen(
                    http_request,
                    timeout=30,
                    context=ssl_context,
                ) as response:
                    return response.read()

            advanced_pending = core.query_v2_prepare_remote(
                advanced_authority,
                advanced_plan.canonical_bytes,
                advanced_invocation.canonical_bytes.decode(),
                advertisement,
                10,
                1 << 20,
                30,
                30_000,
            )
            assert advanced_exchanges == 0
            advanced_remote = advanced_pending.decode_reply(
                post_advanced(bytes(advanced_pending.request_bytes()))
            )
            assert advanced_exchanges == 1
            assert advanced_remote == advanced_local

            exchanges = 0

            async def exchange(request: bytes) -> bytes:
                nonlocal exchanges
                exchanges += 1

                def post() -> bytes:
                    http_request = urllib_request.Request(
                        f"{remote_scheme}://127.0.0.1:{port}/v2/query",
                        data=request,
                        headers={"content-type": "application/json"},
                        method="POST",
                    )
                    with urllib_request.urlopen(
                        http_request,
                        timeout=30,
                        context=ssl_context,
                    ) as response:
                        return response.read()

                return await asyncio.to_thread(post)

            # docs: remote-query-python:start
            remote_session = RemoteQuerySession(
                QueryV2Authority(declared, SCOPE, PROFILE),
                advertisement,
                exchange,
                RemoteQueryLimits(
                    max_items=10,
                    max_bytes=1 << 20,
                    max_collection_members=30,
                    max_graph_nodes=30,
                    max_attribute_values=30,
                    max_role_players=30,
                    deadline_ms=30_000,
                ),
            )
            remote_query, remote_employee = _query(remote_session)
            assert exchanges == 0
            remote_rows = asyncio.run(
                remote_query.rows(
                    limit=10,
                    order_by=(remote_employee.field(ParityPersonName).asc(),),
                )
            )
            # docs: remote-query-python:end
            assert exchanges == 1
            assert _normalized(remote_rows) == _normalized(direct_rows)
            assert all(type(row[0]) is ParityEmployee for row in remote_rows)

            packed = read_v2_authoring_with_packed_node(
                address,
                database_name,
                http_port=http_port,
                declared_fixture=DECLARED_PATH,
                server_url=f"{remote_scheme}://127.0.0.1:{port}",
                typedb_tls_root_ca=node_typedb_tls_root_ca,
                remote_tls_root_ca=node_remote_tls_root_ca,
            )
            if packed is not None:
                expected_rows = [
                    {
                        "assignment": "assignment-1",
                        "concrete": "ParityEmployee",
                        "employee": "Alice",
                        "project": "Alpha",
                        "roleEmployee": "Alice",
                        "roleProject": "Alpha",
                    },
                    {
                        "assignment": "assignment-2",
                        "concrete": "ParityEmployee",
                        "employee": "Bob",
                        "project": "Beta",
                        "roleEmployee": "Bob",
                        "roleProject": "Beta",
                    },
                ]
                assert packed == {
                    "advanced": {
                        "exchanges": 1,
                        "fingerprint": ADVANCED_PLAN_FINGERPRINT,
                        "outcome": json.loads(advanced_local),
                    },
                    "artifact": "packed-v2",
                    "model": {
                        "direct": expected_rows,
                        "exchanges": 1,
                        "remote": expected_rows,
                    },
                }
        finally:
            server.kill()
            server.wait(timeout=30)
    finally:
        database.close()
        cleanup_database = Database(
            address,
            database_name,
            username,
            password,
            http_port=http_port,
            tls=True if tls_enabled else None,
            tls_root_ca=tls_root_ca,
        )
        try:
            cleanup_database.delete_database()
        finally:
            cleanup_database.close()
