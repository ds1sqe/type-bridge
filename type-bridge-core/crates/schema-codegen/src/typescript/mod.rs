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

import { projectedManagerNativeCall } from "@type-bridge/node/runtime-projection";

/** Canonical collection semantics present only in ordered projection resources. */
export interface Multiplicity {
  readonly collection_mode?: "ordered_list";
}

const TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 4 as const;

/** One exact IID plus its immutable complete replacement. */
export type ProjectedBatchUpdate<Complete> = readonly [
  iid: string,
  replacement: Complete,
];

/** Closed comparison vocabulary for canonical generated-manager filters. */
export type ProjectedManagerComparison =
  | "eq"
  | "ne"
  | "lt"
  | "lte"
  | "gt"
  | "gte";

/** Immutable canonical exact-model field-token filter. */
export interface ProjectedModelFilter<Complete, Owner extends string = string> {
  where(): ProjectedModelFilter<Complete, Owner>;
  where<Attribute extends string, Value>(
    field: FieldToken<Owner, Attribute, Value>,
    comparison: ProjectedManagerComparison,
    value: Value,
  ): ProjectedModelFilter<Complete, Owner>;
  all(): readonly Complete[];
  first(): Complete | null;
  count(): bigint;
  exists(): boolean;
}

/** Batch-complete manager surface present only in ordered successor packages. */
export interface OrderedProjectedModelManager<
  Complete,
  Owner extends string = string,
>
  extends ProjectedModelManager<Complete> {
  updateMany(
    updates: readonly ProjectedBatchUpdate<Complete>[],
  ): readonly Complete[];
  deleteMany(iids: readonly string[]): void;
  filter(
    filters: Readonly<Record<string, ProjectedManagerFilterValue>>,
  ): OrderedProjectedModelManager<Complete, Owner>;
  where(): ProjectedModelFilter<Complete, Owner>;
  where<Attribute extends string, Value>(
    field: FieldToken<Owner, Attribute, Value>,
    comparison: ProjectedManagerComparison,
    value: Value,
  ): ProjectedModelFilter<Complete, Owner>;
}

/** Model token whose manager retains the ordered successor batch surface. */
export type OrderedModelToken<
  Id extends string,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields extends object,
  Roles extends object,
> = Omit<
  ModelToken<Id, Complete, CreateFactory, ReferenceFactory, Fields, Roles>,
  "manager"
> & {
  readonly manager: (
    connection: RuntimeProjectionConnection,
  ) => OrderedProjectedModelManager<Complete, Id>;
};

type OrderedProjectedEnvelope = ReturnType<
  NativeProjectedManager["insertProjected"]
>;
type OrderedFacadeProof = NonNullable<
  Parameters<NativeProjectedManager["insertProjected"]>[1][number]
>;
const orderedFacadeProofs = new WeakMap<object, OrderedFacadeProof>();
const orderedModelDefinitions = new Map<
  string,
  ModelDefinition<string, object, object>
>();
const orderedAttributeTypeKeys = new Map<string, string>();

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

function canonicalOrderedTemporalIso(iso: string): string {
  if (
    (iso.startsWith("+") || iso.startsWith("-")) &&
    iso.length > 7 &&
    iso[7] === "-"
  ) {
    const sign = iso.slice(0, 1);
    const magnitude = iso.slice(1, 7).replace(/^0+(?=\d)/, "");
    const year = sign === "-" ? magnitude.padStart(4, "0") : magnitude;
    return `${sign}${year}${iso.slice(7)}`;
  }
  return iso;
}

function orderedScalarToWire(
  valueType: ScalarValueType,
  value: unknown,
): ScalarWire {
  if (
    valueType !== "date" &&
    valueType !== "datetime" &&
    valueType !== "datetime_tz"
  ) {
    return scalarToWire(valueType, value);
  }
  validateScalar("projected attribute value", value, valueType);
  const iso = canonicalOrderedTemporalIso((value as Date).toISOString());
  if (valueType === "date") {
    if (!iso.endsWith("T00:00:00.000Z")) {
      throw new TypeError("date attributes require UTC midnight");
    }
    return { valueType, value: iso.slice(0, iso.indexOf("T")) };
  }
  return {
    valueType,
    value: canonicalDateTime(iso, valueType === "datetime_tz"),
  };
}

