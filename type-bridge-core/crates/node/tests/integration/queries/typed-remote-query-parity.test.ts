/**
 * Public model-query parity: one three-binding subtype/relation query through
 * local QuerySession execution and one-exchange RemoteQuerySession execution.
 */

import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  TYPEDB_ADDRESS,
  TYPEDB_HTTP_PORT,
  TYPEDB_PASSWORD,
  TYPEDB_USERNAME,
} from "../common/index.ts";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const packageRoot = path.resolve(__dirname, "../../..");
const requirePackage = createRequire(import.meta.url);
const typeBridge = requirePackage(
  packageRoot,
) as typeof import("../../../typescript/index.ts");
const queryV2 = requirePackage(
  path.join(packageRoot, "dist/query-v2.js"),
) as typeof import("../../../typescript/query-v2.ts");
const typedBridge = requirePackage(
  path.join(packageRoot, "dist/typed/index.js"),
) as typeof import("../../../typescript/typed/index.ts");

const {
  Card,
  Entity,
  Key,
  Relation,
  TypeFlags,
  attr,
  field,
  role,
} = typeBridge;
const { QueryPlanBuilder, QueryV2Authority } = queryV2;
const {
  QuerySession,
  RemoteQuerySession,
  references,
} = typedBridge;

const DECLARED_PATH = path.resolve(
  packageRoot,
  "../../../tests/fixtures/query-v2-model-remote-parity-declared.json",
);
const SCOPE = "model-remote-parity";
const PROFILE = "typedb-3.12.1/v1";
const ADVANCED_PLAN_FINGERPRINT =
  "85c9504dca956286b46336510af3b24980bba1a72e79465069b7a24e7d52e26f";

const SCHEMA = `
define
attribute parity-person-name, value string;
attribute parity-project-name, value string;
attribute parity-assignment-id, value string;
entity parity-person @abstract,
    owns parity-person-name @key,
    plays parity-assignment:employee;
entity parity-employee sub parity-person;
entity parity-project,
    owns parity-project-name @key,
    plays parity-assignment:project;
relation parity-assignment,
    owns parity-assignment-id @key,
    relates employee @card(1),
    relates project @card(1);
`;

const DATA = `
insert
$alice isa parity-employee, has parity-person-name "Alice";
$bob isa parity-employee, has parity-person-name "Bob";
$alpha isa parity-project, has parity-project-name "Alpha";
$beta isa parity-project, has parity-project-name "Beta";
$first isa parity-assignment,
    links (employee: $alice, project: $alpha),
    has parity-assignment-id "assignment-1";
$second isa parity-assignment,
    links (employee: $bob, project: $beta),
    has parity-assignment-id "assignment-2";
`;

class ParityPersonName extends attr.String("parity-person-name") {}
class ParityProjectName extends attr.String("parity-project-name") {}
class ParityAssignmentId extends attr.String("parity-assignment-id") {}

class ParityPerson extends Entity(
  TypeFlags({ name: "parity-person", abstract: true }),
  { name: field(ParityPersonName, Key) },
) {}

class ParityEmployee extends Entity(
  "parity-employee",
  {},
  { parent: ParityPerson },
) {}

class ParityProject extends Entity("parity-project", {
  name: field(ParityProjectName, Key),
}) {}

class ParityAssignment extends Relation("parity-assignment", {
  assignmentId: field(ParityAssignmentId, Key),
  employee: role(ParityPerson, { cardinality: Card(1, 1) }),
  project: role(ParityProject, { cardinality: Card(1, 1) }),
}) {}

const personRefs = references(ParityPerson);
const assignmentRefs = references(ParityAssignment);

type ParityRow = readonly [
  ParityPerson,
  ParityProject,
  ParityAssignment,
];

function requireSingular<Value>(value: Value | readonly Value[]): Value {
  if (Array.isArray(value)) {
    throw new TypeError("exact-one parity role unexpectedly hydrated as a collection");
  }
  return value as Value;
}

function normalize(rows: readonly ParityRow[]) {
  return rows.map(([employee, project, assignment]) => ({
    assignment: assignment.assignmentId.value,
    concrete: employee.constructor.name,
    employee: employee.name.value,
    project: project.name.value,
    roleEmployee: requireSingular(assignment.employee).name.value,
    roleProject: requireSingular(assignment.project).name.value,
  }));
}

