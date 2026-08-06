"use strict";

/**
 * Unit tests for the generated-only .d.ts parity gate's compare logic.
 *
 * These exercise `compareDeclarations` directly with in-memory file maps — no
 * tsc emit — to prove the gate is a real diff gate, not a no-op:
 *   - identical emit/baseline → pass
 *   - unexplained drift → fail (the negative acceptance criterion)
 *   - drift recorded in allowed-diffs (matching sha256) → pass with rationale
 *   - added / removed declaration files → fail unless allowed
 */

const assert = require("node:assert/strict");
const { describe, test } = require("node:test");

const { compareDeclarations, sha256 } = require("./dts-parity.cjs");

describe("dts-parity compareDeclarations", () => {
  test("identical emit and baseline pass with no violations", () => {
    const content = "export declare const x: number;\n";
    const emit = new Map([["index.d.ts", content]]);
    const baseline = new Map([["index.d.ts", content]]);

    const result = compareDeclarations(emit, baseline, []);
    assert.equal(result.ok, true);
    assert.equal(result.violations.length, 0);
  });

  test("unexplained content drift fails", () => {
    const emit = new Map([["index.d.ts", "export declare const x: string;\n"]]);
    const baseline = new Map([["index.d.ts", "export declare const x: number;\n"]]);

    const result = compareDeclarations(emit, baseline, []);
    assert.equal(result.ok, false);
    assert.equal(result.violations.length, 1);
    assert.equal(result.violations[0].file, "index.d.ts");
    assert.equal(result.violations[0].reason, "content drift vs baseline");
  });

  test("drift recorded in allowed-diffs (matching sha) passes with rationale", () => {
    const drifted = "export declare const x: string;\n";
    const emit = new Map([["index.d.ts", drifted]]);
    const baseline = new Map([["index.d.ts", "export declare const x: number;\n"]]);
    const allowed = [
      { file: "index.d.ts", sha256: sha256(drifted), rationale: "intentional widening" },
    ];

    const result = compareDeclarations(emit, baseline, allowed);
    assert.equal(result.ok, true);
    assert.equal(result.allowed.length, 1);
    assert.equal(result.allowed[0].rationale, "intentional widening");
  });

  test("allowed-diffs entry with a stale sha does NOT mask new drift", () => {
    const emit = new Map([["index.d.ts", "export declare const x: boolean;\n"]]);
    const baseline = new Map([["index.d.ts", "export declare const x: number;\n"]]);
    // Allowance recorded for a DIFFERENT (older) drifted content.
    const allowed = [
      { file: "index.d.ts", sha256: sha256("export declare const x: string;\n"), rationale: "old" },
    ];

    const result = compareDeclarations(emit, baseline, allowed);
    assert.equal(result.ok, false);
    assert.equal(result.violations.length, 1);
  });

  test("an added declaration file fails unless allowed", () => {
    const emit = new Map([
      ["index.d.ts", "a\n"],
      ["extra.d.ts", "b\n"],
    ]);
    const baseline = new Map([["index.d.ts", "a\n"]]);

    const failing = compareDeclarations(emit, baseline, []);
    assert.equal(failing.ok, false);
    assert.equal(failing.violations[0].file, "extra.d.ts");
    assert.equal(failing.violations[0].reason, "added (not in baseline)");

    const allowed = [{ file: "extra.d.ts", sha256: sha256("b\n"), rationale: "new module" }];
    const passing = compareDeclarations(emit, baseline, allowed);
    assert.equal(passing.ok, true);
  });

  test("a removed declaration file fails", () => {
    const emit = new Map([["index.d.ts", "a\n"]]);
    const baseline = new Map([
      ["index.d.ts", "a\n"],
      ["gone.d.ts", "c\n"],
    ]);

    const result = compareDeclarations(emit, baseline, []);
    assert.equal(result.ok, false);
    assert.equal(result.violations[0].file, "gone.d.ts");
    assert.equal(result.violations[0].reason, "removed (present in baseline, not emitted)");
  });
});
