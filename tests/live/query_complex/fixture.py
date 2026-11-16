"""Deterministic chain/star fixture, also reusable for size-scaling measurements."""

from __future__ import annotations


def seed_queries(size: int) -> list[str]:
    if size < 1:
        raise ValueError("fixture size must be positive")
    queries = []
    for i in range(1, size + 1):
        person = "employee" if i % 2 == 0 else "person"
        fields = [f'has identifier "P{i:04d}"']
        fields += ['has tag "a", has tag "z"'] if i % 2 == 0 else ['has tag "n"']
        if i % 3:
            fields.append(f"has score {i * 10}")
        if i % 4:
            fields.append(f'has nickname "Alpha{i}"')
        note = f', has note "job-{i:04d}"' if i % 5 else ""
        blocked = ", has blocked true" if i % 7 == 0 else ""
        site = "North" if i % 2 == 0 else "South"
        queries.append(f'''insert
$p isa {person}, {", ".join(fields)};
$c isa company, has identifier "C{i:04d}";
$d isa department, has identifier "D{i:04d}"{blocked};
$j isa project, has identifier "J{i:04d}", has priority {41 if i == 4 else i % 3};
$t isa task, has identifier "T{i:04d}", has done {str(i % 4 == 0).lower()};
$o isa office, has identifier "O{i:04d}", has site "{site}";
$e isa employment, links (worker: $p, employer: $c){note};
$m isa membership, links (member: $p, department: $d);
$a isa assignment, links (worker: $p, project: $j);
$x isa project-task, links (project: $j, task: $t);
$l isa located, links (department: $d, office: $o);
$z isa company-office, links (company: $c, office: $o);
$f isa friendship, links (participant: $p, peer: $p), has identifier "F{i:04d}";
$r isa relay, links (edge: $f, peer: $p);''')
    if size >= 4:
        queries.append("""match $p isa party, has identifier "P0002";
$c isa company, has identifier "C0004";
insert $e isa employment, links (worker: $p, employer: $c), has note "cross-noise";""")
    return queries


def expected_rows(size: int) -> list[list[str]]:
    return [
        [f"P{i:04d}", f"job-{i:04d}"]
        for i in range(2, size + 1, 2)
        if i % 3 and i % 5 and i % 7 and (i % 4 != 0 or i == 8)
    ]


RAW_QUERY = """match
$p isa party, has identifier $identifier, has tag "a", has tag "z", has score $score;
$score >= 20;
$c isa company, has identifier $company; $company like "^C.*$";
$d isa department; not { $d has blocked true; };
$j isa project, has priority $priority; $priority >= 1;
$t isa task; $o isa office;
{ $t has done false; } or { $t has identifier $task; $task like "^T0008.*$"; };
$o has site $site; $site contains "north";
$e isa employment, links (worker: $p, employer: $c), has note $note;
$m isa membership, links (member: $p, department: $d);
$a isa assignment, links (worker: $p, project: $j);
$x isa project-task, links (project: $j, task: $t);
$l isa located, links (department: $d, office: $o);
$z isa company-office, links (company: $c, office: $o);
fetch { "identifier": $identifier, "note": $note };"""

ORIGINAL_SORTED_RAW_QUERY = RAW_QUERY.replace(
    "\nfetch {", "\nselect $identifier, $note; distinct; sort $identifier, $note;\nfetch {"
)


RAW_QUERY = RAW_QUERY.replace("$priority >= 1;", "$priority >= 1; $score >= $priority;")
SORTED_RAW_QUERY = RAW_QUERY.replace(
    "\nfetch {", "\nselect $identifier, $note; distinct; sort $identifier, $note;\nfetch {"
)


def width_query(width: int) -> str:
    if width == 12:
        return RAW_QUERY
    parts = [
        'match $p isa person, has identifier $identifier, has tag "a", has tag "z", has score $score; $score >= 20;',
        '$e isa employment, links (worker: $p), has note $note; $note like "^job-.*$";',
        '{ $identifier == "P0002"; } or { $identifier == "P0004"; } or { $identifier == "P0008"; };',
    ]
    if width >= 4:
        parts += [
            '$c isa company, has identifier $company; $company like "^C.*$";',
            "$e links (employer: $c); $m isa membership, links (member: $p);",
        ]
    if width >= 6:
        parts += [
            "$d isa department; not { $d has blocked true; }; $m links (department: $d);",
            "$l isa located, links (department: $d);",
        ]
    if width >= 8:
        parts += [
            '$o isa office, has site $site; $site contains "north"; $l links (office: $o);',
            "$z isa company-office, links (company: $c, office: $o);",
        ]
    if width >= 10:
        parts += [
            "$j isa project, has priority $priority; $priority >= 1; $score >= $priority;",
            "$a isa assignment, links (worker: $p, project: $j);",
        ]
    return "\n".join(parts + ['fetch { "identifier": $identifier, "note": $note };'])


def width_expected(size: int, width: int) -> list[list[str]]:
    return (
        expected_rows(size)
        if width == 12
        else [
            [f"P{i:04d}", f"job-{i:04d}"] for i in (2, 4, 8) if i <= size and (width < 10 or i != 4)
        ]
    )


def escape_query(width: int, *, list_membership: bool = False) -> str:
    """Actual list iteration and optional branch, unavailable in SDK builders."""
    query = width_query(width)
    if list_membership:
        query = query.replace(
            '{ $identifier == "P0002"; } or { $identifier == "P0004"; } or { $identifier == "P0008"; };',
            'let $choice in ["P0002", "P0004", "P0008"]; $identifier == $choice;',
        )
    return query.replace("fetch {", "try { $p has nickname $nickname; };\nfetch {").replace(
        '"note": $note }', '"note": $note, "nickname": $nickname }'
    )


def escape_expected(size: int, width: int) -> list[list[str | None]]:
    return [
        [f"P{i:04d}", f"job-{i:04d}", f"Alpha{i}" if i % 4 else None]
        for i in (2, 4, 8)
        if i <= size and (width < 10 or i != 4)
    ]
