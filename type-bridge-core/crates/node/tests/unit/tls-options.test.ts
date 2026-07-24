import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  RustDatabase,
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
