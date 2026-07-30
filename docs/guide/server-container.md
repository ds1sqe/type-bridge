# TypeBridge Server Container

The TypeBridge container product is the V2-capable standalone query server:

```text
ghcr.io/ds1sqe/type-bridge-server:2.0.0
```

The generated Rust SDK is source code, and `typedb/typedb:3.12.1` is an
upstream integration dependency. Neither is republished as another
TypeBridge image.

## Pull by immutable digest

Release notes record the accepted multi-platform digest. Prefer it for
deployments:

```bash
export TYPE_BRIDGE_SERVER_IMAGE='ghcr.io/ds1sqe/type-bridge-server@sha256:<digest-from-v2.0.0-release>'
docker pull "$TYPE_BRIDGE_SERVER_IMAGE"
```

The stable `2.0.0` tag points to the same digest. After acceptance, `2.0`,
`2`, and `latest` are aliases of that exact stable manifest. Candidate
workflows validate `2.0.0-rc.0` bytes without publishing them by default and
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
certificates, custom roots, declared-schema bytes, and optional TypeQL schema
files explicitly; they are not embedded in the generic image.

## Enable V2

A V2 production configuration must bind the live TypeDB authority to the
canonical declared schema:

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
declared_schema_file = "/run/type-bridge/declared-schema.json"
scope = "app-production"
profile = "typedb-3.12.1/v1"
authority_mode = "managed"
```

Supply `TYPEDB_USERNAME` and `TYPEDB_PASSWORD` at runtime. `managed`
authority requires the complete migration-control partition and singleton;
use `query_only` only for a database with neither V2 nor legacy migration
controls.

## Health and version identity

Probe health from outside the container:

```bash
curl --fail --silent http://127.0.0.1:8080/health
docker run --rm "$TYPE_BRIDGE_SERVER_IMAGE" --version
```

The image deliberately has no shell `HEALTHCHECK`. `/health.version` remains
the frozen V1 compatibility value `1.5.11`, while `--version`, the exact tag,
and the OCI version label report `2.0.0`.

## Verify supply-chain evidence

The stable digest has keyless signatures, per-platform SPDX JSON SBOMs, and
GitHub build-provenance attestations. With Cosign:

```bash
cosign verify \
  --certificate-identity-regexp \
    '^https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v2[.]0[.]0$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$TYPE_BRIDGE_SERVER_IMAGE"
```

The release includes `server-oci-release.json` with the multi-platform and
per-platform digests plus attestation URLs. Release acceptance also reports
the compressed layer size, installed runtime packages, closed runtime file
set, and the vulnerability/secret scan result.
