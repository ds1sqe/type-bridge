# C SDK

**Security update:** Upgrade to TypeBridge 2.2.2. Its prebuilt distributions
use rustls 0.23.45, which fixes
[RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).
The [2.2.1 post-release notice](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
records the affected earlier release.

TypeBridge 2.2.2 provides a generated C SDK with native ABI 1.6.0 for
**Ubuntu 24.04, x86_64 GNU/Linux, shared runtime**. C17 and C++17 applications
consume generated schema packages through CMake or pkg-config. The public
release was independently verified on 2026-09-14.

The SDK uses the same Rust-owned schema, CRUD, query, migration, serialization,
cancellation, diagnostics, and resource-limit contracts as the other SDKs.
Connected acceptance uses exactly TypeDB 3.12.3; the canonical semantic profile
is `typedb-3.12.1/v1`. The selected C distribution does not cover static linking,
macOS, Windows, or other architectures.

## Install the runtime

Download the runtime archive and its Sigstore bundle from the
[2.2.2 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.2):

- `type-bridge-c-runtime-2.2.2-abi-1.6-x86_64-unknown-linux-gnu.tar.gz`
- `type-bridge-cli-2.2.2-x86_64-unknown-linux-gnu.tar.gz` for standalone generation
  and migration commands
- `type-bridge-c-sdk-example-2.2.2.tar.gz` for the verified `tb_sdkv3` example

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

The release evidence covers relocated installation, strict C17/C++17 headers,
direct and caller-transport queries, custom-root TLS, migrations, model
serialization, cancellation, resource limits, and cleanup. See the
[release procedure](../development/c-release.md#public-verification-and-support)
for the public verification record and exact support boundary.
