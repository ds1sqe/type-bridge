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

const TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 3 as const;

type OrderedPackagePathSegment =
  | { readonly kind: "type"; readonly typeKey: string }
  | { readonly kind: "field"; readonly name: string }
  | { readonly kind: "role"; readonly name: string }
  | { readonly kind: "index"; readonly value: number };

function orderedMemberKind(
  member: CreateMemberDefinition,
): "field" | "role" {
  return member.accepted === undefined ? "field" : "role";
}

function rejectForeignOrderedValue(
  value: unknown,
  path: readonly OrderedPackagePathSegment[],
  completeRead = false,
): void {
  assertRecord(value, "ordered generated member");
  const typeKey = value["__typebridgeModel"];
  const form = value["__typebridgeForm"];
  const entry = typeof typeKey === "string" ? runtimeModels.get(typeKey) : undefined;
  const localBrand =
    form === "complete"
      ? Object.getOwnPropertyDescriptor(value, COMPLETE_BRAND)?.value
      : form === "reference"
        ? Object.getOwnPropertyDescriptor(value, REFERENCE_BRAND)?.value
        : undefined;
  if (
    typeof typeKey !== "string" ||
    entry === undefined ||
    localBrand !== typeKey
  ) {
    requireProjection().rejectGeneratedTokenPackageMismatch(
      JSON.stringify(path),
    );
    return;
  }
  if (entry.definition.valueType !== null) {
    return;
  }
  const members =
    form === "complete"
      ? completeRead
        ? entry.definition.completeMembers
        : entry.definition.createMembers
      : entry.definition.referenceKeys.map((name) => ({
          name,
          multiplicity: {
            cardinality: { kind: "cardinality" as const, min: "1", max: "1" },
            required: true,
            container: "scalar" as const,
          },
        }));
  for (const member of members) {
    const nested = value[member.name];
    if (nested === undefined || nested === null) {
      continue;
    }
    const values: readonly unknown[] = Array.isArray(nested) ? nested : [nested];
    values.forEach((candidate, index) =>
      rejectForeignOrderedValue(
        candidate,
        [
          ...path,
          { kind: "type", typeKey },
          { kind: orderedMemberKind(member), name: member.name },
          { kind: "index", value: index },
        ],
        completeRead,
      ),
    );
  }
}

function validateOrderedMemberPackages(
  definition: ModelDefinition<string, object, object>,
  input: unknown,
  completeRead = false,
): void {
  if (definition.valueType !== null) {
    return;
  }
  assertRecord(input, `${definition.name}.create result`);
  for (const member of definition.createMembers) {
    const value = input[member.name];
    if (value === undefined || value === null) {
      continue;
    }
    const values: readonly unknown[] = Array.isArray(value) ? value : [value];
    values.forEach((candidate, index) =>
      rejectForeignOrderedValue(
        candidate,
        [
          { kind: "type", typeKey: definition.typeKey },
          { kind: orderedMemberKind(member), name: member.name },
          { kind: "index", value: index },
        ],
        completeRead,
      ),
    );
  }
}

function lowerOrderedHydratedValue(value: unknown): ProjectedWire {
  assertRecord(value, "ordered hydrated value");
  const typeKey = value["__typebridgeModel"];
  const form = value["__typebridgeForm"];
  if (typeof typeKey !== "string" || (form !== "complete" && form !== "reference")) {
    throw new TypeError("ordered hydrated value is not a generated projected model");
  }
  const entry = runtimeModels.get(typeKey);
  if (entry === undefined) {
    throw new TypeError("ordered hydrated value belongs to a different runtime projection");
  }
  if (form === "reference" || entry.definition.valueType !== null) {
    return lowerProjectedValue(value);
  }
  if (Object.getOwnPropertyDescriptor(value, COMPLETE_BRAND)?.value !== typeKey) {
    throw new TypeError("ordered hydrated value has no complete nominal brand");
  }
  const iid = value["iid"];
  if (iid !== null && (typeof iid !== "string" || iid.length === 0)) {
    throw new TypeError("ordered hydrated IID must be null or a non-empty string");
  }
  const values: Record<string, unknown> = {};
  for (const member of entry.definition.completeMembers) {
    const nested = value[member.name];
    values[member.name] = Array.isArray(nested)
      ? nested.map(lowerOrderedHydratedValue)
      : nested === null || nested === undefined
        ? null
        : lowerOrderedHydratedValue(nested);
  }
  return { typeKey, form, iid, value: null, values };
}

