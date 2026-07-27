mod render;
mod reserved;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};

use crate::{GeneratedPackage, invalid};

const PACKAGE_JSON: &[u8] = include_bytes!("package.json");
const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.ts");
const TSCONFIG_JSON: &[u8] = include_bytes!("tsconfig.json");

const PACKAGE_JSON_ID: &str = "typebridge.generator.typescript.package-json";
const RUNTIME_SOURCE_ID: &str = "typebridge.generator.typescript.runtime-source";
const TSCONFIG_JSON_ID: &str = "typebridge.generator.typescript.tsconfig-json";

/// Version-one TypeScript ESM/NodeNext package emitter.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptEmitter;

impl TypeScriptEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::typescript_v1()]
    }

    /// Hash every fixed byte resource consumed by emission.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PACKAGE_JSON_ID, PACKAGE_JSON)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE)?,
            CodeResourceDigest::from_bytes(TSCONFIG_JSON_ID, TSCONFIG_JSON)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit exactly eight deterministic ESM/NodeNext package files.
    pub fn emit(&self, projection: &RuntimeProjection) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.code_resources()?;
        if projection.target() != BindingTarget::TypeScript
            || projection.config() != &ProjectionConfig::typescript()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "typescript_emitter_evidence_mismatch",
                "projection target, handler, or resource evidence does not match this emitter",
            ));
        }
        render::render(projection, RUNTIME_SOURCE, PACKAGE_JSON, TSCONFIG_JSON)
    }
}
