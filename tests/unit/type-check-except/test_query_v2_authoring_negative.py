"""Intentional static failures for the low-level query-v2 authoring builder."""

from __future__ import annotations

from type_bridge.query_v2 import QueryPlanBuilder, QueryV2Authority


def invalid_query_v2_authoring(authority: QueryV2Authority) -> None:
    builder = QueryPlanBuilder(authority)
    value = builder.binding("value")
    builder.isa(value, "struct", "record", False)  # query-v2-error: struct-is-not-queryable
    operand = builder.binding_operand(value)
    local_return = builder.local_return("count", value, "long")
    local = builder.local_function(
        "local",
        (value,),
        (value,),
        ("value",),
        (builder.value("equal", operand, operand),),
        local_return,
    )

    builder.literal_operand("double", 1)  # query-v2-error: exact-double
    builder.literal_operand("boolean", 1)  # query-v2-error: boolean-value
    builder.function_call(value, ())  # query-v2-error: missing-function-target
    builder.function_call(  # query-v2-error: duplicate-function-target
        value,
        (),
        "schema_function",
        local,  # query-v2-error: duplicate-function-argument
    )
    builder.reduce_assignment(  # query-v2-error: count-with-input
        value,
        "count",
        value,  # query-v2-error: count-input-argument
    )
    builder.reduce_assignment(value, "sum")  # query-v2-error: sum-without-input
    builder.local_return("max", value, "long")  # query-v2-error: local-max
    builder.local_return(  # query-v2-error: local-count-double
        "count",
        value,
        "double",  # query-v2-error: local-count-value-type
    )
    builder.local_return(  # query-v2-error: local-sum-boolean
        "sum",
        value,
        "boolean",  # query-v2-error: local-sum-value-type
    )
