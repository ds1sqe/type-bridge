"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const consumerRoot = fs.realpathSync(process.env.TYPE_BRIDGE_PACKED_CONSUMER_ROOT);
const sourcePackageRoot = fs.realpathSync(process.env.TYPE_BRIDGE_SOURCE_PACKAGE_ROOT);
const declaredPath = process.env.TYPE_BRIDGE_V2_DECLARED_FIXTURE;
const serverUrl = process.env.TYPE_BRIDGE_V2_SERVER_URL;
const typedbTlsEnabledText = process.env.TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED;
const typedbTlsRootCa = process.env.TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA;
if (!declaredPath) throw new Error("TYPE_BRIDGE_V2_DECLARED_FIXTURE is not set");
if (!serverUrl) throw new Error("TYPE_BRIDGE_V2_SERVER_URL is not set");
assert.match(
  typedbTlsEnabledText ?? "",
  /^(0|1)$/,
  "TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED must be exactly 0 or 1",
);
const typedbTlsEnabled = typedbTlsEnabledText === "1";
if (typedbTlsEnabled) {
  assert.ok(typedbTlsRootCa, "TypeDB TLS requires an explicit root CA");
  assert.equal(
    fs.statSync(typedbTlsRootCa).isFile(),
    true,
    "TypeDB TLS root CA must be a regular file",
  );
  assert.ok(
    process.env.NODE_EXTRA_CA_CERTS,
    "remote HTTPS requires startup-time NODE_EXTRA_CA_CERTS",
  );
  assert.equal(new URL(serverUrl).protocol, "https:");
} else {
  assert.equal(typedbTlsRootCa, undefined);
  assert.equal(process.env.NODE_EXTRA_CA_CERTS, undefined);
  assert.equal(new URL(serverUrl).protocol, "http:");
}

const requirePackage = createRequire(path.join(consumerRoot, "v2-authoring-parity.cjs"));
const resolvedRoot = fs.realpathSync(requirePackage.resolve("@type-bridge/node"));
const resolvedTyped = fs.realpathSync(requirePackage.resolve("@type-bridge/node/typed"));
const resolvedQueryV2 = fs.realpathSync(requirePackage.resolve("@type-bridge/node/query-v2"));
const installedRoot = fs.realpathSync(
  path.join(consumerRoot, "node_modules", "@type-bridge", "node"),
);

function isWithin(candidate, root) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

for (const [name, resolved] of [
  ["root", resolvedRoot],
  ["typed", resolvedTyped],
  ["query-v2", resolvedQueryV2],
]) {
  assert.ok(isWithin(resolved, installedRoot), `${name} import escaped packed install: ${resolved}`);
  assert.ok(!isWithin(resolved, sourcePackageRoot), `${name} import leaked to source: ${resolved}`);
}
assert.equal(fs.lstatSync(installedRoot).isSymbolicLink(), false);
assert.equal(fs.existsSync(path.join(installedRoot, "typescript")), false);
assert.equal(process.env.TYPE_BRIDGE_NODE_NATIVE_PATH, undefined);

const typeBridge = requirePackage("@type-bridge/node");
const typedBridge = requirePackage("@type-bridge/node/typed");
const queryV2 = requirePackage("@type-bridge/node/query-v2");
const {
  Card,
  Entity,
  Key,
  QueryV2Error,
  Relation,
  RustDatabase,
  TypeFlags,
  attr,
  field,
  queryV2ExecuteLocal,
  queryV2PrepareRemote,
  role,
} = typeBridge;
const { QueryPlanBuilder, QueryV2Authority } = queryV2;
const { QuerySession, RemoteQuerySession, references } = typedBridge;

const SCOPE = "model-remote-parity";
const PROFILE = "typedb-3.12.1/v1";
const ADVANCED_PLAN_FINGERPRINT =
  "85c9504dca956286b46336510af3b24980bba1a72e79465069b7a24e7d52e26f";
const DOCUMENT_REACHABILITY_PLAN_FINGERPRINT =
  "b253c1d8093ff648dd9617db871097e093b1d7dfca7961db26a5a5bfd939ed08";

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

