"""One declarative authoring inventory exercised through the public Python facade."""

from __future__ import annotations

import base64
import json
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

import pytest
import type_bridge_core as native

import type_bridge.query_v2 as public

_FIXTURE_PATH = Path("tests/fixtures/query-v2-authoring-inventory.json")
_FIXTURE_DIRECTORY = _FIXTURE_PATH.parent


def _corpus() -> dict[str, Any]:
    return json.loads(_FIXTURE_PATH.read_text())


def _authority(corpus: Mapping[str, Any]) -> public.QueryV2Authority:
    authority = corpus["authority"]
    declared = (_FIXTURE_DIRECTORY / authority["declared"]).read_bytes().removesuffix(b"\n")
    return public.QueryV2Authority(
        declared,
        authority["scope"],
        authority["profile"],
    )


def _scalar(value_type: str, value: Any) -> Any:
    if value_type == "long":
        return int(str(value))
    return value


def _rows(rows: Sequence[Sequence[Mapping[str, Any] | None]]) -> tuple[tuple[Any, ...], ...]:
    return tuple(
        tuple(None if cell is None else _scalar(cell["type"], cell["value"]) for cell in row)
        for row in rows
    )


def _names(value: Sequence[str] | Mapping[str, Any]) -> tuple[str, ...]:
    if isinstance(value, Mapping):
        repeated = value["repeat"]
        count = value["count"]
        assert isinstance(repeated, str)
        assert isinstance(count, int) and count >= 0
        return (repeated,) * count
    return tuple(value)


def _handles(
    handles: Mapping[str, Any],
    names: Sequence[str] | Mapping[str, Any],
) -> tuple[Any, ...]:
    return tuple(handles[name] for name in _names(names))


def _execute_step(
    builder: public.QueryPlanBuilder,
    handles: dict[str, Any],
    step: Mapping[str, Any],
) -> Any:
    op = step["op"]
    result: Any
    if op == "binding":
        result = builder.binding(step["name"])
    elif op == "input":
        result = builder.input(
            step["name"],
            step["value_type"],
            step["optional"],
        )
    elif op == "binding_operand":
        result = builder.binding_operand(handles[step["binding"]])
    elif op == "literal_operand":
        result = builder.literal_operand(
            step["value_type"],
            _scalar(step["value_type"], step["value"]),
        )
    elif op == "input_operand":
        result = builder.input_operand(handles[step["input"]])
    elif op == "isa":
        result = builder.isa(
            handles[step["binding"]],
            step["type_kind"],
            step["type_label"],
            step["include_subtypes"],
        )
    elif op == "has":
        result = builder.has(
            handles[step["owner"]],
            handles[step["attribute"]],
            step["attribute_label"],
        )
    elif op == "links":
        result = builder.links(
            handles[step["relation"]],
            step["relation_label"],
            _names(step["roles"]),
            _handles(handles, step["players"]),
        )
    elif op == "value":
        result = builder.value(
            step["comparator"],
            handles[step["left"]],
            handles[step["right"]],
        )
    elif op == "not":
        repeat = step.get("repeat", 1)
        assert isinstance(repeat, int) and repeat > 0
        nested = _handles(handles, step["patterns"])
        result = None
        for _ in range(repeat):
            result = builder.not_(nested)
            nested = (result,)
    elif op == "or":
        result = builder.or_(tuple(_handles(handles, branch) for branch in step["branches"]))
    elif op == "try":
        result = builder.try_(_handles(handles, step["patterns"]))
    elif op == "reachable":
        result = builder.reachable(
            handles[step["source"]],
            handles[step["target"]],
            step["relation_label"],
            step["role_from"],
            step["role_to"],
            step["min_depth"],
            step["max_depth"],
        )
    elif op == "function_call":
        local_name = step.get("local_function")
        function_call: Any = builder.function_call
        result = function_call(
            handles[step["assigned"]],
            _handles(handles, step["arguments"]),
            step.get("function_name"),
            local_function=None if local_name is None else handles[local_name],
        )
    elif op == "order":
        result = builder.order(
            handles[step["binding"]],
            step["direction"],
        )
    elif op == "reduce_assignment":
        input_name = step.get("input")
        result = builder.reduce_assignment(
            handles[step["assigned"]],
            step["reducer"],
            None if input_name is None else handles[input_name],
        )
    elif op == "local_return":
        result = builder.local_return(
            step["reducer"],
            handles[step["input"]],
            step["value_type"],
        )
    elif op == "local_function":
        result = builder.local_function(
            step["name"],
            _handles(handles, step["bindings"]),
            _handles(handles, step["parameter_bindings"]),
            _names(step["parameter_labels"]),
            _handles(handles, step["body"]),
            handles[step["returns"]],
        )
    elif op == "match":
        builder.match(_handles(handles, step["patterns"]))
        result = None
    elif op == "select":
        builder.select(_handles(handles, step["bindings"]))
        result = None
    elif op == "require":
        builder.require(_handles(handles, step["bindings"]))
        result = None
    elif op == "distinct":
        builder.distinct()
        result = None
    elif op == "reduce":
        builder.reduce(
            _handles(handles, step["assignments"]),
            _handles(handles, step["groups"]),
        )
        result = None
    elif op == "sort":
        builder.sort(_handles(handles, step["terms"]))
        result = None
    elif op == "offset":
        builder.offset(step["rows"])
        result = None
    elif op == "limit":
        builder.limit(step["rows"])
        result = None
    elif op == "document_binding":
        result = builder.document_binding(
            step["key"],
            handles[step["binding"]],
        )
    elif op == "document_attribute_list":
        result = builder.document_attribute_list(
            step["key"],
            handles[step["owner"]],
            step["attribute_label"],
        )
    elif op == "finalize_rows":
        result = builder.finalize_rows(_handles(handles, step["bindings"]))
    elif op == "finalize_documents":
        result = builder.finalize_documents(_handles(handles, step["fields"]))
    else:
        raise AssertionError(f"unknown inventory operation: {op}")
    if "id" in step:
        assert result is not None, f"{op} declared an id without returning a handle"
        handles[step["id"]] = result
    return result