function authorAdvancedPlan(authority: InstanceType<typeof QueryV2Authority>) {
  const builder = new QueryPlanBuilder(authority);
  const localPerson = builder.binding("lp");
  const localName = builder.binding("ln");
  const localFunction = builder.localFunction(
    "local_name_count",
    [localName, localPerson],
    [localPerson],
    ["parity-person"],
    [
      builder.isa(localPerson, "entity", "parity-person", true),
      builder.has(localPerson, localName, "parity-person-name"),
    ],
    builder.localReturn("count", localName, "long"),
  );

  const person = builder.binding("person");
  const name = builder.binding("name");
  const optionalName = builder.binding("optional_name");
  const localResult = builder.binding("local_result");
  const countResult = builder.binding("count_result");
  const wantedName = builder.input("wanted_name", "string", false);
  const nameOperand = builder.bindingOperand(name);
  const nobody = builder.literalOperand("string", "nobody");
  const equal = builder.value(
    "equal",
    nameOperand,
    builder.inputOperand(wantedName),
  );
  const notEqual = builder.value("not_equal", nameOperand, nobody);
  builder.match([
    builder.isa(person, "entity", "parity-person", true),
    builder.has(person, name, "parity-person-name"),
    builder.or([[equal], [notEqual]]),
    builder.not([builder.value("equal", nameOperand, nobody)]),
    builder.try([
      builder.has(person, optionalName, "parity-person-name"),
    ]),
    builder.functionCall(
      localResult,
      [builder.bindingOperand(person)],
      null,
      localFunction,
    ),
  ]);
  builder.select([person, name, localResult]);
  builder.require([name]);
  builder.distinct();
  const count = builder.reduceAssignment(countResult, "count");
  builder.reduce([count], [name]);
  builder.sort([
    builder.order(name, "ascending"),
    builder.order(countResult, "descending"),
  ]);
  builder.offset(0n);
  builder.limit(10n);
  const plan = builder.finalizeRows([name, countResult]);
  assert.equal(plan.fingerprint, ADVANCED_PLAN_FINGERPRINT);
  return { invocation: plan.rows([["Alice"]]), plan };
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`TLS model-query parity requires ${name}`);
  }
  return value;
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (address === null || typeof address === "string") {
        reject(new Error("no probe address"));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
}

async function waitForPort(
  port: number,
  server: ChildProcess,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`smoke server exited early with code ${server.exitCode}`);
    }
    const reachable = await new Promise<boolean>((resolve) => {
      const socket = net.connect({ host: "127.0.0.1", port, timeout: 1_000 });
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      socket.once("error", () => resolve(false));
      socket.once("timeout", () => {
        socket.destroy();
        resolve(false);
      });
    });
    if (reachable) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error("smoke server never became reachable");
}

// The remote model exchange binds the 3.12.1 semantic profile into its
// prepared authority; legacy servers reject the profile before I/O, so the
// parity proof is meaningful only against a 3.12+ lane.
const parityServerVersion = process.env.TYPEDB_VERSION ?? "3.12.1";
const [parityMajor = 0, parityMinor = 0] = parityServerVersion.split(".").map(Number);
const parityServerIsV2Conformant = parityMajor > 3 || (parityMajor === 3 && parityMinor >= 12);
const paritySkip = parityServerIsV2Conformant ? false : "requires a TypeDB 3.12+ server";

