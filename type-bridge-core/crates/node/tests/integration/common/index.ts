/** Shared connection and raw-TypeQL helpers for retained and generated tests. */

import { createRequire } from "node:module";

const packageRoot = process.env.TYPE_BRIDGE_NODE_PACKAGE_ROOT ?? process.cwd();
const requirePackage = createRequire(import.meta.url);
const packageFacade = requirePackage(packageRoot) as typeof import(
  "../../../typescript/public.js"
);

export const { QueryV2Authority, RustDatabase, ensureDatabase } = packageFacade;

export const TYPEDB_ADDRESS =
  process.env.TYPEDB_ADDRESS ?? "localhost:1730";
export const INTG_DATABASE =
  process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
export const TYPEDB_USERNAME = process.env.TYPEDB_USERNAME ?? "admin";
export const TYPEDB_PASSWORD = process.env.TYPEDB_PASSWORD ?? "password";
export const TYPEDB_HTTP_PORT = Number(
  process.env.TYPEDB_HTTP_PORT ?? "8000",
);
const TYPEDB_TLS_ROOT_CA = process.env.TYPEDB_TLS_ROOT_CA;

function connectionOptions() {
  return {
    username: TYPEDB_USERNAME,
    password: TYPEDB_PASSWORD,
    httpPort: TYPEDB_HTTP_PORT,
    ...(TYPEDB_TLS_ROOT_CA === undefined
      ? {}
      : { tlsEnabled: true, tlsRootCa: TYPEDB_TLS_ROOT_CA }),
  };
}

export function connectIntegration() {
  const options = connectionOptions();
  ensureDatabase(TYPEDB_ADDRESS, INTG_DATABASE, options);
  return RustDatabase.connect(TYPEDB_ADDRESS, INTG_DATABASE, options);
}

export function defineSchema(
  database: ReturnType<typeof connectIntegration>,
  typeql: string,
): void {
  const transaction = database.transaction("schema");
  try {
    transaction.query(typeql);
    transaction.commit();
  } catch (error) {
    transaction.close();
    throw error;
  }
}
