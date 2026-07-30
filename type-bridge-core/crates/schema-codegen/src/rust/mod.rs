mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_contract::schema::{DeclaredSchema, encode_declared_schema};

use crate::{GeneratedPackage, invalid};

const CARGO_TOML: &[u8] = include_bytes!("package.toml");
const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.rs");

const CARGO_TOML_ID: &str = "typebridge.generator.rust.cargo-toml";
const RUNTIME_SOURCE_ID: &str = "typebridge.generator.rust.runtime-source";

/// Split-YAML Rust client schema crate emitter.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustEmitter;

impl RustEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::rust_v1()]
    }

    /// Hash every fixed byte resource consumed by emission.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let mut resources = vec![
            CodeResourceDigest::from_bytes(CARGO_TOML_ID, CARGO_TOML)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit exactly eleven deterministic crate files without filesystem mutation.
    pub fn emit(&self, projection: &RuntimeProjection) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.code_resources()?;
        if projection.target() != BindingTarget::Rust
            || projection.config() != &ProjectionConfig::rust()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "rust_emitter_evidence_mismatch",
                "projection target, config, handler, or resource evidence does not match this emitter",
            ));
        }
        render::render(projection, CARGO_TOML, RUNTIME_SOURCE, None)
    }

    /// Emit a Rust package carrying the canonical declared schema required by
    /// the authenticated remote model-query authority.
    pub fn emit_with_declared_schema(
        &self,
        projection: &RuntimeProjection,
        declared: &DeclaredSchema,
    ) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.code_resources()?;
        if projection.target() != BindingTarget::Rust
            || projection.config() != &ProjectionConfig::rust()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "rust_emitter_evidence_mismatch",
                "projection target, config, handler, or resource evidence does not match this emitter",
            ));
        }
        let declared = encode_declared_schema(declared)?;
        render::render(projection, CARGO_TOML, RUNTIME_SOURCE, Some(&declared))
    }
}
