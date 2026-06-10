/**
 * Transaction commit durability and rollback isolation integration.
 *
 * Two critical properties are asserted:
 *   1. Commit durability — a committed write is visible in a subsequent
 *      database-level read on a different manager.
 *   2. Rollback isolation — an uncommitted write is invisible after rollback;
 *      the database-level manager must not see the rolled-back entity.
 *
 * Mirrors node_entity_transaction_commit_and_rollback_against_typedb.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";

import {
  connectIntegration,
  defineSchema,
  newCrudSchema,
  crudSchemaTypeql,
  personDescriptor,
  string,
  long,
} from "../common/index.ts";

const db = connectIntegration();

describe("transaction commit durability and rollback isolation", () => {
  const s = newCrudSchema("tx");
  defineSchema(db, crudSchemaTypeql(s));

  // Database-level manager used for post-transaction visibility checks.
  const dbManager = db.entityManager(personDescriptor(s));

  test("rollback isolation: rolled-back write is invisible to database manager", () => {
    const rollbackTx = db.transaction("write");
    const txManager = rollbackTx.entityManager(personDescriptor(s));

    txManager.insert({ name: string("Rollback"), age: long(20n) });

    // Within the transaction the write is visible.
    const withinTx = txManager.count({ name: string("Rollback") });
    assert.equal(withinTx, 1n, "write should be visible within its own transaction");

    rollbackTx.rollback();

    // After rollback the database-level manager must not see the entity.
    const afterRollback = dbManager.get({ name: string("Rollback") });
    assert.equal(
      afterRollback.length,
      0,
      "rolled-back write must be invisible to the database manager",
    );
  });

  test("commit durability: committed write is visible to database manager", () => {
    const commitTx = db.transaction("write");
    const txManager = commitTx.entityManager(personDescriptor(s));

    txManager.insert({ name: string("Commit"), age: long(21n) });
    commitTx.commit();

    // After commit the database-level manager must see the entity.
    const afterCommit = dbManager.get({ name: string("Commit") });
    assert.equal(
      afterCommit.length,
      1,
      "committed write must be visible to the database manager",
    );
  });

  test("transaction type is reported correctly", () => {
    const readTx = db.transaction("read");
    assert.equal(readTx.transactionType(), "read");
    readTx.close();

    const writeTx = db.transaction("write");
    assert.equal(writeTx.transactionType(), "write");
    writeTx.rollback();
  });
});