function lowerOrderedProjectedValue(value: unknown): ProjectedWire {
  const wire = lowerProjectedValue(value);
  if (wire.value !== null) {
    assertRecord(value, "ordered projected attribute");
    return {
      ...wire,
      value: orderedScalarToWire(wire.value.valueType, value["value"]),
    };
  }
  assertRecord(value, "ordered projected thing");
  const values: Record<string, unknown> = {};
  for (const name of Object.keys(wire.values)) {
    const nested = value[name];
    values[name] = Array.isArray(nested)
      ? nested.map(lowerOrderedProjectedValue)
      : nested === null || nested === undefined
        ? null
        : lowerOrderedProjectedValue(nested);
  }
  return { ...wire, values };
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
    return lowerOrderedProjectedValue(value);
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
  return hydrateOrderedProjectedWire(
    wire,
    expectedTypeKey,
    () => envelope.rootProof(),
    (roleName, playerIndex) => envelope.roleProof(roleName, playerIndex),
  );
}

function hydrateOrderedProjectedWire<Complete>(
  wire: ProjectedWire,
  expectedTypeKey: string | undefined,
  rootProof: () => OrderedFacadeProof,
  roleProof: (roleName: string, playerIndex: number) => OrderedFacadeProof,
): Complete {
  const hydrated = hydrateProjectedValue(wire, expectedTypeKey);
  return retainOrderedProjectedProofs(
    wire,
    hydrated,
    rootProof,
    roleProof,
  ) as Complete;
}

function validateOrderedProjectedBatchField(
  definition: ModelDefinition<string, object, object>,
  name: string,
  value: unknown,
): void {
  if (value === null) {
    return;
  }
  const token = (definition.fields as Readonly<Record<string, unknown>>)[name];
  if (token === null || typeof token !== "object") {
    throw new TypeError(`native projected wire has no field definition for ${name}`);
  }
  const field = fieldTokenStates.get(token);
  if (field === undefined) {
    throw new TypeError(`native projected wire has an invalid field token for ${name}`);
  }
  const attributeTypeKey = orderedAttributeTypeKeys.get(field.attribute);
  if (attributeTypeKey === undefined) {
    throw new TypeError(`native projected wire has no attribute model for ${name}`);
  }
  const values: readonly unknown[] = Array.isArray(value) ? value : [value];
  for (const candidate of values) {
    assertRecord(candidate, name);
    if (
      candidate["__typebridgeModel"] !== attributeTypeKey ||
      candidate["__typebridgeForm"] !== "complete" ||
      Object.getOwnPropertyDescriptor(candidate, COMPLETE_BRAND)?.value !==
        attributeTypeKey
    ) {
      throw new TypeError(`${name} is not the field's exact attribute value`);
    }
  }
}

function hydrateOrderedProjectedBatchValue(
  wire: ProjectedWire,
  expectedTypeKey?: string,
): object {
  if (expectedTypeKey !== undefined && wire.typeKey !== expectedTypeKey) {
    throw new TypeError(
      "native projected wire returned a different concrete type",
    );
  }
  const definition = orderedModelDefinitions.get(wire.typeKey);
  if (definition === undefined) {
    throw new TypeError(
      "native projected wire references an unregistered model",
    );
  }
  if (definition.valueType !== null) {
    if (
      wire.form !== "complete" ||
      wire.iid !== null ||
      wire.value === null ||
      wire.value.valueType !== definition.valueType ||
      Object.keys(wire.values).length !== 0
    ) {
      throw new TypeError("native attribute wire has an invalid shape");
    }
    const scalar = scalarFromWire(wire.value);
    validateScalar("native projected attribute", scalar, definition.valueType);
    const result: Record<string | symbol, unknown> = {
      __typebridgeModel: wire.typeKey,
      __typebridgeForm: "complete",
      iid: null,
      value: scalar,
    };
    Object.defineProperty(result, COMPLETE_BRAND, {
      value: wire.typeKey,
    });
    return Object.freeze(result);
  }
  if (wire.value !== null) {
    throw new TypeError("native thing wire has an invalid scalar value");
  }
  const names =
    wire.form === "complete"
      ? definition.completeMembers.map((member) => member.name)
      : definition.referenceKeys;
  const receivedNames = Object.keys(wire.values);
  if (
    receivedNames.length !== names.length ||
    names.some((name) => !Object.hasOwn(wire.values, name))
  ) {
    throw new TypeError("native projected wire has an inexact member set");
  }
  const values: Record<string, unknown> = {};
  for (const name of names) {
    values[name] = hydrateOrderedProjectedBatchMember(wire.values[name]);
  }
  if (wire.form === "reference") {
    if (wire.iid === null) {
      throw new TypeError("native reference wire has no IID");
    }
    for (const name of definition.referenceKeys) {
      const value = values[name];
      if (value === null || Array.isArray(value)) {
        throw new TypeError(`native reference key ${name} is not scalar`);
      }
      validateOrderedProjectedBatchField(definition, name, value);
    }
    const result: Record<string | symbol, unknown> = {
      __typebridgeModel: wire.typeKey,
      __typebridgeForm: "reference",
      iid: wire.iid,
      ...values,
    };
    Object.defineProperty(result, REFERENCE_BRAND, {
      value: wire.typeKey,
    });
    return Object.freeze(result);
  }
  if (wire.iid === null) {
    throw new TypeError("native complete thing wire has no IID");
  }
  const result: Record<string | symbol, unknown> = {
    __typebridgeModel: wire.typeKey,
    __typebridgeForm: "complete",
    iid: wire.iid,
  };
  for (const member of definition.completeMembers) {
    const value = values[member.name];
    validateMultiplicity(member.name, value, member.multiplicity);
    if (member.accepted !== undefined) {
      validateAccepted(member.name, value, member.accepted);
    }
    if (Object.hasOwn(definition.fields, member.name)) {
      validateOrderedProjectedBatchField(definition, member.name, value);
    }
    result[member.name] =
      value === null
        ? member.multiplicity.container === "sequence"
          ? Object.freeze([])
          : null
        : Array.isArray(value)
          ? Object.freeze([...value])
          : value;
  }
  Object.defineProperty(result, COMPLETE_BRAND, {
    value: wire.typeKey,
  });
  return Object.freeze(result);
}

