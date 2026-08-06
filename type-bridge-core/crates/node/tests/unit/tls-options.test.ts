import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { RustDatabase, ensureDatabase } from "../../typescript/public.js";

type ConnectOptions = Parameters<typeof RustDatabase.connect>[2];

describe("Node connection transport boundary", () => {
  test("rejects custom-root contradictions before attempting a connection", () => {
    for (const connect of [
      (options: ConnectOptions) =>
        RustDatabase.connect("localhost:1", "unreachable", options),
      (options: ConnectOptions) =>
        ensureDatabase("localhost:1", "unreachable", options),
    ]) {
      assert.throws(
        () => connect({ tlsRootCa: "root.pem" }),
        /requires explicit tlsEnabled=true/i,
      );
      assert.throws(
        () => connect({ tlsEnabled: false, tlsRootCa: "root.pem" }),
        /contradicts explicit tlsEnabled=false/i,
      );
      assert.throws(
        () => connect({ tlsEnabled: true, tlsRootCa: "" }),
        /must not be empty/i,
      );
    }
  });

  test("rejects JavaScript type confusion before attempting a connection", () => {
    assert.throws(
      () =>
        RustDatabase.connect("localhost:1", "unreachable", {
          tlsEnabled: "true" as unknown as boolean,
        }),
      /tlsEnabled must be a boolean/i,
    );
    assert.throws(
      () =>
        RustDatabase.connect("localhost:1", "unreachable", {
          tlsEnabled: true,
          tlsRootCa: 1 as unknown as string,
        }),
      /tlsRootCa must be a string/i,
    );
    assert.throws(
      () =>
        ensureDatabase("localhost:1", "unreachable", {
          tlsEnabled: null as unknown as boolean,
        }),
      /tlsEnabled must be a boolean/i,
    );
  });

  test("rejects own and inherited TLS-like option typos", () => {
    assert.throws(
      () =>
        RustDatabase.connect("localhost:1", "unreachable", {
          tlsEnable: true,
        } as unknown as ConnectOptions),
      /unknown TLS connection option "tlsEnable"/i,
    );

    const inheritedTypo = Object.create({ tls_enabled: true }) as ConnectOptions;
    assert.throws(
      () => RustDatabase.connect("localhost:1", "unreachable", inheritedTypo),
      /unknown TLS connection option "tls_enabled"/i,
    );
  });
});
