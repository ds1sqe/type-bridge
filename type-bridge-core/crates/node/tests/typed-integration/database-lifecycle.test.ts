import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import type {} from "../../typescript/index.js";

type RuntimePackage = typeof import("../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-integration.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = `type_bridge_node_lifecycle_${process.pid}_${Date.now()}`;
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

describe("bound RustDatabase lifecycle methods", () => {
  test("create, delete, and reset operate on the bound database name", () => {
    const db = typeBridge.RustDatabase.connect(address, database, {
      username,
      password,
      httpPort,
    });

    try {
      assert.equal(db.databaseName(), database);

      if (db.databaseExists()) {
        db.deleteDatabase();
      }
      assert.equal(db.databaseExists(), false);

      db.createDatabase();
      assert.equal(db.databaseExists(), true);

      db.createDatabase();
      assert.equal(db.databaseExists(), true);

      db.deleteDatabase();
      assert.equal(db.databaseExists(), false);

      db.deleteDatabase();
      assert.equal(db.databaseExists(), false);

      db.resetDatabase();
      assert.equal(db.databaseExists(), true);

      db.resetDatabase();
      assert.equal(db.databaseExists(), true);
    } finally {
      try {
        db.deleteDatabase();
      } catch {
        // Test cleanup should not mask the assertion that failed first.
      }
    }
  });
});
