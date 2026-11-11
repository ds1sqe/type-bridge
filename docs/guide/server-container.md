# TypeBridge Server Container

The TypeBridge container product is the standalone staged query server:

```text
ghcr.io/ds1sqe/type-bridge-server:3.0.0
```

The image always serves `GET /health`. `[staged].enabled` adds
`POST /v3/staged` and `GET /v3/capabilities`; with it,
`[staged].raw_enabled` admits raw TypeQL at `POST /v3/raw`. Every removed
V1 and V2 route answers HTTP 404 with
`{"error":"route_removed","use":"/v3/staged"}`.

The generated Rust SDK is source code, and `typedb/typedb:3.12.3` is an
upstream integration dependency. Neither is republished as another
TypeBridge image.

## Pull by immutable digest

The accepted multi-platform digest for **3.0.0 is recorded at publication**.
Copy that digest from the release notes before deploying:

```bash
export TYPE_BRIDGE_SERVER_IMAGE='ghcr.io/ds1sqe/type-bridge-server@sha256:<published-3.0.0-digest>'
docker pull "$TYPE_BRIDGE_SERVER_IMAGE"
```

After acceptance, `3.0`, `3`, and `latest` are aliases of the exact `3.0.0`
stable manifest. Manual candidate workflows validate the stable bytes without
publishing them or moving aliases. Publication requires the annotated
`v3.0.0` tag push.

Historical stable digests:

| Release | Multi-platform digest |
| --- | --- |
| 2.3.3 | `sha256:6f722e80528315b6729bb1dd05498674bb7e7901ad29c509ebf94575bb083466` |
| 2.3.2 | `sha256:1a9c9c26193ccac273d58e6b3c78b6741b744b60ead3e90ee221401536e02519` |
| 2.3.1 | `sha256:f33430b590b3e8c09f739e4b0ed36ce7ef254ded7c769f3be8472452d999bc7c` |
| 2.3.0 | `sha256:fe72c2911a9cfa7116eaf671e195028a1440f579614b2b7f8921ab0f683f3209` |

Published platforms are `linux/amd64` and `linux/arm64`.

## Run securely

The image runs as fixed UID/GID `10001:10001` and needs no Linux
capabilities or writable root filesystem:

```bash
docker run --rm \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  -p 8080:8080 \
  -e TYPEDB_ADDRESS=typedb:1729 \
  -e TYPEDB_DATABASE=app \
  -e TYPEDB_USERNAME \
  -e TYPEDB_PASSWORD \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

The credential variables above inherit values from the caller; the command
does not contain or print them. The image's
`/etc/type-bridge/server.toml` is a credential-free example with staged
queries disabled.
For production, mount a complete operator-owned configuration and its schema
authority read-only:

```bash
docker run --rm \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  -p 8080:8080 \
  -e TYPEDB_USERNAME \
  -e TYPEDB_PASSWORD \
  --mount type=bind,src="$PWD/runtime",dst=/run/type-bridge,readonly \
  --mount type=bind,src="$PWD/server.toml",dst=/etc/type-bridge/server.toml,readonly \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

Paths in a mounted config resolve from the config directory. Mount TLS
certificates, custom roots, and the generated schema-authority artifact
explicitly; they are not embedded in the generic image.

## Generate server authority

Split YAML and `typebridge.yaml` remain the only schema/model authoring path.
Configure the source-free server artifact beside the language bindings:

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

Then capture all outputs together:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

The artifact's `typebridge.schema-authority/v1` canonical JSON is an internal,
bounded deployment codec. It binds the declared schema, required capabilities,
managed scope, semantic profile, and recomputed fingerprints so a generic
server can verify intent without importing an application package or compiling
raw YAML. Do not edit it or maintain JSON as a second schema; regenerate it with
every binding package.

## Enable staged queries

A production configuration binds live TypeDB to that generated authority:

