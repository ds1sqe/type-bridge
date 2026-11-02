# type-bridge-server

TypeDB staged transactions with composable host policy hooks.

The default build serves `/v3/staged`, `/v3/capabilities`, and `/health`.
Construct a `pipeline::PipelineBuilder::new()` with the host interceptors,
then pass the built pipeline and a `transport::staged_state::StagedQueryState`
to `transport::v3::create_v3_router`.

#### Embedding the staged executor

`StagedExecutor::execute_staged_request` runs `/v3/staged` in-process,
without axum. The host admits the caller through its pipeline and passes
the signed request bytes; the executor returns the signed reply with the
commit outcome.

This sketch is ignored because it needs a configured pipeline, query state,
and signed request from the host.

```rust,ignore
use type_bridge_server::transport::v3::{StagedExecutor, StagedOutcomeKind};

let executor = Arc::new(StagedExecutor::new(pipeline, query_state));
let outcome = executor.execute_staged_request(ctx, body, deadline).await;
match outcome.kind {
    StagedOutcomeKind::Committed => {}  // outcome.reply holds the answers
    StagedOutcomeKind::Failed => {}     // nothing applied; outcome.code says why
    StagedOutcomeKind::Unknown => {}    // a write may have applied: reconcile
}
```

Hooks run in a fixed order: transport admission, `on_v3_write_authority`
for a write, `on_v3_plan` per plan before anything is reserved, nonce
reservation, staging and one commit, `on_v3_commit` (observe-only) once the
outcome is known, then signing and the response policies. `on_v3_commit`
therefore runs before the reply bytes exist; the test
`library_entry_runs_the_hooks_in_order_and_commits` pins this order.


## Generate staged authority

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
artifact. Every generated package embeds the authority its proxy connection
uses (`connect_proxy` in Python, `connectProxy` in TypeScript,
`ProxyConnectionPolicy` in Rust, `type_bridge_proxy_connection_open_v1` in C);
none reads the standalone artifact or constructs a low-level
`QuerystagedAuthority`. Mount `generated/schema-authority.json` for this generic
server.

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
# Optional retained legacy validation source; not staged schema authority.
source_file = "schema.tql"

[interceptors]
enabled = ["audit-log"]

[interceptors.audit-log]
output = "file"                 # "stdout" or "file"
file_path = "/var/log/audit.jsonl"

[logging]
level = "info"        # default
format = "json"       # default; "text" is also supported

# Additive prepared staged routes; the default build includes the staged transport.
[staged]
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

The staged surface fails startup unless `schema_authority_file` is canonical and
constructor-verified, its embedded semantic profile matches the connected
server, and the selected live authority is valid. `managed` requires the
complete staged migration-control schema and its free singleton for the embedded
scope. `query_only` requires both staged and legacy migration controls to be absent.
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
transaction even though emitted staged TypeQL is read-only, so the server
credential needs that transaction permission and an executing request can
delay concurrent schema work. The default absolute request deadline is 30
seconds and the hard maximum is five minutes.


## HTTP API

`POST /v3/staged` accepts canonical staged requests. Every plan runs inside
one transaction; writes commit once. The signed reply carries each plan's
answer or a typed failure. `commit_outcome_unknown` means a write may have
applied and requires reconciliation before retrying.

`GET /v3/capabilities` returns the staged advertisement after transport
policy admission. Clients must obtain it over authenticated TLS for the
intended server or pin its bytes or fingerprint out of band. Plain HTTP
discovery does not authenticate that trust input.

`GET /health` retains the exact body
`{"status":"ok","version":"1.5.11","typedb_connected":true}` for an open
connection. The version is the frozen health contract identity; use the
binary's `--version` for the package version.

The removed `/query`, `/query/raw`, `/query/validate`, `/schema`, and `/v2/*`
routes return HTTP 404 with `{"error":"route_removed","use":"/v3/staged"}`.

## Testing

Run `cargo test -p type-bridge-server` from the Rust workspace. Live staged
tests additionally need the TypeDB address, HTTP port, and credentials.
