"""Deterministic data and independent predicate witnesses for chain/star queries."""

from __future__ import annotations

from collections.abc import Callable
from typing import TypedDict


class Record(TypedDict):
    uid: str
    kind: str
    tags: list[str]
    score: int | None
    nickname: str | None
    note: str | None
    company: str
    blocked: bool
    priority: int
    task: str
    done: bool
    site: str


PREDICATE_WIDTH = {
    "tag-a": 2,
    "tag-z": 2,
    "score-min": 2,
    "note-present": 2,
    "note-prefix": 2,
    "identifier-choice": 2,
    "company-prefix": 4,
    "unblocked": 6,
    "site-match": 8,
    "priority-min": 10,
    "cross-field": 10,
    "task-choice": 12,
}


def records(size: int) -> list[Record]:
    if size < 1:
        raise ValueError("fixture size must be positive")
    rows: list[Record] = [
        Record(
            uid=f"P{i:04d}",
            kind="employee" if i % 2 == 0 else "person",
            tags=["a", "z"] if i % 2 == 0 else ["n"],
            score=i * 10 if i % 3 else None,
            nickname=f"Alpha{i}" if i % 4 else None,
            note=f"job-{i:04d}" if i % 5 else None,
            company=f"C{i:04d}",
            blocked=i % 7 == 0,
            priority=41 if i == 4 else i % 3,
            task=f"T{i:04d}",
            done=i % 4 == 0,
            site="North" if i % 2 == 0 else "South",
        )
        for i in range(1, size + 1)
    ]
    # A fixed-size witness set makes every filter independently observable,
    # including at the widest join; ordinary size-N data remains parameterized.
    for i, failed in enumerate(PREDICATE_WIDTH, 1):
        row = Record(
            uid=f"W{i:04d}",
            kind="person",
            tags=["a", "z"],
            score=30,
            nickname=f"Witness{i}" if i % 2 else None,
            note=f"job-W{i:04d}",
            company=f"C-W{i:04d}",
            blocked=False,
            priority=1,
            task=f"T-W{i:04d}",
            done=False,
            site="North",
        )
        if failed == "tag-a":
            row["tags"] = ["z"]
        elif failed == "tag-z":
            row["tags"] = ["a"]
        elif failed == "score-min":
            row["score"] = 5
        elif failed == "note-present":
            row["note"] = None
        elif failed == "note-prefix":
            row["note"] = f"cross-W{i:04d}"
        elif failed == "identifier-choice":
            row["uid"] = f"X{i:04d}"
        elif failed == "company-prefix":
            row["company"] = f"B-W{i:04d}"
        elif failed == "unblocked":
            row["blocked"] = True
        elif failed == "site-match":
            row["site"] = "South"
        elif failed == "priority-min":
            row["priority"] = 0
        elif failed == "cross-field":
            row["priority"] = 31
        elif failed == "task-choice":
            row["done"] = True
        assert {name for name, matches in predicate_values(row).items() if not matches} == {failed}
        rows.append(row)
    return rows


def predicate_values(row: Record) -> dict[str, bool]:
    return {
        "tag-a": "a" in row["tags"],
        "tag-z": "z" in row["tags"],
        "score-min": row["score"] is not None and row["score"] >= 20,
        "note-present": row["note"] is not None,
        "note-prefix": row["note"] is None or not row["note"].startswith("cross-"),
        "identifier-choice": row["uid"].startswith(("P", "W")),
        "company-prefix": row["company"].startswith("C"),
        "unblocked": not row["blocked"],
        "site-match": "north" in row["site"].casefold(),
        "priority-min": row["priority"] >= 1,
        "cross-field": row["score"] is not None and row["score"] >= row["priority"],
        "task-choice": not row["done"] or row["task"].startswith("T0008"),
    }


def matches(row: Record, width: int, omit: str | None = None) -> bool:
    return all(
        value
        for name, value in predicate_values(row).items()
        if PREDICATE_WIDTH[name] <= width and name != omit
    )


