# TypeBridge Server Container

**Security update:** Upgrade to TypeBridge 2.2.5. Its prebuilt distributions
use rustls 0.23.46, which fixes RUSTSEC-2026-0346 through RUSTSEC-2026-0349
and the earlier
[RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).
Its server image also installs Debian's fixed Bookworm PCRE2 and Perl
packages.
In 2.2.5, write queries ending in `fetch` return commit failures instead of
documents for writes that were never stored.
The [2.2.1 post-release notice](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
records the affected earlier release.

The TypeBridge container product is the V2-capable standalone query server:

```text
ghcr.io/ds1sqe/type-bridge-server:2.2.5
```

The image contains retained V1 routes and the public `v2-query` capability.
`[v2].enabled` adds or hides V2 routes at runtime; it never replaces the V1
pipeline.

The generated Rust SDK is source code, and `typedb/typedb:3.12.3` is an
upstream integration dependency. Neither is republished as another
TypeBridge image.

## Pull by immutable digest

Release notes record the accepted multi-platform digest. Prefer it for
deployments:

```bash
export TYPE_BRIDGE_SERVER_IMAGE='ghcr.io/ds1sqe/type-bridge-server@sha256:1b3d27e7be74de6d49fe225a39939a454c420bd655c35e0ecb19bad11e5828ee'
docker pull "$TYPE_BRIDGE_SERVER_IMAGE"
```

The stable `2.2.5` tag points to the same digest. After acceptance, `2.2`,
`2`, and `latest` are aliases of that exact stable manifest. Manual candidate
workflows validate the exact `2.2.5` stable bytes without publishing them or
moving aliases. Publication requires the annotated `v2.2.5` tag push.

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
`/etc/type-bridge/server.toml` is a credential-free example with V2 disabled.
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
certificates, custom roots, the generated schema-authority artifact, and
optional TypeQL schema files explicitly; they are not embedded in the generic
image.

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

## Enable V2

A V2 production configuration binds live TypeDB to that generated authority:

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

[v2]
enabled = true
schema_authority_file = "/run/type-bridge/schema-authority.json"
authority_mode = "managed"
```

To accept queries beyond the default query limits, or to hold clients to
stricter ones, name the limits to change under `[v2.query_limits]`:

```toml
[v2.query_limits]
canonical_bytes = 67108864       # 64 MiB plans
remote_request_bytes = 134217728 # 128 MiB request envelopes
```

The server validates the table at startup, advertises these limits in
`/v2/capabilities`, and enforces exactly what it advertises. An unknown name
or an unsupported value aborts startup. Clients older than configurable query
limits cannot read an advertisement that names any, so leave the table out
until every client is upgraded. Limit names and defaults are listed in the
[typed-query contract](../development/typed-query-contract.md#query-limits).

The server starts from the proxy answer defaults (65,536 items, 32 MiB). To
let one query return more than they allow, or to hold clients to less, name
the answer ceilings to change under `[v2.answer_limits]`:

```toml
[v2.answer_limits]
items = 1000000        # provider items and rows per query
bytes = 1073741824     # 1 GiB replies
graph_nodes = 1000000
```

The same rules apply: the table is validated at startup, advertised in
`/v2/capabilities`, and enforced as advertised. A client must raise its own
answer policy too; each request runs under the stricter of the two. Names
and defaults are listed in the
[typed-query contract](../development/typed-query-contract.md#answer-limits).

An enabled `[v2]` section also serves staged transactions at `/v3/staged`.
Staged writes need an interceptor that authenticates the caller; to admit
them without one, opt in explicitly:

```toml
[v2]
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

The image deliberately has no shell `HEALTHCHECK`. `/health.version` remains
the frozen V1 compatibility value `1.5.11`, while `--version`, the exact tag,
and the OCI version label report `2.2.5`.

## Verify supply-chain evidence

The stable digest has keyless signatures, per-platform SPDX JSON SBOMs, and
GitHub build-provenance attestations for the accepted release source.
Verify the tag signature with Cosign 3:

```bash
cosign verify \
  --certificate-identity \
    'https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v2.2.5' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

The release includes `server-oci-release.json` with the multi-platform and
per-platform digests plus attestation URLs. Release acceptance also reports
the compressed layer size, installed runtime packages, closed runtime file
set, and the vulnerability/secret scan result.

The build-provenance attestations bind the image to release source
`51f15f5f92c4b954de94aaf7a7f67e03e877e16c` and its tagged release workflow.
Verify the source and signing identity:

```bash
gh attestation verify "oci://$TYPE_BRIDGE_SERVER_IMAGE" \
  --repo ds1sqe/type-bridge \
  --cert-identity 'https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v2.2.5' \
  --cert-oidc-issuer https://token.actions.githubusercontent.com \
  --source-digest 51f15f5f92c4b954de94aaf7a7f67e03e877e16c \
  --source-ref refs/tags/v2.2.5 \
  --predicate-type 'https://slsa.dev/provenance/v1'
```

The historical 2.2.1 release used publication-recovery attestations. Its
original evidence remains available in that release; the
[release procedure](../development/c-release.md#protected-promotion)
describes the distinction.
