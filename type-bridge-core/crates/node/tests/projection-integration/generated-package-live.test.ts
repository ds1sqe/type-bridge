import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import net from "node:net";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  connectIntegration,
  defineSchema,
  INTG_DATABASE,
  QueryV2Authority,
  TYPEDB_ADDRESS,
  TYPEDB_HTTP_PORT,
  TYPEDB_PASSWORD,
  TYPEDB_USERNAME,
} from "../integration/common/index.js";

const NODE_SOURCE_PACKAGE = process.cwd();
const NODE_RUNTIME_PACKAGE = process.env.TYPE_BRIDGE_NODE_PACKAGE_ROOT ?? NODE_SOURCE_PACKAGE;
const CORE = resolve(NODE_SOURCE_PACKAGE, "../..");
const ROOT = resolve(CORE, "..");
const ACCEPTANCE = resolve(CORE, "crates/schema-codegen/tests/acceptance");
const TYPEDB_VERSION = process.env.TYPEDB_VERSION ?? "3.12.1";
const IS_TYPEDB_3_11 = TYPEDB_VERSION.startsWith("3.11.");
const ACCEPTANCE_SCHEMA = resolve(
  ACCEPTANCE,
  IS_TYPEDB_3_11 ? "schema-3.11.5.yaml" : "schema.yaml",
);
const PROVIDER_SCHEMA = resolve(
  ACCEPTANCE,
  IS_TYPEDB_3_11 ? "provider-3.11.5.tql" : "provider-3.12.1.tql",
);
const REMOTE_SCOPE = "generated-projection-live";
const REMOTE_PROFILE = IS_TYPEDB_3_11 ? "typedb-3.11.5/v1" : "typedb-3.12.1/v1";

