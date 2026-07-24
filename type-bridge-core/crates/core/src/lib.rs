//! # type-bridge-core-lib
//!
//! Pure-Rust core library for **type-bridge**, providing a TypeQL AST,
//! schema parser, query compiler, value coercer, and validation engine.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`ast`] | TypeQL Abstract Syntax Tree — patterns, statements, clauses, and values |
//! | [`schema`] | Schema representation with entity / relation / attribute types and inheritance |
//! | [`validation`] | Schema-aware query validation plus a custom validation-rule DSL |
//! | [`compiler`] | Compiles an AST back into a TypeQL query string |
//! | [`query_parser`] | Parses a TypeQL query string into the AST |
//! | [`value_coercion`] | Coerces raw values into TypeDB value-types and formats TypeQL literals |
//! | [`reserved_words`] | TypeQL reserved-word detection |
//! | [`parser`] | Low-level PEG grammar consumed by [`query_parser`] |
//! | [`version`] | TypeDB compatibility window, protocol-band map, and version gate (SSOT) |

#![warn(missing_docs)]

/// TypeQL Abstract Syntax Tree — patterns, statements, clauses, and values.
pub mod ast;
/// Rust-hosted model generation for Python, TypeScript, and Rust targets.
pub mod bindgen;
/// Compiles an AST back into a TypeQL query string.
pub mod compiler;
/// Canonical TypeDB decimal parsing and semantic comparison.
pub mod decimal;
/// Low-level PEG grammar for TypeQL, consumed by [`query_parser`].
pub mod parser;
/// Parses a TypeQL query string into the AST.
pub mod query_parser;
/// TypeQL reserved-word detection.
pub mod reserved_words;
/// Schema representation with entity / relation / attribute types and inheritance.
pub mod schema;
/// Schema-aware query validation plus a custom validation-rule DSL.
pub mod validation;
mod validation_rule_wire;
/// Coerces raw values into TypeDB value-types and formats TypeQL literals.
pub mod value_coercion;
/// TypeDB compatibility window, protocol-band map, and connect-time version gate.
pub mod version;