/** @internal Define an ordered model whose create facet uses common native admission. */
export function defineOrderedModel<
  Id extends string,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields extends object,
  Roles extends object,
>(
  definition: ModelDefinition<Id, Fields, Roles>,
): ModelToken<Id, Complete, CreateFactory, ReferenceFactory, Fields, Roles> {
  const original = defineModel<
    Id,
    Complete,
    CreateFactory,
    ReferenceFactory,
    Fields,
    Roles
  >(definition);
  const descriptors = Object.getOwnPropertyDescriptors(original) as Record<
    PropertyKey,
    PropertyDescriptor
  >;
  if (typeof original.create === "function") {
    const create = original.create as (input: unknown) => Complete;
    descriptors["create"] = {
      ...descriptors["create"],
      value: (input: unknown): Complete => {
        const result = create(input);
        validateOrderedMemberPackages(definition, result);
        if (definition.valueType === null) {
          requireProjection().validateCreateJson(
            definition.typeKey,
            JSON.stringify(lowerProjectedValue(result)),
          );
        }
        return result;
      },
    };
  }
  const hydrate = original[HYDRATE_COMPLETE_BRAND] as (
    iid: string | null,
    input: unknown,
  ) => Complete;
  descriptors[HYDRATE_COMPLETE_BRAND] = {
    ...descriptors[HYDRATE_COMPLETE_BRAND],
    value: (iid: string | null, input: unknown): Complete => {
      if (definition.valueType !== null) {
        requireProjection().validateHydratedAttributeValueJson(
          definition.typeKey,
          JSON.stringify(scalarToWire(definition.valueType, input)),
        );
      }
      const result = hydrate(iid, input);
      validateOrderedMemberPackages(
        { ...definition, createMembers: definition.completeMembers },
        result,
        true,
      );
      if (definition.valueType === null) {
        requireProjection().validateThingJson(
          definition.typeKey,
          JSON.stringify(lowerOrderedHydratedValue(result)),
        );
      }
      return result;
    },
  };
  const token = Object.create(
    Object.getPrototypeOf(original),
    descriptors,
  ) as ModelToken<
    Id,
    Complete,
    CreateFactory,
    ReferenceFactory,
    Fields,
    Roles
  >;
  const entry = runtimeModels.get(definition.typeKey);
  if (entry === undefined || entry.token !== original) {
    throw new TypeError("ordered model token registration is inconsistent");
  }
  runtimeModels.set(definition.typeKey, { ...entry, token });
  Object.freeze(token);
  return token;
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_runtime_source_remains_byte_exact() {
        assert_eq!(runtime_source(false), RUNTIME_SOURCE);
    }

    #[test]
    fn ordered_runtime_source_installs_whole_create_validation() {
        let source = String::from_utf8(runtime_source(true)).unwrap();

        assert!(source.starts_with(std::str::from_utf8(RUNTIME_SOURCE).unwrap()));
        assert!(source.contains("export function defineOrderedModel<"));
        assert!(source.contains("Object.getOwnPropertyDescriptors(original)"));
        assert!(source.contains("requireProjection().validateCreateJson("));
        assert!(source.contains("descriptors[HYDRATE_COMPLETE_BRAND]"));
        assert!(source.contains("requireProjection().validateHydratedAttributeValueJson("));
        assert!(source.contains("requireProjection().validateThingJson("));
        assert!(!source.contains("__materializeOrderedThing"));
        assert!(source.contains("rejectGeneratedTokenPackageMismatch("));
    }
}
