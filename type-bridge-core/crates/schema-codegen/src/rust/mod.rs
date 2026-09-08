mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::VerifiedSchemaAuthority;

use crate::{GeneratedPackage, embedded_authority, invalid};

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

    /// Emit one deterministic crate bound to exact verified schema authority.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
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
        let authority = embedded_authority(projection, authority)?;
        render::render(projection, &authority, CARGO_TOML, RUNTIME_SOURCE)
    }
}
