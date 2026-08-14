mod render;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CNamingPolicy, CodeResourceDigest, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority, project};

use crate::{
    GeneratedPackage, embedded_authority, invalid, projection_uses_ordered_collections,
    resolved_schema_uses_ordered_collections,
};

const CMAKE_TEMPLATE: &[u8] = include_bytes!("CMakeLists.txt.in");
const CMAKE_PACKAGE_CONFIG_TEMPLATE: &[u8] = include_bytes!("SchemaConfig.cmake.in");
const PKG_CONFIG_TEMPLATE: &[u8] = include_bytes!("schema.pc.in");
const ORDERED_CMAKE_TEMPLATE: &[u8] = include_bytes!("CMakeLists.abi-1-4.txt.in");
const ORDERED_CMAKE_PACKAGE_CONFIG_TEMPLATE: &[u8] =
    include_bytes!("SchemaConfig.abi-1-4.cmake.in");
const ORDERED_PKG_CONFIG_TEMPLATE: &[u8] = include_bytes!("schema.abi-1-4.pc.in");

const CMAKE_TEMPLATE_ID: &str = "typebridge.generator.c.cmake-template";
const CMAKE_PACKAGE_CONFIG_TEMPLATE_ID: &str =
    "typebridge.generator.c.cmake-package-config-template";
const PKG_CONFIG_TEMPLATE_ID: &str = "typebridge.generator.c.pkg-config-template";

/// C schema-package emitter with feature-selected legacy and ordered evidence ledgers.
#[derive(Clone, Copy, Debug, Default)]
pub struct CEmitter;

impl CEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact legacy unordered projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::c_v2()]
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
            vec![ProjectionHandler::c_v3()]
        } else {
            self.generator_handlers()
        }
    }

    fn resources_for_ordered(&self, ordered: bool) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let (cmake_template, cmake_package_config_template, pkg_config_template) =
            selected_templates(ordered);
        let mut resources = vec![
            CodeResourceDigest::from_bytes(CMAKE_TEMPLATE_ID, cmake_template)?,
            CodeResourceDigest::from_bytes(
                CMAKE_PACKAGE_CONFIG_TEMPLATE_ID,
                cmake_package_config_template,
            )?,
            CodeResourceDigest::from_bytes(PKG_CONFIG_TEMPLATE_ID, pkg_config_template)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit one deterministic C source package bound to exact verified authority.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
    ) -> Result<GeneratedPackage, Diagnostic> {
        let ordered = projection_uses_ordered_collections(projection);
        let handlers = self.handlers_for_ordered(ordered);
        let resources = self.resources_for_ordered(ordered)?;
        if projection.target() != BindingTarget::C
            || projection.config().c_naming_policy() != Some(CNamingPolicy::TypeBridgeV1)
            || projection.config().c_symbol_prefix().is_none()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "c_emitter_evidence_mismatch",
                "projection target, config, handler, or resource evidence does not match this emitter",
            ));
        }
        let embedded = embedded_authority(projection, authority)?;
        let canonical_projection = project(
            authority.resolved_schema(),
            BindingTarget::C,
            projection.config(),
            &handlers,
            &resources,
        )
        .map_err(|_| {
            invalid(
                "c_emitter_projection_rejected",
                "verified schema authority could not be projected through the shipped C policy",
            )
        })?;
        if &canonical_projection != projection {
            return Err(invalid(
                "c_emitter_projection_mismatch",
                "C projection is not the exact shipped projection of the verified schema authority",
            ));
        }
        let (cmake_template, cmake_package_config_template, pkg_config_template) =
            selected_templates(ordered);
        render::render(
            projection,
            &embedded,
            cmake_template,
            cmake_package_config_template,
            pkg_config_template,
        )
    }
}

fn selected_templates(ordered: bool) -> (&'static [u8], &'static [u8], &'static [u8]) {
    if ordered {
        (
            ORDERED_CMAKE_TEMPLATE,
            ORDERED_CMAKE_PACKAGE_CONFIG_TEMPLATE,
            ORDERED_PKG_CONFIG_TEMPLATE,
        )
    } else {
        (
            CMAKE_TEMPLATE,
            CMAKE_PACKAGE_CONFIG_TEMPLATE,
            PKG_CONFIG_TEMPLATE,
        )
    }
}
