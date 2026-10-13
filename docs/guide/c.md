# C SDK

**Security update:** Upgrade to TypeBridge 2.2.5. Its prebuilt distributions
use rustls 0.23.46, which fixes RUSTSEC-2026-0346 through RUSTSEC-2026-0349
and the earlier
[RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).
The [2.2.1 post-release notice](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
records the affected earlier release.

TypeBridge 2.2.5 provides a generated C SDK with native ABI 1.6.0 for
**Ubuntu 24.04, x86_64 GNU/Linux, shared runtime**. C17 and C++17 applications
consume generated schema packages through CMake or pkg-config. The public
release was independently verified on 2026-10-15.

The SDK uses the same Rust-owned schema, CRUD, query, migration, serialization,
cancellation, diagnostics, and resource-limit contracts as the other SDKs.
Connected acceptance uses exactly TypeDB 3.12.3; the canonical semantic profile
is `typedb-3.12.1/v1`. The selected C distribution does not cover static linking,
macOS, Windows, or other architectures.

## Install the runtime

Download the runtime archive and its Sigstore bundle from the
[2.2.5 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.5):

- `type-bridge-c-runtime-2.2.5-abi-1.6-x86_64-unknown-linux-gnu.tar.gz`
- `type-bridge-cli-2.2.5-x86_64-unknown-linux-gnu.tar.gz` for standalone generation
  and migration commands
- `type-bridge-c-sdk-example-2.2.5.tar.gz` for the verified `tb_sdkv3` example

Verify the signed release files using the
[documented signing identity and provenance](../development/c-release.md#protected-promotion).
Extract the runtime into a private installation prefix. It contains the public
header, `libtype_bridge_c.so`, CMake configuration, and pkg-config metadata.
The standalone CLI archive has its own `bin/type-bridge` executable.

Application execution needs the shared runtime and its system-library
dependencies. Building an application additionally needs a C17 or C++17
compiler and CMake 3.20+ or pkg-config. Python and Cargo are not application
runtime dependencies.

## Generate an application schema package

Add a C output to your Split-YAML workspace:

```yaml
bindings:
  c:
    output: generated/c
```

Then run:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

Distribute the complete generated package with your application. It contains
typed declarations, generated C source, embedded schema authority, package
identity, and build metadata. Keep the physical schema in Split-YAML and
regenerate rather than editing emitted declarations or runtime resources.

## Link an application

For the released example, CMake discovers both the runtime and generated
schema package:

```cmake
cmake_minimum_required(VERSION 3.20)
project(example LANGUAGES C CXX)
find_package(TypeBridge 1.6.0 EXACT CONFIG REQUIRED)
find_package(tb_sdkv3 1.0.0 EXACT CONFIG REQUIRED)
add_executable(example main.c)
set_target_properties(example PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)
target_link_libraries(example PRIVATE tb_sdkv3::schema)
```

Set `TypeBridge_DIR` to the runtime's `lib/cmake/TypeBridge` directory and
`tb_sdkv3_DIR` to the example package's `lib/cmake/tb_sdkv3` directory. Include
`<tb_sdkv3/tb_sdkv3.h>` from the application. Use the package name emitted for
your own schema when replacing the example.

For pkg-config, add both packages' `lib/pkgconfig` directories to
`PKG_CONFIG_PATH`. Compile the source returned by
`pkg-config --variable=generated_source tb_sdkv3` together with your application,
using `pkg-config --cflags --libs tb_sdkv3`. Configure the dynamic loader to find
the installed runtime's `lib` directory.

## Run a query on a database or a TypeBridge server

A query session authors terminals; it holds no connection. Each connection
type has one execute call, and all of them return the same result family and
diagnostics:

| Connection | Execute call |
| --- | --- |
| `type_bridge_database_t` | `type_bridge_database_query_execute_v1` |
| `type_bridge_read_transaction_t` | `type_bridge_read_transaction_query_execute_v1` |
| `type_bridge_proxy_connection_t` | `type_bridge_proxy_query_execute_v1` |

So one program runs unchanged against TypeDB directly or through a
TypeBridge server; only the connection it opens differs. The proxy connection
is synchronous and blocks the calling thread:

```c
type_bridge_proxy_header_v1_t auth = {
    {(const uint8_t *)"Authorization", 13},
    {(const uint8_t *)token, token_length},
};
type_bridge_proxy_connection_options_v1_t options = {0};
options.struct_size = sizeof options;
options.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
options.endpoint = (type_bridge_byte_view_t){(const uint8_t *)url, strlen(url)};
options.tls_mode = TYPE_BRIDGE_TLS_NATIVE_ROOTS; /* or _DISABLED, _CUSTOM_ROOT_CA */
options.headers = &auth;
options.header_count = 1;
options.answer_limits = &ceiling; /* NULL selects the proxy defaults */

type_bridge_proxy_connection_t *proxy = NULL;
type_bridge_execution_diagnostics_t *diagnostics = NULL;
status = type_bridge_proxy_connection_open_v1(
    package, &options, NULL, &proxy, &diagnostics);
/* ... */
status = type_bridge_proxy_query_execute_v1(
    proxy, terminal, kind, NULL, NULL, &result, &diagnostics);
/* ... */
type_bridge_proxy_connection_close(&proxy);
```

With TLS mode `TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA`, `custom_root_ca_path` names a
PEM bundle. An empty `pinned_advertisement` fetches the server's capability
advertisement during open, bounded by `connection_limits`; a nonempty one is
used as is. `answer_limits` is a ceiling: every terminal on the connection
runs under its own limits constrained by it, and limit values are kept or
refused, never clamped. Manager terminals run only on a database or a read
transaction.

### Caller-owned exchange

A host that owns the route to the server (its own HTTP stack, a message bus,
or an in-process server) uses the remote context instead of the built-in
proxy connection. It is the exchange extension point, not a second session:
`type_bridge_query_remote_context_open_v1` validates the advertisement,
`type_bridge_query_remote_prepare_v1` returns a pending request whose bytes
(`type_bridge_query_remote_pending_request_bytes`) the host sends, and the
reply goes through the one-shot `type_bridge_query_remote_pending_claim` and
`type_bridge_query_remote_claim_decode_v1`. Both paths prepare and validate
requests through the same Rust proxy connection.

The release evidence covers relocated installation, strict C17/C++17 headers,
direct and caller-transport queries, custom-root TLS, migrations, model
serialization, cancellation, resource limits, and cleanup. See the
[release procedure](../development/c-release.md#public-verification-and-support)
for the public verification record and exact support boundary.