function authorAdvancedPlan(authority) {
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

function authorDocumentReachabilityPlan(authority) {
  const builder = new QueryPlanBuilder(authority);
  const employee = builder.binding("employee");
  const employeeName = builder.binding("employee_name");
  const project = builder.binding("project");
  const projectName = builder.binding("project_name");
  builder.match([
    builder.isa(employee, "entity", "parity-person", true),
    builder.isa(project, "entity", "parity-project", false),
    builder.has(employee, employeeName, "parity-person-name"),
    builder.has(project, projectName, "parity-project-name"),
    builder.reachable(
      employee,
      project,
      "parity-assignment",
      "employee",
      "project",
      1,
      1,
    ),
  ]);
  builder.sort([
    builder.order(employeeName, "ascending"),
    builder.order(projectName, "ascending"),
  ]);
  const plan = builder.finalizeDocuments([
    builder.documentBinding("employee", employeeName),
    builder.documentBinding("project", projectName),
  ]);
  assert.equal(plan.fingerprint, DOCUMENT_REACHABILITY_PLAN_FINGERPRINT);
  assert.ok(plan.requiredCapabilities.includes("query.pattern.reachable"));
  assert.ok(plan.requiredCapabilities.includes("query.output.documents"));
  return { invocation: plan.documents([]), plan };
}

function normalizeQueryV2Error(error) {
  assert.ok(error instanceof QueryV2Error);
  return {
    category: error.category,
    code: error.code,
    message: error.diagnosticMessage,
    path: error.path,
    details: error.details,
  };
}

function requireSingular(value) {
  if (Array.isArray(value)) {
    throw new TypeError("exact-one parity role unexpectedly hydrated as a collection");
  }
  return value;
}

function normalize(rows) {
  return rows.map(([employee, project, assignment]) => ({
    assignment: assignment.assignmentId.value,
    concrete: employee.constructor.name,
    employee: employee.name.value,
    project: project.name.value,
    roleEmployee: requireSingular(assignment.employee).name.value,
    roleProject: requireSingular(assignment.project).name.value,
  }));
}

function modelQuery(session) {
  const employee = session.var(ParityPerson, "subtypes");
  const project = session.var(ParityProject);
  const assignment = session.var(ParityAssignment);
  const query = session
    .query(employee, project, assignment)
    .where(
      assignment
        .role(assignmentRefs.roles.employee)
        .connects(employee),
      assignment
        .role(assignmentRefs.roles.project)
        .connects(project),
    );
  return { employee, query };
}

async function postRemote(request) {
  const response = await fetch(`${serverUrl}/v2/query`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: new Uint8Array(request),
  });
  return Buffer.from(await response.arrayBuffer());
}

