#!/usr/bin/env python3
"""Runtime consumer for the extracted public typed-query wheel surface."""

from __future__ import annotations

import argparse
import base64
import importlib.metadata
import json
import sys
from collections.abc import Callable
from dataclasses import FrozenInstanceError, dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

V2_DECLARED_SCHEMA = base64.b64decode(
    "eyJkZWNsYXJlZF9pZGVudGl0eSI6eyJhbGdvcml0aG0iOiJzaGEyNTYiLCJjYW5vbmljYWxpemF0aW9u"
    "IjoidHlwZWJyaWRnZS5zY2hlbWEtY2Fub25pY2FsLWpzb24vdjEiLCJkaWdlc3QiOiJiZGFiNzEzOGE1"
    "NzIzOGVlMjNkZmNlYjY5ZTdmMDk4OTNjZmE3YjUzNmQ5ZTcwMzU2ZDFhOTg2YTEzMjQ5OWZlIiwiZG9t"
    "YWluIjoidHlwZWJyaWRnZS5zY2hlbWEuZGVjbGFyZWQtaWRlbnRpdHkifSwiZmFjdHMiOlt7ImtpbmQi"
    "OiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2UtcGVyc29u"
    "In19fSx7ImtpbmQiOiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImF0dHJpYnV0ZSIsImxhYmVs"
    "Ijoic21va2UtbmFtZSJ9fX0seyJraW5kIjoidmFsdWUiLCJ2YWx1ZSI6eyJpZCI6InNtb2tlLW5hbWUi"
    "LCJ2YWx1ZV90eXBlIjoic3RyaW5nIn19LHsia2luZCI6Im93bnMiLCJ2YWx1ZSI6eyJpZCI6eyJhdHRy"
    "aWJ1dGUiOiJzbW9rZS1uYW1lIiwib3duZXIiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2Ut"
    "cGVyc29uIn19fX1dLCJmb3JtYXRfdmVyc2lvbiI6MSwicmVxdWlyZWRfY2FwYWJpbGl0aWVzIjpbXX0="
)


class ArtifactAcceptanceError(AssertionError):
    """The built artifact violated the public typed-facade contract."""


def require(condition: bool, message: str) -> None:
    """Raise a stable artifact-acceptance failure."""
    if not condition:
        raise ArtifactAcceptanceError(message)


def path_is_within(path: str | Path, root: str | Path) -> bool:
    """Return whether one resolved path is contained by another."""
    candidate = Path(path).expanduser().resolve()
    boundary = Path(root).expanduser().resolve()
    return candidate == boundary or boundary in candidate.parents


def source_package_path(path: str, source_root: Path) -> bool:
    """Identify checkout import roots while retaining prepared site-packages."""
    if not path:
        return False
    try:
        candidate = Path(path).expanduser().resolve()
    except OSError:
        return False
    if not path_is_within(candidate, source_root):
        return False
    return (candidate / "type_bridge").exists() or (candidate / "type_bridge_core").exists()


def activate_artifact(artifact_root: Path, source_root: Path) -> None:
    """Put extracted wheels first and remove checkout package import roots."""
    retained = [path for path in sys.path if path and not source_package_path(path, source_root)]
    sys.path[:] = [str(artifact_root), *retained]


def module_paths(module: ModuleType) -> list[Path]:
    """Collect concrete paths exposed by one imported module."""
    paths: list[Path] = []
    module_file = getattr(module, "__file__", None)
    if module_file:
        paths.append(Path(module_file).resolve())
    module_path = getattr(module, "__path__", None)
    if module_path is not None:
        paths.extend(Path(path).resolve() for path in module_path)
    return paths


def assert_artifact_imports(artifact_root: Path, source_root: Path) -> dict[str, Any]:
    """Require every loaded TypeBridge module and distribution from staging."""
    modules: dict[str, list[str]] = {}
    for name, module in sorted(sys.modules.items()):
        if name != "type_bridge" and not name.startswith(("type_bridge.", "type_bridge_core")):
            continue
        paths = module_paths(module)
        require(bool(paths), f"loaded TypeBridge module has no concrete artifact path: {name}")
        require(
            all(path_is_within(path, artifact_root) for path in paths),
            f"module escaped extracted wheels: {name} -> {paths}",
        )
        require(
            not any(path_is_within(path, source_root) for path in paths),
            f"module leaked from source checkout: {name} -> {paths}",
        )
        modules[name] = [str(path) for path in paths]

    distributions: dict[str, dict[str, str]] = {}
    for name in ("type-bridge", "type-bridge-core"):
        distribution = importlib.metadata.distribution(name)
        location = Path(str(distribution.locate_file(""))).resolve()
        require(
            path_is_within(location, artifact_root),
            f"distribution metadata escaped extracted wheels: {name} -> {location}",
        )
        distributions[name] = {"version": distribution.version, "location": str(location)}
    return {"modules": modules, "distributions": distributions}


