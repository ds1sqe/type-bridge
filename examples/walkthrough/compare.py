"""Compare JSON-line walkthrough transcripts without changing SDK output."""

from __future__ import annotations

import argparse
import json
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcripts", type=Path, nargs="+")
    args = parser.parse_args()
    transcripts = {path.stem: read(path) for path in args.transcripts}
    if len(transcripts) < 2 or len(transcripts) != len(args.transcripts):
        parser.error("supply at least two transcripts with distinct SDK filenames")
    compare(transcripts)
    print(f"Matched {len(next(iter(transcripts.values())))} cases across {len(transcripts)} SDKs")


if __name__ == "__main__":
    main()