def _execute_plan(
    authority: public.QueryV2Authority,
    case: Mapping[str, Any],
) -> public.AuthoredQueryPlan:
    builder = public.QueryPlanBuilder(authority)
    handles: dict[str, Any] = {}
    for step in case["steps"]:
        _execute_step(builder, handles, step)
    plan = handles.get("plan")
    assert isinstance(plan, public.AuthoredQueryPlan)
    return plan


def _invoke(
    plan: public.AuthoredQueryPlan,
    terminal: str,
    rows: Sequence[Sequence[Mapping[str, Any] | None]],
) -> public.AuthoredQueryInvocation:
    operation = getattr(plan, terminal)
    invocation = operation(_rows(rows))
    assert isinstance(invocation, public.AuthoredQueryInvocation)
    return invocation


def _diagnostic(error: native.QueryV2Error) -> dict[str, object]:
    return {
        "category": error.category,
        "code": error.code,
        "message": error.message,
        "path": error.path,
        "details": dict(error.details),
    }


def test_inventory_names_every_public_operation_and_required_variant() -> None:
    corpus = _corpus()
    inventory = corpus["inventory"]
    operations = {step["op"] for case in corpus["plans"] for step in case["steps"]}
    assert operations == set(inventory["builder_operations"])

    expected_coverage = {
        f"{category}:{variant}"
        for category, variants in inventory["coverage"].items()
        for variant in variants
    }
    actual_coverage = {coverage for case in corpus["plans"] for coverage in case["covers"]}
    assert actual_coverage == expected_coverage

    terminals = {
        invocation["terminal"] for case in corpus["plans"] for invocation in case["invocations"]
    }
    assert terminals == set(inventory["invocation_terminals"])
    assert {case["id"] for case in corpus["diagnostics"]} == set(inventory["diagnostic_cases"])


def test_public_python_facade_matches_every_fixed_rust_authority_vector() -> None:
    corpus = _corpus()
    authority = _authority(corpus)
    for case in corpus["plans"]:
        plan = _execute_plan(authority, case)
        expected = case["expected"]
        assert expected is not None, f"missing fixed plan vector for {case['id']}"
        expected_plan_bytes = base64.b64decode(expected["canonical_b64"])
        expected_plan = json.loads(expected_plan_bytes)
        assert plan.canonical_bytes == expected_plan_bytes
        assert plan.fingerprint == expected["fingerprint"]
        assert list(plan.required_capabilities) == (expected_plan["required_capabilities"])

        for invocation_case in case["invocations"]:
            invocation = _invoke(
                plan,
                invocation_case["terminal"],
                invocation_case["rows"],
            )
            expected_invocation = invocation_case["expected"]
            assert expected_invocation is not None, (
                f"missing fixed invocation vector for {case['id']}/{invocation_case['id']}"
            )
            expected_invocation_bytes = base64.b64decode(expected_invocation["canonical_b64"])
            expected_invocation_wire = json.loads(expected_invocation_bytes)
            assert invocation.canonical_bytes == expected_invocation_bytes
            assert invocation.operation == expected_invocation_wire["operation"]
            assert (
                invocation.plan_fingerprint
                == expected_invocation_wire["plan_fingerprint"]["digest"]
            )
            assert (
                list(invocation.required_transport_capabilities)
                == (expected_invocation["required_transport_capabilities"])
            )


def test_public_python_facade_preserves_every_complete_inventory_diagnostic() -> None:
    corpus = _corpus()
    authority = _authority(corpus)
    plans = {case["id"]: _execute_plan(authority, case) for case in corpus["plans"]}
    for case in corpus["diagnostics"]:
        with pytest.raises(native.QueryV2Error) as captured:
            if case["kind"] == "builder":
                builder = public.QueryPlanBuilder(authority)
                handles: dict[str, Any] = {}
                for step in case["steps"]:
                    _execute_step(builder, handles, step)
                _execute_step(builder, handles, case["failure"])
            else:
                _invoke(
                    plans[case["plan"]],
                    case["terminal"],
                    case["rows"],
                )
        assert _diagnostic(captured.value) == case["expected"], case["id"]
