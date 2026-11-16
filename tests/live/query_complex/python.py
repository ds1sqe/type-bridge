"""The twelve-binding query through the generated Python SDK."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import sys
from datetime import datetime

from complex_models import (
    Assignment,
    Blocked,
    Company,
    CompanyOffice,
    Department,
    Done,
    Employment,
    Identifier,
    Located,
    Membership,
    Note,
    Office,
    Party,
    Person,
    Priority,
    Project,
    ProjectedManagerComparison,
    ProjectTask,
    Score,
    Site,
    Tag,
    Task,
    aggregate,
    schema_authority,
)

from type_bridge import Database
from type_bridge.plan import Count, Plan, PlanError


class Statements(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        event = json.loads(record.getMessage())
        if event.get("name") == "type_bridge.io.provider.statement":
            with open(os.environ["TYPE_BRIDGE_COMPLEX_TRACE"], "a") as trace:
                trace.write(json.dumps(event["fields"]) + "\n")


logger = logging.getLogger("type_bridge.io")
logger.setLevel(5)
logger.addHandler(Statements(level=5))


async def main() -> None:
    db = Database(
        address=os.environ["TYPEDB_ADDRESS"],
        database=os.environ["TYPE_BRIDGE_COMPLEX_DATABASE"],
        http_port=int(os.environ["TYPEDB_HTTP_PORT"]),
    )
    limit = int(os.environ["TYPE_BRIDGE_COMPLEX_SIZE"]) + 1
    db.connect()
    try:
        if raw := os.environ.get("TYPE_BRIDGE_COMPLEX_RAW_QUERY"):
            print(json.dumps(db.execute_query(raw)))
            return
        for token, value in [
            (Party.seen, datetime(2020, 1, 1)),
            (Party.score, True),
            (Party.identifier, 3),
        ]:
            plan = Plan.builder()
            item = plan.subtypes(Party)
            plan.where(plan.field(item, token).gt(value)).count()
            try:
                plan.build(schema_authority())
            except PlanError as error:
                assert error.code == "query_plan_value_type_mismatch", error
            else:
                raise AssertionError("ill-typed comparison built successfully")
        plan = Plan.builder()
        left, right = plan.subtypes(Party), plan.exact(Company)
        plan.where(plan.field(left, Party.score).ge(plan.field(right, Company.identifier))).count()
        try:
            plan.build(schema_authority())
        except PlanError as error:
            assert error.code == "query_plan_value_type_mismatch", error
        else:
            raise AssertionError("incompatible field domains built successfully")
        abstract_plan = Plan.builder()
        abstract_plan.exact(Party)
        abstract_plan.count()
        try:
            abstract_plan.build(schema_authority())
        except PlanError as error:
            assert error.code == "query_plan_abstract_exact_read", error
            assert "party" in str(error) and "subtypes" in str(error)
        else:
            raise AssertionError("exact abstract read built successfully")
        abstract_plan = Plan.builder()
        abstract_plan.subtypes(Party)
        abstract_plan.count()
        with db.staged_transaction("read") as transaction:
            abstract_count = transaction.stage(abstract_plan.build(schema_authority()))
        abstract_result = abstract_count.result()
        assert isinstance(abstract_result, Count)
        assert abstract_result.count == Party.manager(db).count()
        session = Party.query(db)
        p = session.subtypes(Person)
        c = session.exact(Company)
        d = session.exact(Department)
        j = session.exact(Project)
        t = session.exact(Task)
        o = session.exact(Office)
        e = session.exact(Employment)
        m = session.exact(Membership)
        a = session.exact(Assignment)
        x = session.exact(ProjectTask)
        location = session.exact(Located)
        z = session.exact(CompanyOffice)
        width = int(os.environ.get("TYPE_BRIDGE_COMPLEX_WIDTH", "12"))
        query = session.query(p, e).where(
            e.role(Employment.worker).links(p),
            p.field(Person.tag).eq(Tag("a")),
            p.field(Person.tag).eq(Tag("z")),
            p.field(Person.score).ge(Score(20)),
            e.field(Employment.note).is_present(),
        )
        if width < 12:
            query = query.where(e.field(Employment.note).starts_with(Note("job-")))
            query = query.where(
                p.field(Person.identifier).eq(Identifier("P0002"))
                | p.field(Person.identifier).eq(Identifier("P0004"))
                | p.field(Person.identifier).eq(Identifier("P0008"))
            )
        if width >= 4:
            query = query.match(c, m).where(
                e.role(Employment.employer).links(c),
                m.role(Membership.member).links(p),
                c.field(Company.identifier).starts_with(Identifier("C")),
            )
        if width >= 6:
            query = query.match(d, location).where(
                m.role(Membership.department).links(d),
                location.role(Located.department).links(d),
                ~d.field(Department.blocked).eq(Blocked(True)),
            )
        if width >= 8:
            query = query.match(o, z).where(
                location.role(Located.office).links(o),
                z.role(CompanyOffice.company).links(c),
                z.role(CompanyOffice.office).links(o),
                o.field(Office.site).contains(Site("north")),
            )
        if width >= 10:
            query = query.match(a, j).where(
                a.role(Assignment.worker).links(p),
                a.role(Assignment.project).links(j),
                j.field(Project.priority).ge(Priority(1)),
                p.field(Person.score).ge_field(j.field(Project.priority)),
            )
        if width >= 12:
            query = query.match(x, t).where(
                x.role(ProjectTask.project).links(j),
                x.role(ProjectTask.task).links(t),
                t.field(Task.done).eq(Done(False))
                | t.field(Task.identifier).starts_with(Identifier("T0008")),
            )
        root_query = (
            session.query(p)
            .match(e)
            .where(
                e.role(Employment.worker).links(p),
                p.field(Person.tag).eq(Tag("a")),
                p.field(Person.tag).eq(Tag("z")),
                p.field(Person.score).ge(Score(20)),
                e.field(Employment.note).is_present(),
            )
        )
        if width < 12:
            root_query = root_query.where(e.field(Employment.note).starts_with(Note("job-")))
            root_query = root_query.where(
                p.field(Person.identifier).eq(Identifier("P0002"))
                | p.field(Person.identifier).eq(Identifier("P0004"))
                | p.field(Person.identifier).eq(Identifier("P0008"))
            )
        if width >= 4:
            root_query = root_query.match(c, m).where(
                e.role(Employment.employer).links(c),
                m.role(Membership.member).links(p),
                c.field(Company.identifier).starts_with(Identifier("C")),
            )
        if width >= 6:
            root_query = root_query.match(d, location).where(
                m.role(Membership.department).links(d),
                location.role(Located.department).links(d),
                ~d.field(Department.blocked).eq(Blocked(True)),
            )
        if width >= 8:
            root_query = root_query.match(o, z).where(
                location.role(Located.office).links(o),
                z.role(CompanyOffice.company).links(c),
                z.role(CompanyOffice.office).links(o),
                o.field(Office.site).contains(Site("north")),
            )
        if width >= 10:
            root_query = root_query.match(a, j).where(
                a.role(Assignment.worker).links(p),
                a.role(Assignment.project).links(j),
                j.field(Project.priority).ge(Priority(1)),
                p.field(Person.score).ge_field(j.field(Project.priority)),
            )
        if width >= 12:
            root_query = root_query.match(x, t).where(
                x.role(ProjectTask.project).links(j),
                x.role(ProjectTask.task).links(t),
                t.field(Task.done).eq(Done(False))
                | t.field(Task.identifier).starts_with(Identifier("T0008")),
            )
        sort = (p.field(Person.identifier).asc(), e.field(Employment.note).asc())
        rows = await query.rows(limit=limit, sort=sort)
        first = await query.first(sort=sort)

        def encode(pair: tuple[Person, Employment]) -> list[str]:
            assert pair[1].note is not None
            return [pair[0].identifier.value, pair[1].note.value]

        page = await root_query.page_by(p, limit=1, offset=1, sort=(sort[0],), include_total=True)
        assert [item.identifier.value for item in page.items] == [
            row[0].identifier.value for row in rows[1:2]
        ]
        assert page.total == len(rows)
        offset_rows = await query.rows(limit=1, offset=1, sort=sort)
        assert [encode(row) for row in offset_rows] == [encode(row) for row in rows[1:2]]
        grouped = await root_query.group_by(p, p.field(Person.identifier)).reduce(aggregate.count())
        assert sorted((key.value, values[0]) for key, values in grouped) == sorted(
            (row[0].identifier.value, 1) for row in rows
        )

        abstract = session.exact(Party)
        assert len(await session.query(abstract).rows(limit=limit)) == Party.manager(db).count()
        manager = Party.manager(db)
        tagged = manager.where(Party.tag, ProjectedManagerComparison.EQ, Tag("a")).where(
            Party.tag, ProjectedManagerComparison.EQ, Tag("z")
        )
        assert len(tagged.all()) == tagged.count()
        assert tagged.exists() == (tagged.count() > 0)
        first_tagged = tagged.where(
            Party.identifier, ProjectedManagerComparison.EQ, Identifier("P0002")
        ).first()
        assert (first_tagged is not None) == (limit > 2)
        print(
            json.dumps(
                {
                    "rows": [encode(row) for row in rows],
                    "page_id": page.items[0].identifier.value if page.items else None,
                    "groups": sorted([key.value, values[0]] for key, values in grouped),
                    "first": encode(first) if first else None,
                    "count": await query.count_by(p),
                    "exists": await query.exists_by(p),
                    "abstract_count": len(manager.all()),
                    "tagged_count": tagged.count(),
                },
                sort_keys=True,
            )
        )
    finally:
        db.close()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except Exception as error:
        print(
            {name: getattr(error, name, None) for name in ("code", "path", "details")},
            file=sys.stderr,
        )
        raise
