import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import type {} from "../../typescript/index.js";

type RuntimePackage = typeof import("../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-integration.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";

describe("connect honors an explicit httpPort", () => {
  test("ensureDatabase and RustDatabase.connect accept explicit httpPort 8000", () => {
    // Pass the default HTTP port explicitly — proves the plumbed path is live
    // without requiring a remapped server.  If the port forwarding is broken,
    // the version probe will fail before any gRPC connection is attempted.
    typeBridge.ensureDatabase(address, database, { username, password, httpPort: 8000 });
    const db = typeBridge.RustDatabase.connect(address, database, {
      username,
      password,
      httpPort: 8000,
    });
    assert.ok(db.isConnected(), "connection with explicit httpPort=8000 must succeed");
  });

  test("an out-of-range httpPort is rejected at the binding boundary", () => {
    // u16 validation happens before any probe I/O, so the failure is
    // synchronous and names the invalid argument instead of timing out.
    assert.throws(
      () => typeBridge.ensureDatabase(address, database, { username, password, httpPort: 70000 }),
      /port|invalid/i,
    );
  });
});
