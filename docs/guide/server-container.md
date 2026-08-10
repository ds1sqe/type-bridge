# TypeBridge Server Container

The TypeBridge container product is the V2-capable standalone query server:

```text
ghcr.io/ds1sqe/type-bridge-server:2.1.0
```

The image contains retained V1 routes and the public `v2-query` capability.
`[v2].enabled` adds or hides V2 routes at runtime; it never replaces the V1
pipeline.

The generated Rust SDK is source code, and `typedb/typedb:3.12.1` is an
upstream integration dependency. Neither is republished as another
TypeBridge image.

## Pull by immutable digest

Release notes record the accepted multi-platform digest. Prefer it for
deployments:

```bash
export TYPE_BRIDGE_SERVER_IMAGE='ghcr.io/ds1sqe/type-bridge-server@sha256:<digest-from-v2.1.0-release>'
docker pull "$TYPE_BRIDGE_SERVER_IMAGE"
```

The stable `2.1.0` tag points to the same digest. After acceptance, `2.1`,
`2`, and `latest` are aliases of that exact stable manifest. Candidate
workflows validate `2.1.0-rc.0` bytes without publishing them by default and
never move `latest`.

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
and the OCI version label report `2.1.0`.

## Verify supply-chain evidence

The stable digest has keyless signatures, per-platform SPDX JSON SBOMs, and
GitHub build-provenance attestations. With Cosign:

```bash
cosign verify \
  --certificate-identity-regexp \
    '^https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v2[.]1[.]0$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

The release includes `server-oci-release.json` with the multi-platform and
per-platform digests plus attestation URLs. Release acceptance also reports
the compressed layer size, installed runtime packages, closed runtime file
set, and the vulnerability/secret scan result.
