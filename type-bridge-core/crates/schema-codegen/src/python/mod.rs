mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority};

use crate::{GeneratedPackage, embedded_authority, invalid};

const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.py");
const RUNTIME_STUB: &[u8] = include_bytes!("runtime.pyi");
const QUERY_SOURCE: &[u8] = include_bytes!("query.py");
const QUERY_STUB: &[u8] = include_bytes!("query.pyi");
const PY_TYPED: &[u8] = b"";

const RUNTIME_SOURCE_ID: &str = "typebridge.generator.python.runtime-source";
const RUNTIME_STUB_ID: &str = "typebridge.generator.python.runtime-stub";
const QUERY_SOURCE_ID: &str = "typebridge.generator.python.query-source";
const QUERY_STUB_ID: &str = "typebridge.generator.python.query-stub";
const PY_TYPED_ID: &str = "typebridge.generator.python.py-typed";

/// Python package emitter with one native runtime evidence ledger.
#[derive(Clone, Copy, Debug, Default)]
pub struct PythonEmitter;

impl PythonEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return the native projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::python_v2()]
    }

    /// Return the same native handler evidence for every resolved schema.
    #[must_use]
    pub fn generator_handlers_for(&self, _schema: &ResolvedSchema) -> Vec<ProjectionHandler> {
        self.generator_handlers()
    }

    /// Hash the exact fixed native runtime resources.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.runtime_resources()
    }

    /// Hash the same fixed native resources for every resolved schema.
    pub fn code_resources_for(
        &self,
        _schema: &ResolvedSchema,
    ) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.runtime_resources()
    }

    fn runtime_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PY_TYPED_ID, PY_TYPED)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE)?,
            CodeResourceDigest::from_bytes(RUNTIME_STUB_ID, RUNTIME_STUB)?,
            CodeResourceDigest::from_bytes(QUERY_SOURCE_ID, QUERY_SOURCE)?,
            CodeResourceDigest::from_bytes(QUERY_STUB_ID, QUERY_STUB)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit one deterministic package bound to exact verified schema authority.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
    ) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.runtime_resources()?;
        if projection.target() != BindingTarget::Python
            || projection.config().python_naming_policy()
                != ProjectionConfig::python().python_naming_policy()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "python_emitter_evidence_mismatch",
                "projection target, handler, or resource evidence does not match this emitter",
            ));
        }
        let authority = embedded_authority(projection, authority)?;
        render::render(
            projection,
            &authority,
            RUNTIME_SOURCE,
            RUNTIME_STUB,
            QUERY_SOURCE,
            QUERY_STUB,
            PY_TYPED,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_runtime_source_validates_complete_candidates_atomically() {
        let source = std::str::from_utf8(RUNTIME_SOURCE).unwrap();
        assert!(source.contains("validate_create(type(instance), instance)"));
        assert!(source.contains("except BaseException:"));
        assert!(source.contains("for owner in type(instance).__mro__:"));
        assert!(source.contains("descriptor.set_value(instance, value)"));
        assert!(!source.contains("globals()["));
        assert!(!source.contains("_LegacyProjectedField"));
        assert!(source.contains("class DirectConnectionPolicy:"));
        assert!(source.contains("password: str = field(default=\"password\", repr=False)"));
        assert!(source.contains("DirectConnectionPolicy([REDACTED])"));
        assert!(source.contains("projection.connect_direct("));
    }

    #[test]
    fn native_runtime_handler_and_resources_are_schema_independent() {
        let emitter = PythonEmitter::new();
        assert_eq!(
            emitter.generator_handlers(),
            [ProjectionHandler::python_v2()]
        );
        let resources = emitter.code_resources().unwrap();
        assert_eq!(resources.len(), 5);
        assert!(
            resources
                .iter()
                .any(|resource| resource.id().as_str() == RUNTIME_SOURCE_ID)
        );
    }
}