```toml
[server]
host = "0.0.0.0"
port = 8080

[typedb]
address = "typedb:1729"
database = "app"
http_port = 8000
tls = true
tls-root-ca = "/run/type-bridge/typedb-root.pem"

[logging]
level = "info"
format = "json"

[staged]
enabled = true
schema_authority_file = "/run/type-bridge/schema-authority.json"
authority_mode = "managed"
```

To accept queries beyond the default query limits, or to hold clients to
stricter ones, name the limits to change under `[staged.query_limits]`:

```toml
[staged.query_limits]
canonical_bytes = 67108864       # 64 MiB plans
remote_request_bytes = 134217728 # 128 MiB request envelopes
```

The server validates the table at startup, advertises these limits in
`/v3/capabilities`, and enforces exactly what it advertises. An unknown name
or an unsupported value aborts startup. Clients older than configurable query
limits cannot read an advertisement that names any, so leave the table out
until every client is upgraded. Limit names and defaults are listed in the
[typed-query contract](../development/typed-query-contract.md#query-limits).

The server starts from the proxy answer defaults (65,536 items, 32 MiB). To
let one query return more than they allow, or to hold clients to less, name
the answer ceilings to change under `[staged.answer_limits]`:

```toml
[staged.answer_limits]
items = 1000000        # provider items and rows per query
bytes = 1073741824     # 1 GiB replies
graph_nodes = 1000000
```

The same rules apply: the table is validated at startup, advertised in
`/v3/capabilities`, and enforced as advertised. A client must raise its own
answer policy too; each request runs under the stricter of the two. Names
and defaults are listed in the
[typed-query contract](../development/typed-query-contract.md#answer-limits).

Staged writes need an interceptor that authenticates the caller; to admit
them without one, opt in explicitly:

```toml
[staged]
allow_unauthenticated_staged_writes = true
```

Supply `TYPEDB_USERNAME` and `TYPEDB_PASSWORD` at runtime. `managed`
authority requires the artifact's exact migration-control partition and free
singleton. Use `query_only` only for a database with neither canonical nor
archived migration controls. Scope and semantic profile come from the verified
artifact and are intentionally not repeated in server configuration.

## Health and version identity

Probe health from outside the container:

```bash
curl --fail --silent http://127.0.0.1:8080/health
docker run --rm "$TYPE_BRIDGE_SERVER_IMAGE" --version
```

The image deliberately has no shell `HEALTHCHECK`. `/health.version`,
`--version`, the exact tag, and the OCI version label all report `3.0.0`.

## Verify supply-chain evidence

The stable digest has keyless signatures, per-platform SPDX JSON SBOMs, and
GitHub build-provenance attestations for the accepted release source.
Verify the tag signature with Cosign 3:

```bash
cosign verify \
  --certificate-identity \
    'https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v3.0.0' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

The release includes `server-oci-release.json` with the multi-platform and
per-platform digests plus attestation URLs. Release acceptance also reports
the compressed layer size, installed runtime packages, closed runtime file
set, and the vulnerability/secret scan result.

The build-provenance attestations bind the image to its tagged release
workflow and release source commit, which the image also carries as its
`org.opencontainers.image.revision` label. Set `RELEASE_SOURCE_COMMIT` to that
commit and verify the source and signing identity:

```bash
gh attestation verify "oci://$TYPE_BRIDGE_SERVER_IMAGE" \
  --repo ds1sqe/type-bridge \
  --cert-identity 'https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v3.0.0' \
  --cert-oidc-issuer https://token.actions.githubusercontent.com \
  --source-digest "$RELEASE_SOURCE_COMMIT" \
  --source-ref refs/tags/v3.0.0 \
  --predicate-type 'https://slsa.dev/provenance/v1'
```

The historical 2.2.1 release used publication-recovery attestations. Its
original evidence remains available in that release; the
[release procedure](../development/c-release.md#protected-promotion)
describes the distinction.
