# Getting started

TypeBridge has several application surfaces over one shared engine. Choose the
surface that owns your application code; you can add schema generation or the
remote server later without changing semantic systems.

| If you are building… | Start here |
| --- | --- |
| A Python application | [Install Python](installation.md#python) → [Python quick start](quickstart.md) |
| A TypeScript or Node application | [Install Node](installation.md#typescript-node) → [TypeScript/Node guide](../guide/typescript.md) |
| A Rust application | [Rust distribution](installation.md#rust) → [Generate a Rust schema crate](../guide/rust.md) |
| A C17 or C++17 application on Ubuntu 24.04 x86_64 | [Install C](installation.md#c) → [C SDK guide](../guide/c.md) |
| A schema-first, multi-SDK workspace | [Install the CLI](installation.md#cli-and-code-generation) → [Schema workflows](../guide/schema-workflows.md) |
| A remote query service | [Server container](../guide/server-container.md) |

## Before connecting

TypeBridge admits TypeDB versions >=3.11.0 for database operations. Supported
lines remain the measured 3.11–3.14 lines; 3.14.0 is recommended. Newer lines
are admitted with a warning that they are
"not tested or supported by this TypeBridge release" and no compatibility
guarantee. They reuse the newest measured profile and protocol assumptions.
Connected migrations enforce the workspace profile floors: 3.12.3+ on 3.12,
or 3.13.6+/3.14.0+ and newer lines on the newest profile. The
selected C release matrix is verified against TypeDB 3.12.3. The exact
interpreter, native-target, provider-band, and server matrix is maintained in [TypeDB compatibility](../development/typedb.md#server-and-driver-compatibility).

Here, `+` admits later patches on the same minor line; it does not admit a new minor line.

Schema parsing, validation, comparison, migration planning, and code generation
can run offline.