function hydrateOrderedProjectedBatchMember(value: unknown): unknown {
  if (value === null) {
    return null;
  }
  if (Array.isArray(value)) {
    return Object.freeze(
      value.map((item) =>
        hydrateOrderedProjectedBatchValue(parseProjectedWire(item)),
      ),
    );
  }
  return hydrateOrderedProjectedBatchValue(parseProjectedWire(value));
}

function hydrateOrderedProjectedBatchWire<Complete>(
  wire: ProjectedWire,
  expectedTypeKey: string,
  rootProof: () => OrderedFacadeProof,
  roleProof: (roleName: string, playerIndex: number) => OrderedFacadeProof,
): Complete {
  return retainOrderedProjectedProofs(
    wire,
    hydrateOrderedProjectedBatchValue(wire, expectedTypeKey),
    rootProof,
    roleProof,
  ) as Complete;
}

function retainOrderedProjectedProofs(
  wire: ProjectedWire,
  hydrated: unknown,
  rootProof: () => OrderedFacadeProof,
  roleProof: (roleName: string, playerIndex: number) => OrderedFacadeProof,
): object {
  assertRecord(hydrated, "ordered projected hydration");
  const entry = runtimeModels.get(wire.typeKey);
  if (entry === undefined || wire.form !== "complete") {
    throw new TypeError("ordered projected result has no complete root model");
  }
  const retained: [object, OrderedFacadeProof][] = [];
  for (const member of entry.definition.completeMembers) {
    if (member.accepted === undefined) continue;
    const nested = hydrated[member.name];
    if (nested === undefined || nested === null) continue;
    const values: readonly unknown[] = Array.isArray(nested) ? nested : [nested];
    values.forEach((candidate, index) => {
      assertRecord(candidate, `ordered hydrated role ${member.name}`);
      retained.push([candidate, roleProof(member.name, index)]);
    });
  }
  retained.push([hydrated, rootProof()]);
  for (const [facade, proof] of retained) {
    orderedFacadeProofs.set(facade, proof);
  }
  return hydrated;
}

function orderedProjectedManager<Complete, Owner extends string>(
  typeKey: Owner,
  connection: RuntimeProjectionConnection,
): OrderedProjectedModelManager<Complete, Owner> {
  const projection = requireProjection();
  return orderedProjectedManagerForNative(
    typeKey,
    projection.manager(typeKey, connection),
  );
}

