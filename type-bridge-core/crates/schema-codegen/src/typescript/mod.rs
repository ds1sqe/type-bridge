mod render;
mod reserved;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority};

use crate::{
    GeneratedPackage, embedded_authority, invalid, projection_uses_ordered_collections,
    resolved_schema_uses_ordered_collections,
};

const PACKAGE_JSON: &[u8] = include_bytes!("package.json");
const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.ts");
const TSCONFIG_JSON: &[u8] = include_bytes!("tsconfig.json");

const PACKAGE_JSON_ID: &str = "typebridge.generator.typescript.package-json";
const RUNTIME_SOURCE_ID: &str = "typebridge.generator.typescript.runtime-source";
const TSCONFIG_JSON_ID: &str = "typebridge.generator.typescript.tsconfig-json";

const ORDERED_RUNTIME_SOURCE_SUFFIX: &[u8] = br#"

/** Canonical collection semantics present only in ordered projection resources. */
export interface Multiplicity {
  readonly collection_mode?: "ordered_list";
}

const TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 2 as const;

/** @internal Install authority-backed evidence for an ordered generated package. */
export function __installOrderedRuntimeProjectionPackage(
  projectionJson: string,
  semanticFingerprintJson: string,
  projectionFingerprintJson: string,
  tokens: readonly object[],
  schemaAuthorityJson: string,
): void {
  if (installedProjection !== null) {
    throw new TypeError("generated runtime projection is already installed");
  }
  const entries = tokens.map((token) => {
    const entry = [...runtimeModels.values()].find(
      (candidate) => candidate.token === token,
    );
    if (entry === undefined) {
      throw new TypeError(
        "runtime projection registration contains an unknown model token",
      );
    }
    return entry;
  });
  const authority = installGeneratedSchemaAuthority({
    schemaAuthorityJson,
    semanticFingerprintJson,
  });
  const projection = installRuntimeProjection({
    schemaAuthorityJson,
    projectionJson,
    semanticFingerprintJson,
    projectionFingerprintJson,
    bindings: entries.map(({ token }) => ({
      typeKey: token.typeKey,
      targetName: token.name,
      create: typeof token.create === "function",
      reference: typeof token.reference === "function",
    })),
  });
  installedProjection = projection;
  installedQueryAuthority = authority;
}
"#;

/// TypeScript emitter with feature-selected legacy and ordered evidence ledgers.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptEmitter;

impl TypeScriptEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return its exact legacy unordered projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::typescript_v1()]
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
            vec![ProjectionHandler::typescript_v2()]
        } else {
            self.generator_handlers()
        }
    }

    fn resources_for_ordered(&self, ordered: bool) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let runtime_source = runtime_source(ordered);
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PACKAGE_JSON_ID, PACKAGE_JSON)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, &runtime_source)?,
            CodeResourceDigest::from_bytes(TSCONFIG_JSON_ID, TSCONFIG_JSON)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit one deterministic ESM package bound to exact verified schema authority.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
    ) -> Result<GeneratedPackage, Diagnostic> {
        let ordered = projection_uses_ordered_collections(projection);
        let handlers = self.handlers_for_ordered(ordered);
        let resources = self.resources_for_ordered(ordered)?;
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
        let authority = embedded_authority(projection, authority)?;
        let runtime_source = runtime_source(ordered);
        render::render(
            projection,
            &authority,
            &runtime_source,
            PACKAGE_JSON,
            TSCONFIG_JSON,
        )
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
