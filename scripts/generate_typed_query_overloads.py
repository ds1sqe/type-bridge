#!/usr/bin/env python3
"""Generate and drift-check direct/remote typed-query overload matrices."""

from __future__ import annotations

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SESSION_TARGET = ROOT / "type_bridge" / "typed" / "session.py"
QUERY_TARGET = ROOT / "type_bridge" / "typed" / "query.py"
REMOTE_SESSION_TARGET = ROOT / "type_bridge" / "typed" / "remote_session.py"
REMOTE_QUERY_TARGET = ROOT / "type_bridge" / "typed" / "remote_query.py"
GENERATED_PYTHON_QUERY_TARGET = (
    ROOT / "type-bridge-core" / "crates" / "schema-codegen" / "src" / "python" / "query.pyi"
)
QUERY_START = "    # BEGIN GENERATED QUERY OVERLOADS"
QUERY_END = "    # END GENERATED QUERY OVERLOADS"
PAGE_START = "    # BEGIN GENERATED PAGE OVERLOADS"
PAGE_END = "    # END GENERATED PAGE OVERLOADS"
REMOTE_QUERY_START = "    # BEGIN GENERATED REMOTE QUERY OVERLOADS"
REMOTE_QUERY_END = "    # END GENERATED REMOTE QUERY OVERLOADS"
REMOTE_PAGE_START = "    # BEGIN GENERATED REMOTE PAGE OVERLOADS"
REMOTE_PAGE_END = "    # END GENERATED REMOTE PAGE OVERLOADS"
AGGREGATE_START = "    # BEGIN GENERATED AGGREGATE OVERLOADS"
AGGREGATE_END = "    # END GENERATED AGGREGATE OVERLOADS"
GROUP_BY_START = "    # BEGIN GENERATED GROUP BY OVERLOADS"
GROUP_BY_END = "    # END GENERATED GROUP BY OVERLOADS"
GROUPED_AGGREGATE_START = "    # BEGIN GENERATED GROUPED AGGREGATE OVERLOADS"
GROUPED_AGGREGATE_END = "    # END GENERATED GROUPED AGGREGATE OVERLOADS"
REMOTE_AGGREGATE_START = "    # BEGIN GENERATED REMOTE AGGREGATE OVERLOADS"
REMOTE_AGGREGATE_END = "    # END GENERATED REMOTE AGGREGATE OVERLOADS"
REMOTE_GROUP_BY_START = "    # BEGIN GENERATED REMOTE GROUP BY OVERLOADS"
REMOTE_GROUP_BY_END = "    # END GENERATED REMOTE GROUP BY OVERLOADS"
REMOTE_GROUPED_AGGREGATE_START = "    # BEGIN GENERATED REMOTE GROUPED AGGREGATE OVERLOADS"
REMOTE_GROUPED_AGGREGATE_END = "    # END GENERATED REMOTE GROUPED AGGREGATE OVERLOADS"


def render_query_overloads(
    query_type: str = "Query",
    *,
    asynchronous: bool = False,
) -> str:
    function = "async def" if asynchronous else "def"
    blocks: list[str] = []
    for arity in range(1, 17):
        parameters = ",\n".join(
            f"        selection{index}: Selection[T{index}]" for index in range(1, arity + 1)
        )
        slots = ", ".join(f"T{index}" for index in range(1, arity + 1))
        type_parameters = ", ".join(f"T{index}" for index in range(1, arity + 1))
        blocks.append(
            "    @overload\n"
            f"    {function} query[{type_parameters}](\n"
            "        self,\n"
            f"{parameters},\n"
            "        /,\n"
            f"    ) -> {query_type}[{slots}]: ..."
        )
    return "\n\n".join(blocks)


def render_page_overloads(
    query_type: str = "Query",
    *,
    asynchronous: bool = False,
    model_bound: str = "TypeDBType",
) -> str:
    function = "async def" if asynchronous else "def"
    options = (
        "        *,\n"
        "        limit: int,\n"
        "        offset: int = 0,\n"
        "        order_by: Iterable[QueryOrder] = (),\n"
        "        include_total: bool = False,\n"
    )
    blocks = [
        "    @overload\n"
        f"    {function} page_by[SlotT, RootT: {model_bound}](\n"
        f"        self: {query_type}[SlotT],\n"
        "        root: BoundVar[RootT],\n"
        f"{options}"
        "    ) -> Page[SlotT]: ..."
    ]
    for arity in range(2, 17):
        for root_position in range(1, arity + 1):
            collected_types = [
                f"Collected{index}T" for index in range(1, arity + 1) if index != root_position
            ]
            type_parameters = ", ".join(
                [f"RootT: {model_bound}"] + [f"{name}: {model_bound}" for name in collected_types]
            )
            slots = [
                "RootT" if index == root_position else f"tuple[Collected{index}T, ...]"
                for index in range(1, arity + 1)
            ]
            row = ", ".join(slots)
            blocks.append(
                "    @overload\n"
                f"    {function} page_by[{type_parameters}](\n"
                f"        self: {query_type}[{row}],\n"
                "        root: BoundVar[RootT],\n"
                f"{options}"
                f"    ) -> Page[tuple[{row}]]: ..."
            )
    return "\n\n".join(blocks)


