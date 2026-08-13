mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority};

use crate::{
    GeneratedPackage, embedded_authority, invalid, projection_uses_ordered_collections,
    resolved_schema_uses_ordered_collections,
};

const CARGO_TOML: &[u8] = include_bytes!("package.toml");
const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.rs");

const CARGO_TOML_ID: &str = "typebridge.generator.rust.cargo-toml";
const RUNTIME_SOURCE_ID: &str = "typebridge.generator.rust.runtime-source";

const ORDERED_RUNTIME_SOURCE_SUFFIX: &[u8] =
    b"\n// Successor resource for ordered collection projections.\nconst _: u16 = 2;\n";

/// Rust schema-crate emitter with feature-selected legacy and ordered evidence ledgers.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustEmitter;

impl RustEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact legacy unordered projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::rust_v1()]
    }

    /// Return projection-handler evidence selected for the resolved schema features.
    #[must_use]
    pub fn generator_handlers_for(&self, schema: &ResolvedSchema) -> Vec<ProjectionHandler> {
        self.handlers_for_ordered(resolved_schema_uses_ordered_collections(schema))
    }

    /// Hash every legacy unordered fixed byte resource consumed by emission.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.resources_for_ordered(false)
    }

    /// Hash every fixed byte resource selected for the resolved schema features.
    pub fn code_resources_for(
        &self,
        schema: &ResolvedSchema,
    ) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.resources_for_ordered(resolved_schema_uses_ordered_collections(schema))
    }

    fn handlers_for_ordered(&self, ordered: bool) -> Vec<ProjectionHandler> {
        if ordered {
            vec![ProjectionHandler::rust_v2()]
        } else {
            self.generator_handlers()
        }
    }

    fn resources_for_ordered(&self, ordered: bool) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let runtime_source = runtime_source(ordered);
        let mut resources = vec![
            CodeResourceDigest::from_bytes(CARGO_TOML_ID, CARGO_TOML)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, &runtime_source)?,
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
        let ordered = projection_uses_ordered_collections(projection);
        let handlers = self.handlers_for_ordered(ordered);
        let resources = self.resources_for_ordered(ordered)?;
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
        let runtime_source = runtime_source(ordered);
        render::render(projection, &authority, CARGO_TOML, &runtime_source)
    }
}

fn runtime_source(ordered: bool) -> Vec<u8> {
    let suffix = ordered.then_some(ORDERED_RUNTIME_SOURCE_SUFFIX);
    let mut bytes = Vec::with_capacity(RUNTIME_SOURCE.len() + suffix.map_or(0, <[u8]>::len));
    bytes.extend_from_slice(RUNTIME_SOURCE);
    if let Some(suffix) = suffix {
        bytes.extend_from_slice(suffix);
    }
    bytes
}
