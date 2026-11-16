import assert from "node:assert/strict";
import test from "node:test";
import { loadNative } from "../../typescript/native.js";

test("musl is refused explicitly and installed packages do not promise a source build", (context) => {
  const platform = Object.getOwnPropertyDescriptor(process, "platform")!;
  const arch = Object.getOwnPropertyDescriptor(process, "arch")!;
  const report = Object.getOwnPropertyDescriptor(process, "report")!;
  const explicit = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  context.after(() => {
    Object.defineProperty(process, "platform", platform);
    Object.defineProperty(process, "arch", arch);
    Object.defineProperty(process, "report", report);
    if (explicit === undefined) delete process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
    else process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = explicit;
  });
  delete process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  Object.defineProperty(process, "platform", { value: "linux", configurable: true });
  Object.defineProperty(process, "arch", { value: "x64", configurable: true });
  Object.defineProperty(process, "report", { value: { getReport: () => ({ header: {} }) }, configurable: true });
  assert.throws(loadNative, (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.match(error.message, /Unsupported.*Linux requires glibc; musl is not shipped/);
    assert.doesNotMatch(error.message, /npm run build:native/);
    assert.match(error.message, /TypeBridge repository/);
    return true;
  });
});
