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
_TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 3

from dataclasses import dataclass, field
from type_bridge_core import QueryCancellation, QueryExecutionResourceLimits


class DirectTlsMode(StrEnum):
    DISABLED = "disabled"
    NATIVE_ROOTS = "native_roots"
    CUSTOM_ROOT = "custom_root"


@dataclass(frozen=True, slots=True, repr=False)
class DirectConnectionPolicy:
    endpoint: str
    database: str
    username: str = "admin"
    password: str = field(default="password", repr=False)
    http_port: int = 8000
    tls: DirectTlsMode = DirectTlsMode.DISABLED
    tls_root_ca: str | None = None
    connection_limits: QueryExecutionResourceLimits | None = None
    answer_limits: QueryExecutionResourceLimits | None = None

    def __repr__(self) -> str:
        return "DirectConnectionPolicy([REDACTED])"


def connect(
    policy: object,
    *,
    cancellation: QueryCancellation | None = None,
) -> Database:
    """Open a database owned and verified by this generated package."""
    if not isinstance(policy, DirectConnectionPolicy):
        raise TypeError("connect requires this generated package's DirectConnectionPolicy")
    projection = _package_runtime_projection
    if projection is None:
        raise RuntimeError("generated package runtime projection is not installed")
    native = projection.connect_direct(
        policy.endpoint,
        policy.database,
        policy.username,
        policy.password,
        policy.http_port,
        policy.tls.value,
        policy.tls_root_ca,
        policy.connection_limits,
        policy.answer_limits,
        cancellation,
    )
    from type_bridge._rust_runtime import database_from_rust

    return database_from_rust(policy.endpoint, policy.database, native)


def _install_runtime_projection_with_authority(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[ModelBase], type[ReferenceBase] | None]],
    schema_authority: bytes,
    structs: Sequence[type[StructValueBase]] = (),
) -> None:
    global _package_models, _package_runtime_projection
    if _package_runtime_projection is not None:
        raise RuntimeError("generated package runtime projection is already installed")
    from type_bridge._runtime_projection import (
        install_runtime_projection_with_authority as install_native,
    )
    from ._authority import SCHEMA_AUTHORITY_BYTES
    if schema_authority != SCHEMA_AUTHORITY_BYTES:
        raise ValueError("generated schema authority does not match the package authority")

    installed = install_native(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        models,
        schema_authority,
        structs=structs,
    )
    for model, _reference in models:
        model.__runtime_projection__ = installed
    for structure in structs:
        structure.__runtime_projection__ = installed
    _package_models = tuple(model for model, _reference in models)
    _package_runtime_projection = installed
    from ._query import install_projection

    install_projection(installed)


globals()["install_runtime_projection"] = _install_runtime_projection_with_authority


class _CreateValidator(Protocol):
    def validate_create(
        self,
        model: type[ModelBase],
        instance: ModelBase,
    ) -> None: ...


_LegacyProjectedField = _ProjectedField


class _WholeCreateProjectedField(_LegacyProjectedField):
    def __set__(self, instance: ModelBase, value: object) -> None:
        values = instance.runtime_values()
        had_value = self.name in values
        previous = values.get(self.name)
        _LegacyProjectedField.__set__(self, instance, value)
        try:
            projection = cast(_CreateValidator, type(instance).__runtime_projection__)
            projection.validate_create(type(instance), instance)
        except BaseException:
            if had_value:
                values[self.name] = previous
            else:
                del values[self.name]
            raise


globals()["_ProjectedField"] = _WholeCreateProjectedField


def _initialize_model_with_validation(
    instance: ModelBase,
    values: Mapping[str, object],
) -> None:
    instance.initialize_runtime_values({})
    for name, value in values.items():
        descriptor: object | None = None
        for owner in type(instance).__mro__:
            if name in owner.__dict__:
                descriptor = owner.__dict__[name]
                break
        if not isinstance(descriptor, _LegacyProjectedField):
            raise TypeError("generated model initializer requires a projected field")
        _LegacyProjectedField.__set__(descriptor, instance, value)
    projection = cast(_CreateValidator, type(instance).__runtime_projection__)
    projection.validate_create(type(instance), instance)


globals()["initialize_model"] = _initialize_model_with_validation


