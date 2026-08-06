# Getting started

TypeBridge has several application surfaces over one shared engine. Choose the
surface that owns your application code; you can add schema generation or the
remote server later without changing semantic systems.

| If you are building… | Start here |
| --- | --- |
| A Python application | [Install Python](installation.md#python) → [Python quick start](quickstart.md) |
| A TypeScript or Node application | [Install Node](installation.md#typescript-node) → [TypeScript/Node guide](../guide/typescript.md) |
| A Rust application | [Rust distribution](installation.md#rust) → [Generate a Rust schema crate](../guide/rust.md) |
| A schema-first, multi-SDK workspace | [Install the CLI](installation.md#cli-and-code-generation) → [Schema workflows](../guide/schema-workflows.md) |
| A remote query service | [Server container](../guide/server-container.md) |

## Before connecting

You need a supported TypeDB 3.x server for database operations. TypeBridge 2.1
supports TypeDB 3.11–3.12. The exact interpreter, native-target, provider-band,
and server matrix is
maintained in [TypeDB compatibility](../development/typedb.md#server-and-driver-compatibility).

Schema parsing, validation, comparison, migration planning, and code generation
can run offline.
