# type-bridge-server

Transport-agnostic query pipeline for TypeDB with composable interceptors.

## Overview

`type-bridge-server` is both a library crate and a standalone binary that
provides a structured query pipeline for TypeDB:

```text
validate → intercept → compile → execute → intercept
```

The pipeline receives structured queries (parsed AST clauses), validates them
against a loaded TypeQL schema, runs request interceptors, compiles to TypeQL,
executes against TypeDB, then runs response interceptors.

## Quick start

### As a standalone server

```bash
cargo install type-bridge-server --version 2.1.0 --locked
type-bridge-server --config config.toml
```

The default standalone build includes both provider bands and the public
`v2-query` feature. The complete configuration below uses `[v2].enabled` to add
the V2 routes; retained V1 routes remain available in either state.

### As a library

```toml
[dependencies]
type-bridge-server = "2.1.0"
```

This sketch is ignored because the executor, interceptor, input, and schema
values are application-defined extension points.

```rust,ignore
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::schema_source::InMemorySchemaSource;

let pipeline = PipelineBuilder::new(my_executor)
    .with_schema_source(InMemorySchemaSource::new(schema_tql))
    .with_default_database("my_db")
    .with_interceptor(my_audit_log)
    .build()?;

let output = pipeline.execute_query(input).await?;
```

## Architecture

```text
                    +-----------+
                    | Transport |   (Axum HTTP, or custom)
                    +-----+-----+
                          |
                    +-----v-----+
                    |  Pipeline  |
                    +-----+-----+
                          |
          +-------+-------+-------+-------+
          |       |       |       |       |
      Validate  Intercept Compile Execute Intercept
      (schema)  (request)  (AST→  (TypeDB) (response)
                           TypeQL)
```

**Components:**

| Component | Trait | Built-in |
|-----------|-------|----------|
| Executor | `QueryExecutor` | `TypeDBClient` (feature: `typedb`) |
| Interceptor | `Interceptor` | `AuditLogInterceptor` |
| Schema source | `SchemaSource` | `FileSchemaSource`, `InMemorySchemaSource` |
| Transport | N/A | Axum HTTP (feature: `axum-transport`) |

## Generate V2 authority

The server is a generic executor, not a schema compiler. Author only Split YAML
and its `typebridge.yaml` workspace, then configure generation:

```yaml
bindings:
  python:
    output: generated/python/app_models
  typescript:
    output: generated/typescript
  rust:
    output: generated/rust

artifacts:
  schema-authority:
    output: generated/schema-authority.json
```

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

One captured workspace produces all configured packages and the server
artifact. Python and TypeScript packages embed the authority for their normal
`RemoteQuerySession`; they never read the standalone artifact or construct a
low-level `QueryV2Authority`. Mount `generated/schema-authority.json` for this
generic server.

The artifact uses the versioned `typebridge.schema-authority/v1` canonical JSON
codec. JSON is bounded, deterministic, source-free deployment evidence—not a
user-maintained schema input. Never edit it or generate it independently from
the packages.

## Configuration

The server reads a TOML config file:

```toml
[server]
host = "0.0.0.0"     # default
port = 8080           # default

# Optional HTTPS listener identity. Relative paths resolve from this file.
[server.tls]
cert-path = "certs/server.pem"
key-path = "certs/server.key"

[typedb]
address = "localhost:1729"
database = "my_database"
username = "admin"
password = "password"
http_port = 8000              # default; used for connect-time version probing
server_version = "3.12.1"     # optional; skips HTTP probing for gRPC-only TypeDB
tls = true
tls-root-ca = "certs/root.pem" # optional; omit for native trust roots

[schema]
# Optional retained V1 validation source; not V2 schema authority.
source_file = "schema.tql"

[interceptors]
enabled = ["audit-log"]

[interceptors.audit-log]
output = "file"                 # "stdout" or "file"
file_path = "/var/log/audit.jsonl"

[logging]
level = "info"        # default
format = "json"       # default; "text" is also supported

# Additive prepared V2 routes; the default build includes v2-query.
[v2]
enabled = true
schema_authority_file = "schema-authority.json"
authority_mode = "managed" # default; or the explicit "query_only"
```

The complete example above is kept as a runtime-parser fixture at
[`tests/fixtures/runtime-server.toml`](tests/fixtures/runtime-server.toml).
Set `typedb.tls = true` without `tls-root-ca` to use native trust roots. The
server validates configured trust and identity files before constructing a
TypeDB client or binding its listener. The configuration itself must be a
regular-file target no larger than 1 MiB; special targets and oversized input
are rejected before parsing.

The V2 surface fails startup unless `schema_authority_file` is canonical and
constructor-verified, its embedded semantic profile matches the connected
server, and the selected live authority is valid. `managed` requires the
complete V2 migration-control schema and its free singleton for the embedded
scope. `query_only` requires both V2 and legacy migration controls to be absent.
It is not an automatic fallback. Scope and profile are intentionally absent
from server configuration because the generated artifact already binds them.

