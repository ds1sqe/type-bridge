# type-bridge-server

Transport-agnostic query pipeline for TypeDB with composable interceptors.

## Overview

`type-bridge-server` is both a library crate and a standalone binary that
provides a structured query pipeline for TypeDB:

```
validate → intercept → compile → execute → intercept
```

The pipeline receives structured queries (parsed AST clauses), validates them
against a loaded TypeQL schema, runs request interceptors, compiles to TypeQL,
executes against TypeDB, then runs response interceptors.

## Quick start

### As a standalone server

```bash
cargo run -p type-bridge-server -- --config config.toml
```

### As a library

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

```
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

## Configuration

The server reads a TOML config file:

```toml
[typedb]
address = "localhost:1729"
database = "my_database"
username = "admin"
password = "password"

[server]
host = "0.0.0.0"     # default
port = 3000           # default

[schema]
file = "schema.tql"   # optional: path to TypeQL schema

[interceptors]
enabled = ["audit-log"]

[audit_log]
output = "file"                 # "stdout" or "file"
file_path = "/var/log/audit.jsonl"

[logging]
level = "info"        # default
format = "text"       # "text" or "json"
```

## HTTP API

All endpoints require `Content-Type: application/json`.

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

Returns `{"status": "ok", "connected": true}`.

### `GET /schema` — loaded schema

Returns the loaded TypeQL schema as JSON, or `500` if no schema is loaded.

## Custom interceptors

Implement the `Interceptor` trait to add cross-cutting concerns:

```rust,ignore
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

Implement `QueryExecutor` for non-TypeDB backends or mocking:

```rust,ignore
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
| `typedb` | yes | Enables `TypeDBClient` and `typedb-driver` dependency |
| `axum-transport` | yes | Enables HTTP server with Axum |

Build as bare library (no transport, no TypeDB):

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
