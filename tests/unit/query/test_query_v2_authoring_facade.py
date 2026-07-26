"""Public low-level V2 authoring identity and canonical-runtime smoke."""

from __future__ import annotations

import base64
import json
import threading
from collections.abc import Callable
from pathlib import Path

import pytest
import type_bridge_core as native

import type_bridge.query_v2 as public

_DECLARED_SCHEMA = base64.b64decode(
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
_PARITY_DECLARED_SCHEMA = (
    Path("tests/fixtures/query-v2-model-remote-parity-declared.json")
    .read_bytes()
    .removesuffix(b"\n")
)
_BUILDER_OPERATIONS = {
    "binding",
    "input",
    "binding_operand",
    "literal_operand",
    "input_operand",
    "isa",
    "has",
    "links",
    "value",
    "not",
    "or",
    "try",
    "reachable",
    "function_call",
    "order",
    "reduce_assignment",
    "local_return",
    "local_function",
    "match",
    "select",
    "require",
    "distinct",
    "reduce",
    "sort",
    "offset",
    "limit",
    "document_binding",
    "document_attribute_list",
    "finalize_rows",
    "finalize_documents",
}


def _author_plan(authority: public.QueryV2Authority) -> public.AuthoredQueryPlan:
    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    name = builder.binding("name")
    builder.match(
        (
            builder.isa(person, "entity", "smoke-person", True),
            builder.has(person, name, "smoke-name"),
        )
    )
    builder.sort((builder.order(name, "ascending"),))
    return builder.finalize_rows((person, name))


def test_public_authoring_classes_are_the_native_classes_by_identity() -> None:
    assert public.QueryV2Authority is native.QueryV2Authority
    assert public.QueryPlanBuilder is native.QueryPlanBuilder
    assert public.AuthoredQueryPlan is native.AuthoredQueryPlan
    assert public.AuthoredQueryInvocation is native.AuthoredQueryInvocation


def test_public_authoring_produces_deterministic_canonical_plan_and_invocation() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    first = _author_plan(authority)
    second = _author_plan(authority)

    assert first.format == "typebridge.query-plan/v2"
    assert first.canonical_bytes == second.canonical_bytes
    assert first.fingerprint == second.fingerprint
    assert first.required_capabilities == tuple(sorted(first.required_capabilities))
    assert len(first.fingerprint) == 64
    assert bytes.fromhex(first.fingerprint)

    decoded_plan = json.loads(first.canonical_bytes)
    assert decoded_plan["format"] == first.format
    assert decoded_plan["required_capabilities"] == list(first.required_capabilities)
    assert (
        json.dumps(decoded_plan, separators=(",", ":"), sort_keys=True).encode()
        == first.canonical_bytes
    )

    invocation = first.rows(())
    assert isinstance(invocation, public.AuthoredQueryInvocation)
    assert invocation.operation == "rows"
    assert invocation.plan_fingerprint == first.fingerprint
    assert invocation.authority_identity.same_authority(first.authority_identity)
    assert (
        json.dumps(
            json.loads(invocation.canonical_bytes),
            separators=(",", ":"),
            sort_keys=True,
        ).encode()
        == invocation.canonical_bytes
    )


def _assert_diagnostic(code: str, operation: Callable[[], object]) -> None:
    with pytest.raises(native.QueryV2Error) as captured:
        operation()
    assert captured.value.code == code


def test_public_authority_rejects_non_scalar_unicode_without_identity_replacement() -> None:
    mixed_astral_variable = "😀" * 64 + "\ud800"
    mixed_astral_label = "😀" * 128 + "\ud800"
    mixed_astral_canonical = "😀" * (1_048_576 // 2) + "\ud800"

    _assert_diagnostic(
        "query_v2_host_string_unicode",
        lambda: public.QueryV2Authority(
            _DECLARED_SCHEMA,
            "binding-\ud800",
            "typedb-3.12.1/v1",
        ),
    )
    _assert_diagnostic(
        "query_v2_host_string_unicode",
        lambda: public.QueryV2Authority(
            _DECLARED_SCHEMA,
            mixed_astral_canonical,
            "typedb-3.12.1/v1",
        ),
    )

    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    relation = builder.binding("relation")
    assigned = builder.binding("assigned")
    _assert_diagnostic(
        "query_v2_host_string_type",
        lambda: builder.binding(
            object(),  # pyright: ignore[reportArgumentType]
        ),
    )
    for operation in (
        lambda: builder.binding("\ud800"),
        lambda: builder.binding(mixed_astral_variable),
        lambda: builder.isa(person, "entity", "\ud800", False),
        lambda: builder.isa(person, "entity", mixed_astral_label, False),
        lambda: builder.links(
            relation,
            "smoke-relation",
            ("\ud800",),
            (person,),
        ),
        lambda: builder.function_call(
            assigned,
            (),
            function_name="\ud800",
        ),
        lambda: builder.document_binding("\ud800", person),
    ):
        _assert_diagnostic("query_v2_host_string_unicode", operation)
    _assert_diagnostic(
        "query_builder_scalar_unicode",
        lambda: builder.literal_operand("string", mixed_astral_canonical),
    )

    builder.match((builder.isa(person, "entity", "smoke-person", False),))
    assert builder.finalize_rows((person,)).format == "typebridge.query-plan/v2"


def test_public_ownership_failures_distinguish_builder_and_authority_and_are_atomic() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    foreign_authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    first = public.QueryPlanBuilder(authority)
    same_authority = public.QueryPlanBuilder(authority)
    other_authority = public.QueryPlanBuilder(foreign_authority)

    same_handle = same_authority.binding("same")
    other_handle = other_authority.binding("other")
    _assert_diagnostic(
        "query_builder_cross_builder_handle",
        lambda: first.binding_operand(same_handle),
    )
    _assert_diagnostic(
        "query_builder_cross_authority_handle",
        lambda: first.binding_operand(other_handle),
    )

    person = first.binding("person")
    first.match((first.isa(person, "entity", "smoke-person", False),))
    assert first.finalize_rows((person,)).format == "typebridge.query-plan/v2"


def test_public_builder_cross_thread_misuse_fails_without_mutation_or_panic() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    failures: list[BaseException] = []

    def misuse() -> None:
        try:
            builder.binding("foreign_thread")
        except BaseException as error:
            failures.append(error)

    worker = threading.Thread(target=misuse)
    worker.start()
    worker.join(timeout=5)
    assert not worker.is_alive()
    assert len(failures) == 1
    assert isinstance(failures[0], native.QueryV2Error)
    assert failures[0].code == "query_builder_cross_thread"

    person = builder.binding("person")
    builder.match((builder.isa(person, "entity", "smoke-person", False),))
    assert builder.finalize_rows((person,)).format == "typebridge.query-plan/v2"


def test_public_integer_boundaries_fail_before_mutation() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    source = builder.binding("source")
    target = builder.binding("target")

    for depth in (True, -1, 1.5, float("inf"), 256, 1 << 200):
        _assert_diagnostic(
            "query_builder_depth_range",
            lambda depth=depth: builder.reachable(
                source,
                target,
                "friendship",
                "friend",
                "friend",
                depth,  # pyright: ignore[reportArgumentType]
                1,
            ),
        )

    for rows in (True, 1.0, -1, 1 << 64):
        _assert_diagnostic(
            "query_builder_unsigned_integer_range",
            lambda rows=rows: builder.offset(
                rows,  # pyright: ignore[reportArgumentType]
            ),
        )

    for scalar in (1 << 63, -(1 << 63) - 1):
        _assert_diagnostic(
            "query_builder_scalar_integer_range",
            lambda scalar=scalar: builder.literal_operand("long", scalar),
        )

    name = builder.binding("name")
    builder.match(
        (
            builder.isa(source, "entity", "smoke-person", False),
            builder.has(source, name, "smoke-name"),
        )
    )
    builder.select((name,))
    builder.sort((builder.order(name, "ascending"),))
    builder.offset(0)
    assert builder.finalize_rows((name,)).format == "typebridge.query-plan/v2"


def test_public_collection_and_invocation_limits_preflight_before_element_access() -> None:
    class HostilePatterns:
        accessed = False

        def __len__(self) -> int:
            return 257

        def __getitem__(self, _index: int) -> object:
            self.accessed = True
            raise AssertionError("pattern access must not occur")

    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    person_isa = builder.isa(person, "entity", "smoke-person", False)
    hostile_patterns = HostilePatterns()
    _assert_diagnostic(
        "query_plan_pattern_limit",
        lambda: builder.match(
            hostile_patterns,  # pyright: ignore[reportArgumentType]
        ),
    )
    assert not hostile_patterns.accessed
    builder.match((person_isa,))
    plan = builder.finalize_rows((person,))

    class HostileRows:
        accessed = False

        def __len__(self) -> int:
            return 4_097

        def __getitem__(self, _index: int) -> object:
            self.accessed = True
            raise AssertionError("row access must not occur")

    hostile_rows = HostileRows()
    _assert_diagnostic(
        "query_invocation_row_limit",
        lambda: plan.rows(
            hostile_rows,  # pyright: ignore[reportArgumentType]
        ),
    )
    assert not hostile_rows.accessed
    assert plan.rows(()).operation == "rows"

    input_builder = public.QueryPlanBuilder(authority)
    input_builder.input("supplied_text", "string", True)
    input_person = input_builder.binding("person")
    input_builder.match((input_builder.isa(input_person, "entity", "smoke-person", False),))
    input_plan = input_builder.finalize_rows((input_person,))
    oversized_chunk = "x" * ((4 * 1_024 * 1_024) // 5 + 32)
    _assert_diagnostic(
        "query_invocation_input_byte_limit",
        lambda: input_plan.exists(tuple((oversized_chunk,) for _ in range(5))),
    )
    _assert_diagnostic(
        "query_builder_scalar_host_type",
        lambda: input_plan.exists(
            ((object(),),),  # pyright: ignore[reportArgumentType]
        ),
    )
    assert input_plan.exists(((None,),)).required_transport_capabilities == (
        "query.input.given-rows",
    )


def test_public_collection_container_type_is_canonical_and_huge_lengths_are_bounded() -> None:
    class HugeLengthPatterns:
        accessed = False

        def __len__(self) -> int:
            return 1 << 100

        def __getitem__(self, _index: int) -> object:
            self.accessed = True
            raise AssertionError("pattern access must not occur")

    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    relation = builder.binding("relation")

    huge_patterns = HugeLengthPatterns()
    _assert_diagnostic(
        "query_plan_pattern_limit",
        lambda: builder.match(
            huge_patterns,  # pyright: ignore[reportArgumentType]
        ),
    )
    assert not huge_patterns.accessed
    _assert_diagnostic(
        "query_builder_host_collection_type",
        lambda: builder.links(
            relation,
            "smoke-relation",
            "role",  # pyright: ignore[reportArgumentType]
            (person,),
        ),
    )
    _assert_diagnostic(
        "query_builder_host_collection_type",
        lambda: builder.match(
            object(),  # pyright: ignore[reportArgumentType]
        ),
    )


def test_public_invocation_row_arity_preflights_before_cell_access() -> None:
    class HostileWrongArityRow:
        accessed = False

        def __len__(self) -> int:
            return 2

        def __getitem__(self, _index: int) -> object:
            self.accessed = True
            raise AssertionError("cell access must not occur")

    class HostileNoInputRows:
        accessed = False

        def __len__(self) -> int:
            return 1

        def __getitem__(self, _index: int) -> object:
            self.accessed = True
            raise AssertionError("row access must not occur")

    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    input_builder = public.QueryPlanBuilder(authority)
    input_builder.input("supplied_text", "string", True)
    input_person = input_builder.binding("person")
    input_builder.match((input_builder.isa(input_person, "entity", "smoke-person", False),))
    input_plan = input_builder.finalize_rows((input_person,))

    wrong_arity = HostileWrongArityRow()
    _assert_diagnostic(
        "query_invocation_row_arity",
        lambda: input_plan.exists(
            (wrong_arity,),  # pyright: ignore[reportArgumentType]
        ),
    )
    assert not wrong_arity.accessed

    no_input_builder = public.QueryPlanBuilder(authority)
    no_input_person = no_input_builder.binding("person")
    no_input_builder.match(
        (no_input_builder.isa(no_input_person, "entity", "smoke-person", False),)
    )
    no_input_plan = no_input_builder.finalize_rows((no_input_person,))
    hostile_no_input = HostileNoInputRows()
    _assert_diagnostic(
        "query_invocation_unexpected_inputs",
        lambda: no_input_plan.rows(
            hostile_no_input,  # pyright: ignore[reportArgumentType]
        ),
    )
    assert not hostile_no_input.accessed


def test_public_operation_specific_collection_limits_and_node_budget_are_canonical() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    relation = builder.binding("relation")
    assigned = builder.binding("assigned")
    person_isa = builder.isa(person, "entity", "smoke-person", False)
    operand = builder.binding_operand(person)
    assignment = builder.reduce_assignment(assigned, "count")
    order = builder.order(person, "ascending")
    too_many_patterns = (person_isa,) * 257
    too_many_bindings = (person,) * 257

    for code, operation in (
        (
            "query_plan_role_player_limit",
            lambda: builder.links(
                relation,
                "smoke-relation",
                ("role",) * 257,
                (person,),
            ),
        ),
        (
            "query_plan_role_player_limit",
            lambda: builder.links(
                relation,
                "smoke-relation",
                ("role",),
                too_many_bindings,
            ),
        ),
        (
            "query_plan_negation_term_limit",
            lambda: builder.not_(too_many_patterns),
        ),
        (
            "query_plan_disjunction_term_limit",
            lambda: builder.or_(tuple((person_isa,) for _ in range(257))),
        ),
        (
            "query_plan_disjunction_term_limit",
            lambda: builder.or_((too_many_patterns,)),
        ),
        (
            "query_plan_try_term_limit",
            lambda: builder.try_(too_many_patterns),
        ),
        (
            "query_plan_function_argument_limit",
            lambda: builder.function_call(
                assigned,
                (operand,) * 257,
                function_name="unknown_function",
            ),
        ),
        (
            "query_plan_pattern_limit",
            lambda: builder.match(too_many_patterns),
        ),
        (
            "query_plan_binding_limit",
            lambda: builder.select(too_many_bindings),
        ),
        (
            "query_plan_binding_limit",
            lambda: builder.require(too_many_bindings),
        ),
        (
            "query_plan_reduce_term_limit",
            lambda: builder.reduce((assignment,) * 257, ()),
        ),
        (
            "query_plan_binding_limit",
            lambda: builder.reduce((assignment,), too_many_bindings),
        ),
        (
            "query_plan_sort_term_limit",
            lambda: builder.sort((order,) * 65),
        ),
    ):
        _assert_diagnostic(code, operation)

    wide = builder.not_((person_isa,) * 256)
    _assert_diagnostic(
        "query_plan_pattern_node_limit",
        lambda: builder.not_((wide,) * 16),
    )

    builder.match((person_isa,))
    assert builder.finalize_rows((person,)).format == "typebridge.query-plan/v2"

    input_builder = public.QueryPlanBuilder(authority)
    input_builder.input("input", "string", True)
    input_person = input_builder.binding("person")
    input_builder.match((input_builder.isa(input_person, "entity", "smoke-person", False),))
    input_plan = input_builder.finalize_rows((input_person,))
    _assert_diagnostic(
        "query_invocation_row_arity",
        lambda: input_plan.exists(((None,) * 257,)),
    )


def test_public_groupby_accepts_sixty_five_groups_and_rejects_the_257th() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    owner = builder.binding("group_owner")
    attributes = tuple(builder.binding(f"group_{index}") for index in range(64))
    groups = (owner, *attributes)
    assigned = builder.binding("count")
    builder.match(
        (
            builder.isa(owner, "entity", "smoke-person", False),
            *(builder.has(owner, attribute, "smoke-name") for attribute in attributes),
        )
    )
    count = builder.reduce_assignment(assigned, "count")
    _assert_diagnostic(
        "query_plan_binding_limit",
        lambda: builder.reduce((count,), (groups[0],) * 257),
    )
    builder.reduce((count,), groups)
    assert builder.finalize_rows((groups[0], assigned)).format == ("typebridge.query-plan/v2")


def test_public_declaration_and_output_boundaries_use_stable_binding_diagnostics() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )

    binding_builder = public.QueryPlanBuilder(authority)
    for index in range(256):
        binding_builder.binding(f"binding_{index}")
    _assert_diagnostic(
        "query_builder_authored_binding_limit",
        lambda: binding_builder.binding("binding_256"),
    )

    input_builder = public.QueryPlanBuilder(authority)
    for index in range(256):
        input_builder.input(f"input_{index}", "string", True)
    _assert_diagnostic(
        "query_plan_input_limit",
        lambda: input_builder.input("input_256", "string", True),
    )

    row_builder = public.QueryPlanBuilder(authority)
    row_owner = row_builder.binding("row_owner")
    row_attributes = tuple(row_builder.binding(f"row_{index}") for index in range(16))
    row_bindings = (row_owner, *row_attributes)
    row_builder.match(
        (
            row_builder.isa(row_owner, "entity", "smoke-person", False),
            *(row_builder.has(row_owner, attribute, "smoke-name") for attribute in row_attributes),
        )
    )
    _assert_diagnostic(
        "query_plan_output_limit",
        lambda: row_builder.finalize_rows(row_bindings),
    )
    assert row_builder.finalize_rows(row_bindings[:16]).format == "typebridge.query-plan/v2"

    document_builder = public.QueryPlanBuilder(authority)
    person = document_builder.binding("document_person")
    name = document_builder.binding("document_name")
    document_builder.match(
        (
            document_builder.isa(person, "entity", "smoke-person", False),
            document_builder.has(person, name, "smoke-name"),
        )
    )
    fields = tuple(document_builder.document_binding(f"field_{index}", name) for index in range(17))
    _assert_diagnostic(
        "query_plan_output_limit",
        lambda: document_builder.finalize_documents(fields),
    )
    assert document_builder.finalize_documents(fields[:16]).format == ("typebridge.query-plan/v2")


def test_dynamic_host_flags_text_and_labels_fail_canonically_and_atomically() -> None:
    authority = public.QueryV2Authority(
        _DECLARED_SCHEMA,
        "binding-smoke",
        "typedb-3.12.1/v1",
    )

    input_builder = public.QueryPlanBuilder(authority)
    _assert_diagnostic(
        "query_builder_boolean_host_type",
        lambda: input_builder.input(
            "prefix",
            "string",
            1,  # pyright: ignore[reportArgumentType]
        ),
    )
    input_builder.input("prefix", "string", True)

    builder = public.QueryPlanBuilder(authority)
    person = builder.binding("person")
    _assert_diagnostic(
        "query_builder_boolean_host_type",
        lambda: builder.isa(
            person,
            "entity",
            "smoke-person",
            1,  # pyright: ignore[reportArgumentType]
        ),
    )
    _assert_diagnostic(
        "query_builder_scalar_host_type",
        lambda: builder.literal_operand(
            "string",
            1,  # pyright: ignore[reportArgumentType, reportCallIssue]
        ),
    )
    _assert_diagnostic(
        "query_builder_scalar_unicode",
        lambda: builder.literal_operand("string", "\ud800"),
    )
    _assert_diagnostic(
        "migration_assertion_invalid_variable",
        lambda: builder.binding(""),
    )
    _assert_diagnostic(
        "malformed_id",
        lambda: builder.isa(person, "entity", "person name", False),
    )

    name = builder.binding("name")
    builder.match(
        (
            builder.isa(person, "entity", "smoke-person", False),
            builder.has(person, name, "smoke-name"),
        )
    )
    assert builder.finalize_rows((person, name)).format == "typebridge.query-plan/v2"


def test_every_builder_operation_executes_with_cross_binding_canonical_parity() -> None:
    authority = public.QueryV2Authority(
        _PARITY_DECLARED_SCHEMA,
        "model-remote-parity",
        "typedb-3.12.1/v1",
    )
    invoked: set[str] = set()

    def invoke[T](name: str, operation: Callable[[], T]) -> T:
        invoked.add(name)
        return operation()

    builder = public.QueryPlanBuilder(authority)
    local_person = invoke("binding", lambda: builder.binding("lp"))
    local_name = builder.binding("ln")
    local_isa = invoke(
        "isa",
        lambda: builder.isa(local_person, "entity", "parity-person", True),
    )
    local_has = invoke(
        "has",
        lambda: builder.has(local_person, local_name, "parity-person-name"),
    )
    local_return = invoke(
        "local_return",
        lambda: builder.local_return("count", local_name, "long"),
    )
    local_function = invoke(
        "local_function",
        lambda: builder.local_function(
            "local_name_count",
            (local_name, local_person),
            (local_person,),
            ("parity-person",),
            (local_isa, local_has),
            local_return,
        ),
    )

    person = builder.binding("person")
    name = builder.binding("name")
    optional_name = builder.binding("optional_name")
    local_result = builder.binding("local_result")
    count_result = builder.binding("count_result")
    wanted_name = invoke(
        "input",
        lambda: builder.input("wanted_name", "string", False),
    )
    person_isa = builder.isa(person, "entity", "parity-person", True)
    name_has = builder.has(person, name, "parity-person-name")
    name_operand = invoke(
        "binding_operand",
        lambda: builder.binding_operand(name),
    )
    input_operand = invoke(
        "input_operand",
        lambda: builder.input_operand(wanted_name),
    )
    nobody = invoke(
        "literal_operand",
        lambda: builder.literal_operand("string", "nobody"),
    )
    equal = invoke(
        "value",
        lambda: builder.value("equal", name_operand, input_operand),
    )
    not_equal = builder.value("not_equal", name_operand, nobody)
    disjunction = invoke(
        "or",
        lambda: builder.or_(((equal,), (not_equal,))),
    )
    negation = invoke(
        "not",
        lambda: builder.not_((builder.value("equal", name_operand, nobody),)),
    )
    optional = invoke(
        "try",
        lambda: builder.try_((builder.has(person, optional_name, "parity-person-name"),)),
    )
    local_call = invoke(
        "function_call",
        lambda: builder.function_call(
            local_result,
            (builder.binding_operand(person),),
            None,
            local_function=local_function,
        ),
    )
    invoke(
        "match",
        lambda: builder.match(
            (
                person_isa,
                name_has,
                disjunction,
                negation,
                optional,
                local_call,
            )
        ),
    )
    invoke("select", lambda: builder.select((person, name, local_result)))
    invoke("require", lambda: builder.require((name,)))
    invoke("distinct", builder.distinct)
    count = invoke(
        "reduce_assignment",
        lambda: builder.reduce_assignment(count_result, "count"),
    )
    invoke("reduce", lambda: builder.reduce((count,), (name,)))
    name_order = invoke(
        "order",
        lambda: builder.order(name, "ascending"),
    )
    count_order = builder.order(count_result, "descending")
    invoke("sort", lambda: builder.sort((name_order, count_order)))
    invoke("offset", lambda: builder.offset(0))
    invoke("limit", lambda: builder.limit(10))
    advanced = invoke(
        "finalize_rows",
        lambda: builder.finalize_rows((name, count_result)),
    )

    relation_builder = public.QueryPlanBuilder(authority)
    source = relation_builder.binding("source")
    target = relation_builder.binding("target")
    assignment = relation_builder.binding("assignment")
    source_isa = relation_builder.isa(source, "entity", "parity-person", True)
    target_isa = relation_builder.isa(target, "entity", "parity-project", False)
    links = invoke(
        "links",
        lambda: relation_builder.links(
            assignment,
            "parity-assignment",
            ("employee", "project"),
            (source, target),
        ),
    )
    reachable = invoke(
        "reachable",
        lambda: relation_builder.reachable(
            source,
            target,
            "parity-assignment",
            "employee",
            "project",
            1,
            1,
        ),
    )
    relation_builder.match((source_isa, target_isa, links, reachable))
    relation = relation_builder.finalize_rows((source, target, assignment))

    document_builder = public.QueryPlanBuilder(authority)
    document_person = document_builder.binding("person")
    document_name = document_builder.binding("name")
    document_builder.match(
        (
            document_builder.isa(document_person, "entity", "parity-person", True),
            document_builder.has(
                document_person,
                document_name,
                "parity-person-name",
            ),
        )
    )
    scalar = invoke(
        "document_binding",
        lambda: document_builder.document_binding("primary_name", document_name),
    )
    attribute_list = invoke(
        "document_attribute_list",
        lambda: document_builder.document_attribute_list(
            "all_names",
            document_person,
            "parity-person-name",
        ),
    )
    documents = invoke(
        "finalize_documents",
        lambda: document_builder.finalize_documents((scalar, attribute_list)),
    )

    assert invoked == _BUILDER_OPERATIONS
    assert advanced.fingerprint == (
        "85c9504dca956286b46336510af3b24980bba1a72e79465069b7a24e7d52e26f"
    )
    assert relation.fingerprint == (
        "0c955b27ba7df589499245fcc8d47f1a14e555a34c15fe8177c07bb8c4293aa8"
    )
    assert documents.fingerprint == (
        "e25be2c81dd1c2252967d889e001a713942d8850af3ae232086bac295752f731"
    )
    assert advanced.rows((("Alice",),)).plan_fingerprint == advanced.fingerprint
    assert documents.documents(()).plan_fingerprint == documents.fingerprint


def test_reducer_error_preserves_the_complete_shared_diagnostic() -> None:
    authority = public.QueryV2Authority(
        _PARITY_DECLARED_SCHEMA,
        "model-remote-parity",
        "typedb-3.12.1/v1",
    )
    builder = public.QueryPlanBuilder(authority)
    assigned = builder.binding("assigned")
    with pytest.raises(native.QueryV2Error) as captured:
        builder.reduce_assignment(
            assigned,
            "max",
            None,  # pyright: ignore[reportArgumentType, reportCallIssue]
        )
    error = captured.value
    assert (
        error.category,
        error.code,
        error.message,
        error.path,
        dict(error.details),
    ) == (
        "invalid_contract",
        "query_plan_reduce_missing_input",
        "count takes no input and every other reducer requires one",
        [],
        {},
    )