A relative `schema_authority_file` is resolved against the configuration-file
directory. Every path component and the final target must be free of symbolic
links; the target must be a non-empty regular file within the canonical
schema-authority size ceiling. The configuration loader reads and compares the
file twice, retains the verified bytes as an immutable snapshot, and does not
reopen it while serving requests. After replacing the file, reload the complete
configuration; changing the public path field on a loaded value is rejected.

Prepared query execution captures the exact schema export under a bounded
TypeDB schema-exclusion fence. On TypeDB 3.12.1 that fence uses a `WRITE`
transaction even though emitted V2 TypeQL is read-only, so the server
credential needs that transaction permission and an executing request can
delay concurrent schema work. The default absolute request deadline is 30
seconds and the hard maximum is five minutes.

## HTTP API

The V1 JSON `POST` endpoints below require
`Content-Type: application/json`. `GET` routes do not.

### `POST /query` — execute structured query

```json
{
  "database": "my_db",
  "transaction_type": "read",
  "clauses": [{ "match": [{ "entity": { "variable": "p", "type_name": "person" }}] }]
}
```

### `POST /query/raw` — execute raw TypeQL

```json
{
  "database": "my_db",
  "transaction_type": "read",
  "query": "match $p isa person;"
}
```

### `POST /query/validate` — validate without executing

```json
{
  "clauses": [{ "match": [{ "entity": { "variable": "p", "type_name": "person" }}] }]
}
```

### `GET /health` — health check

Returns the stable V1 object
`{"status":"ok","version":"1.5.11","typedb_connected":true}`. The version field
is the frozen V1 HTTP identity; use `type-bridge-server --version` for the
installed package version.

### `GET /schema` — loaded schema

Returns the loaded TypeQL schema as JSON, or `500` if no schema is loaded.

### `GET /v2/capabilities` — prepared executor advertisement

When V2 is enabled, returns the canonical capability advertisement after the
configured transport policy and a bounded live schema/profile check. Discovery
does not open a query transaction or acquire the migration-control singleton;
exact fenced admission happens at startup and for the request that executes a
plan. The advertisement carries the executor epoch and reply-signing identity,
so clients must obtain it over authenticated TLS for the intended server or
pin/provision its exact bytes or fingerprint out of band. Plain-HTTP discovery
does not authenticate that trust input.

### `POST /v2/query` — execute a prepared V2 envelope

Accepts the canonical request bytes produced by the prepared bindings. Replay,
executor identity, nonce, plan fingerprint, expiry, capability, and byte-budget
checks run before provider transaction construction. Successes and failures use
the versioned canonical remote envelope; callers should decode them through the
one-shot request handle that created the request.

## Custom interceptors

Implement the `Interceptor` trait to add cross-cutting concerns. Because the
released trait names the shared AST, an application implementing it must also
declare `type-bridge-core-lib = "2.1.0"` directly:

```rust
use type_bridge_server::interceptor::{Interceptor, InterceptError, RequestContext};
use type_bridge_core_lib::ast::Clause;
use std::pin::Pin;
use std::future::Future;

struct RateLimiter { /* ... */ }

impl Interceptor for RateLimiter {
    fn name(&self) -> &str { "rate-limiter" }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            // Check rate limit, reject or pass through
            Ok(clauses)
        })
    }
}
```

Register via `PipelineBuilder::with_interceptor()`.

## Custom executors

Implement `QueryExecutor` for non-TypeDB backends or mocking. The example uses
`serde_json`, so the application must also declare `serde_json = "1"`
directly:

```rust
use type_bridge_server::executor::QueryExecutor;
use type_bridge_server::error::PipelineError;
use std::pin::Pin;
use std::future::Future;

struct MyBackend;

impl QueryExecutor for MyBackend {
    fn execute<'a>(
        &'a self,
        database: &'a str,
        typeql: &'a str,
        transaction_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(serde_json::json!({"ok": true}))
        })
    }
    fn is_connected(&self) -> bool { true }
}
```

## Feature flags

| Feature | Default | Effect |
|---------|---------|--------|
| `typedb` | yes | Enables `TypeDBClient` through the shared TypeDB runtime |
| `band8` | yes | Enables the TypeDB 3.11 provider band |
| `band9` | yes | Enables the TypeDB 3.12 provider band |
| `axum-transport` | yes, through `v2-query` | Enables HTTP server with Axum |
| `v2-query` | yes | Adds `/v2/capabilities` and `/v2/query`; implies `axum-transport` |

The standalone binary requires `typedb` and `v2-query`; the default feature set
satisfies both. Build as a bare library (no transport, no TypeDB) with:

```bash
cargo check -p type-bridge-server --no-default-features
```

## Testing

```bash
# Unit tests (no external dependencies)
cargo test -p type-bridge-server

# MC/DC coverage (requires nightly + cargo-llvm-cov)
./scripts/coverage.sh mcdc --open
```

The crate is released in lockstep with TypeBridge 2.1.0, requires Rust 1.88+,
and supports the retained TypeDB 3.11.x and 3.12.x provider bands.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-server/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