def model_ids(size: int) -> list[str]:
    return sorted(row["uid"] for row in records(size))


def tagged_count(size: int) -> int:
    return sum("a" in row["tags"] and "z" in row["tags"] for row in records(size))


def seed_queries(size: int) -> list[str]:
    queries = []
    for index, row in enumerate(records(size), 1):
        fields = [f'has identifier "{row["uid"]}"']
        fields += [f'has tag "{tag}"' for tag in row["tags"]]
        if row["score"] is not None:
            fields.append(f"has score {row['score']}")
        if row["nickname"] is not None:
            fields.append(f'has nickname "{row["nickname"]}"')
        note = f', has note "{row["note"]}"' if row["note"] is not None else ""
        blocked = ", has blocked true" if row["blocked"] else ""
        suffix = row["uid"][1:] if row["uid"].startswith("P") else row["uid"]
        queries.append(f'''insert
$p isa {row["kind"]}, {", ".join(fields)};
$c isa company, has identifier "{row["company"]}";
$d isa department, has identifier "D{suffix}"{blocked};
$j isa project, has identifier "J{suffix}", has priority {row["priority"]};
$t isa task, has identifier "{row["task"]}", has done {str(row["done"]).lower()};
$o isa office, has identifier "O{suffix}", has site "{row["site"]}";
$e isa employment, links (worker: $p, employer: $c){note};
$m isa membership, links (member: $p, department: $d);
$a isa assignment, links (worker: $p, project: $j);
$x isa project-task, links (project: $j, task: $t);
$l isa located, links (department: $d, office: $o);
$z isa company-office, links (company: $c, office: $o);
$f isa friendship, links (participant: $p, peer: $p), has identifier "F{index:04d}";
$r isa relay, links (edge: $f, peer: $p);''')
    if size >= 4:
        queries.append("""match $p isa party, has identifier "P0002";
$c isa company, has identifier "C0004";
insert $e isa employment, links (worker: $p, employer: $c), has note "cross-noise";""")
    return queries


def width_expected(size: int, width: int) -> list[list[str]]:
    return sorted(
        [row["uid"], row["note"]]
        for row in records(size)
        if row["note"] is not None and matches(row, width)
    )


def expected_rows(size: int) -> list[list[str]]:
    return width_expected(size, 12)


# Preserve the historical sorted statement byte for byte. Its older predicate
# set has a separate known answer once independent witnesses make differences real.
ORIGINAL_SORTED_RAW_QUERY = 'match\n$p isa party, has identifier $identifier, has tag "a", has tag "z", has score $score;\n$score >= 20;\n$c isa company, has identifier $company; $company like "^C.*$";\n$d isa department; not { $d has blocked true; };\n$j isa project, has priority $priority; $priority >= 1;\n$t isa task; $o isa office;\n{ $t has done false; } or { $t has identifier $task; $task like "^T0008.*$"; };\n$o has site $site; $site contains "north";\n$e isa employment, links (worker: $p, employer: $c), has note $note;\n$m isa membership, links (member: $p, department: $d);\n$a isa assignment, links (worker: $p, project: $j);\n$x isa project-task, links (project: $j, task: $t);\n$l isa located, links (department: $d, office: $o);\n$z isa company-office, links (company: $c, office: $o);\nselect $identifier, $note; distinct; sort $identifier, $note;\nfetch { "identifier": $identifier, "note": $note };'


def original_expected_rows(size: int) -> list[list[str]]:
    return sorted(
        [row["uid"], row["note"]]
        for row in records(size)
        if row["note"] is not None
        and all(
            value
            for name, value in predicate_values(row).items()
            if name not in {"identifier-choice", "note-prefix", "cross-field"}
        )
    )


IDENTIFIER_CHOICE = '{ $identifier like "^P.*$"; } or { $identifier like "^W.*$"; };'