async function main() {
  const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
  const database = process.env.TYPE_BRIDGE_PARITY_DATABASE ?? "type_bridge_test";
  const username = process.env.TYPEDB_USERNAME ?? "admin";
  const password = process.env.TYPEDB_PASSWORD ?? "password";
  const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");
  const declaredFile = fs.readFileSync(declaredPath);
  const declared = declaredFile.at(-1) === 0x0a
    ? declaredFile.subarray(0, declaredFile.length - 1)
    : declaredFile;
  const connectionOptions = {
    username,
    password,
    httpPort,
    ...(typedbTlsEnabled
      ? { tlsEnabled: true, tlsRootCa: typedbTlsRootCa }
      : {}),
  };
  const db = RustDatabase.connect(address, database, connectionOptions);
  try {
    const advertisementResponse = await fetch(`${serverUrl}/v2/capabilities`);
    assert.equal(advertisementResponse.status, 200);
    const advertisement = Buffer.from(
      await advertisementResponse.arrayBuffer(),
    );

    const advancedAuthority = new QueryV2Authority(declared, SCOPE, PROFILE);
    const { invocation, plan } = authorAdvancedPlan(advancedAuthority);
    const {
      invocation: documentInvocation,
      plan: documentPlan,
    } = authorDocumentReachabilityPlan(advancedAuthority);
    const invocationJson = Buffer.from(invocation.canonicalBytes).toString("utf8");
    const documentInvocationJson = Buffer.from(
      documentInvocation.canonicalBytes,
    ).toString("utf8");
    const localAuthority = QueryV2Authority.queryOnly(
      db,
      declared,
      SCOPE,
      PROFILE,
    );
    const local = await queryV2ExecuteLocal(
      db,
      localAuthority,
      plan.canonicalBytes,
      invocationJson,
    );
    const documentLocal = await queryV2ExecuteLocal(
      db,
      localAuthority,
      documentPlan.canonicalBytes,
      documentInvocationJson,
    );
    let advancedExchanges = 0;
    const pending = queryV2PrepareRemote(
      advancedAuthority,
      plan.canonicalBytes,
      invocationJson,
      advertisement,
      {
        maxItems: 10n,
        maxBytes: 1n << 20n,
        maxCollectionMembers: 30n,
        deadlineMs: 30_000n,
      },
    );
    assert.equal(advancedExchanges, 0);
    const remote = await pending.decodeReply(
      await (async () => {
        advancedExchanges += 1;
        return postRemote(pending.requestBytes());
      })(),
    );
    assert.equal(advancedExchanges, 1);
    assert.equal(remote, local);

    let documentExchanges = 0;
    const documentPending = queryV2PrepareRemote(
      advancedAuthority,
      documentPlan.canonicalBytes,
      documentInvocationJson,
      advertisement,
      {
        maxItems: 10n,
        maxBytes: 1n << 20n,
        maxCollectionMembers: 30n,
        deadlineMs: 30_000n,
      },
    );
    assert.equal(documentExchanges, 0);
    const documentRemote = await documentPending.decodeReply(
      await (async () => {
        documentExchanges += 1;
        return postRemote(documentPending.requestBytes());
      })(),
    );
    assert.equal(documentExchanges, 1);
    assert.equal(documentRemote, documentLocal);

    let failureExchanges = 0;
    const failurePending = queryV2PrepareRemote(
      advancedAuthority,
      plan.canonicalBytes,
      invocationJson,
      advertisement,
      {
        maxItems: 1n,
        maxBytes: 1n << 20n,
        maxCollectionMembers: 30n,
        deadlineMs: 30_000n,
      },
    );
    assert.equal(failureExchanges, 0);
    let structuredFailure;
    try {
      await failurePending.decodeReply(
        await (async () => {
          failureExchanges += 1;
          return postRemote(failurePending.requestBytes());
        })(),
      );
      assert.fail("the one-item ceiling must reject the two-row provider answer");
    } catch (error) {
      structuredFailure = normalizeQueryV2Error(error);
    }
    assert.equal(failureExchanges, 1);

    const directSession = new QuerySession(db).registerModels(
      ParityEmployee,
      ParityProject,
      ParityAssignment,
    );
    const direct = modelQuery(directSession);
    const directRows = direct.query.rows({
      limit: 10,
      orderBy: [direct.employee.field(personRefs.fields.name).asc()],
    });
    assert.ok(directRows.every(([employee]) => employee instanceof ParityEmployee));

    let modelExchanges = 0;
    const remoteSession = new RemoteQuerySession(
      new QueryV2Authority(declared, SCOPE, PROFILE),
      advertisement,
      async (request) => {
        modelExchanges += 1;
        return postRemote(request);
      },
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
    const remoteQuery = modelQuery(remoteSession);
    assert.equal(modelExchanges, 0);
    const remoteRows = await remoteQuery.query.rows({
      limit: 10,
      orderBy: [remoteQuery.employee.field(personRefs.fields.name).asc()],
    });
    assert.equal(modelExchanges, 1);
    assert.ok(remoteRows.every(([employee]) => employee instanceof ParityEmployee));
    assert.deepEqual(normalize(remoteRows), normalize(directRows));

    process.stdout.write(JSON.stringify({
      advanced: {
        exchanges: advancedExchanges,
        fingerprint: plan.fingerprint,
        outcome: JSON.parse(local),
      },
      artifact: "packed-v2",
      lowLevel: {
        documentReachability: {
          exchanges: documentExchanges,
          fingerprint: documentPlan.fingerprint,
          outcome: JSON.parse(documentLocal),
        },
        structuredFailure: {
          diagnostic: structuredFailure,
          exchanges: failureExchanges,
        },
      },
      model: {
        direct: normalize(directRows),
        exchanges: modelExchanges,
        remote: normalize(remoteRows),
      },
    }));
  } finally {
    db.close();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