test("public remote model query matches local three-binding subtype hydration", { skip: paritySkip }, async () => {
  const tlsEnvironment = [
    process.env.TYPEDB_TLS_ADDRESS,
    process.env.TYPEDB_TLS_HTTP_PORT,
    process.env.TYPEDB_TLS_ROOT_CA,
  ];
  const tlsEnabled = tlsEnvironment.some((value) => value !== undefined);
  const address = tlsEnabled
    ? requiredEnvironment("TYPEDB_TLS_ADDRESS")
    : TYPEDB_ADDRESS;
  const httpPort = tlsEnabled
    ? Number(requiredEnvironment("TYPEDB_TLS_HTTP_PORT"))
    : TYPEDB_HTTP_PORT;
  const tlsRootCa = tlsEnabled
    ? requiredEnvironment("TYPEDB_TLS_ROOT_CA")
    : undefined;
  const connectionOptions = {
    username: TYPEDB_USERNAME,
    password: TYPEDB_PASSWORD,
    httpPort,
    ...(tlsEnabled ? { tlsEnabled: true, tlsRootCa } : {}),
  };
  const serverTlsEnvironment = tlsEnabled
    ? {
        SMOKE_TYPEDB_TLS: "true",
        SMOKE_TYPEDB_TLS_ROOT_CA: requiredEnvironment("TYPEDB_TLS_ROOT_CA"),
        SMOKE_TLS_CERT: requiredEnvironment("SMOKE_TLS_CERT"),
        SMOKE_TLS_KEY: requiredEnvironment("SMOKE_TLS_KEY"),
      }
    : {};
  if (tlsEnabled) {
    requiredEnvironment("NODE_EXTRA_CA_CERTS");
  }
  const remoteScheme = tlsEnabled ? "https" : "http";
  const database = `tb_v2_node_model_parity_${process.pid}_${Date.now()}`;

  typeBridge.ensureDatabase(address, database, connectionOptions);
  const db = typeBridge.RustDatabase.connect(
    address,
    database,
    connectionOptions,
  );
  try {
    const schemaTransaction = db.transaction("schema");
    schemaTransaction.query(SCHEMA);
    schemaTransaction.commit();
    const writeTransaction = db.transaction("write");
    writeTransaction.query(DATA);
    writeTransaction.commit();

    const declaredFile = fs.readFileSync(DECLARED_PATH);
    const declared = declaredFile.at(-1) === 0x0a
      ? declaredFile.subarray(0, declaredFile.length - 1)
      : declaredFile;
    const advancedAuthority = new QueryV2Authority(declared, SCOPE, PROFILE);
    const { invocation: advancedInvocation, plan: advancedPlan } =
      authorAdvancedPlan(advancedAuthority);
    const advancedLocal = await typeBridge.queryV2ExecuteLocal(
      db,
      QueryV2Authority.queryOnly(db, declared, SCOPE, PROFILE),
      advancedPlan.canonicalBytes,
      Buffer.from(advancedInvocation.canonicalBytes).toString("utf8"),
    );
    assert.deepEqual(JSON.parse(advancedLocal), {
      kind: "rows",
      rows: [
        [
          {
            kind: "attribute",
            type_id: {
              kind: "attribute",
              label: "parity-person-name",
            },
            value: { kind: "string", value: "Alice" },
          },
          { kind: "value", value: { kind: "long", value: "1" } },
        ],
        [
          {
            kind: "attribute",
            type_id: {
              kind: "attribute",
              label: "parity-person-name",
            },
            value: { kind: "string", value: "Bob" },
          },
          { kind: "value", value: { kind: "long", value: "1" } },
        ],
      ],
    });

    const directSession = new QuerySession(db).registerModels(
      ParityEmployee,
      ParityProject,
      ParityAssignment,
    );
    const directEmployee = directSession.var(ParityPerson, "subtypes");
    const directProject = directSession.var(ParityProject);
    const directAssignment = directSession.var(ParityAssignment);
    const directRows = directSession
      .query(directEmployee, directProject, directAssignment)
      .where(
        directAssignment
          .role(assignmentRefs.roles.employee)
          .connects(directEmployee),
        directAssignment
          .role(assignmentRefs.roles.project)
          .connects(directProject),
      )
      .rows({
        limit: 10,
        orderBy: [directEmployee.field(personRefs.fields.name).asc()],
      });

    const expected = [
      {
        assignment: "assignment-1",
        concrete: "ParityEmployee",
        employee: "Alice",
        project: "Alpha",
        roleEmployee: "Alice",
        roleProject: "Alpha",
      },
      {
        assignment: "assignment-2",
        concrete: "ParityEmployee",
        employee: "Bob",
        project: "Beta",
        roleEmployee: "Bob",
        roleProject: "Beta",
      },
    ];
    assert.deepEqual(normalize(directRows), expected);
    assert.ok(directRows.every(([employee]) => employee instanceof ParityEmployee));

    const port = await freePort();
    const server = spawn(
      "cargo",
      [
        "run",
        "--quiet",
        "-p",
        "type-bridge-server",
        "--features",
        "v2-query",
        "--example",
        "v2_smoke_server",
      ],
      {
        cwd: path.resolve(packageRoot, "../.."),
        env: {
          ...process.env,
          SMOKE_TYPEDB_ADDRESS: address,
          SMOKE_TYPEDB_USERNAME: TYPEDB_USERNAME,
          SMOKE_TYPEDB_PASSWORD: TYPEDB_PASSWORD,
          SMOKE_TYPEDB_HTTP_PORT: String(httpPort),
          SMOKE_DATABASE: database,
          SMOKE_DECLARED_B64: Buffer.from(declared).toString("base64"),
          SMOKE_SCOPE: SCOPE,
          SMOKE_PROFILE: PROFILE,
          SMOKE_PORT: String(port),
          ...serverTlsEnvironment,
        },
        stdio: "ignore",
      },
    );
    try {
      await waitForPort(port, server, 300_000);
      const advertisementResponse = await fetch(
        `${remoteScheme}://127.0.0.1:${port}/v2/capabilities`,
      );
      assert.equal(advertisementResponse.status, 200);
      const advertisement = Buffer.from(
        await advertisementResponse.arrayBuffer(),
      );
      let advancedExchanges = 0;
      async function postAdvanced(request: Uint8Array): Promise<Buffer> {
        advancedExchanges += 1;
        const response = await fetch(
          `${remoteScheme}://127.0.0.1:${port}/v2/query`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: new Uint8Array(request),
          },
        );
        return Buffer.from(await response.arrayBuffer());
      }
      const advancedPending = typeBridge.queryV2PrepareRemote(
        advancedAuthority,
        advancedPlan.canonicalBytes,
        Buffer.from(advancedInvocation.canonicalBytes).toString("utf8"),
        advertisement,
        {
          maxItems: 10n,
          maxBytes: 1n << 20n,
          maxCollectionMembers: 30n,
          deadlineMs: 30_000n,
        },
      );
      assert.equal(advancedExchanges, 0);
      const advancedRemote = await advancedPending.decodeReply(
        await postAdvanced(advancedPending.requestBytes()),
      );
      assert.equal(advancedExchanges, 1);
      assert.equal(advancedRemote, advancedLocal);

      let exchanges = 0;
      async function postRemote(request: Uint8Array): Promise<Buffer> {
        exchanges += 1;
        const response = await fetch(
          `${remoteScheme}://127.0.0.1:${port}/v2/query`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: new Uint8Array(request),
          },
        );
        return Buffer.from(await response.arrayBuffer());
      }

      // docs: remote-query-node:start
      const remoteSession = new RemoteQuerySession(
        new QueryV2Authority(declared, SCOPE, PROFILE),
        advertisement,
        postRemote,
        {
          maxItems: 10n,
          maxBytes: 1n << 20n,
          maxCollectionMembers: 30n,
          maxGraphNodes: 30n,
          maxAttributeValues: 30n,
          maxRolePlayers: 30n,
          deadlineMs: 30_000n,
        },
      ).registerModels(
        ParityEmployee,
        ParityProject,
        ParityAssignment,
      );
      const remoteEmployee = remoteSession.var(ParityPerson, "subtypes");
      const remoteProject = remoteSession.var(ParityProject);
      const remoteAssignment = remoteSession.var(ParityAssignment);
      const remoteQuery = remoteSession
        .query(remoteEmployee, remoteProject, remoteAssignment)
        .where(
          remoteAssignment
            .role(assignmentRefs.roles.employee)
            .connects(remoteEmployee),
          remoteAssignment
            .role(assignmentRefs.roles.project)
            .connects(remoteProject),
        );
      assert.equal(exchanges, 0);
      const remoteRows = await remoteQuery.rows({
        limit: 10,
        orderBy: [remoteEmployee.field(personRefs.fields.name).asc()],
      });
      // docs: remote-query-node:end

      assert.equal(exchanges, 1);
      assert.deepEqual(normalize(remoteRows), normalize(directRows));
      assert.ok(remoteRows.every(([employee]) => employee instanceof ParityEmployee));
    } finally {
      const exited = new Promise((resolve) => server.once("exit", resolve));
      server.kill("SIGKILL");
      await exited;
    }
  } finally {
    db.deleteDatabase();
  }
});
