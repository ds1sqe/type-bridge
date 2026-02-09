# type-bridge-core

Rust core for the `type-bridge` TypeDB ORM.

## Overview

This crate provides a high-performance, shared type system and query engine for `type-bridge`. It enables:

- **Bidirectional Validation**: Define validation rules once in Rust and enforce them on both client (Python) and server (Rust/WASM).
- **Query Object Portability**: First-class AST objects that can be serialized and moved between runtimes.
- **Performance**: High-speed query compilation and schema parsing.

## Structure

- `src/core`: Pure Rust implementation of the AST, schema, and validation engine. This is runtime-agnostic.
- `src/ast`: PyO3 wrappers for the core AST nodes, providing an idiomatic Python API.
- `src/lib.rs`: PyO3 module definition.

## Building

To build the Python extension:

```bash
cd type-bridge-core
maturin develop
```

## Status

This is an initial implementation following the RFC in issue #95. Key structures are in place, with logic being ported from Python.
