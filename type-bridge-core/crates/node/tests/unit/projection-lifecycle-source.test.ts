import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

const sourcePath = path.resolve(
  process.cwd(),
  "tests/projection-integration/generated-package-live.test.ts",
);

test("remote close I/O is measured before later usable-lineage terminals", () => {
  const source = fs.readFileSync(sourcePath, "utf8");
  const start = source.indexOf("async function workforceV2Lifecycle(");
  const end = source.indexOf("async function runWorkforceV2Journey(", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const lifecycle = source.slice(start, end);

  const snapshot = lifecycle.indexOf(
    "const requestsBeforeRejection = requests.length;",
  );
  const rejectedTerminal = lifecycle.indexOf(
    "await remoteAncestor.one();",
    snapshot,
  );
  const measured = lifecycle.indexOf(
    "const postCloseIoCount = requests.length - requestsBeforeRejection;",
    rejectedTerminal,
  );
  const descendantTerminal = lifecycle.indexOf(
    "workforceV2Key(await remoteDescendant.one())",
    measured,
  );
  assert.notEqual(snapshot, -1);
  assert.notEqual(rejectedTerminal, -1);
  assert.notEqual(measured, -1);
  assert.notEqual(descendantTerminal, -1);
  assert.ok(snapshot < rejectedTerminal);
  assert.ok(rejectedTerminal < measured);
  assert.ok(measured < descendantTerminal);
  assert.match(lifecycle, /post_close_io_count: postCloseIoCount,/);
  assert.doesNotMatch(
    lifecycle,
    /post_close_io_count: requests\.length - requestsBeforeRejection,/,
  );
});
