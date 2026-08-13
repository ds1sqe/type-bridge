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

const ORDERED_RUNTIME_SOURCE_SUFFIX: &[u8] = br#"

# Successor resource for ordered collection projections.
_TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 2


def _install_runtime_projection_with_authority(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[ModelBase], type[ReferenceBase] | None]],
) -> None:
    global _package_models, _package_runtime_projection
    if _package_runtime_projection is not None:
        raise RuntimeError("generated package runtime projection is already installed")
    from type_bridge._runtime_projection import (
        install_runtime_projection_with_authority as install_native,
    )
    from ._authority import SCHEMA_AUTHORITY_BYTES

    installed = install_native(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
        SCHEMA_AUTHORITY_BYTES,
    )
    for model, _reference in models:
        model.__runtime_projection__ = installed
    _package_models = tuple(model for model, _reference in models)
    _package_runtime_projection = installed
    from ._query import install_projection

    install_projection(installed)


globals()["install_runtime_projection"] = _install_runtime_projection_with_authority
"#;

/// Python package emitter with feature-selected legacy and ordered evidence ledgers.
#[derive(Clone, Copy, Debug, Default)]
pub struct PythonEmitter;

impl PythonEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact legacy unordered projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::python_v1()]
    }

    /// Return projection-handler evidence selected for the resolved schema features.
    #[must_use]
    pub fn generator_handlers_for(&self, schema: &ResolvedSchema) -> Vec<ProjectionHandler> {
        self.handlers_for_ordered(resolved_schema_uses_ordered_collections(schema))
    }

    /// Hash its exact legacy unordered fixed output resources.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.resources_for_ordered(false)
    }

    /// Hash the exact fixed output resources selected for the resolved schema features.
    pub fn code_resources_for(
        &self,
        schema: &ResolvedSchema,
    ) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.resources_for_ordered(resolved_schema_uses_ordered_collections(schema))
    }

    fn handlers_for_ordered(&self, ordered: bool) -> Vec<ProjectionHandler> {
        if ordered {
            vec![ProjectionHandler::python_v2()]
        } else {
            self.generator_handlers()
        }
    }

    fn resources_for_ordered(&self, ordered: bool) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let runtime_source = ordered_runtime_source(ordered);
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PY_TYPED_ID, PY_TYPED)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, &runtime_source)?,
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
        let ordered = projection_uses_ordered_collections(projection);
        let handlers = self.handlers_for_ordered(ordered);
        let resources = self.resources_for_ordered(ordered)?;
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
        let authority = embedded_authority(projection, authority)?;
        let runtime_source = ordered_runtime_source(ordered);
        render::render(
            projection,
            &authority,
            &runtime_source,
            RUNTIME_STUB,
            QUERY_SOURCE,
            QUERY_STUB,
            PY_TYPED,
        )
    }
}

fn ordered_runtime_source(ordered: bool) -> Vec<u8> {
    resource_with_suffix(
        RUNTIME_SOURCE,
        ordered.then_some(ORDERED_RUNTIME_SOURCE_SUFFIX),
    )
}

fn resource_with_suffix(resource: &[u8], suffix: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(resource.len() + suffix.map_or(0, <[u8]>::len));
    bytes.extend_from_slice(resource);
    if let Some(suffix) = suffix {
        bytes.extend_from_slice(suffix);
    }
    bytes
}
