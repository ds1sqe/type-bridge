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
//! | [`validation`] | Schema-aware query validation plus a custom validation-rule DSL |
//! | [`compiler`] | Compiles an AST back into a TypeQL query string |
//! | [`query_parser`] | Parses a TypeQL query string into the AST |
//! | [`value_coercion`] | Coerces raw values into TypeDB value-types and formats TypeQL literals |
//! | [`reserved_words`] | TypeQL reserved-word detection |
//! | [`version`] | TypeDB compatibility window, protocol-band map, and version gate (SSOT) |

#![deny(missing_docs)]

/// Frozen direct-TypeQL renderer retained behind compatibility tooling.
#[doc(hidden)]
#[path = "bindgen.rs"]
pub mod _bindgen;
/// Frozen released-TypeQL parser retained behind compatibility tooling.
#[doc(hidden)]
#[path = "parser.rs"]
pub mod _parser;
/// Frozen released-TypeQL schema representation retained behind compatibility tooling.
#[doc(hidden)]
#[path = "schema.rs"]
pub mod _schema;
/// TypeQL Abstract Syntax Tree — patterns, statements, clauses, and values.
pub mod ast;
/// Compiles an AST back into a TypeQL query string.
pub mod compiler;
/// Canonical TypeDB decimal parsing and semantic comparison.
pub mod decimal;
/// Parses a TypeQL query string into the AST.
pub mod query_parser;
/// TypeQL reserved-word detection.
pub mod reserved_words;
/// Schema-aware query validation plus a custom validation-rule DSL.
pub mod validation;
mod validation_rule_wire;
/// Coerces raw values into TypeDB value-types and formats TypeQL literals.
pub mod value_coercion;
/// TypeDB compatibility window, protocol-band map, and connect-time version gate.
pub mod version;