function orderedProjectedManagerForNative<Complete, Owner extends string>(
  typeKey: Owner,
  native: NativeProjectedManager,
  compatibilityFilter: OrderedNativeManagerFilter | null = projectedManagerNativeCall(
    () => native.managerFilter(),
  ),
): OrderedProjectedModelManager<Complete, Owner> {
  const legacy = projectedManagerForNative<Complete>(typeKey, native);
  const exactWire = (instance: Complete, operation: string): ProjectedWire => {
    const wire = lowerOrderedProjectedValue(instance);
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
      if (!Array.isArray(instances)) {
        throw new TypeError("projected manager insertMany requires an array");
      }
      return native.insertManyProjected<Complete>(instances.length, (index) => {
        const instance = instances[index] as Complete;
        const wire = exactWire(instance, "insertMany");
        return {
          instanceJson: JSON.stringify(wire),
          proofs: orderedRoleProofs(typeKey, instance),
        };
      });
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
      if (!Array.isArray(instances)) {
        throw new TypeError("projected manager putMany requires an array");
      }
      return native.putManyProjected<Complete>(instances.length, (index) => {
        const instance = instances[index] as Complete;
        const wire = exactWire(instance, "putMany");
        return {
          instanceJson: JSON.stringify(wire),
          proofs: orderedRoleProofs(typeKey, instance),
        };
      });
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
    updateMany(
      updates: readonly ProjectedBatchUpdate<Complete>[],
    ): readonly Complete[] {
      if (!Array.isArray(updates)) {
        throw new TypeError("projected manager updateMany requires an array");
      }
      return native.updateManyProjected<Complete>(updates.length, (index) => {
        const update = updates[index] as ProjectedBatchUpdate<Complete>;
        if (!Array.isArray(update) || update.length !== 2) {
          throw new TypeError(
            "projected manager updateMany requires [iid, replacement] rows",
          );
        }
        const [iid, replacement] = update;
        if (typeof iid !== "string") {
          throw new TypeError("projected manager updateMany requires string IIDs");
        }
        const wire = exactWire(replacement, "updateMany");
        return {
          iid,
          instanceJson: JSON.stringify(wire),
          proofs: orderedRoleProofs(typeKey, replacement),
        };
      });
    },
    delete(instanceOrIid: Complete | string): void {
      legacy.delete(instanceOrIid);
    },
    deleteMany(iids: readonly string[]): void {
      if (!Array.isArray(iids)) {
        throw new TypeError("projected manager deleteMany requires an array");
      }
      native.deleteManyProjected(iids.length, (index) => {
        const iid = iids[index];
        if (typeof iid !== "string") {
          throw new TypeError("projected manager deleteMany requires string IIDs");
        }
        return iid;
      });
    },
    filter(
      filters: Readonly<Record<string, ProjectedManagerFilterValue>>,
    ): OrderedProjectedModelManager<Complete, Owner> {
      if (
        filters === null ||
        typeof filters !== "object" ||
        Array.isArray(filters)
      ) {
        throw new TypeError(
          "projected manager filters require a string-keyed object",
        );
      }
      const entries = Object.entries(filters);
      const lowered: { readonly name: string; readonly value: unknown }[] = [];
      for (const [name, value] of entries) {
        lowered.push({ name, value: lowerManagerFilterValue(value) });
      }
      let nextCompatibility: OrderedNativeManagerFilter | null = null;
      const predicates = orderedCompatibilityPredicates(typeKey, entries);
      if (compatibilityFilter !== null && predicates !== null) {
        nextCompatibility = compatibilityFilter;
        for (const predicate of predicates) {
          nextCompatibility = projectedManagerNativeCall(() =>
            nextCompatibility!.andProjected(
              predicate.definition.owner,
              predicate.definition.attribute,
              predicate.comparison,
              JSON.stringify(lowerOrderedProjectedValue(predicate.value)),
            ),
          );
        }
      }
      return orderedProjectedManagerForNative(
        typeKey,
        native.filterEntriesJson(JSON.stringify(lowered)),
        nextCompatibility,
      );
    },
    where<Attribute extends string, Value>(
      field?: FieldToken<Owner, Attribute, Value>,
      comparison?: ProjectedManagerComparison,
      value?: Value,
    ): ProjectedModelFilter<Complete, Owner> {
      const filter = orderedProjectedFilterForNative<Complete, Owner>(
        typeKey,
        projectedManagerNativeCall(() => native.managerFilter()),
      );
      if (field === undefined) {
        if (comparison !== undefined || value !== undefined) {
          throw new TypeError("empty manager where accepts no comparison or value");
        }
        return filter;
      }
      if (comparison === undefined || arguments.length !== 3) {
        throw new TypeError(
          "manager where requires a field token, comparison, and exact value",
        );
      }
      return filter.where(field, comparison, value as Value);
    },
    getByIid(iid: string): Complete | null {
      const envelope = native.getByIidProjected(iid);
      return envelope === null
        ? null
        : hydrateOrderedProjectedEnvelope(envelope, typeKey);
    },
    all(): readonly Complete[] {
      return compatibilityFilter === null
        ? legacy.all()
        : orderedProjectedFilterForNative<Complete, Owner>(
            typeKey,
            compatibilityFilter,
          ).all();
    },
    first(): Complete | null {
      return legacy.first();
    },
    count(): bigint {
      return compatibilityFilter === null
        ? legacy.count()
        : projectedManagerNativeCall(() => compatibilityFilter.count());
    },
    exists(): boolean {
      return compatibilityFilter === null
        ? legacy.exists()
        : projectedManagerNativeCall(() => compatibilityFilter.exists());
    },
  });
}