class _WholeCreateProjectedModelManager[ModelT: ModelBase](
    ProjectedModelManager[ModelT]
):
    def update(self, instance: ModelT) -> ModelT:
        self._run_pre(CrudEvent.PRE_UPDATE, instance)
        projection = cast(_CreateValidator, type(instance).__runtime_projection__)
        projection.validate_create(type(instance), instance)
        if self._resolve_instance_iid(instance):
            result = self._native.update(instance)
        else:
            result = instance
        self._run_post(CrudEvent.POST_UPDATE, result)
        return result


globals()["ProjectedModelManager"] = _WholeCreateProjectedModelManager
"#;

const ORDERED_RUNTIME_STUB_SUFFIX: &[u8] = br#"

from dataclasses import dataclass
from type_bridge_core import QueryCancellation, QueryExecutionResourceLimits

class DirectTlsMode(StrEnum):
    DISABLED = "disabled"
    NATIVE_ROOTS = "native_roots"
    CUSTOM_ROOT = "custom_root"

@dataclass(frozen=True, slots=True, repr=False)
class DirectConnectionPolicy:
    endpoint: str
    database: str
    username: str = ...
    password: str = ...
    http_port: int = ...
    tls: DirectTlsMode = ...
    tls_root_ca: str | None = ...
    connection_limits: QueryExecutionResourceLimits | None = ...
    answer_limits: QueryExecutionResourceLimits | None = ...

def connect(
    policy: DirectConnectionPolicy,
    *,
    cancellation: QueryCancellation | None = ...,
) -> Database: ...
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
        let runtime_stub = ordered_runtime_stub(ordered);
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PY_TYPED_ID, PY_TYPED)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, &runtime_source)?,
            CodeResourceDigest::from_bytes(RUNTIME_STUB_ID, &runtime_stub)?,
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
        let runtime_stub = ordered_runtime_stub(ordered);
        render::render(
            projection,
            &authority,
            &runtime_source,
            &runtime_stub,
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

fn ordered_runtime_stub(ordered: bool) -> Vec<u8> {
    resource_with_suffix(RUNTIME_STUB, ordered.then_some(ORDERED_RUNTIME_STUB_SUFFIX))
}

fn resource_with_suffix(resource: &[u8], suffix: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(resource.len() + suffix.map_or(0, <[u8]>::len));
    bytes.extend_from_slice(resource);
    if let Some(suffix) = suffix {
        bytes.extend_from_slice(suffix);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn legacy_runtime_source_remains_byte_exact() {
        assert_eq!(ordered_runtime_source(false), RUNTIME_SOURCE);
        assert_eq!(ordered_runtime_stub(false), RUNTIME_STUB);
    }

    #[test]
    fn ordered_runtime_source_installs_create_validation_override() {
        let source = String::from_utf8(ordered_runtime_source(true)).unwrap();

        assert!(source.contains("_TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 3"));
        assert!(source.contains("projection.validate_create(type(instance), instance)"));
        assert!(source.contains("except BaseException:"));
        assert!(source.contains("for owner in type(instance).__mro__:"));
        assert!(source.contains("_LegacyProjectedField.__set__(descriptor, instance, value)"));
        assert!(source.contains("class _WholeCreateProjectedModelManager"));
        assert!(source.contains("class DirectConnectionPolicy:"));
        assert!(source.contains("password: str = field(default=\"password\", repr=False)"));
        assert!(source.contains("DirectConnectionPolicy([REDACTED])"));
        assert!(source.contains("def connect("));
        assert!(source.contains("projection.connect_direct("));
        assert!(
            source.contains("globals()[\"initialize_model\"] = _initialize_model_with_validation")
        );
        assert!(
            source.contains(
                "globals()[\"ProjectedModelManager\"] = _WholeCreateProjectedModelManager"
            )
        );
    }

    #[test]
    fn ordered_evidence_changes_only_the_runtime_resources() {
        let emitter = PythonEmitter::new();
        let legacy = emitter.resources_for_ordered(false).unwrap();
        let ordered = emitter.resources_for_ordered(true).unwrap();
        let legacy_by_id = legacy
            .iter()
            .map(|resource| (resource.id(), resource))
            .collect::<std::collections::BTreeMap<_, _>>();
        let changed = ordered
            .iter()
            .filter(|resource| legacy_by_id.get(resource.id()) != Some(resource))
            .map(|resource| resource.id().as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            changed,
            BTreeSet::from([
                "typebridge.generator.python.runtime-source",
                "typebridge.generator.python.runtime-stub",
            ]),
        );
    }
}
