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

const LEGACY_MATERIALIZE_THING_SOURCE: &[u8] = br#"function materializeThing(
  state: QueryState,
  thing: RuntimeProjectionMatchThing,
): unknown {
  const encoded = state.projection.materializeMatchThingJson(thing);
  return hydrateProjectedValue(
    parseProjectedWire(JSON.parse(encoded) as unknown),
  );
}"#;

const ORDERED_MATERIALIZE_THING_SOURCE: &[u8] = br#"function materializeThing(
  state: QueryState,
  thing: RuntimeProjectionMatchThing,
): unknown {
  return hydrateOrderedProjectedEnvelope(
    state.projection.materializeMatchThingProjected(thing),
  );
}"#;

const ORDERED_RUNTIME_SOURCE_SUFFIX: &[u8] = br#"

/** Canonical collection semantics present only in ordered projection resources. */
export interface Multiplicity {
  readonly collection_mode?: "ordered_list";
}

const TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 3 as const;

type OrderedProjectedEnvelope = ReturnType<
  NativeProjectedManager["insertProjected"]
>;
type OrderedFacadeProof = ReturnType<OrderedProjectedEnvelope["rootProof"]>;
const orderedFacadeProofs = new WeakMap<object, OrderedFacadeProof>();

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

function orderedRoleProofs(
  typeKey: string,
  value: unknown,
): (OrderedFacadeProof | null)[] {
  assertRecord(value, "ordered projected create");
  const entry = runtimeModels.get(typeKey);
  if (entry === undefined) {
    throw new TypeError("ordered projected manager has no model definition");
  }
  const proofs: (OrderedFacadeProof | null)[] = [];
  for (const member of entry.definition.createMembers) {
    if (member.accepted === undefined) continue;
    const nested = value[member.name];
    if (nested === undefined || nested === null) continue;
    const values: readonly unknown[] = Array.isArray(nested) ? nested : [nested];
    for (const candidate of values) {
      assertRecord(candidate, `ordered role ${member.name}`);
      proofs.push(orderedFacadeProofs.get(candidate) ?? null);
    }
  }
  return proofs;
}

function hydrateOrderedProjectedEnvelope<Complete>(
  envelope: OrderedProjectedEnvelope,
  expectedTypeKey?: string,
): Complete {
  const wire = parseProjectedWire(JSON.parse(envelope.json) as unknown);
  const hydrated = hydrateProjectedValue(wire, expectedTypeKey);
  assertRecord(hydrated, "ordered projected hydration");
  const entry = runtimeModels.get(wire.typeKey);
  if (entry === undefined || wire.form !== "complete") {
    throw new TypeError("ordered projected envelope has no complete root model");
  }
  const retained: [object, OrderedFacadeProof][] = [];
  for (const member of entry.definition.completeMembers) {
    if (member.accepted === undefined) continue;
    const nested = hydrated[member.name];
    if (nested === undefined || nested === null) continue;
    const values: readonly unknown[] = Array.isArray(nested) ? nested : [nested];
    values.forEach((candidate, index) => {
      assertRecord(candidate, `ordered hydrated role ${member.name}`);
      retained.push([candidate, envelope.roleProof(member.name, index)]);
    });
  }
  retained.push([hydrated, envelope.rootProof()]);
  for (const [facade, proof] of retained) {
    orderedFacadeProofs.set(facade, proof);
  }
  return hydrated as Complete;
}

function orderedProjectedManager<Complete>(
  typeKey: string,
  connection: RuntimeProjectionConnection,
): ProjectedModelManager<Complete> {
  const projection = requireProjection();
  return orderedProjectedManagerForNative(
    typeKey,
    projection.manager(typeKey, connection),
  );
}