type OrderedNativeManagerFilter = ReturnType<
  NativeProjectedManager["managerFilter"]
>;

function orderedProjectedFilterForNative<
  Complete,
  Owner extends string,
>(
  typeKey: Owner,
  native: OrderedNativeManagerFilter,
): ProjectedModelFilter<Complete, Owner> {
  return Object.freeze({
    where<Attribute extends string, Value>(
      field?: FieldToken<Owner, Attribute, Value>,
      comparison?: ProjectedManagerComparison,
      value?: Value,
    ): ProjectedModelFilter<Complete, Owner> {
      if (field === undefined) {
        if (comparison !== undefined || value !== undefined) {
          throw new TypeError("empty manager where accepts no comparison or value");
        }
        return orderedProjectedFilterForNative(typeKey, native);
      }
      if (comparison === undefined || arguments.length !== 3) {
        throw new TypeError(
          "manager where requires a field token, comparison, and exact value",
        );
      }
      const definition = fieldTokenStates.get(field);
      if (definition === undefined) {
        return projectedManagerNativeCall(() => native.rejectForeignFieldToken());
      }
      const wire = lowerOrderedProjectedValue(value);
      return orderedProjectedFilterForNative(
        typeKey,
        projectedManagerNativeCall(() =>
          native.andProjected(
            definition.owner,
            definition.attribute,
            comparison,
            JSON.stringify(wire),
          ),
        ),
      );
    },
    all(): readonly Complete[] {
      return Object.freeze(
        projectedManagerNativeCall(() => native.allProjected())
          .map((envelope) =>
            hydrateOrderedProjectedEnvelope<Complete>(envelope, typeKey),
          ),
      );
    },
    first(): Complete | null {
      const envelope = projectedManagerNativeCall(() => native.firstProjected());
      return envelope === null
        ? null
        : hydrateOrderedProjectedEnvelope<Complete>(envelope, typeKey);
    },
    count(): bigint {
      return projectedManagerNativeCall(() => native.count());
    },
    exists(): boolean {
      return projectedManagerNativeCall(() => native.exists());
    },
  });
}

interface OrderedCompatibilityPredicate {
  readonly definition: FieldTokenDefinition<string, string>;
  readonly comparison: ProjectedManagerComparison;
  readonly value: unknown;
}

function orderedCompatibilityPredicates(
  typeKey: string,
  entries: readonly (readonly [string, ProjectedManagerFilterValue])[],
): readonly OrderedCompatibilityPredicate[] | null {
  const model = orderedModelDefinitions.get(typeKey);
  if (model === undefined) return null;
  const fields = Object.values(model.fields)
    .map((token) => fieldTokenStates.get(token))
    .filter(
      (field): field is FieldTokenDefinition<string, string> =>
        field !== undefined,
    );
  const fieldNamed = (
    name: string,
  ): FieldTokenDefinition<string, string> | undefined =>
    fields.find((field) => {
      if (field.name === name) return true;
      try {
        return JSON.parse(field.attribute) === name;
      } catch {
        return false;
      }
    });
  const recognisedOperations = new Set([
    "eq",
    "exact",
    "ne",
    "gt",
    "gte",
    "lt",
    "lte",
    "contains",
    "startswith",
    "endswith",
    "regex",
    "like",
    "in",
    "isnull",
  ]);
  const canonicalOperations = new Set([
    "eq",
    "exact",
    "ne",
    "lt",
    "lte",
    "gt",
    "gte",
  ]);
  const predicates: OrderedCompatibilityPredicate[] = [];
  for (const [lookupName, value] of entries) {
    const direct = fieldNamed(lookupName);
    const separator = lookupName.lastIndexOf("__");
    const prefix = separator < 0 ? lookupName : lookupName.slice(0, separator);
    const suffix = separator < 0 ? "eq" : lookupName.slice(separator + 2);
    const prefixed = recognisedOperations.has(suffix)
      ? fieldNamed(prefix)
      : undefined;
    const definition = prefixed ?? direct;
    const operation = prefixed === undefined && direct !== undefined ? "eq" : suffix;
    if (
      definition === undefined ||
      !canonicalOperations.has(operation) ||
      !isProjectedModelValue(value) ||
      Array.isArray(value)
    ) {
      return null;
    }
    predicates.push({
      definition,
      comparison:
        operation === "exact"
          ? "eq"
          : (operation as ProjectedManagerComparison),
      value,
    });
  }
  return predicates;
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
): OrderedModelToken<
  Id,
  Complete,
  CreateFactory,
  ReferenceFactory,
  Fields,
  Roles