def render_aggregate_overloads(
    *,
    grouped: bool = False,
    asynchronous: bool = False,
) -> str:
    function = "async def" if asynchronous else "def"
    blocks: list[str] = []
    for arity in range(1, 17):
        outputs = [f"Output{index}T" for index in range(1, arity + 1)]
        type_parameters = outputs if grouped else ["RootT: ModelBase", *outputs]
        parameters = [] if grouped else ["        root: BoundVar[RootT]"]
        parameters.extend(
            f"        term{index}: Aggregate[Output{index}T]" for index in range(1, arity + 1)
        )
        values = ", ".join(outputs)
        result = f"tuple[{values}]"
        if grouped:
            result = f"tuple[tuple[GroupT, {result}], ...]"
        blocks.append(
            "    @overload\n"
            f"    {function} aggregate[{', '.join(type_parameters)}](\n"
            "        self,\n"
            f"{',\n'.join(parameters)},\n"
            "        /,\n"
            f"    ) -> {result}: ..."
        )
    return "\n\n".join(blocks)


def render_group_by_overloads(*, remote: bool = False) -> str:
    grouped_type = "RemoteGroupedQuery" if remote else "GroupedQuery"
    blocks = [
        "    @overload\n"
        "    def group_by[RootT: ModelBase, GroupT: ModelBase](\n"
        "        self,\n"
        "        root: BoundVar[RootT],\n"
        "        group: BoundVar[GroupT],\n"
        f"    ) -> {grouped_type}[GroupT]: ...",
        "    @overload\n"
        "    def group_by[RootT: ModelBase, GroupT: AttributeBase](\n"
        "        self,\n"
        "        root: BoundVar[RootT],\n"
        "        group: BoundField[GroupT],\n"
        f"    ) -> {grouped_type}[GroupT]: ...",
    ]
    for arity in range(2, 17):
        groups = [f"Group{index}T" for index in range(1, arity + 1)]
        type_parameters = ", ".join(
            ["RootT: ModelBase", *[f"{group}: AttributeBase" for group in groups]]
        )
        parameters = ",\n".join(
            f"        group{index}: BoundField[Group{index}T]" for index in range(1, arity + 1)
        )
        blocks.append(
            "    @overload\n"
            f"    def group_by[{type_parameters}](\n"
            "        self,\n"
            "        root: BoundVar[RootT],\n"
            f"{parameters},\n"
            f"    ) -> {grouped_type}[tuple[{', '.join(groups)}]]: ..."
        )
    return "\n\n".join(blocks)


def updated_source(source: str, start: str, end: str, rendered: str) -> str:
    before, rest = source.split(f"{start}\n", 1)
    _, after = rest.split(end, 1)
    return f"{before}{start}\n{rendered}\n\n{end}{after}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    arguments = parser.parse_args()
    session_source = SESSION_TARGET.read_text()
    expected_session = updated_source(
        session_source,
        QUERY_START,
        QUERY_END,
        render_query_overloads(),
    )
    query_source = QUERY_TARGET.read_text()
    expected_query = updated_source(
        query_source,
        PAGE_START,
        PAGE_END,
        render_page_overloads(),
    )
    remote_session_source = REMOTE_SESSION_TARGET.read_text()
    expected_remote_session = updated_source(
        remote_session_source,
        REMOTE_QUERY_START,
        REMOTE_QUERY_END,
        render_query_overloads("RemoteQuery"),
    )
    remote_query_source = REMOTE_QUERY_TARGET.read_text()
    expected_remote_query = updated_source(
        remote_query_source,
        REMOTE_PAGE_START,
        REMOTE_PAGE_END,
        render_page_overloads("RemoteQuery", asynchronous=True),
    )
    generated_python_source = GENERATED_PYTHON_QUERY_TARGET.read_text()
    expected_generated_python = updated_source(
        generated_python_source,
        QUERY_START,
        QUERY_END,
        render_query_overloads(),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        PAGE_START,
        PAGE_END,
        render_page_overloads(model_bound="ModelBase"),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        REMOTE_QUERY_START,
        REMOTE_QUERY_END,
        render_query_overloads("RemoteQuery"),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        REMOTE_PAGE_START,
        REMOTE_PAGE_END,
        render_page_overloads(
            "RemoteQuery",
            asynchronous=True,
            model_bound="ModelBase",
        ),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        AGGREGATE_START,
        AGGREGATE_END,
        render_aggregate_overloads(),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        GROUP_BY_START,
        GROUP_BY_END,
        render_group_by_overloads(),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        GROUPED_AGGREGATE_START,
        GROUPED_AGGREGATE_END,
        render_aggregate_overloads(grouped=True),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        REMOTE_AGGREGATE_START,
        REMOTE_AGGREGATE_END,
        render_aggregate_overloads(asynchronous=True),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        REMOTE_GROUP_BY_START,
        REMOTE_GROUP_BY_END,
        render_group_by_overloads(remote=True),
    )
    expected_generated_python = updated_source(
        expected_generated_python,
        REMOTE_GROUPED_AGGREGATE_START,
        REMOTE_GROUPED_AGGREGATE_END,
        render_aggregate_overloads(grouped=True, asynchronous=True),
    )
    if arguments.write:
        SESSION_TARGET.write_text(expected_session)
        QUERY_TARGET.write_text(expected_query)
        REMOTE_SESSION_TARGET.write_text(expected_remote_session)
        REMOTE_QUERY_TARGET.write_text(expected_remote_query)
        GENERATED_PYTHON_QUERY_TARGET.write_text(expected_generated_python)
        return
    if (
        session_source != expected_session
        or query_source != expected_query
        or remote_session_source != expected_remote_session
        or remote_query_source != expected_remote_query
        or generated_python_source != expected_generated_python
    ):
        raise SystemExit(
            "typed-query overloads drifted; run "
            "`python scripts/generate_typed_query_overloads.py --write`"
        )


if __name__ == "__main__":
    main()