def expect_error(
    error_type: type[BaseException] | tuple[type[BaseException], ...],
    operation: Callable[[], object],
    message: str,
) -> BaseException:
    """Run a nullary operation and require one exact public error family."""
    try:
        operation()
    except error_type as error:
        return error
    except BaseException as error:
        expected = (
            error_type.__name__
            if isinstance(error_type, type)
            else " or ".join(item.__name__ for item in error_type)
        )
        raise ArtifactAcceptanceError(
            f"{message}: expected {expected}, got {type(error).__name__}: {error}"
        ) from error
    expected = (
        error_type.__name__
        if isinstance(error_type, type)
        else " or ".join(item.__name__ for item in error_type)
    )
    raise ArtifactAcceptanceError(f"{message}: expected {expected}")


def invoke_untyped(function: object, /, *args: object, **kwargs: object) -> object:
    """Invoke a runtime-only hostile call after checking callability."""
    if not callable(function):
        raise TypeError("artifact runtime boundary requires a callable")
    return function(*args, **kwargs)


def run(artifact_root: Path, source_root: Path) -> dict[str, Any]:
    """Exercise public native-backed construction and fail-closed terminals."""
    activate_artifact(artifact_root, source_root)

    import type_bridge_core

    import type_bridge
    from type_bridge import (
        Entity,
        Flag,
        Key,
        Relation,
        Role,
        String,
        TypeDBServerDeprecationWarning,
        TypeFlags,
    )
    from type_bridge.query import Query as RawQuery
    from type_bridge.query_v2 import (
        AuthoredQueryInvocation,
        AuthoredQueryPlan,
        QueryPlanBuilder,
        QueryV2Authority,
    )
    from type_bridge.typed import (
        Page,
        QuerySession,
        RemoteQuerySession,
        TypedQueryConnectionError,
        TypedQueryWindowError,
    )
    from type_bridge.typed import (
        Query as TypedQuery,
    )

    # These probe models live inside ``run`` while postponed annotations are
    # enabled. SchemaScanner resolves them through module globals, including
    # the generic role constructor used by the relation annotations.
    globals()["Role"] = Role

    require(type_bridge.Query is RawQuery, "package-root Query identity changed")
    require(TypedQuery is not RawQuery, "typed Query aliases the package-root raw Query")
    require(type_bridge_core.MatchSessionHandle is not None, "native match session is absent")
    require(
        QueryPlanBuilder is type_bridge_core.QueryPlanBuilder,
        "packaged authoring facade does not preserve native builder identity",
    )
    require(
        QueryV2Authority is type_bridge_core.QueryV2Authority,
        "packaged authoring facade does not preserve native authority identity",
    )
    require(isinstance(RemoteQuerySession, type), "packaged remote query session is absent")
    require(
        issubclass(TypeDBServerDeprecationWarning, FutureWarning),
        "server warning is not a public FutureWarning subclass",
    )
    require(
        TypeDBServerDeprecationWarning.code
        == type_bridge_core.TYPEDB_LEGACY_SERVER_DEPRECATION_CODE,
        "Python warning code differs from the packaged native SSOT",
    )
    legacy_notice = type_bridge_core.typedb_server_deprecation_notice("3.10.4")
    require(
        legacy_notice is not None
        and "TypeDB 3.10.4" in legacy_notice
        and "band 7" not in legacy_notice,
        "packaged native legacy-server notice drifted",
    )
    require(
        type_bridge_core.typedb_server_deprecation_notice("3.12.1") is None,
        "packaged native helper warns for TypeDB 3.12.1",
    )

    v2_authority = QueryV2Authority(
        V2_DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    v2_builder = QueryPlanBuilder(v2_authority)
    v2_person = v2_builder.binding("person")
    v2_name = v2_builder.binding("name")
    v2_wanted = v2_builder.input("wanted_name", "string", False)
    v2_builder.match(
        (
            v2_builder.isa(v2_person, "entity", "smoke-person", True),
            v2_builder.has(v2_person, v2_name, "smoke-name"),
            v2_builder.value(
                "equal",
                v2_builder.binding_operand(v2_name),
                v2_builder.input_operand(v2_wanted),
            ),
        )
    )
    v2_builder.select((v2_person, v2_name))
    v2_builder.require((v2_name,))
    v2_builder.distinct()
    v2_builder.sort((v2_builder.order(v2_name, "ascending"),))
    v2_plan: AuthoredQueryPlan = v2_builder.finalize_rows((v2_person, v2_name))
    v2_invocation: AuthoredQueryInvocation = v2_plan.rows((("Alice",),))
    require(v2_plan.format == "typebridge.query-plan/v2", "packaged V2 format drifted")
    require(len(v2_plan.fingerprint) == 64, "packaged V2 fingerprint is malformed")
    require(
        v2_invocation.plan_fingerprint == v2_plan.fingerprint,
        "packaged invocation lost its exact plan binding",
    )
    finalized_error = expect_error(
        type_bridge_core.QueryV2Error,
        lambda: v2_builder.binding("after_finalize"),
        "packaged builder accepted use after finalization",
    )
    require(
        getattr(finalized_error, "code", None) == "query_builder_finalized",
        "packaged builder lost the shared finalized diagnostic",
    )

    class ArtifactName(String):
        pass

    class ArtifactCode(String):
        pass

    globals()["ArtifactName"] = ArtifactName
    globals()["ArtifactCode"] = ArtifactCode

    class ArtifactPerson(Entity):
        flags = TypeFlags(name="artifact-person")
        name: ArtifactName = Flag(Key)

    class ArtifactCompany(Entity):
        flags = TypeFlags(name="artifact-company")
        name: ArtifactName = Flag(Key)

    globals()["ArtifactPerson"] = ArtifactPerson
    globals()["ArtifactCompany"] = ArtifactCompany

    class ArtifactEmployment(Relation):
        flags = TypeFlags(name="artifact-employment")
        code: ArtifactCode = Flag(Key)
        employee: Role[ArtifactPerson] = Role("employee", ArtifactPerson)
        employer: Role[ArtifactCompany] = Role("employer", ArtifactCompany)

    globals()["ArtifactEmployment"] = ArtifactEmployment
    employment_code = ArtifactCode

    @dataclass(frozen=True, slots=True)
    class PersonWork:
        person: ArtifactPerson
        employments: tuple[ArtifactEmployment, ...]
        companies: tuple[ArtifactCompany, ...]

    expect_error(
        TypeError,
        lambda: invoke_untyped(QuerySession),
        "QuerySession accepted a missing public connection",
    )
    expect_error(
        TypeError,
        lambda: invoke_untyped(QuerySession, None),
        "QuerySession accepted an explicit null public connection",
    )
    session = QuerySession._diagnostic()
    person = session.var(ArtifactPerson)
    second_person = session.var(ArtifactPerson)
    company = session.var(ArtifactCompany)
    employment = session.var(ArtifactEmployment)

    connected = (
        session.query(person, company)
        .match(employment)
        .where(
            employment.role(ArtifactEmployment.employee).connects(person),
            employment.role(ArtifactEmployment.employer).connects(company),
        )
    )
    require(type(connected) is TypedQuery, "typed query did not use the public wrapper")
    require(
        connected is not session.query(person, company),
        "persistent typed builder returned an aliased wrapper",
    )
    repeated = session.query(person, second_person)
    require(type(repeated) is TypedQuery, "repeated model handles failed at runtime")

    named = session.query_as(
        PersonWork,
        person=person,
        employments=employment.collect().order_by(employment.field(employment_code).asc()),
        companies=company.collect().distinct(),
    ).where(
        employment.role(ArtifactEmployment.employee).connects(person),
        employment.role(ArtifactEmployment.employer).connects(company),
    )
    require(type(named) is TypedQuery, "named collected query construction failed")

    expect_error(TypeError, TypedQuery, "typed Query constructor became public")
    expect_error(
        AttributeError,
        lambda: setattr(connected, "replacement", object()),
        "typed Query became mutable",
    )
    window_error = expect_error(
        TypedQueryWindowError,
        lambda: session.query(person).rows(limit=0),
        "invalid result window did not fail closed",
    )
    require(
        getattr(window_error, "code", None) == "invalid_window_limit",
        "typed window error code drifted",
    )
    connection_error = expect_error(
        TypedQueryConnectionError,
        lambda: session.query(person).count_by(person),
        "validated terminal without a connection did not fail closed",
    )
    require(
        getattr(connection_error, "code", None) == "execution_connection_required",
        "typed connection error code drifted",
    )

    page = Page([ArtifactPerson(name=ArtifactName("Alice"))], offset=0, limit=1, total=1)
    require(isinstance(page.items, tuple), "Page.items is not immutable tuple storage")
    require(page.offset == 0 and page.limit == 1 and page.total == 1, "Page window drifted")
    expect_error(
        (FrozenInstanceError, AttributeError),
        lambda: setattr(page, "limit", 2),
        "Page envelope became mutable",
    )

    locations = assert_artifact_imports(artifact_root, source_root)
    return {
        "status": "ok",
        "root_query": f"{RawQuery.__module__}.{RawQuery.__name__}",
        "typed_query": f"{TypedQuery.__module__}.{TypedQuery.__name__}",
        "v2_plan_fingerprint": v2_plan.fingerprint,
        "named_row": PersonWork.__name__,
        "page_items": len(page.items),
        "locations": locations,
    }


def main() -> int:
    """Parse explicit staging boundaries and emit one machine report."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    args = parser.parse_args()
    print(
        json.dumps(
            run(args.artifact_root.resolve(), args.source_root.resolve()),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