function run(command: string, args: readonly string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    env: {
      ...process.env,
      TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE: REMOTE_PROFILE,
    },
    stdio: "inherit",
  });
  if (result.error !== undefined) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status ?? "unknown"}`);
  }
}

function assertIid(value: unknown): asserts value is string {
  assert.equal(typeof value, "string");
  assert.notEqual(value, "");
}

async function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close(() => reject(new TypeError("failed to reserve a remote query port")));
        return;
      }
      server.close((error) => error === undefined ? resolvePort(address.port) : reject(error));
    });
  });
}

async function waitForPort(port: number, child: ChildProcess, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`generated remote query server exited with code ${child.exitCode}`);
    }
    const connected = await new Promise<boolean>((resolveConnection) => {
      const socket = net.createConnection({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        resolveConnection(true);
      });
      socket.once("error", () => resolveConnection(false));
    });
    if (connected) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  }
  throw new Error("timed out waiting for generated remote query server");
}

test(`generated package round-trips exact models on TypeDB ${TYPEDB_VERSION}`, { timeout: 360_000 }, async () => {
  const suppliedStage = process.env.TYPE_BRIDGE_GENERATED_NODE_STAGE;
  const stage = suppliedStage === undefined
    ? await mkdtemp(join(tmpdir(), "type-bridge-node-projection-"))
    : resolve(suppliedStage);
  const ownsStage = suppliedStage === undefined;
  const generatedDirectory = resolve(stage, "generated_v2");
  const foreignDirectory = resolve(stage, "generated_foreign");
  let database;
  let failure: unknown;

  try {
    if (ownsStage) {
      run(
        "cargo",
        [
          "run",
          "--quiet",
          "--manifest-path",
          resolve(CORE, "Cargo.toml"),
          "-p",
          "type-bridge-schema-codegen",
          "--example",
          "emit_typescript_acceptance",
          "--",
          ACCEPTANCE_SCHEMA,
          generatedDirectory,
          resolve(stage, "declared-schema.json"),
        ],
        ROOT,
      );
      run(
        "cargo",
        [
          "run",
          "--quiet",
          "--manifest-path",
          resolve(CORE, "Cargo.toml"),
          "-p",
          "type-bridge-schema-codegen",
          "--example",
          "emit_typescript_acceptance",
          "--",
          ACCEPTANCE_SCHEMA,
          foreignDirectory,
        ],
        ROOT,
      );
    }

    const packageScope = resolve(stage, "node_modules/@type-bridge");
    await mkdir(packageScope, { recursive: true });
    await rm(resolve(packageScope, "node"), { recursive: true, force: true });
    await symlink(
      NODE_RUNTIME_PACKAGE,
      resolve(packageScope, "node"),
      process.platform === "win32" ? "junction" : "dir",
    );
    if (ownsStage) {
      run(
        resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
        ["--project", resolve(generatedDirectory, "tsconfig.json")],
        generatedDirectory,
      );
      run(
        resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
        ["--project", resolve(foreignDirectory, "tsconfig.json")],
        foreignDirectory,
      );
    }

    const generated = await import(pathToFileURL(resolve(generatedDirectory, "dist/index.js")).href);
    const foreign = await import(pathToFileURL(resolve(foreignDirectory, "dist/index.js")).href);
    const {
      Actor,
      Aliases,
      Container,
      Counter,
      CounterValue,
      Employee,
      Employment,
      Event,
      FooBar,
      Identifier,
      Interaction,
      Manager,
      ManagerNote,
      Membership,
      NetworkLink,
      Nickname,
      Party,
      PartyName,
      Person,
      PlainActivity,
      QuerySession,
      RemoteQuerySession,
      PROJECTION_FINGERPRINT_JSON,
      Rank,
      Robot,
      RobotId,
      RUNTIME_PROJECTION_JSON,
      Score,
      ScoreGte,
      SEMANTIC_SCHEMA_FINGERPRINT_JSON,
      ValBool,
      ValConstrained,
      ValDate,
      ValDatetime,
      ValDatetimeTz,
      ValDecimal,
      ValDouble,
      ValDuration,
      aggregate,
    } = generated;

    const semanticFingerprint = JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON);
    const runtimeProjection = JSON.parse(RUNTIME_PROJECTION_JSON);
    assert.equal(semanticFingerprint.semantic_profile, REMOTE_PROFILE);
    assert.deepEqual(runtimeProjection.semantic_fingerprint, semanticFingerprint);
    assert.deepEqual(runtimeProjection.projection_fingerprint, JSON.parse(PROJECTION_FINGERPRINT_JSON));

    database = connectIntegration();
    database.resetDatabase();
    defineSchema(database, await readFile(PROVIDER_SCHEMA, "utf8"));

    const personManager = Person.manager(database);
    const generatedPerson = (identifierValue: string, scoreValue: bigint) => Person.create({
      identifier: Identifier.create(identifierValue),
      score: Score.create(scoreValue),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    const generatedPersonWithOwnerships = (
      identifierValue: string,
      scoreValue: bigint,
      nicknameValue: string | null,
      aliasValues: readonly string[],
    ) => Person.create({
      aliases: aliasValues.map((value) => Aliases.create(value)),
      identifier: Identifier.create(identifierValue),
      ...(nicknameValue === null ? {} : { nickname: Nickname.create(nicknameValue) }),
      score: Score.create(scoreValue),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    const personInput = Person.create({
      identifier: Identifier.create("person-1"),
      nickname: Nickname.create("alice"),
      aliases: [Aliases.create("alpha"), Aliases.create("beta")],
      score: Score.create(3n),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    assert.deepEqual(personManager.insertMany([]), []);
    assert.deepEqual(personManager.putMany([]), []);
    const insertedPerson = personManager.insert(personInput);
    assertIid(insertedPerson.iid);
    assert.equal(insertedPerson.__typebridgeModel, Person.typeKey);
    assert.equal(insertedPerson.__typebridgeForm, "complete");
    const putPerson = personManager.put(Person.create({
      identifier: Identifier.create("person-1"),
      nickname: Nickname.create("alice"),
      aliases: [Aliases.create("alpha"), Aliases.create("beta")],
      score: Score.create(3n),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    }));
    assert.equal(putPerson.iid, insertedPerson.iid);
    assert.deepEqual(
      personManager.filter({ score__gte: Score.create(3n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ score__in: [Score.create(2n), Score.create(3n)] })
        .all()
        .map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ aliases__isnull: false }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ iid__in: [insertedPerson.iid] }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    const filteredManager = personManager.filter({ score__gte: Score.create(3n) });
    assert.throws(
      () => personManager.filter({ identifier: undefined } as any),
      /filter values must be finite JSON scalars/i,
    );
    assert.throws(
      () => personManager.filter({ iid__in: [insertedPerson.iid, undefined] } as any),
      /filter values must be finite JSON scalars/i,
    );
    assert.equal(filteredManager.first()?.iid, insertedPerson.iid);
    assert.equal(filteredManager.count(), 1n);
    assert.equal(filteredManager.exists(), true);
    const missingManager = personManager.filter({ score__gt: Score.create(3n) });
    assert.equal(missingManager.first(), null);
    assert.equal(missingManager.count(), 0n);
    assert.equal(missingManager.exists(), false);
    assert.deepEqual(
      personManager.filter({ scoreGte__eq: ScoreGte.create(8n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ fooBar: FooBar.create(7n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );

    const storedPerson = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(storedPerson, null);
    assert.equal(storedPerson.iid, insertedPerson.iid);
    assert.equal(storedPerson.__typebridgeModel, Person.typeKey);
    assert.equal(storedPerson.__typebridgeForm, "complete");
    assert.equal(storedPerson.identifier.__typebridgeModel, Identifier.typeKey);
    assert.equal(storedPerson.identifier.value, "person-1");
    assert.equal(storedPerson.nickname.__typebridgeModel, Nickname.typeKey);
    assert.equal(storedPerson.nickname.value, "alice");
    assert.deepEqual(
      new Set(storedPerson.aliases.map((alias: { readonly value: string }) => alias.value)),
      new Set(["alpha", "beta"]),
    );
    assert.ok(storedPerson.aliases.every(
      (alias: { readonly __typebridgeModel: string }) => alias.__typebridgeModel === Aliases.typeKey,
    ));

    const updatedPerson = personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, "ada", ["alpha", "beta"]),
    );
    assert.equal(updatedPerson.iid, insertedPerson.iid);
    const updatedStoredPerson = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(updatedStoredPerson, null);
    assert.equal(updatedStoredPerson.nickname.value, "ada");
    const specialAlias = "quote'\"\\line\nunicode-λ";
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, null, [specialAlias]),
    );
    const replacedOwnerships = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(replacedOwnerships, null);
    assert.equal(replacedOwnerships.nickname, null);
    assert.deepEqual(replacedOwnerships.aliases.map((alias) => alias.value), [specialAlias]);
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, null, []),
    );
    const clearedOwnerships = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(clearedOwnerships, null);
    assert.equal(clearedOwnerships.nickname, null);
    assert.deepEqual(clearedOwnerships.aliases, []);
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, "ada", ["alpha", "beta"]),
    );
    const keyPreserved = personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-key-mutated", 3n, "ada", ["alpha", "beta"]),
    );
    assert.equal(keyPreserved.identifier.value, "person-1");
    assert.equal(personManager.getByIid(insertedPerson.iid)?.identifier.value, "person-1");

    const stalePerson = personManager.insert(generatedPerson("person-stale-update", 4n));
    assertIid(stalePerson.iid);
    personManager.delete(stalePerson.iid);
    assert.throws(
      () => personManager.update(stalePerson.iid, generatedPerson("person-stale-update", 5n)),
      /not found after update/i,
    );

    const batchPeople = [
      generatedPerson("person-2", 5n),
      generatedPerson("person-3", 7n),
    ] as const;
    const insertedBatch = personManager.insertMany(batchPeople);
    assert.ok(insertedBatch.every((candidate) => candidate.iid !== null));
    const batchIids = insertedBatch.map((candidate) => candidate.iid);
    assert.deepEqual(
      personManager.filter({ aliases__isnull: true }).all().map((candidate) => candidate.iid),
      batchIids,
    );
    assert.deepEqual(personManager.putMany(insertedBatch).map((candidate) => candidate.iid), batchIids);

    const insertedEmployee = Employee.manager(database).insert(Employee.create({
      identifier: Identifier.create("employee-1"),
      partyName: PartyName.create("employee"),
      rank: Rank.create(1n),
    }));
    const insertedManager = Manager.manager(database).insert(Manager.create({
      identifier: Identifier.create("manager-1"),
      managerNote: ManagerNote.create("lead"),
      partyName: PartyName.create("manager"),
      rank: Rank.create(2n),
    }));
    assertIid(insertedEmployee.iid);
    assertIid(insertedManager.iid);

    const writeTransaction = database.transaction("write");
    const transactionPerson = generatedPerson("person-4", 9n);
    let transactionPersonIid: string | null = null;
    try {
      const transactionManager = Person.manager(writeTransaction);
      transactionPersonIid = transactionManager.insert(transactionPerson).iid;
      assert.notEqual(transactionPersonIid, null);
      writeTransaction.commit();
    } catch (error) {
      writeTransaction.rollback();
      throw error;
    }
    assertIid(transactionPersonIid);

    const rollbackTransaction = database.transaction("write");
    const rollbackPerson = generatedPerson("person-rollback", 11n);
    const rollbackManager = Person.manager(rollbackTransaction);
    const rollbackIid = rollbackManager.insert(rollbackPerson).iid;
    assertIid(rollbackIid);
    assert.equal(rollbackManager.getByIid(rollbackIid)?.iid, rollbackIid);
    rollbackTransaction.rollback();
    assert.equal(
      personManager.filter({ identifier: Identifier.create("person-rollback") }).exists(),
      false,
    );

    const readTransaction = database.transaction("read");
    try {
      const transactionSession = new QuerySession(readTransaction);
      const transactionPersonVar = transactionSession.var(Person);
      const transactionQuery = transactionSession
        .query(transactionPersonVar)
        .where(
          transactionPersonVar.field(Person.identifier).eq(Identifier.create("person-4")),
      );
      assert.equal(transactionQuery.countBy(transactionPersonVar), 1n);
      assert.equal(transactionQuery.first()?.iid, transactionPersonIid);
    } finally {
      readTransaction.close();
    }
    assert.equal(personManager.count(), 4n);

    const membershipManager = Membership.manager(database);
    assert.deepEqual(membershipManager.insertMany([]), []);
    assert.deepEqual(membershipManager.putMany([]), []);
    const insertedMembership = membershipManager.insert(Membership.create({ member: updatedPerson }));
    assertIid(insertedMembership.iid);
    const storedMembership = membershipManager.getByIid(insertedMembership.iid);
    assert.notEqual(storedMembership, null);
    assert.equal(storedMembership.__typebridgeModel, Membership.typeKey);
    assert.equal(storedMembership.member.__typebridgeModel, Person.typeKey);
    assert.equal(storedMembership.member.iid, insertedPerson.iid);

    const employmentManager = Employment.manager(database);
    const insertedEmployment = employmentManager.insert(Employment.create({ employee: updatedPerson }));
    assertIid(insertedEmployment.iid);
    const storedEmployment = employmentManager.getByIid(insertedEmployment.iid);
    assert.notEqual(storedEmployment, null);
    assert.equal(storedEmployment.__typebridgeModel, Employment.typeKey);
    assert.equal(storedEmployment.employee.__typebridgeModel, Person.typeKey);
    assert.equal(storedEmployment.employee.iid, insertedPerson.iid);
    assert.equal(Object.hasOwn(storedEmployment, "member"), false);
    assert.equal(membershipManager.getByIid(insertedEmployment.iid), null);
    assert.ok(membershipManager.all().some((model: { readonly iid: string }) => model.iid === insertedMembership.iid));
    assert.ok(membershipManager.all().every((model: { readonly iid: string }) => model.iid !== insertedEmployment.iid));

    const eventManager = Event.manager(database);
    const insertedEvent = eventManager.insert(Event.create({ subject: updatedPerson }));
    assertIid(insertedEvent.iid);
    const storedEvent = eventManager.getByIid(insertedEvent.iid);
    assert.notEqual(storedEvent, null);
    assert.equal(storedEvent.__typebridgeModel, Event.typeKey);
    assert.equal(storedEvent.subject.__typebridgeModel, Person.typeKey);
    assert.equal(storedEvent.subject.iid, insertedPerson.iid);

    const containerManager = Container.manager(database);
    const insertedContainer = containerManager.insert(Container.create({
      item: [Event.reference(insertedEvent.iid, {})],
    }));
    assertIid(insertedContainer.iid);
    const storedContainer = containerManager.getByIid(insertedContainer.iid);
    assert.notEqual(storedContainer, null);
    assert.equal(storedContainer.__typebridgeModel, Container.typeKey);
    assert.equal(storedContainer.iid, insertedContainer.iid);
    assert.equal(storedContainer.item.length, 1);
    const storedEventReference = storedContainer.item[0];
    assert.equal(storedEventReference.__typebridgeModel, Event.typeKey);
    assert.equal(storedEventReference.__typebridgeForm, "reference");
    assert.equal(storedEventReference.iid, insertedEvent.iid);
    assert.equal(Object.hasOwn(storedEventReference, "subject"), false);

    const networkManager = NetworkLink.manager(database);
    const insertedNetwork = networkManager.insert(NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("primary"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assertIid(insertedNetwork.iid);
    const putNetwork = networkManager.put(NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("primary"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assert.equal(putNetwork.iid, insertedNetwork.iid);
    const updatedNetwork = networkManager.update(insertedNetwork.iid, NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("updated"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assert.equal(updatedNetwork.iid, insertedNetwork.iid);
    assert.equal(networkManager.getByIid(insertedNetwork.iid)?.nickname?.value, "updated");
    const filteredNetworks = networkManager.filter({ identifier: Identifier.create("network-1") });
    assert.deepEqual(filteredNetworks.all().map((candidate) => candidate.iid), [insertedNetwork.iid]);
    assert.equal(filteredNetworks.first()?.iid, insertedNetwork.iid);
    assert.equal(filteredNetworks.count(), 1n);
    assert.equal(filteredNetworks.exists(), true);

    const querySession = new QuerySession(database);
    assert.throws(
      () => querySession.var(foreign.Person),
      /exact package model token/i,
    );

    const scalarDomainVariable = querySession.exact(Person);
    const scalarDomainPerson = querySession
      .query(scalarDomainVariable)
      .where(
        scalarDomainVariable.field(Person.identifier).eq(Identifier.create("person-1")),
        scalarDomainVariable.field(Person.valBool).eq(ValBool.create(true)),
        scalarDomainVariable.field(Person.valDouble).gte(ValDouble.create(3.5)),
        scalarDomainVariable.field(Person.valDecimal).gte(ValDecimal.create("3.5")),
        scalarDomainVariable
          .field(Person.valDate)
          .gte(ValDate.create(new Date("2026-07-29T00:00:00Z"))),
        scalarDomainVariable
          .field(Person.valDatetime)
          .gte(ValDatetime.create(new Date("2026-07-29T12:34:56Z"))),
        scalarDomainVariable
          .field(Person.valDatetimeTz)
          .gte(ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z"))),
        scalarDomainVariable.field(Person.valDuration).eq(ValDuration.create("PT3S")),
      )
      .one();
    assert.equal(scalarDomainPerson.iid, insertedPerson.iid);
    assert.deepEqual(
      personManager.filter({
        identifier: Identifier.create("person-1"),
        valBool: ValBool.create(true),
        valDate__gte: ValDate.create(new Date("2026-07-29T00:00:00Z")),
        valDatetime__gte: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
        valDatetimeTz__gte: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
        valDecimal__gte: ValDecimal.create("3.5"),
        valDouble__gte: ValDouble.create(3.5),
        valDuration: ValDuration.create("PT3S"),
      }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );

    const counterManager = Counter.manager(database);
    const detachedCounter = Counter.create({ counterValue: CounterValue.create(42n) });
    assert.throws(
      () => counterManager.delete(detachedCounter),
      /attached TypeDB IID/,
    );
    const insertedCounter = counterManager.insert(detachedCounter);
    assertIid(insertedCounter.iid);
    const storedCounter = counterManager.getByIid(insertedCounter.iid);
    assert.notEqual(storedCounter, null);
    assert.equal(storedCounter.__typebridgeModel, Counter.typeKey);
    assert.equal(storedCounter.counterValue.__typebridgeModel, CounterValue.typeKey);
    assert.equal(storedCounter.counterValue.value, 42n);
    const counterVariable = querySession.exact(Counter);
    const counterQuery = querySession
      .query(counterVariable)
      .where(counterVariable.field(Counter.counterValue).eq(CounterValue.create(42n)));
    assert.equal(counterQuery.one().iid, insertedCounter.iid);
    assert.throws(
      () => counterQuery.rows({ limit: 2n }),
      /bounded result identity requires a present unique scalar descriptor field/,
    );
    counterManager.delete(insertedCounter);
    assert.equal(counterManager.getByIid(insertedCounter.iid), null);
    assert.equal(counterManager.count(), 0n);

    const plainActivityManager = PlainActivity.manager(database);
    const insertedPlainActivity = plainActivityManager.insert(
      PlainActivity.create({ participant: insertedPerson }),
    );
    assertIid(insertedPlainActivity.iid);
    const storedPlainActivity = plainActivityManager.getByIid(insertedPlainActivity.iid);
    assert.notEqual(storedPlainActivity, null);
    assert.equal(storedPlainActivity.__typebridgeModel, PlainActivity.typeKey);
    assert.equal(storedPlainActivity.participant.__typebridgeModel, Person.typeKey);
    assert.equal(storedPlainActivity.participant.iid, insertedPerson.iid);
    const plainActivityVariable = querySession.exact(PlainActivity);
    const plainParticipantVariable = querySession.exact(Person);
    const [queriedPlainActivity, queriedPlainParticipant] = querySession
      .query(plainActivityVariable, plainParticipantVariable)
      .where(
        plainActivityVariable.role(PlainActivity.participant).connects(plainParticipantVariable),
        plainParticipantVariable
          .field(Person.identifier)
          .eq(Identifier.create("person-1")),
      )
      .one();
    assert.equal(queriedPlainActivity.iid, insertedPlainActivity.iid);
    assert.equal(queriedPlainParticipant.iid, insertedPerson.iid);
    plainActivityManager.delete(insertedPlainActivity);
    assert.equal(plainActivityManager.getByIid(insertedPlainActivity.iid), null);

    const robotManager = Robot.manager(database);
    const integerKeyValues = [-42n, 1n, 100n, 9999n] as const;
    const insertedRobots = robotManager.insertMany(integerKeyValues.map((value, index) => Robot.create({
      ...(value === -42n ? { nickname: Nickname.create("actor-robot") } : {}),
      robotId: RobotId.create(value),
      valConstrained: ValConstrained.create(BigInt(index + 1)),
    })));
    assert.equal(robotManager.count(), BigInt(integerKeyValues.length));
    for (const [index, value] of integerKeyValues.entries()) {
      const integerKeyMatch = robotManager.filter({ robotId: RobotId.create(value) });
      assert.equal(integerKeyMatch.count(), 1n);
      assert.equal(integerKeyMatch.first()?.iid, insertedRobots[index]?.iid);
      assert.equal(integerKeyMatch.first()?.robotId.value, value);
    }
    assert.deepEqual(
      new Set(
        robotManager
          .filter({ robotId__in: [RobotId.create(-42n), RobotId.create(9999n)] })
          .all()
          .map((candidate) => candidate.robotId.value),
      ),
      new Set([-42n, 9999n]),
    );

    const insertedRobot = insertedRobots[0];
    assert.notEqual(insertedRobot, undefined);
    assertIid(insertedRobot.iid);
    const robotMembership = membershipManager.insert(Membership.create({ member: insertedRobot }));
    assertIid(robotMembership.iid);
    const storedRobotMembership = membershipManager.getByIid(robotMembership.iid);
    assert.notEqual(storedRobotMembership, null);
    assert.equal(storedRobotMembership.member.__typebridgeModel, Robot.typeKey);
    assert.equal(storedRobotMembership.member.iid, insertedRobot.iid);
    assert.equal(storedRobotMembership.member.robotId.value, -42n);

    const interactionManager = Interaction.manager(database);
    const insertedInteractions = interactionManager.insertMany([
      Interaction.create({
        actor: insertedRobot,
        identifier: Identifier.create("interaction-robot"),
        nickname: Nickname.create("assist"),
        target: updatedPerson,
      }),
      Interaction.create({
        actor: updatedPerson,
        identifier: Identifier.create("interaction-person"),
        nickname: Nickname.create("read"),
        target: insertedBatch[0],
      }),
    ]);
    const robotInteraction = insertedInteractions[0];
    const personInteraction = insertedInteractions[1];
    assert.notEqual(robotInteraction, undefined);
    assert.notEqual(personInteraction, undefined);
    assertIid(robotInteraction.iid);
    assertIid(personInteraction.iid);

    const actorVariable = querySession.subtypes(Actor);
    const interactionVariable = querySession.exact(Interaction);
    const polymorphicActorRows = querySession
      .query(interactionVariable)
      .match(actorVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(actorVariable),
        actorVariable.field(Actor.nickname).contains(Nickname.create("a")),
      )
      .rows({
        limit: 10n,
        orderBy: [interactionVariable.field(Interaction.identifier).asc()],
      });
    assert.deepEqual(
      new Set(polymorphicActorRows.map((relation) => relation.actor?.__typebridgeModel)),
      new Set([Person.typeKey, Robot.typeKey]),
    );
    assert.deepEqual(
      new Set(polymorphicActorRows.map((relation) => relation.iid)),
      new Set([robotInteraction.iid, personInteraction.iid]),
    );

    const robotVariable = querySession.exact(Robot);
    const targetVariable = querySession.exact(Person);
    const [queriedRobotInteraction, queriedRobot, queriedTarget] = querySession
      .query(interactionVariable, robotVariable, targetVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(robotVariable),
        interactionVariable.role(Interaction.target).connects(targetVariable),
        interactionVariable.field(Interaction.nickname).eq(Nickname.create("assist")),
        robotVariable.field(Robot.robotId).eq(RobotId.create(-42n)),
        robotVariable.field(Robot.valConstrained).lt(ValConstrained.create(10n)),
        targetVariable.field(Person.identifier).eq(Identifier.create("person-1")),
      )
      .one();
    assert.equal(queriedRobotInteraction.iid, robotInteraction.iid);
    assert.equal(queriedRobot.__typebridgeModel, Robot.typeKey);
    assert.equal(queriedRobot.iid, insertedRobot.iid);
    assert.equal(queriedTarget.iid, insertedPerson.iid);

    const personActorVariable = querySession.exact(Person);
    const [queriedPersonInteraction, queriedPersonActor] = querySession
      .query(interactionVariable, personActorVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(personActorVariable),
        interactionVariable.field(Interaction.nickname).eq(Nickname.create("read")),
        personActorVariable.field(Person.score).gte(Score.create(3n)),
      )
      .one();
    assert.equal(queriedPersonInteraction.iid, personInteraction.iid);
    assert.equal(queriedPersonActor.iid, insertedPerson.iid);
    interactionManager.delete(queriedPersonInteraction);
    assert.equal(interactionManager.getByIid(personInteraction.iid), null);

    membershipManager.delete(robotMembership);
    assert.equal(membershipManager.getByIid(robotMembership.iid), null);
    robotManager.delete(insertedRobot);
    assert.equal(robotManager.getByIid(insertedRobot.iid), null);
    const survivingInteraction = interactionManager.getByIid(robotInteraction.iid);
    assert.notEqual(survivingInteraction, null);
    assert.equal(survivingInteraction.actor, null);
    assert.equal(survivingInteraction.target.__typebridgeModel, Person.typeKey);
    assert.equal(survivingInteraction.target.iid, insertedPerson.iid);
    interactionManager.delete(survivingInteraction);
    assert.equal(interactionManager.getByIid(robotInteraction.iid), null);
    for (const remainingRobot of insertedRobots.slice(1)) {
      robotManager.delete(remainingRobot);
    }
    assert.equal(robotManager.count(), 0n);

    const personVariable = querySession.var(Person);
    const collectedPersonVariable = querySession.var(Person);
    const eventVariable = querySession.var(Event);
    const identifierOrder = personVariable.field(Person.identifier).asc();
    const personPredicate = personVariable
      .field(Person.identifier)
      .eq(Identifier.create("person-1"));
    const eventPredicate = eventVariable.role(Event.subject).connects(personVariable);
    const sameIdentifierPredicate = collectedPersonVariable
      .field(Person.identifier)
      .eqField(personVariable.field(Person.identifier));
    const directQuery = querySession
      .query(personVariable, eventVariable)
      .where(personPredicate, eventPredicate);
    const directOne = directQuery.one();
    assert.equal(directOne[0].iid, insertedPerson.iid);
    assert.equal(directOne[1].iid, insertedEvent.iid);
    assert.equal(directOne[1].subject.iid, insertedPerson.iid);
    const directRows = querySession
      .query(personVariable)
      .where(personPredicate)
      .rows({ limit: 10n, orderBy: [identifierOrder] });
    assert.equal(directRows.length, 1);
    assert.equal(directRows[0].iid, insertedPerson.iid);
    assert.equal(directRows[0].identifier.value, "person-1");
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isPresent())
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isMissing())
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [...batchIids, transactionPersonIid],
    );
    assert.equal(
      querySession.query(personVariable).where(personVariable.iid(insertedPerson.iid)).one().iid,
      insertedPerson.iid,
    );
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.iidIn([insertedPerson.iid, insertedBatch[0].iid]))
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );
    assert.deepEqual(
      querySession.query(personVariable).rows({
        limit: 2n,
        offset: 1n,
        orderBy: [identifierOrder],
      }).map((candidate: { readonly identifier: { readonly value: string } }) => candidate.identifier.value),
      ["person-2", "person-3"],
    );
    const identifierField = personVariable.field(Person.identifier);
    const identifierValue = Identifier.create("person-1");
    const generatedExpression = identifierField.startsWith(Identifier.create("person-"))
      .and(identifierField.contains(Identifier.create("son-")))
      .and(identifierField.endsWith(Identifier.create("-1")))
      .and(identifierField.regex(Identifier.create("^person-1$")))
      .and(identifierField.ne(identifierValue).not())
      .and(
        identifierField.eq(identifierValue)
          .or(identifierField.eq(Identifier.create("does-not-exist"))),
      );
    assert.equal(
      querySession.query(personVariable).where(generatedExpression).one().iid,
      insertedPerson.iid,
    );
    const crossLeft = querySession.exact(Person);
    const crossRight = querySession.exact(Person);
    const crossPair = querySession
      .query(crossLeft, crossRight)
      .allowCrossJoin(crossLeft, crossRight)
      .where(
        crossLeft.field(Person.identifier).eq(Identifier.create("person-1")),
        crossRight.field(Person.identifier).eq(Identifier.create("person-2")),
      )
      .one();
    assert.deepEqual(
      crossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );

    const partyVariable = querySession.subtypes(Party);
    const partyRows = querySession.query(partyVariable).rows({
      limit: 10n,
      orderBy: [partyVariable.field(Party.identifier).asc()],
    });
    assert.deepEqual(
      partyRows.map((candidate: { readonly __typebridgeModel: string }) => candidate.__typebridgeModel),
      [Employee.typeKey, Manager.typeKey],
    );
    assert.deepEqual(
      partyRows.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedEmployee.iid, insertedManager.iid],
    );

    const membershipSubtypeVariable = querySession.subtypes(Membership);
    const membershipFamily = querySession.query(membershipSubtypeVariable);
    assert.equal(membershipFamily.countBy(membershipSubtypeVariable), 2n);
    const queriedBaseRelation = membershipFamily
      .where(membershipSubtypeVariable.iid(insertedMembership.iid))
      .one();
    assert.equal(queriedBaseRelation.__typebridgeModel, Membership.typeKey);
    assert.equal(queriedBaseRelation.member.iid, insertedPerson.iid);
    const queriedSubtypeRelation = membershipFamily
      .where(membershipSubtypeVariable.iid(insertedEmployment.iid))
      .one() as unknown as {
        readonly __typebridgeModel: string;
        readonly employee: { readonly iid: string | null };
        readonly member?: unknown;
      };
    assert.equal(queriedSubtypeRelation.__typebridgeModel, Employment.typeKey);
    assert.equal(queriedSubtypeRelation.employee.iid, insertedPerson.iid);
    assert.equal(Object.hasOwn(queriedSubtypeRelation, "member"), false);

    const reachableSource = querySession.exact(Person);
    const reachableTarget = querySession.exact(Person);
    const reachable = querySession.reachable(
      reachableSource,
      reachableTarget,
      NetworkLink,
      NetworkLink.origin,
      NetworkLink.destination,
      { minDepth: 1, maxDepth: 1 },
    );
    const reachablePair = querySession
      .query(reachableSource, reachableTarget)
      .where(
        reachable,
        reachableSource.field(Person.identifier).eq(Identifier.create("person-1")),
        reachableTarget.field(Person.identifier).eq(Identifier.create("person-2")),
      )
      .one();
    assert.deepEqual(
      reachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );
    const networkVariable = querySession.exact(NetworkLink);
    const queriedNetwork = querySession
      .query(networkVariable)
      .where(
        networkVariable.iid(insertedNetwork.iid),
        networkVariable.field(NetworkLink.nickname).isPresent(),
      )
      .one();
    assert.equal(queriedNetwork.iid, insertedNetwork.iid);

    const allPeopleQuery = querySession.query(personVariable);
    assert.equal(allPeopleQuery.first({ orderBy: [identifierOrder] })?.iid, insertedPerson.iid);
    const scoreField = personVariable.field(Person.score);
    const directAggregate = allPeopleQuery.aggregate(personVariable, [
      aggregate.count(),
      aggregate.sum(scoreField),
      aggregate.min(scoreField),
      aggregate.max(scoreField),
      aggregate.mean(scoreField),
      aggregate.median(scoreField),
      aggregate.std(scoreField),
    ] as const);
    assert.deepEqual(directAggregate.slice(0, 6), [4n, 24n, 3n, 9n, 6, 6]);
    assert.equal(typeof directAggregate[6], "number");
    const directFieldGroupedAggregate = allPeopleQuery
      .groupBy(personVariable, personVariable.field(Person.valBool))
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.equal(directFieldGroupedAggregate.length, 1);
    assert.equal(directFieldGroupedAggregate[0][0].value, true);
    assert.deepEqual(directFieldGroupedAggregate[0][1], [4n, 24n]);
    const directTupleFieldGroupedAggregate = allPeopleQuery
      .groupBy(personVariable, personVariable.field(Person.valBool), scoreField)
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.deepEqual(
      directTupleFieldGroupedAggregate.map(([[boolGroup, scoreGroup], values]) => [
        boolGroup.value,
        scoreGroup.value,
        values,
      ]),
      [
        [true, 3n, [1n, 3n]],
        [true, 5n, [1n, 5n]],
        [true, 7n, [1n, 7n]],
        [true, 9n, [1n, 9n]],
      ],
    );
    assert.throws(
      () => allPeopleQuery.aggregate(personVariable, [
        aggregate.sum(personVariable.field(Person.identifier)),
      ] as const),
      /long|double|numeric|reduc/i,
    );

    const directGroupedAggregate = directQuery
      .groupBy(personVariable, eventVariable)
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.equal(directGroupedAggregate.length, 1);
    assert.equal(directGroupedAggregate[0][0].iid, insertedEvent.iid);
    assert.deepEqual(directGroupedAggregate[0][1], [1n, 3n]);

    const named = querySession
      .queryNamed({ person: personVariable })
      .where(personPredicate)
      .one();
    assert.equal(named.person.iid, insertedPerson.iid);
    assert.equal(directQuery.countBy(personVariable), 1n);
    assert.equal(directQuery.existsBy(personVariable), true);

    const collectedPage = querySession
      .query(personVariable, collectedPersonVariable.collect().distinct())
      .where(personPredicate, sameIdentifierPredicate)
      .pageBy(personVariable, { limit: 10n, includeTotal: true });
    assert.equal(collectedPage.offset, 0n);
    assert.equal(collectedPage.limit, 10n);
    assert.equal(collectedPage.total, 1n);
    assert.equal(collectedPage.items.length, 1);
    assert.equal(collectedPage.items[0][0].iid, insertedPerson.iid);
    assert.deepEqual(
      collectedPage.items[0][1].map((item: { readonly iid: string }) => item.iid),
      [insertedPerson.iid],
    );

    const declared = await readFile(resolve(stage, "declared-schema.json"));
    const port = await freePort();
    const suppliedServer = process.env.TYPE_BRIDGE_V2_SMOKE_SERVER;
    const server = spawn(
      suppliedServer ?? "cargo",
      suppliedServer === undefined
        ? [
            "run",
            "--quiet",
            "-p",
            "type-bridge-server",
            "--features",
            "v2-query",
            "--example",
            "v2_smoke_server",
          ]
        : [],
      {
        cwd: CORE,
        env: {
          ...process.env,
          SMOKE_TYPEDB_ADDRESS: TYPEDB_ADDRESS,
          SMOKE_TYPEDB_USERNAME: TYPEDB_USERNAME,
          SMOKE_TYPEDB_PASSWORD: TYPEDB_PASSWORD,
          SMOKE_TYPEDB_HTTP_PORT: String(TYPEDB_HTTP_PORT),
          SMOKE_DATABASE: INTG_DATABASE,
          SMOKE_DECLARED_B64: declared.toString("base64"),
          SMOKE_SCOPE: REMOTE_SCOPE,
          SMOKE_PROFILE: REMOTE_PROFILE,
          SMOKE_PORT: String(port),
        },
        stdio: "ignore",
      },
    );
    try {
      await waitForPort(port, server, 300_000);
      const advertisementResponse = await fetch(`http://127.0.0.1:${port}/v2/capabilities`);
      assert.equal(advertisementResponse.status, 200);
      const advertisement = Buffer.from(await advertisementResponse.arrayBuffer());
      const requests: Buffer[] = [];
      async function exchange(request: Uint8Array): Promise<Buffer> {
        requests.push(Buffer.from(request));
        const response = await fetch(`http://127.0.0.1:${port}/v2/query`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: new Uint8Array(request),
        });
        assert.equal(response.status, 200);
        return Buffer.from(await response.arrayBuffer());
      }

      // docs: remote-query-node:start
      const remoteSession = new RemoteQuerySession(
        new QueryV2Authority(declared, REMOTE_SCOPE, REMOTE_PROFILE),
        advertisement,
        exchange,
        {
          maxItems: 100n,
          maxBytes: 1n << 20n,
          maxCollectionMembers: 100n,
          maxGraphNodes: 1_000n,
          maxAttributeValues: 1_000n,
          maxRolePlayers: 1_000n,
          deadlineMs: 30_000n,
        },
      );
      const remotePerson = remoteSession.var(Person);
      const remoteCollectedPerson = remoteSession.var(Person);
      const remoteEvent = remoteSession.var(Event);
      const remotePersonPredicate = remotePerson
        .field(Person.identifier)
        .eq(Identifier.create("person-1"));
      const remoteEventPredicate = remoteEvent.role(Event.subject).connects(remotePerson);
      const remoteDirectQuery = remoteSession
        .query(remotePerson, remoteEvent)
        .where(remotePersonPredicate, remoteEventPredicate);
      const remoteOne = await remoteDirectQuery.one();
      assert.deepEqual(
        [remoteOne[0].iid, remoteOne[1].iid],
        [directOne[0].iid, directOne[1].iid],
      );
      const remotePersonOnly = remoteSession
        .query(remotePerson)
        .where(remotePersonPredicate);
      const remoteRows = await remotePersonOnly.rows({
        limit: 10n,
        orderBy: [remotePerson.field(Person.identifier).asc()],
      });
      assert.deepEqual(remoteRows.map((item: { readonly iid: string }) => item.iid), [insertedPerson.iid]);
      const remoteFirst = await remotePersonOnly.first({
        orderBy: [remotePerson.field(Person.identifier).asc()],
      });
      assert.equal(remoteFirst?.iid, insertedPerson.iid);
      assert.deepEqual(
        (await remoteSession
          .query(remotePerson)
          .where(remotePerson.field(Person.aliases).isPresent())
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] }))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedPerson.iid],
      );
      assert.deepEqual(
        (await remoteSession
          .query(remotePerson)
          .where(remotePerson.field(Person.aliases).isMissing())
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] }))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [...batchIids, transactionPersonIid],
      );
      assert.equal(
        (await remoteSession.query(remotePerson).where(remotePerson.iid(insertedPerson.iid)).one()).iid,
        insertedPerson.iid,
      );
      assert.deepEqual(
        (await remoteSession
          .query(remotePerson)
          .where(remotePerson.iidIn([insertedPerson.iid, insertedBatch[0].iid]))
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] }))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedPerson.iid, insertedBatch[0].iid],
      );
      const remoteNetwork = remoteSession.exact(NetworkLink);
      assert.equal(
        (await remoteSession
          .query(remoteNetwork)
          .where(
            remoteNetwork.iid(insertedNetwork.iid),
            remoteNetwork.field(NetworkLink.nickname).isPresent(),
          )
          .one()).iid,
        insertedNetwork.iid,
      );

      const remoteParty = remoteSession.subtypes(Party);
      const remotePartyRows = await remoteSession.query(remoteParty).rows({
        limit: 10n,
        orderBy: [remoteParty.field(Party.identifier).asc()],
      });
      assert.deepEqual(
        remotePartyRows.map((candidate: { readonly __typebridgeModel: string }) => candidate.__typebridgeModel),
        [Employee.typeKey, Manager.typeKey],
      );
      assert.deepEqual(
        remotePartyRows.map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedEmployee.iid, insertedManager.iid],
      );
      // docs: remote-query-node:end
      const remoteNamed = await remoteSession
        .queryNamed({ person: remotePerson })
        .where(remotePersonPredicate)
        .one();
      assert.equal(remoteNamed.person.iid, named.person.iid);
      assert.equal(await remoteDirectQuery.countBy(remotePerson), directQuery.countBy(personVariable));
      assert.equal(await remoteDirectQuery.existsBy(remotePerson), directQuery.existsBy(personVariable));

      const remoteScoreField = remotePerson.field(Person.score);
      const requestsBeforeReduction = requests.length;
      await assert.rejects(
        remoteSession.query(remotePerson).aggregate(remotePerson, [
          aggregate.count(),
          aggregate.sum(remoteScoreField),
          aggregate.min(remoteScoreField),
          aggregate.max(remoteScoreField),
          aggregate.mean(remoteScoreField),
          aggregate.median(remoteScoreField),
          aggregate.std(remoteScoreField),
        ] as const),
        /query_remote_v2_native_only_operation/,
      );
      assert.equal(requests.length, requestsBeforeReduction);
      const requestsBeforeGroupedReduction = requests.length;
      await assert.rejects(
        remoteDirectQuery
          .groupBy(remotePerson, remoteEvent)
          .aggregate([aggregate.count(), aggregate.sum(remoteScoreField)] as const),
        /query_remote_v2_native_only_operation/,
      );
      assert.equal(requests.length, requestsBeforeGroupedReduction);

      const remoteSameIdentifier = remoteCollectedPerson
        .field(Person.identifier)
        .eqField(remotePerson.field(Person.identifier));
      const remotePage = await remoteSession
        .query(remotePerson, remoteCollectedPerson.collect().distinct())
        .where(remotePersonPredicate, remoteSameIdentifier)
        .pageBy(remotePerson, { limit: 10n, includeTotal: true });
      assert.deepEqual(
        {
          items: remotePage.items.map((item: readonly [
            { readonly iid: string },
            readonly { readonly iid: string }[],
          ]) => [item[0].iid, item[1].map((member) => member.iid)]),
          limit: remotePage.limit,
          offset: remotePage.offset,
          total: remotePage.total,
        },
        {
          items: collectedPage.items.map((item: readonly [
            { readonly iid: string },
            readonly { readonly iid: string }[],
          ]) => [item[0].iid, item[1].map((member) => member.iid)]),
          limit: collectedPage.limit,
          offset: collectedPage.offset,
          total: collectedPage.total,
        },
      );

      const remoteReachableSource = remoteSession.exact(Person);
      const remoteReachableTarget = remoteSession.exact(Person);
      const remoteReachable = remoteSession.reachable(
        remoteReachableSource,
        remoteReachableTarget,
        NetworkLink,
        NetworkLink.origin,
        NetworkLink.destination,
        { minDepth: 1, maxDepth: 1 },
      );
      const remoteReachablePair = await remoteSession
        .query(remoteReachableSource, remoteReachableTarget)
        .where(
          remoteReachable,
          remoteReachableSource.field(Person.identifier).eq(Identifier.create("person-1")),
          remoteReachableTarget.field(Person.identifier).eq(Identifier.create("person-2")),
        )
        .one();
      assert.deepEqual(
        remoteReachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
        reachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      );
      const remoteCrossLeft = remoteSession.exact(Person);
      const remoteCrossRight = remoteSession.exact(Person);
      const remoteCrossPair = await remoteSession
        .query(remoteCrossLeft, remoteCrossRight)
        .allowCrossJoin(remoteCrossLeft, remoteCrossRight)
        .where(
          remoteCrossLeft.field(Person.identifier).eq(Identifier.create("person-1")),
          remoteCrossRight.field(Person.identifier).eq(Identifier.create("person-2")),
        )
        .one();
      assert.deepEqual(
        remoteCrossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
        crossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      );
      assert.equal(requests.length, 15);
      assert.ok(requests.every((request) => request.length > 0));
    } finally {
      if (server.exitCode === null) {
        const exited = new Promise((resolveExit) => server.once("exit", resolveExit));
        server.kill("SIGKILL");
        await exited;
      }
    }

    const relationBatch = networkManager.insertMany([
      NetworkLink.create({
        destination: insertedBatch[1],
        identifier: Identifier.create("network-2"),
        origin: insertedBatch[0],
        participant: [insertedBatch[0], insertedBatch[1]],
      }),
      NetworkLink.create({
        destination: insertedPerson,
        identifier: Identifier.create("network-3"),
        origin: insertedBatch[1],
        participant: [insertedBatch[1], insertedPerson],
      }),
    ]);
    assert.ok(relationBatch.every((candidate) => candidate.iid !== null));
    assert.deepEqual(
      networkManager.putMany(relationBatch).map((candidate) => candidate.iid),
      relationBatch.map((candidate) => candidate.iid),
    );
    for (const relation of relationBatch) {
      assertIid(relation.iid);
      networkManager.delete(relation);
      assert.equal(networkManager.getByIid(relation.iid), null);
    }

    membershipManager.delete(insertedMembership);
    assert.equal(membershipManager.getByIid(insertedMembership.iid), null);
    networkManager.delete(insertedNetwork);
    assert.equal(networkManager.getByIid(insertedNetwork.iid), null);
    personManager.delete(transactionPersonIid);
    assert.equal(personManager.getByIid(transactionPersonIid), null);
  } catch (error) {
    failure = error;
  }

  try {
    database?.deleteDatabase();
  } catch (error) {
    failure ??= error;
  }
  try {
    await rm(
      ownsStage ? stage : resolve(stage, "node_modules", "@type-bridge", "node"),
      { recursive: true, force: true },
    );
  } catch (error) {
    failure ??= error;
  }
  if (failure !== undefined) {
    throw failure;
  }
});
