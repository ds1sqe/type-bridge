import assert from "node:assert/strict";
import { MigrationCancellation } from "@type-bridge/node";
import {
  Person, Gender, QuerySession, DirectConnectionPolicy, connect, openMigrationCatalog,
} from "./dist/index.js";

const db = connect(new DirectConnectionPolicy(
  process.env.TYPEDB_ADDRESS,
  process.env.TYPE_BRIDGE_CALLBACK_DATABASE,
  {
    username: process.env.TYPEDB_USERNAME ?? "admin",
    password: process.env.TYPEDB_PASSWORD ?? "password",
    httpPort: Number(process.env.TYPEDB_HTTP_PORT),
  },
));
assert.equal(db.inspectDatabasePair(), "absent");
db.createDatabaseOutcome();
const catalog = openMigrationCatalog();
const ids = Array.from({ length: catalog.length }, (_, i) => catalog.entry(i).id);

function plan(applied, targets, reverse = false) {
  const preview = reverse
    ? catalog.previewRollback(applied, targets)
    : catalog.previewApply(applied, targets);
  const builder = preview.approvalBuilder();
  for (let i = 0; i < preview.migrationCount(); i++) {
    if (["opaque", "destructive"].includes(preview.migration(i).safety)) builder.approve(i);
  }
  return preview.authorize(builder.finish());
}

console.log((await plan([], [ids[0]]).execute(db, "node-initial")).status);
await db.executeQuery(
  'insert $p isa person, has ssn "1231"; $q isa person, has ssn "1232";', "write",
);
const escaped = [];
let fail = true;
let cancellation;
catalog.register("derive-gender", async tx => {
  escaped.push(tx);
  const manager = Person.manager(tx);
  const people = manager.all();
  assert.equal(people.length, 2);
  const session = new QuerySession(tx);
  const binding = session.exact(Person);
  assert.equal(await session.query(binding).countBy(binding), 2n);
  for (const person of people) {
    manager.update(person.iid, Person.create({
      ssn: person.ssn,
      gender: Gender.create(Number(person.ssn.value.at(-1)) % 2 ? "male" : "female"),
    }));
  }
  if (fail) throw new Error("private failure detail");
  if (cancellation) {
    cancellation.cancel();
    await new Promise(resolve => setTimeout(resolve, 50));
    return;
  }
  const answer = await tx.executeQuery(
    'match $p isa person; fetch { "ssn": $p.ssn, "gender": $p.gender };',
  );
  assert.equal(answer.answers.length, 2);
}, async tx => {
  await tx.executeQuery("match $p isa person, has gender $g; delete has $g of $p;");
});

const genderValues = 'match $p isa person, has gender $g; fetch {"gender":$g};';
await assert.rejects(
  plan([ids[0]], [ids[1]]).execute(db, "node-error"), /migration_user_step_failed.*private failure detail/,
);
assert.equal((await db.executeQuery(genderValues, "read")).answers.length, 0);
fail = false;
cancellation = new MigrationCancellation();
await assert.rejects(
  plan([ids[0]], [ids[1]]).executeControlled(db, "node-cancel", { cancellation }),
  /migration_execution_cancelled/,
);
cancellation = undefined;
assert.equal((await db.executeQuery(genderValues, "read")).answers.length, 0);
console.log((await plan([ids[0]], [ids[1]]).execute(db, "node-data")).status);
for (const tx of escaped) {
  await assert.rejects(
    tx.executeQuery('match $p isa person; fetch {"ssn":$p.ssn};'),
    error => error.code === "raw_transaction_closed",
  );
}
console.log((await plan(ids, [ids[1]], true).execute(db, "node-reverse")).status);
console.log((await plan([ids[0]], [ids[1]]).execute(db, "node-reapply")).status);
console.log("PASS Node managers + queries + raw + error/cancellation rollback + reverse + escaped handles");
db.close();