function orderedProjectedManagerForNative<Complete>(
  typeKey: string,
  native: NativeProjectedManager,
): ProjectedModelManager<Complete> {
  const legacy = projectedManagerForNative<Complete>(typeKey, native);
  const exactWire = (instance: Complete, operation: string): ProjectedWire => {
    const wire = lowerProjectedValue(instance);
    if (wire.typeKey !== typeKey || wire.form !== "complete") {
      throw new TypeError(
        `projected ${operation} requires the manager's exact complete model`,
      );
    }
    return wire;
  };
  return Object.freeze({
    insert(instance: Complete): Complete {
      const wire = exactWire(instance, "insert");
      return hydrateOrderedProjectedEnvelope(
        native.insertProjected(
          JSON.stringify(wire),
          orderedRoleProofs(typeKey, instance),
        ),
        typeKey,
      );
    },
    insertMany(instances: readonly Complete[]): readonly Complete[] {
      return legacy.insertMany(instances);
    },
    put(instance: Complete): Complete {
      const wire = exactWire(instance, "put");
      return hydrateOrderedProjectedEnvelope(
        native.putProjected(
          JSON.stringify(wire),
          orderedRoleProofs(typeKey, instance),
        ),
        typeKey,
      );
    },
    putMany(instances: readonly Complete[]): readonly Complete[] {
      return legacy.putMany(instances);
    },
    update(iid: string, replacement: Complete): Complete {
      if (typeof iid !== "string" || iid.length === 0) {
        throw new TypeError(
          "projected manager update requires a non-empty TypeDB IID",
        );
      }
      const wire = exactWire(replacement, "update");
      return hydrateOrderedProjectedEnvelope(
        native.updateProjected(
          iid,
          JSON.stringify(wire),
          orderedRoleProofs(typeKey, replacement),
        ),
        typeKey,
      );
    },
    delete(instanceOrIid: Complete | string): void {
      legacy.delete(instanceOrIid);
    },
    filter(
      filters: Readonly<Record<string, ProjectedManagerFilterValue>>,
    ): ProjectedModelManager<Complete> {
      if (
        filters === null ||
        typeof filters !== "object" ||
        Array.isArray(filters)
      ) {
        throw new TypeError(
          "projected manager filters require a string-keyed object",
        );
      }
      const lowered: Record<string, unknown> = {};
      for (const [name, value] of Object.entries(filters)) {
        lowered[name] = lowerManagerFilterValue(value);
      }
      return orderedProjectedManagerForNative(
        typeKey,
        native.filterJson(JSON.stringify(lowered)),
      );
    },
    getByIid(iid: string): Complete | null {
      const envelope = native.getByIidProjected(iid);
      return envelope === null
        ? null
        : hydrateOrderedProjectedEnvelope(envelope, typeKey);
    },
    all(): readonly Complete[] {
      return legacy.all();
    },
    first(): Complete | null {
      return legacy.first();
    },
    count(): bigint {
      return legacy.count();
    },
    exists(): boolean {
      return legacy.exists();
    },
  });
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
  descriptors["manager"] = {
    ...descriptors["manager"],
    value: (
      connection: RuntimeProjectionConnection,
    ): ProjectedModelManager<Complete> =>
      orderedProjectedManager(definition.typeKey, connection),
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
  const authority = installGeneratedSchemaAuthority({
    schemaAuthorityJson,
    semanticFingerprintJson,
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
        let runtime_source = runtime_source(ordered)?;
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
        let runtime_source = runtime_source(ordered)?;
        render::render(
            projection,
            &authority,
            &runtime_source,
            PACKAGE_JSON,
            TSCONFIG_JSON,
        )
    }
}

fn runtime_source(ordered: bool) -> Result<Vec<u8>, Diagnostic> {
    if !ordered {
        return Ok(RUNTIME_SOURCE.to_vec());
    }
    ordered_runtime_source(RUNTIME_SOURCE)
}

fn ordered_runtime_source(base: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    let matches = base
        .windows(LEGACY_MATERIALIZE_THING_SOURCE.len())
        .enumerate()
        .filter_map(|(offset, candidate)| {
            (candidate == LEGACY_MATERIALIZE_THING_SOURCE).then_some(offset)
        })
        .collect::<Vec<_>>();
    let [offset] = matches.as_slice() else {
        return Err(invalid(
            "typescript_ordered_runtime_anchor_mismatch",
            format!(
                "ordered TypeScript runtime requires exactly one legacy materialization anchor, found {}",
                matches.len()
            ),
        ));
    };
    let tail = offset + LEGACY_MATERIALIZE_THING_SOURCE.len();
    let mut bytes = Vec::with_capacity(
        base.len() - LEGACY_MATERIALIZE_THING_SOURCE.len()
            + ORDERED_MATERIALIZE_THING_SOURCE.len()
            + ORDERED_RUNTIME_SOURCE_SUFFIX.len(),
    );
    bytes.extend_from_slice(&base[..*offset]);
    bytes.extend_from_slice(ORDERED_MATERIALIZE_THING_SOURCE);
    bytes.extend_from_slice(&base[tail..]);
    bytes.extend_from_slice(ORDERED_RUNTIME_SOURCE_SUFFIX);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_runtime_source_remains_byte_exact() {
        assert_eq!(runtime_source(false).unwrap(), RUNTIME_SOURCE);
    }

    #[test]
    fn ordered_runtime_source_installs_whole_create_validation() {
        let source = String::from_utf8(runtime_source(true).unwrap()).unwrap();

        assert!(!source.contains(std::str::from_utf8(LEGACY_MATERIALIZE_THING_SOURCE).unwrap()));
        assert_eq!(
            source
                .matches(std::str::from_utf8(ORDERED_MATERIALIZE_THING_SOURCE).unwrap())
                .count(),
            1
        );
        assert!(source.contains("export function defineOrderedModel<"));
        assert!(source.contains("Object.getOwnPropertyDescriptors(original)"));
        assert!(source.contains("requireProjection().validateCreateJson("));
        assert!(source.contains("descriptors[HYDRATE_COMPLETE_BRAND]"));
        assert!(source.contains("requireProjection().validateHydratedAttributeValueJson("));
        assert!(source.contains("requireProjection().validateThingJson("));
        assert!(!source.contains("materializeOrderedThing"));
        assert!(source.contains("rejectGeneratedTokenPackageMismatch("));
        let successor = &source[source
            .find("/** Canonical collection semantics present only in ordered projection resources. */")
            .unwrap()..];
        let projection = successor
            .find("const projection = installRuntimeProjection({")
            .unwrap();
        let authority = successor
            .find("const authority = installGeneratedSchemaAuthority({")
            .unwrap();
        assert!(projection < authority);
    }

    #[test]
    fn ordered_runtime_source_anchor_must_be_exactly_unique() {
        let missing = ordered_runtime_source(b"function materializeThing() {}").unwrap_err();
        assert_eq!(
            missing.code().as_str(),
            "typescript_ordered_runtime_anchor_mismatch"
        );

        let duplicate = [
            LEGACY_MATERIALIZE_THING_SOURCE,
            b"\n",
            LEGACY_MATERIALIZE_THING_SOURCE,
        ]
        .concat();
        let duplicate = ordered_runtime_source(&duplicate).unwrap_err();
        assert_eq!(
            duplicate.code().as_str(),
            "typescript_ordered_runtime_anchor_mismatch"
        );
    }

    #[test]
    fn ordered_runtime_changes_only_the_runtime_resource() {
        let emitter = TypeScriptEmitter::new();
        let legacy = emitter.resources_for_ordered(false).unwrap();
        let ordered = emitter.resources_for_ordered(true).unwrap();
        let changed = legacy
            .iter()
            .zip(&ordered)
            .filter(|(left, right)| left != right)
            .collect::<Vec<_>>();

        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0.id().as_str(), RUNTIME_SOURCE_ID);
        assert_eq!(changed[0].1.id().as_str(), RUNTIME_SOURCE_ID);
    }
}
