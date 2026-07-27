import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { connectIntegration, defineSchema } from "../integration/common/index.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const CORE = resolve(HERE, "../../../..");
const ROOT = resolve(CORE, "..");
const NODE_PACKAGE = resolve(CORE, "crates/node");
const ACCEPTANCE = resolve(CORE, "crates/schema-codegen/tests/acceptance");
const ACCEPTANCE_SCHEMA = resolve(ACCEPTANCE, "schema.yaml");
const PROVIDER_SCHEMA = resolve(ACCEPTANCE, "provider-3.12.1.tql");

function run(command: string, args: readonly string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    env: process.env,
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

test("generated package round-trips exact Phase 3 models on TypeDB 3.12.1", { timeout: 180_000 }, async () => {
  const stage = await mkdtemp(join(tmpdir(), "type-bridge-node-projection-"));
  const generatedDirectory = resolve(stage, "generated_v2");
  let database;
  let failure: unknown;

  try {
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
      ],
      ROOT,
    );

    const packageScope = resolve(stage, "node_modules/@type-bridge");
    await mkdir(packageScope, { recursive: true });
    await symlink(NODE_PACKAGE, resolve(packageScope, "node"), process.platform === "win32" ? "junction" : "dir");
    run(
      resolve(NODE_PACKAGE, "node_modules/.bin/tsc"),
      ["--project", resolve(generatedDirectory, "tsconfig.json")],
      generatedDirectory,
    );

    const generated = await import(pathToFileURL(resolve(generatedDirectory, "dist/index.js")).href);
    const {
      Aliases,
      Container,
      Employment,
      Event,
      Identifier,
      Membership,
      Nickname,
      Person,
      PROJECTION_FINGERPRINT_JSON,
      RUNTIME_PROJECTION_JSON,
      SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    } = generated;

    const runtimeProjection = JSON.parse(RUNTIME_PROJECTION_JSON);
    assert.deepEqual(runtimeProjection.semantic_fingerprint, JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON));
    assert.deepEqual(runtimeProjection.projection_fingerprint, JSON.parse(PROJECTION_FINGERPRINT_JSON));

    database = connectIntegration();
    database.resetDatabase();
    defineSchema(database, await readFile(PROVIDER_SCHEMA, "utf8"));

    const personManager = Person.manager(database);
    const personInput = Person.create({
      identifier: Identifier.create("person-1"),
      nickname: Nickname.create("alice"),
      aliases: [Aliases.create("alpha"), Aliases.create("beta")],
    });
    const insertedPerson = personManager.insert(personInput);
    assertIid(insertedPerson.iid);
    assert.equal(insertedPerson.__typebridgeModel, Person.typeKey);
    assert.equal(insertedPerson.__typebridgeForm, "complete");

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

    const membershipManager = Membership.manager(database);
    const insertedMembership = membershipManager.insert(Membership.create({ member: storedPerson }));
    assertIid(insertedMembership.iid);
    const storedMembership = membershipManager.getByIid(insertedMembership.iid);
    assert.notEqual(storedMembership, null);
    assert.equal(storedMembership.__typebridgeModel, Membership.typeKey);
    assert.equal(storedMembership.member.__typebridgeModel, Person.typeKey);
    assert.equal(storedMembership.member.iid, insertedPerson.iid);

    const employmentManager = Employment.manager(database);
    const insertedEmployment = employmentManager.insert(Employment.create({ employee: storedPerson }));
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
    const insertedEvent = eventManager.insert(Event.create({ subject: storedPerson }));
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
  } catch (error) {
    failure = error;
  }

  try {
    database?.deleteDatabase();
  } catch (error) {
    failure ??= error;
  }
  try {
    await rm(stage, { recursive: true, force: true });
  } catch (error) {
    failure ??= error;
  }
  if (failure !== undefined) {
    throw failure;
  }
});