> {
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
            JSON.stringify(lowerOrderedProjectedValue(result)),
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
          JSON.stringify(orderedScalarToWire(definition.valueType, input)),
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
    ): OrderedProjectedModelManager<Complete, Id> =>
      orderedProjectedManager(definition.typeKey, connection),
  };
  const token = Object.create(
    Object.getPrototypeOf(original),
    descriptors,
  ) as OrderedModelToken<
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
  orderedModelDefinitions.set(definition.typeKey, definition);
  if (definition.valueType !== null) {
    assertRecord(definition.id, "ordered attribute definition identity");
    const label = definition.id["label"];
    if (definition.id["kind"] !== "attribute" || typeof label !== "string") {
      throw new TypeError("ordered attribute definition has an invalid identity");
    }
    orderedAttributeTypeKeys.set(JSON.stringify(label), definition.typeKey);
  }
  Object.freeze(token);
  return token;
}

const materializeOrderedProjectedBatch: NonNullable<
  Parameters<typeof installRuntimeProjection>[0]["projectedBatchMaterializer"]
> = (typeKey, ordinal, json, authority): object => {
  const wire = parseProjectedWire(JSON.parse(json) as unknown);
  return hydrateOrderedProjectedBatchWire<object>(
    wire,
    typeKey,
    () => Object.freeze({
      kind: "root" as const,
      authority,
      row: ordinal,
    }),
    (roleName, playerIndex) => Object.freeze({
      kind: "role" as const,
      authority,
      row: ordinal,
      roleName,
      playerIndex,
    }),
  );
};

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
    projectedBatchMaterializer: materializeOrderedProjectedBatch,
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
    use sha2::{Digest as _, Sha256};
    use std::collections::BTreeSet;

    fn exact_source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        assert_eq!(source.matches(start).count(), 1, "non-unique start {start}");
        let offset = source.find(start).unwrap();
        let tail = &source[offset..];
        let length = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing end {end}"));
        &tail[..length]
    }

    fn identifier_calls(source: &str) -> BTreeSet<String> {
        let normalized = source.replace("<Complete>", "").replace("<object>", "");
        let bytes = normalized.as_bytes();
        let mut calls = BTreeSet::new();
        let mut offset = 0;
        while offset < bytes.len() {
            if bytes[offset].is_ascii_alphabetic() || bytes[offset] == b'_' {
                let start = offset;
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset].is_ascii_alphanumeric() || bytes[offset] == b'_')
                {
                    offset += 1;
                }
                let mut next = offset;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                if next < bytes.len() && bytes[next] == b'(' {
                    calls.insert(normalized[start..offset].to_owned());
                }
            } else {
                offset += 1;
            }
        }
        calls
    }

    #[test]
    fn legacy_runtime_source_remains_byte_exact() {
        assert_eq!(runtime_source(false).unwrap(), RUNTIME_SOURCE);
        assert_eq!(RUNTIME_SOURCE.len(), 118_050);
        assert_eq!(
            format!("{:x}", Sha256::digest(RUNTIME_SOURCE)),
            "d457565b506c106f42f89daee4d2bf711f26420a1bfe4ae75852639387a7f120"
        );
        let resource = CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE).unwrap();
        assert_eq!(
            resource.content_fingerprint().digest().to_hex(),
            "34a67c3894c90ce2697cfefc1fc8b6d2a58773259d42a601613d1589b9b33ab0"
        );
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
        assert!(source.contains("TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 4"));
        assert!(source.contains("export type ProjectedBatchUpdate<Complete>"));
        assert!(source.contains("export interface OrderedProjectedModelManager<"));
        assert!(source.contains("export interface ProjectedModelFilter<"));
        assert!(source.contains(
            "import { projectedManagerNativeCall } from \"@type-bridge/node/runtime-projection\";"
        ));
        assert!(source.contains("const entries = Object.entries(filters);"));
        assert!(source.contains("orderedCompatibilityPredicates(typeKey, entries)"));
        assert!(!source.contains("orderedCompatibilityPredicates(typeKey, filters)"));
        assert!(source.contains("return JSON.parse(field.attribute) === name;"));
        assert!(
            source.contains("projectedManagerNativeCall(() => native.rejectForeignFieldToken())")
        );
        assert!(source.contains("export type OrderedModelToken<"));
        assert!(source.contains("native.insertManyProjected<Complete>"));
        assert!(source.contains("native.putManyProjected<Complete>"));
        assert!(source.contains("native.updateManyProjected<Complete>"));
        assert!(source.contains("native.deleteManyProjected("));
        assert!(source.contains("projectedBatchMaterializer: materializeOrderedProjectedBatch"));
        assert!(source.contains("function canonicalOrderedTemporalIso(iso: string): string"));
        assert!(
            source.contains("function lowerOrderedProjectedValue(value: unknown): ProjectedWire")
        );
        assert!(source.contains("const wire = lowerOrderedProjectedValue(instance)"));
        assert!(source.contains("JSON.stringify(lowerOrderedProjectedValue(result))"));
        assert!(
            source.contains("JSON.stringify(orderedScalarToWire(definition.valueType, input))")
        );
        assert!(source.contains("kind: \"root\" as const"));
        assert!(source.contains("kind: \"role\" as const"));
        assert!(source.contains("Parameters<NativeProjectedManager[\"insertProjected\"]>"));
        assert!(source.contains("const orderedAttributeTypeKeys = new Map<string, string>()"));
        assert!(
            source.contains(
                "orderedAttributeTypeKeys.set(JSON.stringify(label), definition.typeKey)"
            )
        );
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
        assert!(!successor.contains("legacy.insertMany(instances)"));
        assert!(!successor.contains("legacy.putMany(instances)"));
        let projection = successor
            .find("const projection = installRuntimeProjection({")
            .unwrap();
        let authority = successor
            .find("const authority = installGeneratedSchemaAuthority({")
            .unwrap();
        assert!(projection < authority);
    }

    #[test]
    fn ordered_batch_materializer_transitive_slice_is_pure_and_closed() {
        let source = String::from_utf8(runtime_source(true).unwrap()).unwrap();
        let slices = [
            exact_source_slice(
                &source,
                "function assertRecord(",
                "\n\nfunction validateMultiplicity(",
            ),
            exact_source_slice(
                &source,
                "function validateMultiplicity(",
                "\n\nfunction validateAccepted(",
            ),
            exact_source_slice(
                &source,
                "function validateAccepted(",
                "\n\nfunction validateScalar(",
            ),
            exact_source_slice(
                &source,
                "function validateScalar(",
                "\n\nfunction freezeValue(",
            ),
            exact_source_slice(
                &source,
                "function parseProjectedWire(",
                "\n\nfunction hydrateProjectedValue(",
            ),
            exact_source_slice(
                &source,
                "function scalarFromWire(",
                "\n\nfunction isScalarValueType(",
            ),
            exact_source_slice(
                &source,
                "function isScalarValueType(",
                "\n\ndeclare const SELECTION_BRAND",
            ),
            exact_source_slice(
                &source,
                "function validateOrderedProjectedBatchField(",
                "\n\nfunction hydrateOrderedProjectedBatchValue(",
            ),
            exact_source_slice(
                &source,
                "function hydrateOrderedProjectedBatchValue(",
                "\n\nfunction hydrateOrderedProjectedBatchMember(",
            ),
            exact_source_slice(
                &source,
                "function hydrateOrderedProjectedBatchMember(",
                "\n\nfunction hydrateOrderedProjectedBatchWire<",
            ),
            exact_source_slice(
                &source,
                "function hydrateOrderedProjectedBatchWire<",
                "\n\nfunction retainOrderedProjectedProofs(",
            ),
            exact_source_slice(
                &source,
                "function retainOrderedProjectedProofs(",
                "\n\nfunction orderedProjectedManager<",
            ),
            exact_source_slice(
                &source,
                "const materializeOrderedProjectedBatch:",
                "\n\n/** @internal Install authority-backed evidence",
            ),
        ];
        let graph = slices.join("\n");

        assert_eq!(
            identifier_calls(&graph),
            BTreeSet::from([
                "BigInt".to_owned(),
                "Date".to_owned(),
                "RangeError".to_owned(),
                "TypeError".to_owned(),
                "assertRecord".to_owned(),
                "defineProperty".to_owned(),
                "for".to_owned(),
                "forEach".to_owned(),
                "freeze".to_owned(),
                "get".to_owned(),
                "getOwnPropertyDescriptor".to_owned(),
                "getTime".to_owned(),
                "hydrateOrderedProjectedBatchMember".to_owned(),
                "hydrateOrderedProjectedBatchValue".to_owned(),
                "hydrateOrderedProjectedBatchWire".to_owned(),
                "hasOwn".to_owned(),
                "if".to_owned(),
                "includes".to_owned(),
                "isArray".to_owned(),
                "isFinite".to_owned(),
                "isScalarValueType".to_owned(),
                "keys".to_owned(),
                "map".to_owned(),
                "parse".to_owned(),
                "parseProjectedWire".to_owned(),
                "push".to_owned(),
                "retainOrderedProjectedProofs".to_owned(),
                "return".to_owned(),
                "roleProof".to_owned(),
                "rootProof".to_owned(),
                "scalarFromWire".to_owned(),
                "set".to_owned(),
                "some".to_owned(),
                "switch".to_owned(),
                "validateAccepted".to_owned(),
                "validateMultiplicity".to_owned(),
                "validateOrderedProjectedBatchField".to_owned(),
                "validateScalar".to_owned(),
            ])
        );
        for forbidden in [
            "requireProjection",
            "validateAttributeValueJson",
            "validateFieldValueJson",
            "validateHydratedAttributeValueJson",
            "validateThingJson",
            "validateOwnedMember",
            "canonicalOrderedTemporalIso",
            "orderedScalarToWire",
            "lowerOrderedProjectedValue",
            "manager(",
            ".manager(",
            "session(",
            ".session(",
            "query(",
            ".query(",
            "transaction(",
            ".transaction(",
            "provider(",
            ".provider(",
            "Promise",
            "async ",
            "await ",
        ] {
            assert!(
                !graph.contains(forbidden),
                "forbidden batch call {forbidden}"
            );
        }
        for required_guard in [
            "wire.iid !== null",
            "wire.value.valueType !== definition.valueType",
            "receivedNames.length !== names.length",
            "names.some((name) => !Object.hasOwn(wire.values, name))",
            "native complete thing wire has no IID",
            "orderedAttributeTypeKeys.get(field.attribute)",
            "candidate[\"__typebridgeModel\"] !== attributeTypeKey",
            "validateMultiplicity(member.name, value, member.multiplicity)",
            "validateAccepted(member.name, value, member.accepted)",
            "validateOrderedProjectedBatchField(definition, member.name, value)",
        ] {
            assert!(
                graph.contains(required_guard),
                "missing wire guard {required_guard}"
            );
        }

        let materializer = slices.last().unwrap();
        assert!(materializer.contains(
            "() => Object.freeze({\n      kind: \"root\" as const,\n      authority,\n      row: ordinal,\n    })"
        ));
        assert!(materializer.contains(
            "(roleName, playerIndex) => Object.freeze({\n      kind: \"role\" as const,\n      authority,\n      row: ordinal,\n      roleName,\n      playerIndex,\n    })"
        ));
        let retained = slices[slices.len() - 2];
        let first_set = retained.find("orderedFacadeProofs.set(").unwrap();
        assert!(retained.rfind("retained.push(").unwrap() < first_set);
        assert!(retained.rfind("rootProof()").unwrap() < first_set);
        assert!(retained.rfind("roleProof(").unwrap() < first_set);
        assert_eq!(retained.matches("orderedFacadeProofs.set(").count(), 1);

        assert_eq!(
            source.matches("materializeOrderedProjectedBatch").count(),
            2
        );
        assert_eq!(source.matches("projectedBatchMaterializer").count(), 2);
        for call in [
            "native.insertManyProjected<Complete>(instances.length, (index) => {",
            "native.putManyProjected<Complete>(instances.length, (index) => {",
            "native.updateManyProjected<Complete>(updates.length, (index) => {",
            "native.deleteManyProjected(iids.length, (index) => {",
        ] {
            assert!(
                source.contains(call),
                "missing rowCount + rowAt call {call}"
            );
        }
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
