#!/usr/bin/env python3
"""Generate and drift-check QuerySession.query overloads for slots 1 through 16."""

from __future__ import annotations

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SESSION_TARGET = ROOT / "type_bridge" / "typed" / "session.py"
QUERY_TARGET = ROOT / "type_bridge" / "typed" / "query.py"
QUERY_START = "    # BEGIN GENERATED QUERY OVERLOADS"
QUERY_END = "    # END GENERATED QUERY OVERLOADS"
PAGE_START = "    # BEGIN GENERATED PAGE OVERLOADS"
PAGE_END = "    # END GENERATED PAGE OVERLOADS"


def render_query_overloads() -> str:
    blocks: list[str] = []
    for arity in range(1, 17):
        parameters = ",\n".join(
            f"        selection{index}: Selection[T{index}]" for index in range(1, arity + 1)
        )
        slots = ", ".join(f"T{index}" for index in range(1, arity + 1))
        type_parameters = ", ".join(f"T{index}" for index in range(1, arity + 1))
        blocks.append(
            "    @overload\n"
            f"    def query[{type_parameters}](\n"
            "        self,\n"
            f"{parameters},\n"
            "        /,\n"
            f"    ) -> Query[{slots}]: ..."
        )
    return "\n\n".join(blocks)


def render_page_overloads() -> str:
    options = (
        "        *,\n"
        "        limit: int,\n"
        "        offset: int = 0,\n"
        "        order_by: Iterable[QueryOrder] = (),\n"
        "        include_total: bool = False,\n"
    )
    blocks = [
        "    @overload\n"
        "    def page_by[SlotT, RootT: TypeDBType](\n"
        "        self: Query[SlotT],\n"
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
                ["RootT: TypeDBType"] + [f"{name}: TypeDBType" for name in collected_types]
            )
            slots = [
                "RootT" if index == root_position else f"tuple[Collected{index}T, ...]"
                for index in range(1, arity + 1)
            ]
            row = ", ".join(slots)
            blocks.append(
                "    @overload\n"
                f"    def page_by[{type_parameters}](\n"
                f"        self: Query[{row}],\n"
                "        root: BoundVar[RootT],\n"
                f"{options}"
                f"    ) -> Page[tuple[{row}]]: ..."
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
    if arguments.write:
        SESSION_TARGET.write_text(expected_session)
        QUERY_TARGET.write_text(expected_query)
        return
    if session_source != expected_session or query_source != expected_query:
        raise SystemExit(
            "typed-query overloads drifted; run "
            "`python scripts/generate_typed_query_overloads.py --write`"
        )


if __name__ == "__main__":
    main()
