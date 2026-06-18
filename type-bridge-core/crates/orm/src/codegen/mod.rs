//! Code generator: TypeQL schema → Rust source files.
//!
//! Parses a TypeQL `define` block and generates Rust structs with derive macros.

pub mod generator;
pub mod naming;

pub use generator::{
    GeneratedModels, generate_for_target, generate_from_typeql, generate_rust_models,
};
