mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};

use crate::{GeneratedPackage, invalid};

const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.py");
const RUNTIME_STUB: &[u8] = include_bytes!("runtime.pyi");
const PY_TYPED: &[u8] = b"";

const RUNTIME_SOURCE_ID: &str = "typebridge.generator.python.runtime-source";
const RUNTIME_STUB_ID: &str = "typebridge.generator.python.runtime-stub";
const PY_TYPED_ID: &str = "typebridge.generator.python.py-typed";

/// Version-one Python package emitter.
#[derive(Clone, Copy, Debug, Default)]
pub struct PythonEmitter;

impl PythonEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self { Self }

    /// Return its exact projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::python_v1()]
    }

    /// Hash its exact fixed output resources.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PY_TYPED_ID, PY_TYPED)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE)?,
            CodeResourceDigest::from_bytes(RUNTIME_STUB_ID, RUNTIME_STUB)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit exactly eight deterministic files without filesystem mutation.
    pub fn emit(&self, projection: &RuntimeProjection) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.code_resources()?;
        if projection.target() != BindingTarget::Python
            || projection.config() != &ProjectionConfig::python()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "python_emitter_evidence_mismatch",
                "projection target, handler, or resource evidence does not match this emitter",
            ));
        }
        render::render(projection, RUNTIME_SOURCE, RUNTIME_STUB, PY_TYPED)
    }
}
