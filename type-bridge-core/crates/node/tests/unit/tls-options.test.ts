import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { describe, test } from "node:test";

import {
  RustDatabase,
  TYPE_DB_SERVER_DEPRECATION_CODE,
  TYPE_DB_SERVER_DEPRECATION_WARNING,
  loadNative,
  type NativeRuntime,
  type NativeRustDatabase,
} from "../../typescript/index.js";

function recordingRuntime(calls: unknown[][]): NativeRuntime {
  return {
    ensureRustDatabase(...args: unknown[]): void {
      calls.push(args);
    },
    connectRustDatabase(...args: unknown[]): NativeRustDatabase {
      calls.push(args);
      return {} as NativeRustDatabase;
    },
  };
}

describe("Node TLS connection options", () => {
  test("emits one filterable legacy-server warning per successful connection", () => {
    const emitted: Array<{ message: string; type?: string; code?: string }> = [];
    const original = process.emitWarning;
    process.emitWarning = ((
      warning: string | Error,
      options?: string | { type?: string; code?: string },
    ): void => {
      const structured = typeof options === "object" ? options : {};
      emitted.push({
        message: typeof warning === "string" ? warning : warning.message,
        ...structured,
      });
    }) as typeof process.emitWarning;
    try {
      const database = {
        serverDeprecationNotice: () => ({
          code: TYPE_DB_SERVER_DEPRECATION_CODE,
          message: "shared legacy notice",
        }),
      } as NativeRustDatabase;
      const native = {
        ensureRustDatabase(): void {},
        connectRustDatabase(): NativeRustDatabase {
          return database;
        },
      } as NativeRuntime;

      RustDatabase.connect(native, "localhost:1729", "legacy");

      assert.deepEqual(emitted, [{
        message: "shared legacy notice",
        type: TYPE_DB_SERVER_DEPRECATION_WARNING,
        code: TYPE_DB_SERVER_DEPRECATION_CODE,
      }]);
    } finally {
      process.emitWarning = original;
    }
  });

  test("uses the standard warning type suppressed by --no-deprecation", () => {
    assert.equal(TYPE_DB_SERVER_DEPRECATION_WARNING, "DeprecationWarning");
    assert.equal(TYPE_DB_SERVER_DEPRECATION_CODE, "TYPE_BRIDGE_TYPEDB_LEGACY_SERVER");
    assert.equal(
      TYPE_DB_SERVER_DEPRECATION_CODE,
      loadNative().TYPE_DB_SERVER_DEPRECATION_CODE,
      "the public filter constant must equal the real Rust addon export",
    );
    const probe = spawnSync(
      process.execPath,
      [
        "--no-deprecation",
        "-e",
        `process.emitWarning("legacy", { type: ${JSON.stringify(
          TYPE_DB_SERVER_DEPRECATION_WARNING,
        )}, code: ${JSON.stringify(TYPE_DB_SERVER_DEPRECATION_CODE)} })`,
      ],
      { encoding: "utf8" },
    );
    assert.equal(probe.status, 0, probe.stderr);
    assert.equal(probe.stderr, "");
  });

  test("does not warn for unknown band-8/9 or known 3.12-over-band-8 classifications", () => {
    let emitted = 0;
    const original = process.emitWarning;
    process.emitWarning = (() => {
      emitted += 1;
    }) as typeof process.emitWarning;
    try {
      for (const classification of [
        "unknown-band8",
        "unknown-band9",
        "known-3.12-over-band8",
      ]) {
        const database = {
          serverDeprecationNotice: () => null,
        } as NativeRustDatabase;
        const native = {
          ensureRustDatabase(): void {},
          connectRustDatabase(): NativeRustDatabase {
            return database;
          },
        } as NativeRuntime;

        RustDatabase.connect(native, "localhost:1729", classification);
      }
      assert.equal(emitted, 0);
    } finally {
      process.emitWarning = original;
    }
  });

  test("preserves a successful connection when a synchronous emitWarning replacement throws", () => {
    const warningFailure = new Error("warning delivery failed");
    let closeCalls = 0;
    const original = process.emitWarning;
    process.emitWarning = (() => {
      throw warningFailure;
    }) as typeof process.emitWarning;
    try {
      const nativeDatabase = {
        serverDeprecationNotice: () => ({
          code: TYPE_DB_SERVER_DEPRECATION_CODE,
          message: "shared legacy notice",
        }),
        close(): void {
          closeCalls += 1;
        },
      } as NativeRustDatabase;
      const native = {
        ensureRustDatabase(): void {},
        connectRustDatabase(): NativeRustDatabase {
          return nativeDatabase;
        },
      } as NativeRuntime;

      const database = RustDatabase.connect(native, "localhost:1729", "legacy");
      assert.equal(closeCalls, 0);
      database.close();
      assert.equal(closeCalls, 1);
    } finally {
      process.emitWarning = original;
    }
  });

  test("strict deprecation policy cannot abort a successful connection", () => {
    let emitted = 0;
    const originalEmitWarning = process.emitWarning;
    const originalThrowDeprecation = process.throwDeprecation;
    process.emitWarning = (() => {
      emitted += 1;
    }) as typeof process.emitWarning;
    process.throwDeprecation = true;
    try {
      const nativeDatabase = {
        serverDeprecationNotice: () => ({
          code: TYPE_DB_SERVER_DEPRECATION_CODE,
          message: "shared legacy notice",
        }),
      } as NativeRustDatabase;
      const native = {
        ensureRustDatabase(): void {},
        connectRustDatabase(): NativeRustDatabase {
          return nativeDatabase;
        },
      } as NativeRuntime;

      assert.ok(RustDatabase.connect(native, "localhost:1729", "legacy"));
      assert.equal(emitted, 0);
    } finally {
      process.throwDeprecation = originalThrowDeprecation;
      process.emitWarning = originalEmitWarning;
    }
  });

  test("RustDatabase.close delegates to the native handle", () => {
    let closeCalls = 0;
    const nativeDatabase = {
      close(): void {
        closeCalls += 1;
      },
    } as NativeRustDatabase;
    const native = {
      ensureRustDatabase(): void {},
      connectRustDatabase(): NativeRustDatabase {
        return nativeDatabase;
      },
    } as NativeRuntime;
    const database = RustDatabase.connect(native, "localhost:1729", "close-test");

    database.close();
    database.close();

    assert.equal(closeCalls, 2);
  });

  test("forwards explicit native-root and custom-root modes", () => {
    const calls: unknown[][] = [];
    const native = recordingRuntime(calls);

    RustDatabase.connect(native, "localhost:1729", "plain");
    RustDatabase.connect(native, "localhost:1729", "explicit-plain", {
      tlsEnabled: false,
    });
    RustDatabase.connect(native, "localhost:1729", "native", {
      tlsEnabled: true,
    });
    RustDatabase.connect(native, "localhost:1729", "custom", {
      tlsEnabled: true,
      tlsRootCa: "root.pem",
    });

    assert.deepEqual(calls.map((call) => call.slice(-2)), [
      [null, null],
      [false, null],
      [true, null],
      [true, "root.pem"],
    ]);
  });

  test("forwards released schemes but rejects root contradictions before native code", () => {
    const calls: unknown[][] = [];
    const native = recordingRuntime(calls);

    RustDatabase.connect(native, "http://localhost:1729", "released-plain");
    RustDatabase.connect(native, "https://localhost:1729", "secure", {
      tlsEnabled: true,
    });
    assert.throws(
      () => RustDatabase.connect(native, "localhost:1729", "db", { tlsRootCa: "root.pem" }),
      /requires explicit tlsEnabled=true/i,
    );
    assert.throws(
      () =>
        RustDatabase.connect(native, "localhost:1729", "db", {
          tlsEnabled: false,
          tlsRootCa: "root.pem",
      }),
      /contradicts explicit tlsEnabled=false/i,
    );
    assert.deepEqual(
      calls.map((call) => [call[0], call.at(-2)]),
      [
        ["http://localhost:1729", null],
        ["https://localhost:1729", true],
      ],
    );
  });

  test("rejects JavaScript type confusion before invoking native code", () => {
    const calls: unknown[][] = [];
    const native = recordingRuntime(calls);

    assert.throws(
      () =>
        RustDatabase.connect(native, "localhost:1729", "db", {
          tlsEnabled: "true" as unknown as boolean,
        }),
      /tlsEnabled must be a boolean/i,
    );
    assert.throws(
      () =>
        RustDatabase.connect(native, "localhost:1729", "db", {
          tlsEnabled: true,
          tlsRootCa: 1 as unknown as string,
        }),
      /tlsRootCa must be a string/i,
    );
    assert.throws(
      () =>
        RustDatabase.connect(native, "localhost:1729", "db", {
          tlsEnabled: null as unknown as boolean,
        }),
      /tlsEnabled must be a boolean/i,
    );
    assert.deepEqual(calls, []);
  });

  test("rejects TLS-like option typos without closing unrelated metadata", () => {
    const calls: unknown[][] = [];
    const native = recordingRuntime(calls);

    assert.throws(
      () =>
        RustDatabase.connect(native, "localhost:1729", "db", {
          tlsEnable: true,
        } as unknown as Parameters<typeof RustDatabase.connect>[3]),
      /unknown TLS connection option "tlsEnable"/i,
    );

    const inheritedTypo = Object.create({ tls_enabled: true }) as Parameters<
      typeof RustDatabase.connect
    >[3];
    assert.throws(
      () => RustDatabase.connect(native, "localhost:1729", "db", inheritedTypo),
      /unknown TLS connection option "tls_enabled"/i,
    );

    RustDatabase.connect(native, "localhost:1729", "metadata-compatible", {
      requestTag: "retained",
    } as unknown as Parameters<typeof RustDatabase.connect>[3]);
    assert.equal(calls.length, 1);
  });
});
