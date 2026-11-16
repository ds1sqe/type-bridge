import assert from "node:assert/strict";
import { Database, Plan } from "@type-bridge/node";
import {
  Association, Friendship, Relay,
  Assignment, Blocked, Company, CompanyOffice, Department, Done, Employment,
  Identifier, Note, Located, Membership, Office, Party, Person, Priority, Project,
  ProjectTask, QuerySession, Score, Site, Tag, Task, schemaAuthority, aggregate,
} from "./index.js";

async function main(): Promise<void> {
  for (const comparison of ["eq", "ne", "lt", "le", "gt", "ge"] as const) {
    const unsafe = Plan.builder(), owner = unsafe.subtypes(Party);
    const value = unsafe.field(owner, Party.score);
    unsafe.where(value[comparison](value)).count();
    assert.throws(() => unsafe.build(schemaAuthority()), /query_plan_attribute_self_comparison/);
  }
  const db = Database.connect(process.env.TYPEDB_ADDRESS!, process.env.TYPE_BRIDGE_COMPLEX_DATABASE!, {
    username: "admin", password: "password", httpPort: Number(process.env.TYPEDB_HTTP_PORT),
  });
  try {
    if (process.env.TYPE_BRIDGE_COMPLEX_RAW_QUERY) {
      console.log(JSON.stringify(await db.executeQuery(process.env.TYPE_BRIDGE_COMPLEX_RAW_QUERY)));
      return;
    }
    assert.throws(() => {
      const plan = Plan.builder();
      const item = plan.subtypes(Party);
      plan.where(plan.field(item, Party.score).gt(true)).count().build(schemaAuthority());
    }, /query_plan_value_type_mismatch/);
    const abstractExact = Plan.builder();
    abstractExact.exact(Party); abstractExact.count();
    assert.throws(() => abstractExact.build(schemaAuthority()), /query_plan_abstract_exact_read/);
    const abstractSubtypes = Plan.builder();
    abstractSubtypes.subtypes(Party); abstractSubtypes.count();
    const abstractTransaction = db.stagedTransaction("read");
    try {
      const pending = abstractTransaction.stage(abstractSubtypes.build(schemaAuthority()));
      await abstractTransaction.commit();
      const answer = pending.result();
      assert.equal(answer.kind, "count");
      assert(answer.kind === "count");
      assert.equal(answer.value, Number(Party.manager(db).count()));
    } finally { abstractTransaction.close(); }
    const size = BigInt(process.env.TYPE_BRIDGE_COMPLEX_SIZE!);
    const partyIds = process.env.TYPE_BRIDGE_COMPLEX_MODEL_IDS!.split(",");
    const modelCount = BigInt(process.env.TYPE_BRIDGE_COMPLEX_MODEL_COUNT!);
    const limit = modelCount + 1n;
    const s = new QuerySession(db);
    if (!process.env.TYPE_BRIDGE_COMPLEX_WIDTH) {
      const cases = [
        async () => {
          const model = Party, edge = s.exact(Friendship);
          for (const root of [s.exact(Party), s.subtypes(Party)]) {
            const field = root.field(Party.identifier), q = s.query(root);
            const expected = partyIds;
            assert.deepEqual((await q.rows({ limit, sort: [field.asc()] })).map(x => x.identifier.value), expected);
            assert.equal((await q.first({ sort: [field.asc()] }))!.identifier.value, expected[0]);
            assert.equal((await q.where(field.eq(Identifier.create(expected[0]!))).one()).identifier.value, expected[0]);
            assert.equal(await q.countBy(root), modelCount); assert(await q.existsBy(root));
            const page = await q.pageBy(root, { limit, includeTotal: true, sort: [field.asc()] });
            assert.equal(page.total, modelCount); assert.deepEqual(page.items.map(x => x.identifier.value), expected);
            assert.equal((await q.groupBy(root, field).reduce([aggregate.count()])).length, Number(modelCount));
            const link = edge.role(Friendship.participant).links(root);
            assert.equal((await s.query(edge).match(root).where(link).groupBy(edge, root).reduce([aggregate.count()])).length, Number(modelCount));
            assert.equal(await s.query(root).match(edge).where(link).countBy(root), modelCount);
            const nested = await s.query(edge).match(root).where(link).rows({ limit });
            assert.equal(nested.length, Number(modelCount));
            assert(nested.every(x => expected.includes(x.participant.identifier.value)));
            const collected = await s.query(edge, root.collect()).where(link).pageBy(edge, { limit, includeTotal: true });
            assert.equal(collected.total, modelCount);
            assert.deepEqual(collected.items.flatMap(([,items]) => items.map(x => x.identifier.value)).sort(), expected);
          }
          assert.equal(model.manager(db).count(), modelCount); assert.equal(model.manager(db).all().length, Number(modelCount));
          const canonical = model.manager(db).where(Party.identifier, "eq", Identifier.create("P0001"));
          assert.equal(canonical.count(), 1n); assert(canonical.exists());
          assert.equal(canonical.all().length, 1); assert.equal(canonical.first()!.identifier.value, "P0001");
          const stored = model.manager(db).filter({ identifier: Identifier.create("P0001") }).first()!;
          assert.equal(model.manager(db).getByIid(stored.iid!)!.iid!.toString(), stored.iid!.toString());
        },
        async () => {
          const model = Association, edge = s.exact(Relay);
          for (const root of [s.exact(Association), s.subtypes(Association)]) {
            const field = root.field(Association.identifier), q = s.query(root);
            const expected = Array.from({ length: Number(modelCount) }, (_, i) => `F${String(i + 1).padStart(4, "0")}`).sort();
            assert.deepEqual((await q.rows({ limit, sort: [field.asc()] })).map(x => x.identifier.value), expected);
            assert.equal((await q.first({ sort: [field.asc()] }))!.identifier.value, expected[0]);
            assert.equal((await q.where(field.eq(Identifier.create(expected[0]!))).one()).identifier.value, expected[0]);
            assert.equal(await q.countBy(root), modelCount); assert(await q.existsBy(root));
            const page = await q.pageBy(root, { limit, includeTotal: true, sort: [field.asc()] });
            assert.equal(page.total, modelCount); assert.deepEqual(page.items.map(x => x.identifier.value), expected);
            assert.equal((await q.groupBy(root, field).reduce([aggregate.count()])).length, Number(modelCount));
            const link = edge.role(Relay.edge).links(root);
            assert.equal((await s.query(edge).match(root).where(link).groupBy(edge, root).reduce([aggregate.count()])).length, Number(modelCount));
            assert.equal(await s.query(root).match(edge).where(link).countBy(root), modelCount);
            const nested = await s.query(edge).match(root).where(link).rows({ limit });
            assert.equal(nested.length, Number(modelCount));
            assert(nested.every(x => expected.includes(x.edge.identifier.value)));
            const collected = await s.query(edge, root.collect()).where(link).pageBy(edge, { limit, includeTotal: true });
            assert.equal(collected.total, modelCount);
            assert.deepEqual(collected.items.flatMap(([,items]) => items.map(x => x.identifier.value)).sort(), expected);
          }
          assert.equal(model.manager(db).count(), modelCount); assert.equal(model.manager(db).all().length, Number(modelCount));
          const canonical = model.manager(db).where(Association.identifier, "eq", Identifier.create("F0001"));
          assert.equal(canonical.count(), 1n); assert(canonical.exists());
          assert.equal(canonical.all().length, 1); assert.equal(canonical.first()!.identifier.value, "F0001");
          const stored = model.manager(db).filter({ identifier: Identifier.create("F0001") }).first()!;
          assert.equal(model.manager(db).getByIid(stored.iid!)!.iid!.toString(), stored.iid!.toString());
        },
      ];
      for (const test of cases) await test();
    }
    const p = s.subtypes(Person), c = s.exact(Company), d = s.exact(Department);
    const j = s.exact(Project), t = s.exact(Task), o = s.exact(Office);
    const e = s.exact(Employment), m = s.exact(Membership), a = s.exact(Assignment);
    const x = s.exact(ProjectTask), l = s.exact(Located), z = s.exact(CompanyOffice);
    const width = Number(process.env.TYPE_BRIDGE_COMPLEX_WIDTH ?? "12");
    let query = s.query(p, e).where(
      e.role(Employment.worker).links(p), p.field(Person.tag).eq(Tag.create("a")),
      p.field(Person.tag).eq(Tag.create("z")), p.field(Person.score).ge(Score.create(20n)),
      e.field(Employment.note).isPresent(),
    );
    query = query.where(e.field(Employment.note).startsWith(Note.create("cross-")).not());
    query = query.where(p.field(Person.identifier).startsWith(Identifier.create("P")).or(p.field(Person.identifier).startsWith(Identifier.create("W"))));
    if (width >= 4) query = query.match(c, m).where(e.role(Employment.employer).links(c), m.role(Membership.member).links(p), c.field(Company.identifier).startsWith(Identifier.create("C")));
    if (width >= 6) query = query.match(d, l).where(m.role(Membership.department).links(d), l.role(Located.department).links(d), d.field(Department.blocked).eq(Blocked.create(true)).not());
    if (width >= 8) query = query.match(o, z).where(l.role(Located.office).links(o), z.role(CompanyOffice.company).links(c), z.role(CompanyOffice.office).links(o), o.field(Office.site).contains(Site.create("north")));
    if (width >= 10) query = query.match(a, j).where(a.role(Assignment.worker).links(p), a.role(Assignment.project).links(j), j.field(Project.priority).ge(Priority.create(1n)), p.field(Person.score).geField(j.field(Project.priority)));
    if (width >= 12) query = query.match(x, t).where(x.role(ProjectTask.project).links(j), x.role(ProjectTask.task).links(t), t.field(Task.done).eq(Done.create(false)).or(t.field(Task.identifier).startsWith(Identifier.create("T0008"))));
    let rootQuery = s.query(p).match(e).where(
      e.role(Employment.worker).links(p), p.field(Person.tag).eq(Tag.create("a")),
      p.field(Person.tag).eq(Tag.create("z")), p.field(Person.score).ge(Score.create(20n)),
      e.field(Employment.note).isPresent(),
    );
    rootQuery = rootQuery.where(e.field(Employment.note).startsWith(Note.create("cross-")).not());
    rootQuery = rootQuery.where(p.field(Person.identifier).startsWith(Identifier.create("P")).or(p.field(Person.identifier).startsWith(Identifier.create("W"))));
    if (width >= 4) rootQuery = rootQuery.match(c, m).where(e.role(Employment.employer).links(c), m.role(Membership.member).links(p), c.field(Company.identifier).startsWith(Identifier.create("C")));
    if (width >= 6) rootQuery = rootQuery.match(d, l).where(m.role(Membership.department).links(d), l.role(Located.department).links(d), d.field(Department.blocked).eq(Blocked.create(true)).not());
    if (width >= 8) rootQuery = rootQuery.match(o, z).where(l.role(Located.office).links(o), z.role(CompanyOffice.company).links(c), z.role(CompanyOffice.office).links(o), o.field(Office.site).contains(Site.create("north")));
    if (width >= 10) rootQuery = rootQuery.match(a, j).where(a.role(Assignment.worker).links(p), a.role(Assignment.project).links(j), j.field(Project.priority).ge(Priority.create(1n)), p.field(Person.score).geField(j.field(Project.priority)));
    if (width >= 12) rootQuery = rootQuery.match(x, t).where(x.role(ProjectTask.project).links(j), x.role(ProjectTask.task).links(t), t.field(Task.done).eq(Done.create(false)).or(t.field(Task.identifier).startsWith(Identifier.create("T0008"))));
    const sort = [p.field(Person.identifier).asc(), e.field(Employment.note).asc()];
    const rows = await query.rows({ limit, sort });
    const first = await query.first({ sort });
    const encode = (pair: readonly [Person, Employment]) => [pair[0].identifier.value, pair[1].note!.value];
    const page = await rootQuery.pageBy(p, { limit: 1n, offset: 1n, sort: [sort[0]!], includeTotal: true });
    assert.deepEqual(page.items.map((item) => item.identifier.value), rows.slice(1, 2).map((row) => row[0].identifier.value));
    assert.equal(page.total, BigInt(rows.length));
    assert.deepEqual((await query.rows({ limit: 1n, offset: 1n, sort })).map(encode), rows.slice(1, 2).map(encode));
    const grouped = await rootQuery.groupBy(p, p.field(Person.identifier)).reduce([aggregate.count()] as const);
    assert.deepEqual(grouped.map(([key, values]) => [key.value, Number(values[0])]).sort(), rows.map((row) => [row[0].identifier.value, 1]).sort());
    const abstract = s.exact(Party);
    assert.equal((await s.query(abstract).rows({ limit })).length, Number(Party.manager(db).count()));
    const manager = Party.manager(db);
    const tagged = manager.where(Party.tag, "eq", Tag.create("a"))
      .where(Party.tag, "eq", Tag.create("z"));
    assert.equal((await tagged.all()).length, Number(await tagged.count()));
    assert.equal(await tagged.exists(), (await tagged.count()) > 0n);
    const firstTagged = await tagged.where(Party.identifier, "eq", Identifier.create("P0002")).first();
    assert.equal(firstTagged !== null, size >= 2n);
    console.log(JSON.stringify({
      page_id: page.items[0]?.identifier.value ?? null,
      groups: grouped.map(([key, values]) => [key.value, Number(values[0])]).sort(),
      rows: rows.map(encode), first: first ? encode(first) : null,
      count: Number(await query.countBy(p)), exists: await query.existsBy(p),
      abstract_count: (await manager.all()).length, tagged_count: Number(await tagged.count()),
    }));
  } finally { db.close(); }
}
main().catch((error) => { console.error(error); process.exitCode = 1; });
