//! TOML schema DSL transpiler for type-bridge.
//!
//! Converts a TOML schema document into a canonical TypeQL `define` block.
//! This crate has no PyO3, no `type-bridge-*` dependencies, and no runtime
//! dependency beyond `toml`, `serde`, `indexmap`, and `thiserror`.

mod emit;
mod model;

use thiserror::Error;

/// Errors that can occur during TOML-to-TypeQL transpilation.
#[derive(Debug, Error)]
pub enum TranspileError {
    /// The TOML document could not be parsed or the schema structure did not
    /// match the expected shape (e.g. unknown key, missing required field).
    #[error("TOML parse or deserialisation error: {0}")]
    Toml(#[from] toml::de::Error),
}

/// Parse `toml_text` as a type-bridge schema document and emit a canonical
/// TypeQL `define` block.
///
/// # Errors
///
/// Returns [`TranspileError::Toml`] when the input is not valid TOML or does
/// not conform to the schema model (unknown keys, missing required fields).
pub fn toml_to_typeql(toml_text: &str) -> Result<String, TranspileError> {
    let schema: model::TomlSchema = toml::from_str(toml_text)?;
    Ok(emit::emit(&schema))
}