def raw_predicates(width: int) -> dict[str, str]:
    predicates = {
        "tag-a": '$p has tag "a";',
        "tag-z": '$p has tag "z";',
        "score-min": "$score >= 20;",
        "note-present": "$e has note $note;",
        "note-prefix": 'not { $e has note $bad_note; $bad_note like "^cross-.*$"; };',
        "identifier-choice": IDENTIFIER_CHOICE,
        "company-prefix": '$c has identifier $company; $company like "^C.*$";',
        "unblocked": "not { $d has blocked true; };",
        "site-match": '$o has site $site; $site contains "north";',
        "priority-min": "$priority >= 1;",
        "cross-field": "$score >= $priority;",
        "task-choice": '{ $t has done false; } or { $t has identifier $task; $task like "^T0008.*$"; };',
    }
    return {name: text for name, text in predicates.items() if PREDICATE_WIDTH[name] <= width}


def width_query(width: int, omit: str | None = None) -> str:
    if width not in (2, 4, 6, 8, 10, 12):
        raise ValueError("unsupported join width")
    topology = [
        "match $p isa person, has identifier $identifier, has score $score;",
        "$e isa employment, links (worker: $p);",
    ]
    if width >= 4:
        topology += [
            "$c isa company; $e links (employer: $c);",
            "$m isa membership, links (member: $p);",
        ]
    if width >= 6:
        topology += [
            "$d isa department; $m links (department: $d);",
            "$l isa located, links (department: $d);",
        ]
    if width >= 8:
        topology += [
            "$o isa office; $l links (office: $o);",
            "$z isa company-office, links (company: $c, office: $o);",
        ]
    if width >= 10:
        topology += [
            "$j isa project, has priority $priority;",
            "$a isa assignment, links (worker: $p, project: $j);",
        ]
    if width >= 12:
        topology += ["$t isa task; $x isa project-task, links (project: $j, task: $t);"]
    topology += [text for name, text in raw_predicates(width).items() if name != omit]
    if omit == "note-present":
        topology += ["try { $e has note $note; };"]
    return "\n".join(topology) + '\nfetch { "identifier": $identifier, "note": $note };'


RAW_QUERY = width_query(12)
SORTED_RAW_QUERY = RAW_QUERY.replace(
    "\nfetch {", "\nselect $identifier, $note; distinct; sort $identifier, $note;\nfetch {"
)


def verify_predicate_witnesses(size: int, oracle: Callable[[str], int]) -> list[dict[str, object]]:
    """Assert independent exclusion in generated data AND on the live raw oracle."""
    evidence: list[dict[str, object]] = []
    rows = records(size)
    for width in (2, 4, 6, 8, 10, 12):
        baseline = sum(matches(row, width) for row in rows)
        observed = oracle(width_query(width))
        assert observed == baseline, (width, observed, baseline)
        for predicate in raw_predicates(width):
            expected = sum(matches(row, width, predicate) for row in rows)
            assert expected > baseline, (width, predicate, "missing independent seed witness")
            dropped = oracle(width_query(width, predicate))
            assert dropped > observed, (width, predicate, observed, dropped)
            evidence.append(
                dict(width=width, predicate=predicate, baseline=observed, dropped=dropped)
            )
    return evidence


def escape_query(width: int, *, list_membership: bool = False, size: int = 16) -> str:
    query = width_query(width)
    if list_membership:
        choices = ", ".join(f'"{uid}"' for uid in model_ids(size) if uid.startswith(("P", "W")))
        query = query.replace(
            IDENTIFIER_CHOICE, f"let $choice in [{choices}]; $identifier == $choice;"
        )
    return query.replace("fetch {", "try { $p has nickname $nickname; };\nfetch {").replace(
        '"note": $note }', '"note": $note, "nickname": $nickname }'
    )


def escape_expected(size: int, width: int) -> list[list[str | None]]:
    values = {row["uid"]: row for row in records(size)}
    return [[uid, note, values[uid]["nickname"]] for uid, note in width_expected(size, width)]
