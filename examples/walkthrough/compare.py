"""Compare JSON-line walkthrough transcripts without changing SDK output."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


def read(path: Path) -> dict[str, dict[str, Any]]:
    cases: dict[str, dict[str, Any]] = {}
    for line in path.read_text().splitlines():
        record = json.loads(line)
        case = cases.setdefault(record["case"], {})
        step = record["step"]
        if step in case:
            raise ValueError(f"{path}: duplicate {record['case']}/{step}")
        case[step] = record["value"]
    if not cases:
        raise ValueError(f"{path}: empty transcript")
    for name, case in cases.items():
        expected = (
            ["call", "unchecked", "raw"]
            if name == "raw_unchecked"
            else ["call", "plan", "explain", "raw", "models"]
        )
        if list(case) != expected:
            raise ValueError(f"{path}: {name} steps must be {expected}")
    return cases


def normalize(value: Any) -> Any:
    iids: dict[str, str] = {}

    def visit(item: Any, key: str = "") -> Any:
        if isinstance(item, dict):
            return {name: visit(item[name], name) for name in sorted(item)}
        if isinstance(item, list):
            return [visit(member, key) for member in item]
        if isinstance(item, str) and (key == "iid" or key.endswith("_iid")):
            return iids.setdefault(item, f"iid:{len(iids)}")
        return item

    return visit(value)


MANAGER_CASES = {
    "insert",
    "put",
    "typed_iid",
    "filtered_fetch",
    "update",
    "insert_many",
    "delete_returning",
}
RAW_SEAM = "raw documents: not exposed by manager result"


def compare(transcripts: dict[str, dict[str, dict[str, Any]]]) -> None:
    first = next(iter(transcripts.values()))
    for sdk, cases in transcripts.items():
        if list(cases) != list(first):
            raise ValueError(f"{sdk}: cases or their order differ")
        for name, case in cases.items():
            if name == "raw_unchecked":
                continue
            for step in ("plan", "explain", "models"):
                actual, expected = case[step], first[name][step]
                if step == "models":
                    actual, expected = normalize(actual), normalize(expected)
                if actual != expected:
                    raise ValueError(f"{sdk}: {name}/{step} differs")
    for name in first:
        exposed = {}
        for sdk, cases in transcripts.items():
            raw = cases[name]["raw"]
            if raw == RAW_SEAM:
                if name not in MANAGER_CASES:
                    raise ValueError(f"{sdk}: {name}/raw must expose typed-plan documents")
                continue
            exposed[sdk] = normalize(raw)
        if exposed:
            expected = next(iter(exposed.values()))
            for sdk, actual in exposed.items():
                if actual != expected:
                    raise ValueError(f"{sdk}: {name}/raw differs")


def pretty(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False)


def page_blocks(
    transcripts: dict[str, dict[str, dict[str, Any]]],
) -> dict[str, tuple[str, str]]:
    """The page uses exact plan bytes and unmodified statement strings."""
    blocks = {}
    first = next(iter(transcripts.values()))
    for name, case in first.items():
        for sdk, cases in transcripts.items():
            blocks[f"{name}:call:{sdk}"] = (sdk, cases[name]["call"])
        if name != "raw_unchecked":
            blocks[f"{name}:plan"] = ("json", pretty(case["plan"]))
            explanations = case["explain"].get("plans", [case["explain"]])
            statements = [statement for plan in explanations for statement in plan["statements"]]
            blocks[f"{name}:typeql"] = ("typeql", "\n\n".join(statements))
            inputs = [
                {"given": plan["given"], "inputs": plan["inputs"]}
                for plan in explanations
                if "given" in plan
            ]
            if inputs:
                blocks[f"{name}:inputs"] = ("json", pretty(inputs))
        for step in ("raw", "models"):
            if step not in case:
                continue
            value = normalize(case[step])
            if isinstance(value, list) and len(value) > 10:
                value = {"count": len(value), "first": value[:2], "last": value[-1:]}
            blocks[f"{name}:{step}"] = ("json", pretty(value))
    return blocks


def check_page(path: Path, transcripts: dict[str, dict[str, dict[str, Any]]]) -> None:
    # Decode bytes directly: universal-newline translation must not hide drift.
    page = path.read_bytes().decode("utf-8")
    pattern = r"<!-- walkthrough:([^\n]+) -->\n```([^\n]+)\n(.*?)\n```"
    found = {}
    for marker, language, content in re.findall(pattern, page, flags=re.DOTALL):
        if marker in found:
            raise ValueError(f"{path}: duplicate page block {marker}")
        found[marker] = (language, content)
    expected = page_blocks(transcripts)
    # A focused --sdk run still checks every shared block and its own call.
    for marker, block in expected.items():
        if marker not in found:
            raise ValueError(f"{path}: missing page block {marker}")
        if found[marker] != block:
            raise ValueError(f"{path}: page block {marker} differs byte for byte")
    sdk_names = {"python", "typescript", "rust", "c"}
    for marker in found.keys() - expected.keys():
        parts = marker.split(":")
        if (
            len(parts) == 3
            and parts[1] == "call"
            and parts[2] in sdk_names - transcripts.keys()
            and parts[0] in next(iter(transcripts.values()))
        ):
            continue
        raise ValueError(f"{path}: unexpected page block {marker}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcripts", type=Path, nargs="+")
    parser.add_argument("--page", type=Path)
    args = parser.parse_args()
    transcripts = {path.stem: read(path) for path in args.transcripts}
    if len(transcripts) != len(args.transcripts):
        parser.error("supply transcripts with distinct SDK filenames")
    compare(transcripts)
    if args.page:
        check_page(args.page, transcripts)
        print(f"Checked page blocks byte for byte: {args.page}")
    print(f"Matched {len(next(iter(transcripts.values())))} cases across {len(transcripts)} SDKs")


if __name__ == "__main__":
    main()
